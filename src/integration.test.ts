import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const CLI = join(import.meta.dir, "cli.ts");

interface CliResult {
  code: number;
  stdout: string;
  stderr: string;
}

async function run(cwd: string, argv: string[], env: Record<string, string> = {}): Promise<CliResult> {
  const proc = Bun.spawn(["bun", CLI, ...argv], {
    cwd,
    env: { ...process.env, ...env },
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { code, stdout, stderr };
}

async function sh(cwd: string, argv: string[]): Promise<string> {
  const proc = Bun.spawn(argv, { cwd, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (code !== 0) throw new Error(`${argv.join(" ")} failed (${code}): ${stderr}`);
  return stdout;
}

/** Init a scratch repo with local-only identity; never touches global git config. */
async function initRepo(path: string): Promise<void> {
  await mkdir(path, { recursive: true });
  await sh(path, ["git", "init", "-q", "-b", "main"]);
  await configureRepo(path);
}

async function configureRepo(path: string): Promise<void> {
  await sh(path, ["git", "config", "user.name", "Test Runner"]);
  await sh(path, ["git", "config", "user.email", "runner@test.local"]);
  await sh(path, ["git", "config", "commit.gpgsign", "false"]);
}

async function writeCheck(repo: string, name: string, body: string): Promise<void> {
  const path = join(repo, ".preceipts", "checks", name);
  await writeFile(path, body, { mode: 0o755 });
}

async function commitAll(repo: string, message: string): Promise<void> {
  await sh(repo, ["git", "add", "-A"]);
  await sh(repo, ["git", "commit", "-q", "-m", message]);
}

let scratch: string;

beforeAll(async () => {
  scratch = await mkdtemp(join(tmpdir(), "preceipts-itest-"));
});

afterAll(async () => {
  await rm(scratch, { recursive: true, force: true });
});

describe("full loop: init → run (fail) → fix → run (green) → land", () => {
  let repo: string;

  beforeAll(async () => {
    repo = join(scratch, "loop");
    await initRepo(repo);
    await writeFile(join(repo, "app.txt"), "v1\n");
    await commitAll(repo, "initial");
  });

  test("init scaffolds .preceipts", async () => {
    const result = await run(repo, ["init"]);
    expect(result.code).toBe(0);
    expect(result.stdout).toContain("created .preceipts/checks/");
    expect(await Bun.file(join(repo, ".preceipts", "config.toml")).text()).toContain("[required]");
    // idempotent
    const again = await run(repo, ["init"]);
    expect(again.code).toBe(0);
    expect(again.stdout).toContain("already present");
  });

  test("run with a passing and a failing check mints receipts and exits 1", async () => {
    await writeCheck(repo, "greet", "#!/bin/bash\necho hello-from-greet\nexit 0\n");
    await writeCheck(repo, "flaky", '#!/bin/bash\nif [ -f fixed.txt ]; then echo fixed; else echo broken >&2; exit 1; fi\n');
    await writeFile(
      join(repo, ".preceipts", "config.toml"),
      '[required]\nchecks = ["greet", "flaky"]\n',
    );
    await commitAll(repo, "add checks");

    const result = await run(repo, ["run"]);
    expect(result.code).toBe(1);
    expect(result.stdout).toContain("[greet] hello-from-greet");
    expect(result.stdout).toContain("[flaky] broken");
    expect(result.stderr).not.toContain("dirty");
  });

  test("status reflects the failure and exits non-zero", async () => {
    const result = await run(repo, ["status", "--json"]);
    expect(result.code).toBe(1);
    const status = JSON.parse(result.stdout);
    expect(status.green).toBe(false);
    const states = Object.fromEntries(
      status.rows.map((row: { check: string; state: string }) => [row.check, row.state]),
    );
    expect(states).toEqual({ greet: "ok", flaky: "fail" });
  });

  test("log prints the stored output for the failing check", async () => {
    const result = await run(repo, ["log", "flaky"]);
    expect(result.stdout).toContain("broken");
    expect(result.code).toBe(1); // failing receipt → non-zero
    const greetLog = await run(repo, ["log", "greet"]);
    expect(greetLog.stdout).toContain("hello-from-greet");
    expect(greetLog.code).toBe(0);
  });

  test("dirty worktree warns and mints against the dirty tree, not HEAD", async () => {
    await writeFile(join(repo, "fixed.txt"), "yes\n");
    const result = await run(repo, ["run", "--json"]);
    expect(result.code).toBe(0);
    expect(result.stderr).toContain("worktree is dirty — this receipt becomes valid the moment");
    const summary = JSON.parse(result.stdout.trim().split("\n").at(-1)!);
    expect(summary.dirty).toBe(true);
    const headTree = (await sh(repo, ["git", "rev-parse", "HEAD^{tree}"])).trim();
    expect(summary.tree).not.toBe(headTree);
    // default status inspects the working tree — the receipts just minted count
    const worktreeStatus = await run(repo, ["status", "--json"]);
    expect(worktreeStatus.code).toBe(0);
    expect(JSON.parse(worktreeStatus.stdout).tree).toBe(summary.tree);
    // explicit HEAD still shows the old (failing) receipts
    const headStatus = await run(repo, ["status", "HEAD"]);
    expect(headStatus.code).toBe(1);
  });

  test("committing exactly the tested state turns status green", async () => {
    await commitAll(repo, "fix flaky");
    const result = await run(repo, ["status", "--json"]);
    expect(result.code).toBe(0);
    const status = JSON.parse(result.stdout);
    expect(status.green).toBe(true);
    expect(status.tree).toBe((await sh(repo, ["git", "rev-parse", "HEAD^{tree}"])).trim());
  });

  test("run --events emits the PRD NDJSON events", async () => {
    const result = await run(repo, ["run", "greet", "--events"], {
      PRECEIPTS_AGENT: "claude-code/2.x",
    });
    expect(result.code).toBe(0);
    const events = result.stdout
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    const kinds = events.map((event) => event.event);
    expect(kinds[0]).toBe("check-started");
    expect(kinds).toContain("output");
    expect(kinds).toContain("check-finished");
    expect(kinds.at(-1)).toBe("receipt-minted");
    const started = events[0];
    expect(started.check).toBe("greet");
    expect(typeof started.tree).toBe("string");
    expect(typeof started.ts).toBe("string");
    const finished = events.find((event) => event.event === "check-finished");
    expect(finished.ok).toBe(true);
    expect(finished.exit).toBe(0);
    expect(typeof finished.duration_ms).toBe("number");
    const minted = events.find((event) => event.event === "receipt-minted");
    expect(minted.log).toStartWith("blob:");
    // agent label recorded
    const status = JSON.parse((await run(repo, ["status", "--json"])).stdout);
    const greet = status.rows.find((row: { check: string }) => row.check === "greet");
    expect(greet.receipt.runner.agent).toBe("claude-code/2.x");
  });

  test("editing a check marks its receipts stale-definition", async () => {
    await writeCheck(repo, "greet", "#!/bin/bash\necho hello-v2\nexit 0\n");
    // inspect HEAD: its receipts exist but were minted by the old script — the
    // worktree default would show "missing" (editing the check changed the tree)
    const result = await run(repo, ["status", "HEAD", "--json"]);
    const status = JSON.parse(result.stdout);
    const greet = status.rows.find((row: { check: string }) => row.check === "greet");
    expect(greet.state).toBe("stale-definition");
    expect(status.green).toBe(false);
    // restore: clean worktree again, default status == HEAD status, green
    await sh(repo, ["git", "checkout", "--", ".preceipts/checks/greet"]);
    expect(JSON.parse((await run(repo, ["status", "--json"])).stdout).green).toBe(true);
  });

  test("land squashes the branch onto base with trailers, tree preserved", async () => {
    // build a feature branch off main with green receipts
    await sh(repo, ["git", "checkout", "-q", "-b", "feature"]);
    await writeFile(join(repo, "app.txt"), "v2-feature\n");
    const runResult = await run(repo, ["run"]);
    expect(runResult.code).toBe(0);
    await commitAll(repo, "feature work");
    await sh(repo, ["git", "checkout", "-q", "main"]);

    const branchTree = (await sh(repo, ["git", "rev-parse", "feature^{tree}"])).trim();
    const landResult = await run(repo, ["land", "feature", "--onto", "main", "--no-push", "--json"]);
    expect(landResult.code).toBe(0);
    const landed = JSON.parse(landResult.stdout);
    expect(landed.stale).toBeNull();
    expect(landed.tree).toBe(branchTree);

    // base fast-forwarded to the squash commit; its tree IS the branch tree
    const mainHead = (await sh(repo, ["git", "rev-parse", "main"])).trim();
    expect(mainHead).toBe(landed.commit);
    expect((await sh(repo, ["git", "rev-parse", "main^{tree}"])).trim()).toBe(branchTree);

    const message = await sh(repo, ["git", "log", "-1", "--format=%B", "main"]);
    expect(message).toContain("feature (squash)");
    expect(message).toMatch(/Receipts: .*greet ✓/);
    expect(message).toContain(`Receipts-Tree: ${branchTree}`);
    expect(message).toMatch(/Receipts-Runner: runner@/);
    expect(message).not.toContain("Receipts-Stale");
    // squash commit has exactly one parent: old main
    const parents = (await sh(repo, ["git", "rev-list", "--parents", "-1", "main"])).trim().split(" ");
    expect(parents.length).toBe(2);
  });

  test("land refuses when required receipts are missing", async () => {
    await sh(repo, ["git", "checkout", "-q", "-b", "unproven", "main"]);
    await writeFile(join(repo, "app.txt"), "v3-unproven\n");
    await commitAll(repo, "unproven work");
    await sh(repo, ["git", "checkout", "-q", "main"]);
    const result = await run(repo, ["land", "unproven", "--onto", "main", "--no-push"]);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain("not green");
    expect(result.stderr).toContain("missing");
  });
});

describe("stale land", () => {
  let repo: string;

  beforeAll(async () => {
    repo = join(scratch, "stale");
    await initRepo(repo);
    await run(repo, ["init"]);
    await writeCheck(repo, "ok", "#!/bin/bash\ntrue\n");
    await writeFile(join(repo, ".preceipts", "config.toml"), '[required]\nchecks = ["ok"]\n');
    await writeFile(join(repo, "base.txt"), "base\n");
    await commitAll(repo, "initial");

    // feature branch with receipts
    await sh(repo, ["git", "checkout", "-q", "-b", "feature"]);
    await writeFile(join(repo, "feature.txt"), "feature\n");
    expect((await run(repo, ["run"])).code).toBe(0);
    await commitAll(repo, "feature");

    // meanwhile main moves
    await sh(repo, ["git", "checkout", "-q", "main"]);
    await writeFile(join(repo, "base.txt"), "base moved\n");
    await commitAll(repo, "main moves");
  });

  test("--require-fresh exits non-zero when base moved", async () => {
    const result = await run(repo, ["land", "feature", "--onto", "main", "--no-push", "--require-fresh"]);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain("base");
    expect(result.stderr).toContain("moved");
  });

  test("non-interactive land without flags states the situation and does not land", async () => {
    const before = (await sh(repo, ["git", "rev-parse", "main"])).trim();
    const result = await run(repo, ["land", "feature", "--onto", "main", "--no-push"]);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain("--allow-stale");
    expect((await sh(repo, ["git", "rev-parse", "main"])).trim()).toBe(before);
  });

  test("--allow-stale lands and records the Receipts-Stale trailer", async () => {
    const result = await run(repo, ["land", "feature", "--onto", "main", "--no-push", "--allow-stale"]);
    expect(result.code).toBe(0);
    const message = await sh(repo, ["git", "log", "-1", "--format=%B", "main"]);
    expect(message).toMatch(/Receipts-Stale: base moved [0-9a-f]{12}→[0-9a-f]{12}/);
    // tree is still the branch tree (that's what "unproven" means, honestly recorded)
    expect((await sh(repo, ["git", "rev-parse", "main^{tree}"])).trim()).toBe(
      (await sh(repo, ["git", "rev-parse", "feature^{tree}"])).trim(),
    );
  });
});

describe("sync between two clones of a bare remote", () => {
  let bare: string;
  let cloneA: string;
  let cloneB: string;

  beforeAll(async () => {
    bare = join(scratch, "remote.git");
    await sh(scratch, ["git", "init", "-q", "--bare", "-b", "main", bare]);

    cloneA = join(scratch, "clone-a");
    await initRepo(cloneA);
    await sh(cloneA, ["git", "remote", "add", "origin", bare]);
    await run(cloneA, ["init", "--yes"]);
    await writeCheck(cloneA, "alpha", "#!/bin/bash\necho alpha-ok\n");
    await writeCheck(cloneA, "beta", "#!/bin/bash\necho beta-ok\n");
    await writeFile(
      join(cloneA, ".preceipts", "config.toml"),
      '[required]\nchecks = ["alpha", "beta"]\n',
    );
    await commitAll(cloneA, "initial");
    await sh(cloneA, ["git", "push", "-q", "origin", "main"]);

    cloneB = join(scratch, "clone-b");
    await sh(scratch, ["git", "clone", "-q", bare, cloneB]);
    await configureRepo(cloneB);
    await run(cloneB, ["init", "--yes"]);
  });

  test("receipts minted in A are visible in B after both sync", async () => {
    expect((await run(cloneA, ["run", "alpha"])).code).toBe(0);
    expect((await run(cloneA, ["sync"])).code).toBe(0);
    expect((await run(cloneB, ["sync"])).code).toBe(0);

    const status = JSON.parse((await run(cloneB, ["status", "--json"])).stdout);
    const alpha = status.rows.find((row: { check: string }) => row.check === "alpha");
    expect(alpha.state).toBe("ok");
  });

  test("concurrent mints in both clones merge via cat_sort_uniq without loss", async () => {
    // Same committed tree in both clones; each mints a different check before
    // seeing the other's note → same-note conflict on the same tree object.
    expect((await run(cloneA, ["run", "beta"])).code).toBe(0);
    expect((await run(cloneB, ["run", "alpha"])).code).toBe(0);

    expect((await run(cloneA, ["sync"])).code).toBe(0); // A pushes first
    expect((await run(cloneB, ["sync"])).code).toBe(0); // B must merge A's note, then push
    expect((await run(cloneA, ["sync"])).code).toBe(0); // A picks up the merged result

    for (const clone of [cloneA, cloneB]) {
      const status = JSON.parse((await run(clone, ["status", "--json"])).stdout);
      expect(status.green).toBe(true);
      const states = Object.fromEntries(
        status.rows.map((row: { check: string; state: string }) => [row.check, row.state]),
      );
      expect(states).toEqual({ alpha: "ok", beta: "ok" });
    }
  });

  test("init --yes wired refspecs so plain git push carries receipts", async () => {
    const pushSpecs = await sh(cloneA, ["git", "config", "--get-all", "remote.origin.push"]);
    expect(pushSpecs).toContain("refs/notes/receipts*:refs/notes/receipts*");
    expect(pushSpecs).toContain("refs/receipts/logs/*:refs/receipts/logs/*");
    expect(pushSpecs).toContain("HEAD");
    const fetchSpecs = await sh(cloneA, ["git", "config", "--get-all", "remote.origin.fetch"]);
    expect(fetchSpecs).toContain("refs/notes/receipts*:refs/notes/remotes/origin/receipts*");
  });
});

describe("gc", () => {
  let repo: string;

  beforeAll(async () => {
    repo = join(scratch, "gc");
    await initRepo(repo);
    await run(repo, ["init"]);
    await writeCheck(repo, "pass", "#!/bin/bash\necho fine\n");
    await writeCheck(repo, "fail", "#!/bin/bash\necho nope; exit 1\n");
    await writeFile(join(repo, ".preceipts", "config.toml"), '[required]\nchecks = ["pass"]\n');
    await commitAll(repo, "initial");
    await run(repo, ["run"]);
  });

  test("fresh logs survive default retention", async () => {
    const result = await run(repo, ["gc", "--json"]);
    expect(result.code).toBe(0);
    const gcResult = JSON.parse(result.stdout);
    expect(gcResult.deleted.length).toBe(0);
    expect(gcResult.kept).toBe(2);
  });

  test("success logs age out before failure logs", async () => {
    // 0s success retention: the passing check's log ref dies, the failing one lives
    const result = await run(repo, ["gc", "--keep-success", "0s", "--keep-failure", "90d", "--json"]);
    const gcResult = JSON.parse(result.stdout);
    expect(gcResult.deleted.length).toBe(1);
    expect(gcResult.kept).toBe(1);
    // failure log is still readable; the success receipt line outlives its log ref
    const failLog = await run(repo, ["log", "fail"]);
    expect(failLog.stdout).toContain("nope");
    const status = JSON.parse((await run(repo, ["status", "--json"])).stdout);
    const pass = status.rows.find((row: { check: string }) => row.check === "pass");
    expect(pass.state).toBe("ok");
  });
});

describe("honest errors", () => {
  test("not a git repository", async () => {
    const dir = await mkdtemp(join(tmpdir(), "preceipts-norepo-"));
    try {
      const result = await run(dir, ["status"]);
      expect(result.code).not.toBe(0);
      expect(result.stderr).toContain("not a git repository");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  test("missing .preceipts directory", async () => {
    const repo = join(scratch, "bare-repo");
    await initRepo(repo);
    await writeFile(join(repo, "x.txt"), "x\n");
    await commitAll(repo, "initial");
    const result = await run(repo, ["run"]);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain(".preceipts");
    expect(result.stderr).toContain("preceipts init");
  });

  test("non-executable check script", async () => {
    const repo = join(scratch, "noexec");
    await initRepo(repo);
    await run(repo, ["init"]);
    await writeFile(join(repo, ".preceipts", "checks", "quiet"), "#!/bin/bash\ntrue\n", {
      mode: 0o644,
    });
    await writeFile(join(repo, "x.txt"), "x\n");
    await commitAll(repo, "initial");
    const result = await run(repo, ["run"]);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain("not executable");
    expect(result.stderr).toContain("chmod +x");
  });

  test("unknown check name", async () => {
    const repo = join(scratch, "unknown-check");
    await initRepo(repo);
    await run(repo, ["init"]);
    await writeCheck(repo, "real", "#!/bin/bash\ntrue\n");
    await writeFile(join(repo, "x.txt"), "x\n");
    await commitAll(repo, "initial");
    const result = await run(repo, ["run", "imaginary"]);
    expect(result.code).not.toBe(0);
    expect(result.stderr).toContain('unknown check "imaginary"');
    expect(result.stderr).toContain("real");
  });

  test("check without shebang runs under bash by default", async () => {
    const repo = join(scratch, "no-shebang");
    await initRepo(repo);
    await run(repo, ["init"]);
    await writeCheck(repo, "plain", 'echo "ran-under-bash $((1 + 1))"\n');
    await writeFile(join(repo, "x.txt"), "x\n");
    await commitAll(repo, "initial");
    const result = await run(repo, ["run"]);
    expect(result.code).toBe(0);
    expect(result.stdout).toContain("ran-under-bash 2");
  });
});

describe("hud", () => {
  let repo: string;

  async function hud(): Promise<Record<string, unknown> & { conflictFiles: string[] }> {
    const result = await run(repo, ["hud", "--json"]);
    expect(result.code).toBe(0);
    return JSON.parse(result.stdout);
  }

  beforeAll(async () => {
    const bare = join(scratch, "hud-remote.git");
    await sh(scratch, ["git", "init", "-q", "--bare", "-b", "main", bare]);

    repo = join(scratch, "hud-repo");
    await initRepo(repo);
    await sh(repo, ["git", "remote", "add", "origin", bare]);
    await run(repo, ["init"]);
    await writeCheck(repo, "ok", "#!/bin/bash\necho fine\n");
    await writeFile(join(repo, ".preceipts", "config.toml"), '[required]\nchecks = ["ok"]\n');
    await writeFile(join(repo, "base.txt"), "line1\nline2\n");
    await commitAll(repo, "initial");
    await sh(repo, ["git", "push", "-q", "origin", "main"]);

    // A feature branch that edits base.txt — clean against origin/main for now.
    await sh(repo, ["git", "checkout", "-q", "-b", "feature"]);
    await writeFile(join(repo, "base.txt"), "feature-v1\nline2\n");
    await commitAll(repo, "feature edit");
  });

  test("clean fresh branch: ahead, merge clean, land fresh, green after run", async () => {
    expect((await run(repo, ["run"])).code).toBe(0);
    const payload = await hud();
    expect(payload["branch"]).toBe("feature");
    expect(payload["base"]).toBe("origin/main");
    expect(payload["ahead"]).toBe(1);
    expect(payload["behind"]).toBe(0);
    expect(payload["mergeClean"]).toBe(true);
    expect(payload.conflictFiles).toEqual([]);
    expect(payload["landFresh"]).toBe(true);
    expect(payload["dirty"]).toBe(false);
    expect(payload["green"]).toBe(true);
    // Minted locally, never synced; and this repo has never fetched.
    expect(payload["unsyncedReceipts"]).toBeGreaterThanOrEqual(1);
    expect(payload["fetchAgeMs"]).toBeNull();
  });

  test("sync zeroes unsynced receipts and stamps fetch age", async () => {
    expect((await run(repo, ["sync"])).code).toBe(0);
    const payload = await hud();
    expect(payload["unsyncedReceipts"]).toBe(0);
    expect(typeof payload["fetchAgeMs"]).toBe("number");
  });

  test("base moving with a conflicting edit: behind, conflicts, stale land", async () => {
    await sh(repo, ["git", "checkout", "-q", "main"]);
    await writeFile(join(repo, "base.txt"), "main-v2\nline2\n");
    await commitAll(repo, "main edit");
    await sh(repo, ["git", "push", "-q", "origin", "main"]);
    await sh(repo, ["git", "checkout", "-q", "feature"]);

    const payload = await hud();
    expect(payload["behind"]).toBe(1);
    expect(payload["mergeClean"]).toBe(false);
    expect(payload.conflictFiles).toEqual(["base.txt"]);
    expect(payload["landFresh"]).toBe(false);
  });

  test("dirty worktree flips dirty and greenness follows the disk state", async () => {
    await writeFile(join(repo, "scratch.txt"), "uncommitted\n");
    const payload = await hud();
    expect(payload["dirty"]).toBe(true);
    expect(payload["green"]).toBe(false); // receipts were minted for the committed tree
    await rm(join(repo, "scratch.txt"));
  });

  test("detached HEAD reports branch null", async () => {
    await sh(repo, ["git", "checkout", "-q", "--detach"]);
    const payload = await hud();
    expect(payload["branch"]).toBeNull();
    await sh(repo, ["git", "checkout", "-q", "feature"]);
  });

  test("human-readable output states the situation", async () => {
    const result = await run(repo, ["hud"]);
    expect(result.code).toBe(0);
    expect(result.stdout).toContain("feature vs origin/main");
    expect(result.stdout).toContain("CONFLICTS (1): base.txt");
    expect(result.stdout).toContain("stale (base moved");
  });
});
