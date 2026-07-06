import { mkdir, stat } from "node:fs/promises";
import { join } from "node:path";
import { CHECKS_DIR, CONFIG_FILE } from "./config.ts";
import { git, gitOut, hasRemote } from "./git.ts";

const CONFIG_STUB = `# preceipts configuration — https://github.com/jokull/preceipts
# Checks are executable files in .preceipts/checks/; filename = check name.
# Exit 0 = pass. Shebang decides the interpreter (bash by default).

[required]
checks = []

# Per-check settings (timeout defaults to 10m):
# [check.test]
# timeout = "15m"

# Normalization (format, codegen) runs serially BEFORE the receipt tree is
# computed — checks must never mutate the worktree (that invalidates the run):
# [prepare]
# commands = ["format"]
# [prepare.format]
# cmd = "pnpm format"
`;

/**
 * Refspecs that make ordinary `git push` / `git fetch` carry receipts.
 * The notes refs use trailing-glob patterns (refs/notes/receipts*) so pushes
 * don't fail before the first receipt exists — non-glob refspecs error on a
 * missing source, globs match nothing silently.
 */
export const PUSH_REFSPECS = [
  "HEAD",
  "refs/notes/receipts*:refs/notes/receipts*",
  "refs/receipts/logs/*:refs/receipts/logs/*",
];

export const FETCH_REFSPECS = [
  "+refs/notes/receipts*:refs/notes/remotes/origin/receipts*",
  "refs/receipts/logs/*:refs/receipts/logs/*",
];

export interface InitOptions {
  /** Add receipt refspecs to origin without asking. */
  yes?: boolean;
  /** Interactive confirmation hook; consulted only when `yes` is not set. */
  confirmRefspecs?: () => Promise<boolean>;
}

export interface InitResult {
  createdChecksDir: boolean;
  createdConfig: boolean;
  /** Refspecs actually added to remote.origin (empty when skipped or present). */
  addedRefspecs: string[];
  hasOrigin: boolean;
}

/** Scaffold .preceipts/ and optionally wire receipt refspecs into origin. */
export async function init(root: string, options: InitOptions = {}): Promise<InitResult> {
  const checksDir = join(root, CHECKS_DIR);
  const configPath = join(root, CONFIG_FILE);

  const checksDirExisted = (await stat(checksDir).catch(() => null))?.isDirectory() === true;
  await mkdir(checksDir, { recursive: true });

  let createdConfig = false;
  if (!(await Bun.file(configPath).exists())) {
    await Bun.write(configPath, CONFIG_STUB);
    createdConfig = true;
  }

  const hasOrigin = await hasRemote(root, "origin");
  const addedRefspecs: string[] = [];
  if (hasOrigin) {
    const wantRefspecs =
      options.yes === true || (await options.confirmRefspecs?.()) === true;
    if (wantRefspecs) {
      const existingPush = (await git(["config", "--get-all", "remote.origin.push"], { cwd: root }))
        .stdout.split("\n").filter(Boolean);
      const existingFetch = (await git(["config", "--get-all", "remote.origin.fetch"], { cwd: root }))
        .stdout.split("\n").filter(Boolean);
      for (const refspec of PUSH_REFSPECS) {
        if (!existingPush.includes(refspec)) {
          await gitOut(["config", "--add", "remote.origin.push", refspec], { cwd: root });
          addedRefspecs.push(`push: ${refspec}`);
        }
      }
      for (const refspec of FETCH_REFSPECS) {
        if (!existingFetch.includes(refspec)) {
          await gitOut(["config", "--add", "remote.origin.fetch", refspec], { cwd: root });
          addedRefspecs.push(`fetch: ${refspec}`);
        }
      }
    }
  }

  return { createdChecksDir: !checksDirExisted, createdConfig, addedRefspecs, hasOrigin };
}
