import { describe, expect, test } from "bun:test";
import {
  encodeReceipt,
  latestByCheck,
  logBlobSha,
  parseReceiptLine,
  parseReceipts,
  type Receipt,
} from "./receipt.ts";

function makeReceipt(overrides: Partial<Receipt> = {}): Receipt {
  return {
    v: 1,
    check: "typecheck",
    cmd: ".preceipts/checks/typecheck",
    tree: "8f3a000000000000000000000000000000000000",
    ok: true,
    exit: 0,
    started: "2026-07-06T12:40:11Z",
    duration_ms: 48211,
    runner: { name: "Jökull", email: "jokull@example.com", host: "mbp.local" },
    dirty: false,
    log: "blob:1c9e000000000000000000000000000000000000",
    check_blob: "9a1f000000000000000000000000000000000000",
    ...overrides,
  };
}

describe("receipt encode/decode", () => {
  test("round-trips", () => {
    const receipt = makeReceipt({ runner: { name: "A", email: "a@b.c", host: "h", agent: "claude-code/2.x" } });
    const line = encodeReceipt(receipt);
    expect(line).not.toContain("\n");
    expect(parseReceiptLine(line)).toEqual(receipt);
  });

  test("agent field omitted when unset", () => {
    const line = encodeReceipt(makeReceipt());
    expect(line).not.toContain("agent");
    expect(parseReceiptLine(line)?.runner.agent).toBeUndefined();
  });

  test("blank and malformed lines parse to null", () => {
    expect(parseReceiptLine("")).toBeNull();
    expect(parseReceiptLine("   ")).toBeNull();
    expect(parseReceiptLine("not json")).toBeNull();
    expect(parseReceiptLine('{"v":2,"check":"x"}')).toBeNull();
    expect(parseReceiptLine('"just a string"')).toBeNull();
  });

  test("parseReceipts skips blank separator lines from notes append", () => {
    const a = encodeReceipt(makeReceipt({ check: "lint" }));
    const b = encodeReceipt(makeReceipt({ check: "test" }));
    const receipts = parseReceipts(`${a}\n\n${b}\n`);
    expect(receipts.map((r) => r.check)).toEqual(["lint", "test"]);
  });
});

describe("latestByCheck", () => {
  test("picks the most recent receipt per check", () => {
    const older = makeReceipt({ started: "2026-07-06T10:00:00Z", ok: false });
    const newer = makeReceipt({ started: "2026-07-06T12:00:00Z", ok: true });
    const other = makeReceipt({ check: "lint", started: "2026-07-06T11:00:00Z" });
    const latest = latestByCheck([newer, older, other]);
    expect(latest.get("typecheck")?.ok).toBe(true);
    expect(latest.get("lint")?.started).toBe("2026-07-06T11:00:00Z");
  });
});

describe("logBlobSha", () => {
  test("extracts sha", () => {
    expect(logBlobSha(makeReceipt())).toBe("1c9e000000000000000000000000000000000000");
    expect(logBlobSha(makeReceipt({ log: "" }))).toBeNull();
  });
});
