import { join } from "node:path";
import { CHECKS_DIR, loadConfig } from "./config.ts";
import { PreceiptsError } from "./errors.ts";
import { git } from "./git.ts";
import { readReceipts } from "./notes.ts";
import { latestByCheck, type Receipt } from "./receipt.ts";

export type CheckState = "ok" | "fail" | "missing" | "stale-definition";

export interface StatusRow {
  check: string;
  required: boolean;
  state: CheckState;
  receipt: Receipt | null;
}

export interface Status {
  ref: string;
  tree: string;
  rows: StatusRow[];
  /** True iff every required check has an ok receipt whose definition still matches. */
  green: boolean;
}

export interface StatusOptions {
  /**
   * Where "the current check definition" lives for stale-definition detection:
   * - "worktree" (default): the script as it exists on disk right now — status
   *   answers "do these receipts speak to my checks as currently defined?"
   * - "tree": the script inside the inspected ref's own tree — land uses this,
   *   because a branch that legitimately changes a check is still self-consistent.
   */
  definitionSource?: "worktree" | "tree";
}

/** Resolve a ref (HEAD, branch name, commit sha, tree sha) to its tree sha. */
export async function resolveTree(root: string, ref: string): Promise<string> {
  const result = await git(["rev-parse", "--verify", "--quiet", `${ref}^{tree}`], { cwd: root });
  if (result.code !== 0) {
    throw new PreceiptsError(`cannot resolve "${ref}" to a tree — is it a branch, commit, or tree sha?`);
  }
  return result.stdout.trim();
}

/** Blob sha of the check script as it exists in the current worktree, or null if absent. */
async function worktreeCheckBlob(root: string, check: string): Promise<string | null> {
  const path = join(root, CHECKS_DIR, check);
  if (!(await Bun.file(path).exists())) return null;
  const result = await git(["hash-object", "--", path], { cwd: root });
  if (result.code !== 0) return null;
  return result.stdout.trim();
}

/** Blob sha of the check script inside a tree, or null if the tree lacks it. */
async function treeCheckBlob(root: string, tree: string, check: string): Promise<string | null> {
  const result = await git(["ls-tree", tree, "--", `${CHECKS_DIR}/${check}`], { cwd: root });
  if (result.code !== 0) return null;
  // "<mode> blob <sha>\t<path>"
  const sha = result.stdout.trim().split(/\s+/)[2];
  return sha ?? null;
}

function stateFor(receipt: Receipt | undefined, currentBlob: string | null): CheckState {
  if (!receipt) return "missing";
  if (currentBlob === null || receipt.check_blob !== currentBlob) return "stale-definition";
  return receipt.ok ? "ok" : "fail";
}

/**
 * Receipt table for a ref's tree against the required set. Required checks come
 * from the current worktree's config; "stale-definition" means the receipt's
 * check script no longer matches the current definition (see StatusOptions).
 */
export async function computeStatus(
  root: string,
  ref: string,
  options: StatusOptions = {},
): Promise<Status> {
  const source = options.definitionSource ?? "worktree";
  const config = await loadConfig(root);
  const tree = await resolveTree(root, ref);
  const receipts = latestByCheck(await readReceipts(root, tree));

  const names = [...config.required];
  const extras = [...receipts.keys()].filter((name) => !config.required.includes(name)).sort();
  names.push(...extras);

  const rows: StatusRow[] = [];
  for (const name of names) {
    const receipt = receipts.get(name);
    const currentBlob =
      source === "worktree"
        ? await worktreeCheckBlob(root, name)
        : await treeCheckBlob(root, tree, name);
    rows.push({
      check: name,
      required: config.required.includes(name),
      state: stateFor(receipt, currentBlob),
      receipt: receipt ?? null,
    });
  }

  const green = rows.filter((row) => row.required).every((row) => row.state === "ok");
  return { ref, tree, rows, green };
}
