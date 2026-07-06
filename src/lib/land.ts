import { formatDuration } from "./duration.ts";
import { PreceiptsError } from "./errors.ts";
import { git, gitOut, hasRemote, revParse } from "./git.ts";
import { LOG_REF_PREFIX, NOTES_REF } from "./notes.ts";
import type { Receipt } from "./receipt.ts";
import { computeStatus, type Status } from "./status.ts";

export interface StaleInfo {
  /** Merge-base of base and branch — where the branch's receipts were minted from. */
  mergeBase: string;
  /** Where the base ref points now. */
  baseHead: string;
}

/** Build the PRD's merge-commit trailers from the receipts that back a land. */
export function buildTrailers(receipts: Receipt[], tree: string, stale?: StaleInfo): string[] {
  const summary = receipts
    .map((r) => `${r.check} ${r.ok ? "✓" : "✗"} ${formatDuration(r.duration_ms)}`)
    .join(" · ");
  const trailers: string[] = [];
  if (summary) trailers.push(`Receipts: ${summary}`);
  trailers.push(`Receipts-Tree: ${tree}`);
  const latest = receipts.reduce<Receipt | null>(
    (best, r) => (best === null || r.started >= best.started ? r : best),
    null,
  );
  if (latest) {
    const localPart = latest.runner.email.split("@")[0] || latest.runner.name || "unknown";
    const agent = latest.runner.agent ? ` (${latest.runner.agent.split("/")[0]})` : "";
    trailers.push(`Receipts-Runner: ${localPart}@${latest.runner.host}${agent}`);
  }
  if (stale) {
    trailers.push(
      `Receipts-Stale: base moved ${stale.mergeBase.slice(0, 12)}→${stale.baseHead.slice(0, 12)}`,
    );
  }
  return trailers;
}

export interface LandOptions {
  onto?: string;
  noPush?: boolean;
  allowStale?: boolean;
  requireFresh?: boolean;
  message?: string;
  /**
   * Called when the base has moved past the merge-base. Return true to land
   * anyway (the CLI wires this to an interactive TTY confirm). Only consulted
   * when neither allowStale nor requireFresh is set.
   */
  confirmStale?: (info: StaleInfo) => Promise<boolean>;
}

export interface LandResult {
  branch: string;
  base: string;
  tree: string;
  commit: string;
  stale: StaleInfo | null;
  pushed: boolean;
  message: string;
  status: Status;
}

/**
 * Squash-land a branch onto a base: verify required receipts for the branch
 * tree, create `git commit-tree <branch-tree> -p <base-head>` with receipt
 * trailers, fast-forward the base ref, and push (base + receipt refs).
 */
export async function land(root: string, branch: string, options: LandOptions = {}): Promise<LandResult> {
  const base = options.onto ?? "main";

  const branchSha = await revParse(root, branch);
  if (branchSha === null) throw new PreceiptsError(`cannot resolve branch "${branch}"`);
  const baseHead = await revParse(root, `refs/heads/${base}`);
  if (baseHead === null) throw new PreceiptsError(`base branch "${base}" does not exist locally`);

  // Judge receipts against the branch tree's own check definitions: a branch
  // that changes a check script is still self-consistent evidence for itself.
  const status = await computeStatus(root, branch, { definitionSource: "tree" });
  const requiredRows = status.rows.filter((row) => row.required);
  if (requiredRows.length === 0) {
    throw new PreceiptsError(
      `no required checks configured in .preceipts/config.toml — nothing to verify, refusing to land`,
    );
  }
  if (!status.green) {
    const problems = requiredRows
      .filter((row) => row.state !== "ok")
      .map((row) => `  ${row.check}: ${row.state}`)
      .join("\n");
    throw new PreceiptsError(
      `required receipts are not green for ${branch} (tree ${status.tree.slice(0, 12)}):\n${problems}\nrun \`preceipts run\` on that tree, or check \`preceipts status ${branch}\``,
    );
  }

  const mergeBaseResult = await git(["merge-base", base, branch], { cwd: root });
  if (mergeBaseResult.code !== 0) {
    throw new PreceiptsError(
      `cannot find a merge-base between "${base}" and "${branch}": ${mergeBaseResult.stderr.trim()}`,
    );
  }
  const mergeBase = mergeBaseResult.stdout.trim();

  let stale: StaleInfo | null = null;
  if (baseHead !== mergeBase) {
    stale = { mergeBase, baseHead };
    if (options.requireFresh) {
      throw new PreceiptsError(
        `base "${base}" moved since receipts were minted (${mergeBase.slice(0, 12)} → ${baseHead.slice(0, 12)}) — rebase & re-run (--require-fresh set)`,
      );
    }
    if (!options.allowStale) {
      if (!options.confirmStale) {
        throw new PreceiptsError(
          `base "${base}" moved since receipts were minted (${mergeBase.slice(0, 12)} → ${baseHead.slice(0, 12)}) — the squashed tree would be unproven.\nRebase & re-run, or pass --allow-stale to land anyway (records a Receipts-Stale trailer). --require-fresh makes this a hard failure for scripts.`,
        );
      }
      const proceed = await options.confirmStale(stale);
      if (!proceed) throw new PreceiptsError(`aborted: base "${base}" moved; not landing`, { exitCode: 1 });
    }
  }

  const receipts = status.rows.flatMap((row) => (row.receipt ? [row.receipt] : []));
  const trailers = buildTrailers(receipts, status.tree, stale ?? undefined);
  const subject = options.message?.trim() || `${branch} (squash)`;
  const message = `${subject}\n\n${trailers.join("\n")}\n`;

  const commit = (
    await gitOut(["commit-tree", status.tree, "-p", baseHead], { cwd: root, input: message })
  ).trim();

  // Fast-forward the base ref. If base is checked out here, go through
  // merge --ff-only so the worktree and index follow; otherwise update the ref
  // directly (guarded against concurrent movement by the old-value check).
  const headRef = (await git(["symbolic-ref", "--quiet", "HEAD"], { cwd: root })).stdout.trim();
  if (headRef === `refs/heads/${base}`) {
    await gitOut(["merge", "--ff-only", commit], { cwd: root });
  } else {
    await gitOut(["update-ref", `refs/heads/${base}`, commit, baseHead], { cwd: root });
  }

  let pushed = false;
  if (!options.noPush) {
    if (await hasRemote(root, "origin")) {
      const refspecs = [`refs/heads/${base}:refs/heads/${base}`];
      if ((await revParse(root, NOTES_REF)) !== null) refspecs.push(`${NOTES_REF}:${NOTES_REF}`);
      const logRefs = await git(["for-each-ref", "--format=%(refname)", `${LOG_REF_PREFIX}*`], {
        cwd: root,
      });
      if (logRefs.stdout.trim() !== "") refspecs.push(`${LOG_REF_PREFIX}*:${LOG_REF_PREFIX}*`);
      await gitOut(["push", "origin", ...refspecs], { cwd: root });
      pushed = true;
    }
  }

  return { branch, base, tree: status.tree, commit, stale, pushed, message, status };
}
