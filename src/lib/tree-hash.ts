import { mkdtemp, rm } from "node:fs/promises";
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
 * read HEAD's tree into a temp index, `git add -A` on top of it, `git write-tree`.
 * Untracked-but-ignored files stay excluded (normal gitignore semantics); the
 * real index is never touched.
 */
export async function computeWorkingTree(root: string): Promise<WorkingTree> {
  const scratch = await mkdtemp(join(tmpdir(), "preceipts-index-"));
  const indexFile = join(scratch, "index");
  const env = { GIT_INDEX_FILE: indexFile };
  try {
    const headTree = await revParse(root, "HEAD^{tree}");
    if (headTree !== null) {
      await gitOut(["read-tree", "HEAD"], { cwd: root, env });
    } else {
      await gitOut(["read-tree", "--empty"], { cwd: root, env });
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
