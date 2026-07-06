export const LOG_HEAD_CAP = 64 * 1024;
export const LOG_TOTAL_CAP = 1024 * 1024;

/**
 * Cap a check log for storage: logs at or under `totalCap` bytes are kept whole;
 * larger logs keep the first `headCap` bytes and as much of the tail as fits in
 * `totalCap`, with an elision marker in between. The failure tail is the
 * valuable part, so the tail gets the bulk of the budget.
 */
export function capLog(
  log: Uint8Array,
  headCap: number = LOG_HEAD_CAP,
  totalCap: number = LOG_TOTAL_CAP,
): Uint8Array {
  if (log.byteLength <= totalCap) return log;
  const omitted = log.byteLength - totalCap; // approximate; marker not counted against budget
  const marker = new TextEncoder().encode(
    `\n\n[preceipts] log truncated: ${omitted} bytes omitted (kept first ${headCap} and last ${totalCap - headCap} bytes)\n\n`,
  );
  const head = log.slice(0, headCap);
  const tailLength = totalCap - headCap;
  const tail = log.slice(log.byteLength - tailLength);
  const out = new Uint8Array(head.byteLength + marker.byteLength + tail.byteLength);
  out.set(head, 0);
  out.set(marker, head.byteLength);
  out.set(tail, head.byteLength + marker.byteLength);
  return out;
}
