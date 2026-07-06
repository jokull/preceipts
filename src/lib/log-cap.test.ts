import { describe, expect, test } from "bun:test";
import { capLog } from "./log-cap.ts";

function bytes(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

describe("capLog", () => {
  test("small logs pass through untouched", () => {
    const log = bytes("hello\nworld\n");
    expect(capLog(log)).toBe(log);
  });

  test("logs exactly at the cap pass through", () => {
    const log = new Uint8Array(100).fill(97);
    expect(capLog(log, 10, 100)).toBe(log);
  });

  test("oversized logs keep head and tail with a marker", () => {
    const head = "H".repeat(10);
    const middle = "M".repeat(1000);
    const tail = "T".repeat(90);
    const capped = new TextDecoder().decode(capLog(bytes(head + middle + tail), 10, 100));
    expect(capped.startsWith("H".repeat(10))).toBe(true);
    expect(capped.endsWith("T".repeat(90))).toBe(true);
    expect(capped).toContain("[preceipts] log truncated");
    expect(capped).not.toContain("M".repeat(50));
  });

  test("tail gets the bulk of the budget", () => {
    const log = bytes("a".repeat(2_000_000));
    const capped = capLog(log);
    // head 64KB + tail (1MB - 64KB) + marker
    expect(capped.byteLength).toBeGreaterThanOrEqual(1024 * 1024);
    expect(capped.byteLength).toBeLessThan(1024 * 1024 + 256);
  });
});
