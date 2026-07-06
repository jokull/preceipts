export interface Runner {
  name: string;
  email: string;
  host: string;
  agent?: string;
}

/** One receipt: a single check run, keyed to a git tree. Stored as one JSON line. */
export interface Receipt {
  v: 1;
  check: string;
  cmd: string;
  tree: string;
  ok: boolean;
  exit: number;
  started: string;
  duration_ms: number;
  runner: Runner;
  dirty: boolean;
  /** "blob:<sha>" reference to the stored log. */
  log: string;
  /** Blob sha of the check script that ran. */
  check_blob: string;
}

/** Encode a receipt as a single JSON line (no trailing newline), stable field order. */
export function encodeReceipt(r: Receipt): string {
  const runner: Runner = { name: r.runner.name, email: r.runner.email, host: r.runner.host };
  if (r.runner.agent !== undefined) runner.agent = r.runner.agent;
  return JSON.stringify({
    v: r.v,
    check: r.check,
    cmd: r.cmd,
    tree: r.tree,
    ok: r.ok,
    exit: r.exit,
    started: r.started,
    duration_ms: r.duration_ms,
    runner,
    dirty: r.dirty,
    log: r.log,
    check_blob: r.check_blob,
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Parse one receipt line; returns null for blank lines or anything malformed. */
export function parseReceiptLine(line: string): Receipt | null {
  const trimmed = line.trim();
  if (trimmed === "") return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return null;
  }
  if (!isRecord(parsed)) return null;
  const r = parsed;
  if (r["v"] !== 1) return null;
  if (typeof r["check"] !== "string" || typeof r["tree"] !== "string") return null;
  if (typeof r["ok"] !== "boolean" || typeof r["started"] !== "string") return null;
  const runnerRaw = isRecord(r["runner"]) ? r["runner"] : {};
  const runner: Runner = {
    name: typeof runnerRaw["name"] === "string" ? runnerRaw["name"] : "",
    email: typeof runnerRaw["email"] === "string" ? runnerRaw["email"] : "",
    host: typeof runnerRaw["host"] === "string" ? runnerRaw["host"] : "",
  };
  if (typeof runnerRaw["agent"] === "string") runner.agent = runnerRaw["agent"];
  return {
    v: 1,
    check: r["check"],
    cmd: typeof r["cmd"] === "string" ? r["cmd"] : "",
    tree: r["tree"],
    ok: r["ok"],
    exit: typeof r["exit"] === "number" ? r["exit"] : -1,
    started: r["started"],
    duration_ms: typeof r["duration_ms"] === "number" ? r["duration_ms"] : 0,
    runner,
    dirty: r["dirty"] === true,
    log: typeof r["log"] === "string" ? r["log"] : "",
    check_blob: typeof r["check_blob"] === "string" ? r["check_blob"] : "",
  };
}

/** Parse a whole note (JSONL, possibly with blank separator lines) into receipts. */
export function parseReceipts(text: string): Receipt[] {
  const receipts: Receipt[] = [];
  for (const line of text.split("\n")) {
    const receipt = parseReceiptLine(line);
    if (receipt) receipts.push(receipt);
  }
  return receipts;
}

/** Latest receipt per check name, by `started` timestamp (ISO strings sort lexically). */
export function latestByCheck(receipts: Receipt[]): Map<string, Receipt> {
  const latest = new Map<string, Receipt>();
  for (const receipt of receipts) {
    const existing = latest.get(receipt.check);
    if (!existing || receipt.started >= existing.started) {
      latest.set(receipt.check, receipt);
    }
  }
  return latest;
}

/** Extract the blob sha from a receipt `log` field ("blob:<sha>"); null if absent. */
export function logBlobSha(receipt: Receipt): string | null {
  return receipt.log.startsWith("blob:") ? receipt.log.slice("blob:".length) : null;
}
