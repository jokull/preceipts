import { readdir, stat } from "node:fs/promises";
import { join } from "node:path";
import { parseDuration } from "./duration.ts";
import { PreceiptsError } from "./errors.ts";

export const PRECEIPTS_DIR = ".preceipts";
export const CHECKS_DIR = join(PRECEIPTS_DIR, "checks");
export const CONFIG_FILE = join(PRECEIPTS_DIR, "config.toml");

export const DEFAULT_TIMEOUT_MS = 10 * 60 * 1000;

export interface PrepareStep {
  name: string;
  /** Shell command, run via `bash -c` from the repo root. */
  cmd: string;
  timeoutMs: number;
}

export interface Config {
  /** Check names that must be green for `status`/`land`. */
  required: string[];
  /** Per-check timeout overrides in ms. */
  timeouts: Record<string, number>;
  /**
   * Normalization commands (format, codegen) run serially BEFORE the receipt
   * tree is computed. Mutating the worktree here is expected; a check that
   * mutates it invalidates the whole run instead.
   */
  prepare: PrepareStep[];
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
  if (!(await file.exists())) return { required: [], timeouts: {}, prepare: [] };

  let parsed: unknown;
  try {
    parsed = Bun.TOML.parse(await file.text());
  } catch (error) {
    throw new PreceiptsError(`cannot parse ${CONFIG_FILE}: ${String(error)}`);
  }
  if (!isRecord(parsed)) return { required: [], timeouts: {}, prepare: [] };

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

  const prepare = parsePrepare(parsed["prepare"]);

  return { required, timeouts, prepare };
}

/**
 * [prepare] commands = ["format", …] with a [prepare.<name>] table per step.
 * The commands list is the (serial) execution order and the source of truth:
 * a listed name without a table, or a table without a listing, is an error —
 * a half-configured normalization step should never fail silently.
 */
function parsePrepare(section: unknown): PrepareStep[] {
  if (section === undefined) return [];
  if (!isRecord(section)) {
    throw new PreceiptsError(`[prepare] in ${CONFIG_FILE} must be a table`);
  }
  const commandsRaw = section["commands"];
  const names: string[] = [];
  if (commandsRaw !== undefined) {
    if (!Array.isArray(commandsRaw)) {
      throw new PreceiptsError(`[prepare].commands in ${CONFIG_FILE} must be an array of step names`);
    }
    for (const name of commandsRaw) {
      if (typeof name !== "string") {
        throw new PreceiptsError(`[prepare].commands entries in ${CONFIG_FILE} must be strings`);
      }
      names.push(name);
    }
  }

  const steps: PrepareStep[] = [];
  for (const name of names) {
    const table = section[name];
    if (!isRecord(table) || typeof table["cmd"] !== "string" || table["cmd"].trim() === "") {
      throw new PreceiptsError(
        `prepare step "${name}" is listed in [prepare].commands but has no [prepare.${name}] table with a cmd string`,
      );
    }
    const timeoutMs =
      typeof table["timeout"] === "string" ? parseDuration(table["timeout"]) : DEFAULT_TIMEOUT_MS;
    steps.push({ name, cmd: table["cmd"], timeoutMs });
  }

  for (const key of Object.keys(section)) {
    if (key === "commands") continue;
    if (!names.includes(key)) {
      throw new PreceiptsError(
        `[prepare.${key}] is defined in ${CONFIG_FILE} but "${key}" is not listed in [prepare].commands — add it there (order matters) or remove the table`,
      );
    }
  }

  return steps;
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
