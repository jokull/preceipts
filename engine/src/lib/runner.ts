import { open, readFile, rm } from "node:fs/promises";
import { hostname, userInfo } from "node:os";
import { join, relative } from "node:path";
import {
  CHECKS_DIR,
  DEFAULT_TIMEOUT_MS,
  listChecks,
  loadConfig,
  type CheckFile,
  type PrepareStep,
} from "./config.ts";
import { PreceiptsError } from "./errors.ts";
import type { RunEventHandler } from "./events.ts";
import { git, gitOut } from "./git.ts";
import { capLog } from "./log-cap.ts";
import { appendReceiptLine, storeLog } from "./notes.ts";
import { encodeReceipt, type Receipt, type Runner } from "./receipt.ts";
import { computeWorkingTree, type WorkingTree } from "./tree-hash.ts";

export interface RunOptions {
  /** Check names to run; empty/omitted means every check in .preceipts/checks/. */
  checks?: string[];
  /** Value for runner.agent; callers usually pass process.env.PRECEIPTS_AGENT. */
  agent?: string;
  onEvent?: RunEventHandler;
}

export interface RunInvalidated {
  /** Tree the checks ran against. */
  before: string;
  /** Tree the worktree held when they finished. */
  after: string;
  changed: string[];
}

export interface RunResult {
  /** The (post-prepare) tree the checks ran against. */
  workingTree: WorkingTree;
  /** Minted receipts; empty when the run was invalidated. */
  receipts: Receipt[];
  /** True when every check passed AND receipts were minted. */
  ok: boolean;
  /** Paths the prepare phase normalized ([] when it changed nothing). */
  prepareChanged: string[];
  /** Set when the worktree changed during the run — nothing was minted. */
  invalidated: RunInvalidated | null;
}

async function resolveRunnerIdentity(root: string, agent?: string): Promise<Runner> {
  const [nameResult, emailResult] = await Promise.all([
    git(["config", "user.name"], { cwd: root }),
    git(["config", "user.email"], { cwd: root }),
  ]);
  const runner: Runner = {
    name: nameResult.code === 0 ? nameResult.stdout.trim() : userInfo().username,
    email: emailResult.code === 0 ? emailResult.stdout.trim() : "",
    host: hostname(),
  };
  if (agent) runner.agent = agent;
  return runner;
}

async function scriptHasShebang(path: string): Promise<boolean> {
  const file = Bun.file(path);
  const head = await file.slice(0, 2).text();
  return head === "#!";
}

/** ISO timestamp truncated to whole seconds, matching the PRD's receipt example. */
function isoNow(): string {
  return new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
}

/**
 * Run one prepare step via `bash -c`, streaming output. Prepare is
 * normalization (format, codegen): mutating the worktree here is the point.
 * A failing step aborts the whole run — an unnormalizable worktree has no
 * honest tree to mint against.
 */
async function runPrepareStep(root: string, step: PrepareStep, onEvent: RunEventHandler): Promise<void> {
  onEvent({ event: "prepare-started", step: step.name, ts: isoNow() });
  const startedAt = performance.now();

  const proc = Bun.spawn(["bash", "-c", step.cmd], {
    cwd: root,
    env: { ...process.env, PRECEIPTS_PREPARE: step.name },
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });

  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    proc.kill("SIGKILL");
  }, step.timeoutMs);

  const decoder = new TextDecoder();
  async function drain(stream: ReadableStream<Uint8Array>): Promise<void> {
    for await (const chunk of stream) {
      onEvent({ event: "prepare-output", step: step.name, chunk: decoder.decode(chunk) });
    }
  }
  await Promise.all([drain(proc.stdout), drain(proc.stderr)]);
  const rawExit = await proc.exited;
  clearTimeout(timer);

  const durationMs = Math.round(performance.now() - startedAt);
  const exit = timedOut ? -1 : rawExit;
  const ok = !timedOut && rawExit === 0;
  onEvent({ event: "prepare-finished", step: step.name, ok, exit, duration_ms: durationMs });

  if (!ok) {
    throw new PreceiptsError(
      timedOut
        ? `prepare step "${step.name}" timed out after ${step.timeoutMs}ms — no receipts minted`
        : `prepare step "${step.name}" failed (exit ${exit}) — fix it or remove it from [prepare].commands; no receipts minted`,
    );
  }
}

/** Paths that differ between two trees (git diff-tree, recursive, names only). */
async function treeChangedPaths(root: string, before: string, after: string): Promise<string[]> {
  const result = await git(["diff-tree", "-r", "--name-only", before, after], { cwd: root });
  if (result.code !== 0) return [];
  return result.stdout.split("\n").filter((line) => line !== "");
}

/** A finished check whose receipt is NOT yet minted — minting waits for the
 * post-run tree-stability verification. */
interface PendingReceipt {
  receipt: Omit<Receipt, "log">;
  logBytes: Uint8Array;
}

async function runOneCheck(
  root: string,
  check: CheckFile,
  tree: string,
  dirty: boolean,
  runner: Runner,
  timeoutMs: number,
  onEvent: RunEventHandler,
): Promise<PendingReceipt> {
  const relPath = relative(root, check.path);
  const checkBlob = (await gitOut(["hash-object", "-w", "--", check.path], { cwd: root })).trim();

  const started = isoNow();
  const startedAt = performance.now();
  onEvent({ event: "check-started", check: check.name, tree, ts: started });

  const argv = (await scriptHasShebang(check.path)) ? [check.path] : ["bash", check.path];
  const proc = Bun.spawn(argv, {
    cwd: root,
    env: { ...process.env, PRECEIPTS_CHECK: check.name, PRECEIPTS_TREE: tree },
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });

  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    proc.kill("SIGKILL");
  }, timeoutMs);

  const chunks: Uint8Array[] = [];
  const decoder = new TextDecoder();
  async function drain(stream: ReadableStream<Uint8Array>): Promise<void> {
    for await (const chunk of stream) {
      chunks.push(chunk);
      onEvent({ event: "output", check: check.name, chunk: decoder.decode(chunk) });
    }
  }
  await Promise.all([drain(proc.stdout), drain(proc.stderr)]);
  const rawExit = await proc.exited;
  clearTimeout(timer);

  const durationMs = Math.round(performance.now() - startedAt);
  const exit = timedOut ? -1 : rawExit;
  const ok = !timedOut && rawExit === 0;

  if (timedOut) {
    const note = `\n[preceipts] check "${check.name}" timed out after ${timeoutMs}ms and was killed\n`;
    chunks.push(new TextEncoder().encode(note));
    onEvent({ event: "output", check: check.name, chunk: note });
  }

  onEvent({ event: "check-finished", check: check.name, ok, exit, duration_ms: durationMs });

  return {
    receipt: {
      v: 1,
      check: check.name,
      cmd: relPath,
      tree,
      ok,
      exit,
      started,
      duration_ms: durationMs,
      runner,
      dirty,
      check_blob: checkBlob,
    },
    logBytes: capLog(Buffer.concat(chunks)),
  };
}

/**
 * The full run: prepare (serial normalization) → compute the receipt tree →
 * checks in parallel → verify the worktree did not change under the checks →
 * mint one receipt per check onto the tree.
 *
 * Prepare exists so formatting/codegen is orchestration, not a gate: a hook
 * that rewrites files AFTER receipts are minted would make every receipt a
 * lie about the commit. Mutation before the tree is computed is expected and
 * reported; mutation during the checks invalidates the run — nothing mints.
 */
function isPidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // EPERM means the process exists but belongs to someone else.
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/**
 * One run per worktree at a time. Two concurrent runs race each other's
 * prepare mutations and duplicate every check; the second invocation (an
 * agent's CLI run while the cockpit runs, or vice versa) must fail fast
 * instead. The lock lives in the worktree's git dir so parallel worktrees
 * of the same repo stay independent. A lock left by a dead process (kill -9,
 * crash) is detected by pid and taken over.
 */
async function acquireRunLock(root: string): Promise<() => Promise<void>> {
  const gitDir = await git(["rev-parse", "--absolute-git-dir"], { cwd: root });
  if (gitDir.code !== 0) throw new PreceiptsError("not a git repository");
  const lockPath = join(gitDir.stdout.trim(), "preceipts-run.lock");
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const handle = await open(lockPath, "wx");
      await handle.writeFile(String(process.pid));
      await handle.close();
      return async () => {
        await rm(lockPath, { force: true });
      };
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      const raw = await readFile(lockPath, "utf8").catch(() => "");
      const pid = Number.parseInt(raw.trim(), 10);
      if (Number.isFinite(pid) && pid > 0 && isPidAlive(pid)) {
        throw new PreceiptsError(
          `another preceipts run is already in progress (pid ${pid}) — a second run would race prepare and duplicate checks on this worktree; wait for it or kill it`,
        );
      }
      await rm(lockPath, { force: true }); // stale lock from a dead process
    }
  }
  throw new PreceiptsError("could not acquire the run lock");
}

export async function runChecks(root: string, options: RunOptions = {}): Promise<RunResult> {
  const releaseLock = await acquireRunLock(root);
  try {
    return await runChecksLocked(root, options);
  } finally {
    await releaseLock();
  }
}

async function runChecksLocked(root: string, options: RunOptions = {}): Promise<RunResult> {
  const config = await loadConfig(root);
  const available = await listChecks(root);
  if (available.length === 0) {
    throw new PreceiptsError(`no checks defined in ${CHECKS_DIR}/ — add an executable script there`);
  }

  let selected: CheckFile[];
  if (options.checks && options.checks.length > 0) {
    selected = options.checks.map((name) => {
      const found = available.find((check) => check.name === name);
      if (!found) {
        const known = available.map((check) => check.name).join(", ");
        throw new PreceiptsError(`unknown check "${name}" (available: ${known})`);
      }
      return found;
    });
  } else {
    selected = available;
  }

  for (const check of selected) {
    if (!check.executable) {
      throw new PreceiptsError(
        `check script ${relative(root, check.path)} is not executable — fix with: chmod +x ${relative(root, check.path)}`,
      );
    }
  }

  const onEvent: RunEventHandler = options.onEvent ?? (() => undefined);

  // Prepare: normalize the worktree BEFORE the receipt tree exists.
  let prepareChanged: string[] = [];
  if (config.prepare.length > 0) {
    const before = await computeWorkingTree(root);
    for (const step of config.prepare) {
      await runPrepareStep(root, step, onEvent);
    }
    const after = await computeWorkingTree(root);
    if (after.tree !== before.tree) {
      prepareChanged = await treeChangedPaths(root, before.tree, after.tree);
      onEvent({ event: "tree-normalized", tree: after.tree, changed: prepareChanged });
    }
  }

  const workingTree = await computeWorkingTree(root);
  onEvent({ event: "run-started", tree: workingTree.tree, dirty: workingTree.dirty });

  const runner = await resolveRunnerIdentity(root, options.agent);
  const runs = await Promise.all(
    selected.map((check) =>
      runOneCheck(
        root,
        check,
        workingTree.tree,
        workingTree.dirty,
        runner,
        config.timeouts[check.name] ?? DEFAULT_TIMEOUT_MS,
        onEvent,
      ),
    ),
  );

  // Receipts must describe a tree that actually held still while the checks
  // read it. If it moved, minting anything would be a lie — for the before
  // tree AND the after tree.
  const finalTree = await computeWorkingTree(root);
  if (finalTree.tree !== workingTree.tree) {
    const changed = await treeChangedPaths(root, workingTree.tree, finalTree.tree);
    onEvent({
      event: "worktree-changed",
      before: workingTree.tree,
      after: finalTree.tree,
      changed,
    });
    return {
      workingTree,
      receipts: [],
      ok: false,
      prepareChanged,
      invalidated: { before: workingTree.tree, after: finalTree.tree, changed },
    };
  }

  const receipts: Receipt[] = [];
  for (const run of runs) {
    const logSha = await storeLog(root, run.logBytes);
    const receipt: Receipt = { ...run.receipt, log: `blob:${logSha}` };
    await appendReceiptLine(root, workingTree.tree, encodeReceipt(receipt));
    onEvent({ event: "receipt-minted", check: receipt.check, tree: workingTree.tree, log: receipt.log });
    receipts.push(receipt);
  }

  return {
    workingTree,
    receipts,
    ok: receipts.every((receipt) => receipt.ok),
    prepareChanged,
    invalidated: null,
  };
}
