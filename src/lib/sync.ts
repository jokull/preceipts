import { PreceiptsError } from "./errors.ts";
import { git, gitOut, hasRemote, revParse } from "./git.ts";
import { LOG_REF_PREFIX, NOTES_REF, NOTES_REMOTE_TRACKING_REF } from "./notes.ts";

export interface SyncResult {
  fetched: boolean;
  merged: boolean;
  pushed: boolean;
}

/**
 * Fetch receipt refs from origin, merge remote receipts into local notes with
 * the lossless cat_sort_uniq strategy, then push receipt refs back.
 */
export async function sync(root: string): Promise<SyncResult> {
  if (!(await hasRemote(root, "origin"))) {
    throw new PreceiptsError(`no "origin" remote configured — nothing to sync with`);
  }

  // Log refs are content-addressed (name == value), so a plain wildcard fetch
  // can never conflict. The notes ref lands in a tracking ref, never directly.
  await gitOut(["fetch", "origin", `${LOG_REF_PREFIX}*:${LOG_REF_PREFIX}*`], { cwd: root });
  const notesFetch = await git(
    ["fetch", "origin", `+${NOTES_REF}:${NOTES_REMOTE_TRACKING_REF}`],
    { cwd: root },
  );
  const remoteHasNotes = notesFetch.code === 0;
  if (!remoteHasNotes && !/couldn't find remote ref/i.test(notesFetch.stderr)) {
    throw new PreceiptsError(`fetching ${NOTES_REF} from origin failed: ${notesFetch.stderr.trim()}`);
  }

  let merged = false;
  if (remoteHasNotes) {
    const local = await revParse(root, NOTES_REF);
    const remote = await revParse(root, NOTES_REMOTE_TRACKING_REF);
    if (remote !== null && local === null) {
      await gitOut(["update-ref", NOTES_REF, remote], { cwd: root });
      merged = true;
    } else if (remote !== null && local !== remote) {
      const mergeResult = await git(
        ["notes", `--ref=${NOTES_REF}`, "merge", "--strategy=cat_sort_uniq", NOTES_REMOTE_TRACKING_REF],
        { cwd: root },
      );
      if (mergeResult.code !== 0) {
        throw new PreceiptsError(
          `notes merge (cat_sort_uniq) failed:\n${mergeResult.stderr.trim()}\nresolve with \`git notes --ref=${NOTES_REF} merge --abort\` or inspect .git/NOTES_MERGE_WORKTREE`,
        );
      }
      merged = true;
    }
  }

  const refspecs: string[] = [];
  if ((await revParse(root, NOTES_REF)) !== null) refspecs.push(`${NOTES_REF}:${NOTES_REF}`);
  const logRefs = await git(["for-each-ref", "--format=%(refname)", `${LOG_REF_PREFIX}*`], {
    cwd: root,
  });
  if (logRefs.stdout.trim() !== "") refspecs.push(`${LOG_REF_PREFIX}*:${LOG_REF_PREFIX}*`);

  let pushed = false;
  if (refspecs.length > 0) {
    await gitOut(["push", "origin", ...refspecs], { cwd: root });
    pushed = true;
  }

  return { fetched: true, merged, pushed };
}
