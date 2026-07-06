import { readdir, stat } from "node:fs/promises";
import { join } from "node:path";
import { parseDuration } from "./duration.ts";
import { PreceiptsError } from "./errors.ts";

export const PRECEIPTS_DIR = ".preceipts";
export const CHECKS_DIR = join(PRECEIPTS_DIR, "checks");
export const CONFIG_FILE = join(PRECEIPTS_DIR, "config.toml");

export const DEFAULT_TIMEOUT_MS = 10 * 60 * 1000;

export interface Config {
  /** Check names that must be green for `status`/`land`. */
  required: string[];
  /** Per-check timeout overrides in ms. */
  timeouts: Record<string, number>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Load `.preceipts/config.toml`. Throws honestly if `.preceipts/` is missing. */
export async function loadConfig(root: string): Promise<Config> {
  const dir = join(root, PRECEIPTS_DIR);
  const dirStat = await stat(dir).catch(() => null);
  if (!dirStat?.isDirectory()) {
    throw new PreceiptsError(
      `no ${PRECEIPTS_DIR}/ directory in ${root} — run \`preceipts init\` first`,
    );
  }
  const configPath = join(root, CONFIG_FILE);
  const file = Bun.file(configPath);
  if (!(await file.exists())) return { required: [], timeouts: {} };

  let parsed: unknown;
  try {
    parsed = Bun.TOML.parse(await file.text());
  } catch (error) {
    throw new PreceiptsError(`cannot parse ${CONFIG_FILE}: ${String(error)}`);
  }
  if (!isRecord(parsed)) return { required: [], timeouts: {} };

  const required: string[] = [];
  const requiredSection = parsed["required"];
  if (isRecord(requiredSection) && Array.isArray(requiredSection["checks"])) {
    for (const name of requiredSection["checks"]) {
      if (typeof name === "string") required.push(name);
    }
  }

  const timeouts: Record<string, number> = {};
  const checkSection = parsed["check"];
  if (isRecord(checkSection)) {
    for (const [name, value] of Object.entries(checkSection)) {
      if (isRecord(value) && typeof value["timeout"] === "string") {
        timeouts[name] = parseDuration(value["timeout"]);
      }
    }
  }

  return { required, timeouts };
}

export interface CheckFile {
  name: string;
  /** Absolute path to the script. */
  path: string;
  executable: boolean;
}

/** List check scripts in `.preceipts/checks/` (regular files only, sorted by name). */
export async function listChecks(root: string): Promise<CheckFile[]> {
  const dir = join(root, CHECKS_DIR);
  const entries = await readdir(dir, { withFileTypes: true }).catch(() => null);
  if (entries === null) {
    throw new PreceiptsError(`no ${CHECKS_DIR}/ directory in ${root} — run \`preceipts init\` first`);
  }
  const checks: CheckFile[] = [];
  for (const entry of entries) {
    if (!entry.isFile() && !entry.isSymbolicLink()) continue;
    if (entry.name.startsWith(".")) continue;
    const path = join(dir, entry.name);
    const info = await stat(path).catch(() => null);
    if (!info?.isFile()) continue;
    checks.push({ name: entry.name, path, executable: (info.mode & 0o111) !== 0 });
  }
  checks.sort((a, b) => a.name.localeCompare(b.name));
  return checks;
}
