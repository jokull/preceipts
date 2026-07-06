import { git, gitOut } from "./git.ts";
import { LOG_REF_PREFIX } from "./notes.ts";
import { readAllReceipts } from "./notes.ts";
import { logBlobSha } from "./receipt.ts";

export const DEFAULT_KEEP_SUCCESS_MS = 30 * 86_400_000;
export const DEFAULT_KEEP_FAILURE_MS = 90 * 86_400_000;

export interface GcOptions {
  keepSuccessMs?: number;
  keepFailureMs?: number;
  /** Injectable clock for tests. */
  now?: () => number;
}

export interface GcResult {
  deleted: string[];
  kept: number;
  /** Log refs no receipt references; left alone. */
  orphans: number;
}

/**
 * Delete refs/receipts/logs/* refs whose receipts are older than retention
 * (success and failure logs age out on different schedules). Receipt lines are
 * permanent and outlive their logs; unreferenced log refs are kept untouched.
 */
export async function gc(root: string, options: GcOptions = {}): Promise<GcResult> {
  const keepSuccess = options.keepSuccessMs ?? DEFAULT_KEEP_SUCCESS_MS;
  const keepFailure = options.keepFailureMs ?? DEFAULT_KEEP_FAILURE_MS;
  const now = options.now?.() ?? Date.now();

  // Latest expiry per log blob: keep a log if ANY receipt referencing it is
  // still within its retention window.
  const expiry = new Map<string, number>();
  for (const receipt of await readAllReceipts(root)) {
    const sha = logBlobSha(receipt);
    if (!sha) continue;
    const startedMs = Date.parse(receipt.started);
    if (Number.isNaN(startedMs)) continue;
    const expiresAt = startedMs + (receipt.ok ? keepSuccess : keepFailure);
    expiry.set(sha, Math.max(expiry.get(sha) ?? 0, expiresAt));
  }

  const refsOut = await git(["for-each-ref", "--format=%(refname)", `${LOG_REF_PREFIX}*`], {
    cwd: root,
  });
  const deleted: string[] = [];
  let kept = 0;
  let orphans = 0;
  for (const refname of refsOut.stdout.split("\n")) {
    if (!refname.startsWith(LOG_REF_PREFIX)) continue;
    const sha = refname.slice(LOG_REF_PREFIX.length);
    const expiresAt = expiry.get(sha);
    if (expiresAt === undefined) {
      orphans += 1;
      continue;
    }
    if (expiresAt <= now) {
      await gitOut(["update-ref", "-d", refname], { cwd: root });
      deleted.push(sha);
    } else {
      kept += 1;
    }
  }
  return { deleted, kept, orphans };
}
