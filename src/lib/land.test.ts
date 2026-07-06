import { describe, expect, test } from "bun:test";
import { buildTrailers } from "./land.ts";
import type { Receipt } from "./receipt.ts";

const TREE = "8f3a1b2c3d4e5f60718293a4b5c6d7e8f9012345";

function makeReceipt(overrides: Partial<Receipt> = {}): Receipt {
  return {
    v: 1,
    check: "typecheck",
    cmd: ".preceipts/checks/typecheck",
    tree: TREE,
    ok: true,
    exit: 0,
    started: "2026-07-06T12:40:11Z",
    duration_ms: 48211,
    runner: { name: "Jökull Sólberg", email: "jokull@triptojapan.com", host: "mbp.local" },
    dirty: false,
    log: "blob:1c9e",
    check_blob: "9a1f",
    ...overrides,
  };
}

describe("buildTrailers", () => {
  test("matches the PRD trailer shape", () => {
    const receipts = [
      makeReceipt({ check: "typecheck", duration_ms: 48_000 }),
      makeReceipt({ check: "test", duration_ms: 192_000, started: "2026-07-06T12:45:00Z" }),
      makeReceipt({ check: "lint", duration_ms: 9_000 }),
    ];
    const trailers = buildTrailers(receipts, TREE);
    expect(trailers).toEqual([
      "Receipts: typecheck ✓ 48s · test ✓ 3m12s · lint ✓ 9s",
      `Receipts-Tree: ${TREE}`,
      "Receipts-Runner: jokull@mbp.local",
    ]);
  });

  test("agent runs are labeled and failures marked", () => {
    const receipts = [
      makeReceipt({
        ok: false,
        exit: 1,
        runner: { name: "J", email: "jokull@x.com", host: "mbp.local", agent: "claude-code/2.x" },
      }),
    ];
    const trailers = buildTrailers(receipts, TREE);
    expect(trailers[0]).toBe("Receipts: typecheck ✗ 48s");
    expect(trailers[2]).toBe("Receipts-Runner: jokull@mbp.local (claude-code)");
  });

  test("stale land records a Receipts-Stale trailer", () => {
    const trailers = buildTrailers([makeReceipt()], TREE, {
      mergeBase: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      baseHead: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    });
    expect(trailers.at(-1)).toBe("Receipts-Stale: base moved aaaaaaaaaaaa→bbbbbbbbbbbb");
  });

  test("no receipts still yields the tree trailer", () => {
    const trailers = buildTrailers([], TREE);
    expect(trailers).toEqual([`Receipts-Tree: ${TREE}`]);
  });
});
