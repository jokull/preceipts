import { describe, expect, test } from "bun:test";
import { formatDuration, parseDuration } from "./duration.ts";

describe("parseDuration", () => {
  test("single units", () => {
    expect(parseDuration("30d")).toBe(30 * 86_400_000);
    expect(parseDuration("12h")).toBe(12 * 3_600_000);
    expect(parseDuration("15m")).toBe(15 * 60_000);
    expect(parseDuration("90s")).toBe(90_000);
    expect(parseDuration("250ms")).toBe(250);
  });

  test("compound", () => {
    expect(parseDuration("1h30m")).toBe(90 * 60_000);
    expect(parseDuration("2m30s")).toBe(150_000);
  });

  test("whitespace tolerated at edges", () => {
    expect(parseDuration(" 10m ")).toBe(600_000);
  });

  test("rejects junk", () => {
    expect(() => parseDuration("")).toThrow();
    expect(() => parseDuration("30")).toThrow();
    expect(() => parseDuration("d30")).toThrow();
    expect(() => parseDuration("30x")).toThrow();
    expect(() => parseDuration("10m junk")).toThrow();
  });
});

describe("formatDuration", () => {
  test("matches PRD trailer style", () => {
    expect(formatDuration(9_000)).toBe("9s");
    expect(formatDuration(48_211)).toBe("48s");
    expect(formatDuration(192_000)).toBe("3m12s");
    expect(formatDuration(3_600_000)).toBe("1h");
    expect(formatDuration(3_840_000)).toBe("1h4m");
    expect(formatDuration(412)).toBe("412ms");
    expect(formatDuration(60_000)).toBe("1m");
  });
});
