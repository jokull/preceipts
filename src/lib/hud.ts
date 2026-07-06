import { stat } from "node:fs/promises";
import { join } from "node:path";
import { PreceiptsError } from "./errors.ts";
import { catFileBatch, git, hasRemote, revParse } from "./git.ts";
import { NOTES_REF, NOTES_REMOTE_TRACKING_REF } from "./notes.ts";
import { computeStatus, type Status } from "./status.ts";
import { computeWorkingTree } from "./tree-hash.ts";

export interface Hud {
  /** Current branch name, or null when HEAD is detached. */
  branch: string | null;
  /** The ref conflicts/freshness were computed against (e.g. "origin/main"). */
  base: string;
  head: string;
  /** Working-tree hash — what `run` would mint against right now. */
  tree: string;
  dirty: boolean;
  /** Commits on HEAD that base lacks / commits on base that HEAD lacks. */
  ahead: number;
  behind: number;
  /**
   * Would `git merge HEAD` into base apply cleanly? null when it can't be
   * answered (no merge base, or git lacks merge-tree --write-tree).
   */
  mergeClean: boolean | null;
  conflictFiles: string[];
  /**
   * Would the squash commit's tree be the proven tree? True iff the base head
   * is the merge-base — the same predicate `land` calls stale.
   */
  landFresh: boolean;
  /** ms since the last fetch from any remote (FETCH_HEAD mtime); null = never. */
  fetchAgeMs: number | null;
  /** Local receipt lines origin doesn't have yet; null when there's no origin. */
  unsyncedReceipts: number | null;
  /** Receipt table for the working tree — coherence with what's on disk now. */
  status: Status;
  green: boolean;
}

export interface HudOptions {
  /** Base branch name (default "main"). origin/<base> is preferred when it exists. */
  base?: string;
}

/** Prefer the remote-tracking view of the base; fall back to the local branch. */
async function resolveBase(root: string, base: string): Promise<{ name: string; sha: string }> {
  const remote = await revParse(root, `refs/remotes/origin/${base}`);
  if (remote !== null) return { name: `origin/${base}`, sha: remote };
  const local = await revParse(root, `refs/heads/${base}`);
  if (local !== null) return { name: base, sha: local };
  throw new PreceiptsError(`base "${base}" resolves to neither origin/${base} nor a local branch`);
}

/** In-memory merge verdict via merge-tree --write-tree (git ≥ 2.38). */
async function mergeVerdict(
  root: string,
  baseSha: string,
  headSha: string,
): Promise<{ clean: boolean | null; conflictFiles: string[] }> {
  const result = await git(
    ["merge-tree", "--write-tree", "--name-only", baseSha, headSha],
    { cwd: root },
  );
  if (result.code === 0) return { clean: true, conflictFiles: [] };
  if (result.code !== 1) return { clean: null, conflictFiles: [] };
  // Exit 1 output: tree oid, conflicted file names, blank line, then
  // informational messages — only the section before the blank line is files.
  const lines = result.stdout.split("\n").slice(1);
  const files: string[] = [];
  for (const line of lines) {
    if (line === "") break;
    files.push(line);
  }
  return { clean: false, conflictFiles: files };
}

/** All receipt lines stored under a notes ref, as a set of raw JSONL lines. */
async function noteLines(root: string, notesRef: string): Promise<Set<string>> {
  const lines = new Set<string>();
  const list = await git(["notes", `--ref=${notesRef}`, "list"], { cwd: root });
  if (list.code !== 0) return lines; // ref doesn't exist yet
  const shas = list.stdout
    .split("\n")
    .map((line) => line.split(" ")[0])
    .filter((sha): sha is string => Boolean(sha));
  if (shas.length === 0) return lines;
  const contents = await catFileBatch(root, shas);
  for (const text of contents.values()) {
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (trimmed !== "") lines.add(trimmed);
    }
  }
  return lines;
}

/** ms since FETCH_HEAD was last written, or null if this repo never fetched. */
async function fetchAge(root: string, now: number): Promise<number | null> {
  const dirs = await git(["rev-parse", "--absolute-git-dir", "--git-common-dir"], { cwd: root });
  if (dirs.code !== 0) return null;
  const candidates = dirs.stdout.split("\n").map((dir) => dir.trim()).filter(Boolean);
  for (const dir of candidates) {
    try {
      const info = await stat(join(dir, "FETCH_HEAD"));
      return Math.max(0, now - info.mtimeMs);
    } catch {
      // keep looking
    }
  }
  return null;
}

/**
 * One read-only payload of branch situational awareness: "what happens if I
 * land right now?" Never fetches, never touches the worktree or index.
 */
export async function computeHud(root: string, options: HudOptions = {}): Promise<Hud> {
  const baseName = options.base ?? "main";

  const head = await revParse(root, "HEAD");
  if (head === null) throw new PreceiptsError("HEAD does not resolve — unborn branch?");
  const branchResult = await git(["symbolic-ref", "--quiet", "--short", "HEAD"], { cwd: root });
  const branch = branchResult.code === 0 ? branchResult.stdout.trim() : null;

  const base = await resolveBase(root, baseName);

  const [workingTree, counts, verdict, fetchAgeMs, origin] = await Promise.all([
    computeWorkingTree(root),
    git(["rev-list", "--left-right", "--count", `${base.sha}...${head}`], { cwd: root }),
    mergeVerdict(root, base.sha, head),
    fetchAge(root, Date.now()),
    hasRemote(root, "origin"),
  ]);

  let ahead = 0;
  let behind = 0;
  if (counts.code === 0) {
    const [left, right] = counts.stdout.trim().split(/\s+/);
    behind = Number(left ?? 0) || 0;
    ahead = Number(right ?? 0) || 0;
  }

  const mergeBaseResult = await git(["merge-base", base.sha, head], { cwd: root });
  const mergeBase = mergeBaseResult.code === 0 ? mergeBaseResult.stdout.trim() : null;
  const landFresh = mergeBase !== null && mergeBase === base.sha;

  let unsyncedReceipts: number | null = null;
  if (origin) {
    const [local, remote] = await Promise.all([
      noteLines(root, NOTES_REF),
      noteLines(root, NOTES_REMOTE_TRACKING_REF),
    ]);
    let count = 0;
    for (const line of local) if (!remote.has(line)) count += 1;
    unsyncedReceipts = count;
  }

  const status = await computeStatus(root, workingTree.tree);

  return {
    branch,
    base: base.name,
    head,
    tree: workingTree.tree,
    dirty: workingTree.dirty,
    ahead,
    behind,
    mergeClean: verdict.clean,
    conflictFiles: verdict.conflictFiles,
    landFresh,
    fetchAgeMs,
    unsyncedReceipts,
    status,
    green: status.green,
  };
}
