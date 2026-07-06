import { PreceiptsError } from "./errors.ts";

export interface GitResult {
  code: number;
  stdout: string;
  stderr: string;
}

export interface GitOptions {
  cwd: string;
  env?: Record<string, string>;
  input?: string | Uint8Array;
}

/** Run `git` with the given args. Never throws on non-zero exit; returns the result. */
export async function git(args: string[], opts: GitOptions): Promise<GitResult> {
  const stdin =
    opts.input === undefined
      ? "ignore"
      : typeof opts.input === "string"
        ? new TextEncoder().encode(opts.input)
        : opts.input;
  const proc = Bun.spawn(["git", ...args], {
    cwd: opts.cwd,
    env: { ...process.env, ...opts.env },
    stdin,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { code, stdout, stderr };
}

/** Run `git` and throw a PreceiptsError (carrying git's stderr) on non-zero exit. */
export async function gitOut(args: string[], opts: GitOptions): Promise<string> {
  const result = await git(args, opts);
  if (result.code !== 0) {
    const detail = result.stderr.trim() || result.stdout.trim() || `exit ${result.code}`;
    throw new PreceiptsError(`git ${args[0] ?? ""} failed: ${detail}`);
  }
  return result.stdout;
}

/** Resolve the repo root for a cwd, or throw an honest "not a git repository" error. */
export async function repoRoot(cwd: string): Promise<string> {
  const result = await git(["rev-parse", "--show-toplevel"], { cwd });
  if (result.code !== 0) {
    throw new PreceiptsError(`not a git repository (or any parent): ${cwd}`);
  }
  return result.stdout.trim();
}

/** Resolve a revision to a full sha; returns null if it doesn't resolve. */
export async function revParse(root: string, rev: string): Promise<string | null> {
  const result = await git(["rev-parse", "--verify", "--quiet", rev], { cwd: root });
  if (result.code !== 0) return null;
  return result.stdout.trim();
}

/** True if the repo has a remote with this name. */
export async function hasRemote(root: string, name: string): Promise<boolean> {
  const result = await git(["remote"], { cwd: root });
  return result.code === 0 && result.stdout.split("\n").includes(name);
}

/**
 * Read many objects in one `git cat-file --batch` call.
 * Returns a map of sha -> content for objects that exist; missing shas are omitted.
 */
export async function catFileBatch(root: string, shas: string[]): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  if (shas.length === 0) return out;
  const result = await git(["cat-file", "--batch"], {
    cwd: root,
    input: shas.join("\n") + "\n",
  });
  if (result.code !== 0) {
    throw new PreceiptsError(`git cat-file --batch failed: ${result.stderr.trim()}`);
  }
  const buf = result.stdout;
  let pos = 0;
  while (pos < buf.length) {
    const nl = buf.indexOf("\n", pos);
    if (nl === -1) break;
    const header = buf.slice(pos, nl);
    pos = nl + 1;
    const parts = header.split(" ");
    const sha = parts[0];
    if (!sha) break;
    if (parts[1] === "missing") continue;
    const size = Number(parts[2]);
    if (!Number.isFinite(size)) break;
    out.set(sha, buf.slice(pos, pos + size));
    pos = pos + size + 1; // trailing newline after content
  }
  return out;
}

/** Store content as a blob in the object database; returns its sha. */
export async function hashObjectWrite(root: string, content: string | Uint8Array): Promise<string> {
  const result = await git(["hash-object", "-w", "--stdin"], { cwd: root, input: content });
  if (result.code !== 0) {
    throw new PreceiptsError(`git hash-object failed: ${result.stderr.trim()}`);
  }
  return result.stdout.trim();
}
