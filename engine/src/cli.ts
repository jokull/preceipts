#!/usr/bin/env bun
import { parseArgs } from "node:util";
import { CHECKS_DIR } from "./lib/config.ts";
import { formatDuration, parseDuration } from "./lib/duration.ts";
import { PreceiptsError } from "./lib/errors.ts";
import type { RunEvent } from "./lib/events.ts";
import { gc } from "./lib/gc.ts";
import { repoRoot } from "./lib/git.ts";
import { computeHud } from "./lib/hud.ts";
import { init } from "./lib/init.ts";
import { land, type StaleInfo } from "./lib/land.ts";
import { readLog, readReceipts } from "./lib/notes.ts";
import { latestByCheck, logBlobSha } from "./lib/receipt.ts";
import { runChecks } from "./lib/runner.ts";
import { computeStatus, resolveTree } from "./lib/status.ts";
import { sync } from "./lib/sync.ts";

const USAGE = `preceipts — local CI, proven by trees, merged by humans

Usage:
  preceipts init [--yes]                     scaffold .preceipts/, offer receipt refspecs on origin
  preceipts run [check…] [options]           prepare (format/codegen), then run checks and mint
                                             receipts against the normalized working tree; fails
                                             without minting if checks mutate the worktree
      --events                               stream NDJSON events on stdout
      --json                                 print a JSON summary at the end
      --sync                                 push receipts after minting
  preceipts status [ref] [--json]            receipt table for ref's tree (default: working tree); exit 0 iff green
  preceipts log <check> [ref]                print the stored log for that check/tree
  preceipts hud [--base main] [--json]       branch situational awareness: conflicts vs base,
                                             ahead/behind, land freshness, fetch age,
                                             unsynced receipts, worktree greenness
  preceipts land <branch> [options]          verify receipts, squash onto base, fast-forward, push
      --onto <base>                          base branch (default: main)
      --message <subject>                    squash commit subject
      --no-push                              skip pushing base + receipt refs
      --allow-stale                          land even if base moved (records Receipts-Stale trailer)
      --require-fresh                        exit non-zero if base moved
  preceipts sync                             fetch + cat_sort_uniq-merge + push receipt refs
  preceipts gc [--keep-success 30d] [--keep-failure 90d]
                                             prune old refs/receipts/logs/* refs

Receipts live in refs/notes/receipts, keyed by tree hash. PRECEIPTS_AGENT in the
environment labels receipts as agent-minted.
`;

function isInteractive(): boolean {
  return process.stdin.isTTY === true && process.stdout.isTTY === true;
}

const DIRTY_WARNING =
  "warning: worktree is dirty — this receipt becomes valid the moment you commit exactly this state.";

// ---------------------------------------------------------------------------

async function cmdInit(root: string, argv: string[]): Promise<number> {
  const { values } = parseArgs({
    args: argv,
    options: { yes: { type: "boolean", default: false } },
    allowPositionals: false,
  });
  const result = await init(root, {
    yes: values.yes,
    confirmRefspecs: isInteractive()
      ? async () =>
          confirm(
            "Add receipt refspecs to origin so ordinary push/fetch carries refs/notes/receipts and refs/receipts/logs/*?",
          )
      : undefined,
  });
  if (result.createdChecksDir) console.log(`created ${CHECKS_DIR}/`);
  if (result.createdConfig) console.log("created .preceipts/config.toml");
  if (!result.createdChecksDir && !result.createdConfig) console.log(".preceipts/ already present");
  if (result.addedRefspecs.length > 0) {
    console.log("added refspecs to remote.origin:");
    for (const refspec of result.addedRefspecs) console.log(`  ${refspec}`);
    console.log(
      "note: an explicit remote.origin.push list replaces git's push.default behavior;\n" +
        '"HEAD" was included so plain `git push` still pushes the current branch.',
    );
  } else if (result.hasOrigin) {
    console.log("refspecs unchanged (already configured, or declined — rerun with --yes to add)");
  } else {
    console.log("no origin remote — skipped refspec setup (rerun init after adding one)");
  }
  console.log(`\nnext: drop executable check scripts into ${CHECKS_DIR}/ and list the`);
  console.log("required ones under [required] checks in .preceipts/config.toml");
  return 0;
}

// ---------------------------------------------------------------------------

function summarizePaths(changed: string[]): string {
  const shown = changed.slice(0, 8).join(", ");
  const more = changed.length > 8 ? `, +${changed.length - 8} more` : "";
  return `${changed.length} file(s): ${shown}${more}`;
}

function makeStreamPrinter(): (event: RunEvent) => void {
  const partial = new Map<string, string>();
  const flushLines = (label: string, chunk: string) => {
    const buffered = (partial.get(label) ?? "") + chunk;
    const lines = buffered.split("\n");
    partial.set(label, lines.pop() ?? "");
    for (const line of lines) console.log(`[${label}] ${line}`);
  };
  const flushRest = (label: string) => {
    const rest = partial.get(label);
    if (rest) console.log(`[${label}] ${rest}`);
    partial.delete(label);
  };
  return (event) => {
    switch (event.event) {
      case "prepare-started":
        console.error(`▶ prepare: ${event.step}`);
        break;
      case "prepare-output":
        flushLines(`prepare:${event.step}`, event.chunk);
        break;
      case "prepare-finished": {
        flushRest(`prepare:${event.step}`);
        const mark = event.ok ? "✓" : "✗";
        console.error(
          `${mark} prepare: ${event.step} (exit ${event.exit}, ${formatDuration(event.duration_ms)})`,
        );
        break;
      }
      case "tree-normalized":
        console.error(`prepare normalized the worktree — ${summarizePaths(event.changed)}`);
        break;
      case "run-started":
        if (event.dirty) console.error(DIRTY_WARNING);
        break;
      case "worktree-changed":
        console.error(
          `✗ the worktree changed while checks ran (${event.before.slice(0, 12)} → ${event.after.slice(0, 12)}) — ${summarizePaths(event.changed)}`,
        );
        console.error(
          "  no receipts minted. If a check formats or generates code, move that command to [prepare] in .preceipts/config.toml — prepare runs before the receipt tree is computed.",
        );
        break;
      case "check-started":
        console.error(`▶ ${event.check}`);
        break;
      case "output":
        flushLines(event.check, event.chunk);
        break;
      case "check-finished": {
        flushRest(event.check);
        const mark = event.ok ? "✓" : "✗";
        console.error(`${mark} ${event.check} (exit ${event.exit}, ${formatDuration(event.duration_ms)})`);
        break;
      }
      case "receipt-minted":
        break;
    }
  };
}

async function cmdRun(root: string, argv: string[]): Promise<number> {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      sync: { type: "boolean", default: false },
      events: { type: "boolean", default: false },
      json: { type: "boolean", default: false },
    },
    allowPositionals: true,
  });

  const onEvent = values.events
    ? (event: RunEvent) => console.log(JSON.stringify(event))
    : makeStreamPrinter();

  const agent = process.env["PRECEIPTS_AGENT"];
  const result = await runChecks(root, {
    checks: positionals,
    ...(agent ? { agent } : {}),
    onEvent,
  });

  if (values.json) {
    console.log(
      JSON.stringify({
        tree: result.workingTree.tree,
        dirty: result.workingTree.dirty,
        ok: result.ok,
        receipts: result.receipts,
        prepareChanged: result.prepareChanged,
        invalidated: result.invalidated,
      }),
    );
  } else if (!values.events) {
    if (result.invalidated === null) {
      const passed = result.receipts.filter((r) => r.ok).length;
      console.error(
        `\n${passed}/${result.receipts.length} checks passed · receipts minted for tree ${result.workingTree.tree.slice(0, 12)}${result.workingTree.dirty ? " (dirty)" : ""}`,
      );
    }
    // The invalidated case already printed its explanation via the stream printer.
  }

  if (values.sync) {
    await sync(root);
    if (!values.events && !values.json) console.error("receipts synced with origin");
  }

  return result.ok ? 0 : 1;
}

// ---------------------------------------------------------------------------

async function cmdStatus(root: string, argv: string[]): Promise<number> {
  const { values, positionals } = parseArgs({
    args: argv,
    options: { json: { type: "boolean", default: false } },
    allowPositionals: true,
  });
  const ref = positionals[0] ?? null; // default: the working tree — the same tree `run` mints against
  const status = await computeStatus(root, ref);

  if (values.json) {
    console.log(JSON.stringify(status));
    return status.green ? 0 : 1;
  }

  console.log(`ref ${status.ref} → tree ${status.tree.slice(0, 12)}\n`);
  if (status.rows.length === 0) {
    console.log("no required checks configured and no receipts found for this tree");
  } else {
    const nameWidth = Math.max(...status.rows.map((row) => row.check.length), 5);
    for (const row of status.rows) {
      const mark =
        row.state === "ok" ? "✓" : row.state === "fail" ? "✗" : row.state === "missing" ? "∅" : "≠";
      const details = row.receipt
        ? `${formatDuration(row.receipt.duration_ms)} · ${row.receipt.started} · ${row.receipt.runner.email || row.receipt.runner.name}${row.receipt.runner.agent ? ` (${row.receipt.runner.agent})` : ""}`
        : "";
      const required = row.required ? "required" : "extra   ";
      console.log(
        `  ${mark} ${row.check.padEnd(nameWidth)}  ${required}  ${row.state.padEnd(16)}  ${details}`,
      );
    }
    console.log(status.green ? "\ngreen: required set is covered" : "\nnot green");
  }
  return status.green ? 0 : 1;
}

// ---------------------------------------------------------------------------

async function cmdLog(root: string, argv: string[]): Promise<number> {
  const { positionals } = parseArgs({ args: argv, options: {}, allowPositionals: true });
  const check = positionals[0];
  if (!check) throw new PreceiptsError("usage: preceipts log <check> [ref]");
  const ref = positionals[1] ?? "HEAD";

  const tree = await resolveTree(root, ref);
  const receipt = latestByCheck(await readReceipts(root, tree)).get(check);
  if (!receipt) {
    throw new PreceiptsError(`no receipt for check "${check}" on tree ${tree.slice(0, 12)} (${ref})`);
  }
  const blobSha = logBlobSha(receipt);
  if (!blobSha) throw new PreceiptsError(`receipt for "${check}" has no log reference`);
  const log = await readLog(root, blobSha);
  if (log === null) {
    throw new PreceiptsError(
      `log blob ${blobSha.slice(0, 12)} is gone (pruned by preceipts gc + git gc); the receipt itself survives`,
    );
  }
  process.stdout.write(log);
  return receipt.ok ? 0 : 1;
}

// ---------------------------------------------------------------------------

async function cmdHud(root: string, argv: string[]): Promise<number> {
  const { values } = parseArgs({
    args: argv,
    options: {
      base: { type: "string", default: "main" },
      json: { type: "boolean", default: false },
    },
    allowPositionals: false,
  });
  const hud = await computeHud(root, { base: values.base });

  if (values.json) {
    console.log(JSON.stringify(hud));
    return 0;
  }

  const merge =
    hud.mergeClean === true
      ? "merge clean"
      : hud.mergeClean === false
        ? `CONFLICTS (${hud.conflictFiles.length}): ${hud.conflictFiles.slice(0, 5).join(", ")}${hud.conflictFiles.length > 5 ? ", …" : ""}`
        : "merge: unknown";
  const fetch =
    hud.fetchAgeMs === null ? "never fetched" : `fetched ${formatDuration(hud.fetchAgeMs)} ago`;
  const receipts = hud.green ? "green" : "not green";
  const sync =
    hud.unsyncedReceipts === null
      ? "no origin"
      : hud.unsyncedReceipts === 0
        ? "receipts synced"
        : `${hud.unsyncedReceipts} unsynced receipt line(s)`;

  console.log(`${hud.branch ?? "(detached)"} vs ${hud.base} — ↑${hud.ahead} ↓${hud.behind}`);
  console.log(`  ${merge}`);
  console.log(
    `  land: ${hud.landFresh ? "fresh (base is the merge-base)" : "stale (base moved — squash tree would be unproven)"}`,
  );
  console.log(
    `  worktree ${hud.tree.slice(0, 12)}${hud.dirty ? " (dirty)" : ""} — receipts ${receipts}`,
  );
  console.log(`  ${fetch} · ${sync}`);
  return 0;
}

// ---------------------------------------------------------------------------

async function cmdLand(root: string, argv: string[]): Promise<number> {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      onto: { type: "string" },
      message: { type: "string" },
      "no-push": { type: "boolean", default: false },
      "allow-stale": { type: "boolean", default: false },
      "require-fresh": { type: "boolean", default: false },
      json: { type: "boolean", default: false },
    },
    allowPositionals: true,
  });
  const branch = positionals[0];
  if (!branch) throw new PreceiptsError("usage: preceipts land <branch> [--onto <base>]");

  const result = await land(root, branch, {
    ...(values.onto !== undefined ? { onto: values.onto } : {}),
    ...(values.message !== undefined ? { message: values.message } : {}),
    noPush: values["no-push"],
    allowStale: values["allow-stale"],
    requireFresh: values["require-fresh"],
    ...(isInteractive()
      ? {
          confirmStale: async (info: StaleInfo) =>
            confirm(
              `base moved since receipts were minted (${info.mergeBase.slice(0, 12)} → ${info.baseHead.slice(0, 12)}) — the squashed tree would be unproven.\nRebase & re-run is the honest path. Land anyway (records Receipts-Stale)?`,
            ),
        }
      : {}),
  });

  if (values.json) {
    console.log(
      JSON.stringify({
        branch: result.branch,
        base: result.base,
        commit: result.commit,
        tree: result.tree,
        stale: result.stale,
        pushed: result.pushed,
      }),
    );
  } else {
    console.log(`landed ${branch} → ${result.base} as ${result.commit.slice(0, 12)}`);
    console.log(result.message.trimEnd().split("\n").map((line) => `  ${line}`).join("\n"));
    console.log(result.pushed ? "pushed base + receipt refs to origin" : "not pushed");
  }
  return 0;
}

// ---------------------------------------------------------------------------

async function cmdSync(root: string, argv: string[]): Promise<number> {
  const { values } = parseArgs({
    args: argv,
    options: { json: { type: "boolean", default: false } },
    allowPositionals: false,
  });
  const result = await sync(root);
  if (values.json) console.log(JSON.stringify(result));
  else {
    console.log(
      `fetched receipt refs${result.merged ? ", merged remote receipts (cat_sort_uniq)" : ""}${result.pushed ? ", pushed local receipts" : ""}`,
    );
  }
  return 0;
}

// ---------------------------------------------------------------------------

async function cmdGc(root: string, argv: string[]): Promise<number> {
  const { values } = parseArgs({
    args: argv,
    options: {
      "keep-success": { type: "string", default: "30d" },
      "keep-failure": { type: "string", default: "90d" },
      json: { type: "boolean", default: false },
    },
    allowPositionals: false,
  });
  const result = await gc(root, {
    keepSuccessMs: parseDuration(values["keep-success"]),
    keepFailureMs: parseDuration(values["keep-failure"]),
  });
  if (values.json) console.log(JSON.stringify(result));
  else {
    console.log(
      `deleted ${result.deleted.length} log ref(s), kept ${result.kept}${result.orphans > 0 ? `, left ${result.orphans} orphan(s) alone` : ""}`,
    );
  }
  return 0;
}

// ---------------------------------------------------------------------------

export async function main(argv: string[]): Promise<number> {
  const [command, ...rest] = argv;
  if (!command || command === "help" || command === "--help" || command === "-h") {
    process.stdout.write(USAGE);
    return command ? 0 : 1;
  }

  const root = await repoRoot(process.cwd());
  switch (command) {
    case "init":
      return cmdInit(root, rest);
    case "run":
      return cmdRun(root, rest);
    case "status":
      return cmdStatus(root, rest);
    case "log":
      return cmdLog(root, rest);
    case "hud":
      return cmdHud(root, rest);
    case "land":
      return cmdLand(root, rest);
    case "sync":
      return cmdSync(root, rest);
    case "gc":
      return cmdGc(root, rest);
    default:
      console.error(`unknown command "${command}"\n`);
      process.stderr.write(USAGE);
      return 1;
  }
}

if (import.meta.main) {
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (error) => {
      if (error instanceof PreceiptsError) {
        console.error(`preceipts: ${error.message}`);
        process.exit(error.exitCode);
      }
      console.error(error);
      process.exit(1);
    },
  );
}
