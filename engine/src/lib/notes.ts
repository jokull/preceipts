import { catFileBatch, git, gitOut, hashObjectWrite } from "./git.ts";
import { PreceiptsError } from "./errors.ts";
import { parseReceipts, type Receipt } from "./receipt.ts";

export const NOTES_REF = "refs/notes/receipts";
export const NOTES_REMOTE_TRACKING_REF = "refs/notes/remotes/origin/receipts";
export const LOG_REF_PREFIX = "refs/receipts/logs/";

// Appends to the notes ref race within a single process (parallel checks finishing
// together); serialize them through a promise chain.
let appendChain: Promise<unknown> = Promise.resolve();

/** Append one receipt line to the note attached to `tree` under refs/notes/receipts. */
export function appendReceiptLine(root: string, tree: string, line: string): Promise<void> {
  const next = appendChain.then(async () => {
    const result = await git(["notes", `--ref=${NOTES_REF}`, "append", "-m", line, tree], {
      cwd: root,
    });
    if (result.code !== 0) {
      throw new PreceiptsError(`failed to append receipt note on tree ${tree}: ${result.stderr.trim()}`);
    }
  });
  // Keep the chain alive even if this append fails, so later appends still run.
  appendChain = next.catch(() => undefined);
  return next as Promise<void>;
}

/** Read the raw note text for a tree, or null when no note exists. */
export async function readNote(root: string, tree: string): Promise<string | null> {
  const result = await git(["notes", `--ref=${NOTES_REF}`, "show", tree], { cwd: root });
  if (result.code !== 0) return null;
  return result.stdout;
}

/** All receipts recorded for a tree (parsed, malformed lines skipped). */
export async function readReceipts(root: string, tree: string): Promise<Receipt[]> {
  const note = await readNote(root, tree);
  if (note === null) return [];
  return parseReceipts(note);
}

/** Every receipt in the notes ref, across all trees. Uses one cat-file batch. */
export async function readAllReceipts(root: string): Promise<Receipt[]> {
  const listResult = await git(["notes", `--ref=${NOTES_REF}`, "list"], { cwd: root });
  if (listResult.code !== 0) return []; // no notes ref yet
  const noteShas: string[] = [];
  for (const line of listResult.stdout.split("\n")) {
    const [noteSha] = line.split(" ");
    if (noteSha) noteShas.push(noteSha);
  }
  if (noteShas.length === 0) return [];
  const contents = await catFileBatch(root, noteShas);
  const receipts: Receipt[] = [];
  for (const text of contents.values()) receipts.push(...parseReceipts(text));
  return receipts;
}

/** Store a log blob and pin it with a refs/receipts/logs/<sha> ref. Returns the blob sha. */
export async function storeLog(root: string, content: Uint8Array): Promise<string> {
  const sha = await hashObjectWrite(root, content);
  await gitOut(["update-ref", `${LOG_REF_PREFIX}${sha}`, sha], { cwd: root });
  return sha;
}

/** Read a stored log blob; null when the blob is gone (e.g. pruned then gc'd). */
export async function readLog(root: string, blobSha: string): Promise<string | null> {
  const result = await git(["cat-file", "blob", blobSha], { cwd: root });
  if (result.code !== 0) return null;
  return result.stdout;
}
