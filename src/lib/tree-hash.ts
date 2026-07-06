import { copyFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { git, gitOut, revParse } from "./git.ts";
import { PreceiptsError } from "./errors.ts";

export interface WorkingTree {
  /** Tree sha of the working tree as it stands right now. */
  tree: string;
  /** Tree sha of HEAD's tree, or null on an unborn branch. */
  headTree: string | null;
  /** True when the working tree differs from HEAD's tree. */
  dirty: boolean;
}

/**
 * Compute the working tree's hash via a temporary index:
 * seed a temp index, `git add -A` on top of it, `git write-tree`.
 * Untracked-but-ignored files stay excluded (normal gitignore semantics); the
 * real index is never touched.
 *
 * The temp index is seeded by *copying the real index* whenever possible: the
 * copy carries git's stat cache, so `add -A` re-hashes only files that
 * actually changed (~0.2s on a large monorepo). A `read-tree HEAD` seed has
 * no stat data and forces a full re-hash of every tracked file (many seconds)
 * — it remains only as the fallback for repos without an index file yet.
 */
export async function computeWorkingTree(root: string): Promise<WorkingTree> {
  const scratch = await mkdtemp(join(tmpdir(), "preceipts-index-"));
  const indexFile = join(scratch, "index");
  const env = { GIT_INDEX_FILE: indexFile };
  try {
    const headTree = await revParse(root, "HEAD^{tree}");

    let seeded = false;
    const realIndex = (
      await git(["rev-parse", "--path-format=absolute", "--git-path", "index"], { cwd: root })
    ).stdout.trim();
    if (realIndex !== "") {
      try {
        await copyFile(realIndex, indexFile);
        seeded = true;
      } catch {
        // no index file yet (fresh repo) — fall through to read-tree
      }
    }
    if (!seeded) {
      if (headTree !== null) {
        await gitOut(["read-tree", "HEAD"], { cwd: root, env });
      } else {
        await gitOut(["read-tree", "--empty"], { cwd: root, env });
      }
    }

    const addResult = await git(["add", "-A"], { cwd: root, env });
    if (addResult.code !== 0) {
      throw new PreceiptsError(`failed to stage working tree into temp index: ${addResult.stderr.trim()}`);
    }
    const tree = (await gitOut(["write-tree"], { cwd: root, env })).trim();
    return { tree, headTree, dirty: tree !== headTree };
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}
