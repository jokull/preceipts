import { PreceiptsError } from "./errors.ts";

const UNIT_MS: Record<string, number> = {
  ms: 1,
  s: 1000,
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
};

/**
 * Parse a human duration like "30d", "12h", "10m", "90s", "1h30m" into milliseconds.
 * Throws on anything it doesn't understand.
 */
export function parseDuration(input: string): number {
  const text = input.trim();
  const re = /(\d+(?:\.\d+)?)(ms|s|m|h|d)/gy;
  let total = 0;
  let matchedLength = 0;
  for (const match of text.matchAll(re)) {
    const value = Number(match[1]);
    const unit = UNIT_MS[match[2] ?? ""];
    if (unit === undefined) break;
    total += value * unit;
    matchedLength += match[0].length;
  }
  if (matchedLength !== text.length || text.length === 0) {
    throw new PreceiptsError(
      `cannot parse duration "${input}" (expected forms like 30d, 12h, 15m, 90s, 1h30m)`,
    );
  }
  return Math.round(total);
}

/** Format a millisecond duration the way the PRD trailers do: 9s, 48s, 3m12s, 1h4m. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  const totalSeconds = Math.round(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h${minutes > 0 ? `${minutes}m` : ""}`;
  if (minutes > 0) return `${minutes}m${seconds > 0 ? `${seconds}s` : ""}`;
  return `${seconds}s`;
}
