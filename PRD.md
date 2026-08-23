# preceipts — local CI, proven by trees, merged by humans

*Name: `preceipts` (CLI: `preceipts`). PRD v1 — 2026-07-06.*

## Problem

Teams (and agent-heavy solo devs) that don't want remote CI gating merges still
want *evidence*: which checks ran against exactly this code, when, by whom, with
what output. GitHub's PR flow couples evidence to a CI server and couples merge
to that evidence. We want the inverse: checks run on dev machines where the
environment already exists, results travel *through the repo itself*, and merge
is a human (or agent) decision informed by receipts — never blocked by them.

## Philosophy

- **Honor system.** Receipts are assertions, not proofs. No signing, no
  verification theater. Runner identity is recorded for context, not audit.
- **Git, not GitHub.** No forge dependency. The review unit is a branch vs a
  base ref. Merging is local git plumbing plus a push. Anything fancier
  (queue-merge, batching) is scripted by the user or their agent on top of the
  primitives.
- **The worktree you have.** Checks run in the current worktree — node_modules,
  .env, caches all warm. No sandboxes, no clean clones.

## The core mechanic: tree-keyed receipts

A receipt attaches to a **git tree hash**, not a commit hash.

Why this is load-bearing:

1. **History rewrites don't invalidate work.** Amend the commit message, rebase
   without conflicts, reorder commits — same tree, receipts still valid.
2. **Squash-merge validity is a tree-equality check.** A local squash of branch
   onto base (`git commit-tree <branch-tree> -p <base>`) produces a commit whose
   tree *is* the branch tree — receipts provably describe the merged result —
   **iff base hasn't moved past the merge-base**. If base moved, the post-merge
   tree is new and unproven. The tool surfaces this truthfully ("base moved
   since receipts were minted — rebase & re-run, or land anyway") and never
   blocks.
3. **Dirty worktrees get a real key.** The runner computes the working tree's
   hash via a temporary index (`GIT_INDEX_FILE=tmp git read-tree HEAD && git
   add -A && git write-tree`). Clean worktree → equals HEAD's tree. Dirty
   worktree → the receipt is minted against the *actual state that was tested*,
   with a warning: "worktree is dirty — this receipt becomes valid the moment
   you commit exactly this state." Run checks first, commit second, waste
   nothing.

## Storage: git-native, no server

- **Receipts** live in `refs/notes/receipts` — git notes annotating **tree
  objects** (notes plumbing accepts any object sha). One note per tree, one
  JSON line per receipt. Concurrent minting from multiple machines merges
  losslessly with the `cat_sort_uniq` notes-merge strategy.
- **Logs** are content-addressed blobs kept reachable via `refs/receipts/logs/
  <blob-sha>` refs. Receipts reference their log blob. `preceipts gc --keep 30d`
  prunes old log refs; receipt lines outlive their logs gracefully.
- **Sync** is `git push`/`fetch` of those refs. `preceipts sync` wraps
  fetch + `notes merge` (cat_sort_uniq) + push; `preceipts init` offers to add
  the refspecs to the remote config so normal pushes carry receipts.

### Receipt line (JSONL)

```json
{
  "v": 1,
  "check": "typecheck",
  "cmd": "pnpm typecheck",
  "tree": "8f3a…",
  "ok": true,
  "exit": 0,
  "started": "2026-07-06T12:40:11Z",
  "duration_ms": 48211,
  "runner": { "name": "Jökull Sólberg", "email": "jokull@…", "host": "mbp.local", "agent": "claude-code/2.x" },
  "dirty": false,
  "log": "blob:1c9e…",
  "check_blob": "9a1f…"
}
```

`check_blob` records the blob sha of the check's script (see below), so a
changed check definition is visibly distinguishable from an unchanged one.
`runner.agent` distinguishes human runs from agent runs — receipts from agents
are first-class, labeled, never privileged.

## Checks: executable files, not a workflow DSL

We deliberately do **not** mirror GitHub Actions YAML. A GHA workflow is mostly
environment setup (checkout, toolchain, dependency install, cache) around a few
`run:` lines — and locally the environment already exists; that's the premise
of the tool. What remains of a workflow *is* a shell script. So checks use the
git-hooks model:

```
.preceipts/
  checks/
    typecheck        # executable; shebang decides interpreter (bash by default)
    test
    lint
  config.toml        # the little that isn't the script itself
```

- **A check is any executable file.** Filename = check name. Exit 0 = pass.
  Bash by default, but a shebang can point at bun, python, anything — no DSL,
  no emulation layer, nothing to learn. Agents read and write these natively.
- **Contract:** cwd = repo root; stdout/stderr captured and streamed;
  `PRECEIPTS_CHECK`, `PRECEIPTS_TREE` provided in env. Checks run in parallel
  by default (they're independent scripts, same as parallel GHA jobs).
- **The tree hash covers the check definition for free.** Because
  `.preceipts/checks/*` is committed, the working-tree hash a receipt is keyed
  to *already includes the exact script that ran*. Edit a check and every prior
  receipt stops matching current trees automatically — no invalidation logic,
  it falls out of content addressing. The receipt additionally records the
  script's blob sha (`check_blob`) so tooling can say "check definition
  changed" rather than just "no receipt".
- **GHA migration** is copying the `run:` lines into a script. A `preceipts
  import` scaffolder that does this mechanically from `.github/workflows/` is
  a v2 nicety, not a dependency.

`config.toml` stays minimal — only what isn't expressible as the script itself:

```toml
[required]
checks = ["typecheck", "test", "lint"]   # what "green" means for `status`/`land`

[check.test]
timeout = "15m"                           # default 10m
```

### The prepare phase: normalization is orchestration, not a gate

Formatting and codegen cannot be ordinary checks. A check (or a hook it
triggers) that rewrites files *after* the receipt tree is computed makes every
receipt a lie about the eventual commit: checks proved tree A, the commit is
tree B, and the gate correctly says "missing receipts". Discovered by
dogfooding in trip — oxfmt in a pre-commit hook busted a run's receipts. The
fix is first-class, not an agent instruction: **the tool makes the correct
path the easy path.**

```toml
[prepare]
commands = ["format", "sync-prompt"]      # serial, in this order

[prepare.format]
cmd = "pnpm format"                        # bash -c, from the repo root

[prepare.sync-prompt]
cmd = "pnpm --filter @trip/copilot sync-prompt"
timeout = "5m"                             # default 10m, same as checks
```

`preceipts run` is therefore: **prepare (serial) → compute the receipt tree →
checks (parallel) → verify the tree held still → mint.** Concretely:

1. Prepare commands run serially; mutating the worktree here is the point.
2. If they changed files, the changed paths are reported
   (`tree-normalized` event; "prepare normalized the worktree — N file(s)").
3. The receipt tree is computed *after* prepare (`run-started` event).
4. Checks run against that normalized tree.
5. The tree is recomputed when checks finish. If it changed — a check mutated
   the worktree, or files were edited mid-run — the run is **invalid**:
   nothing is minted (receipts would lie about the before *and* after trees),
   exit is non-zero, and the message says to move mutating commands to
   `[prepare]` (`worktree-changed` event).
6. Receipts mint only for the verified-stable tree — minting is deferred to
   the end of the run, never per-check.

So `format --write` is prepare (local-CI orchestration); `format:check` is a
check (a CI-style gate). A failing prepare step aborts the run before any
check: an unnormalizable worktree has no honest tree to mint against. A
half-configured `[prepare]` (listed name without a table, or a table not
listed in `commands`) is an error, never a silent skip.

## CLI surface (phase 1)

```
preceipts run [check…]        # run in current worktree; mint against working-tree hash;
                             # warn (not fail) on dirty worktree; stream output live
preceipts status [ref]        # receipt table for ref's tree vs required set; exit code
                             # reflects greenness (scriptable) but nothing enforces it
preceipts log <check> [ref]   # stored log for that check/tree (tail of failures first)
preceipts hud [--base main]   # one JSON payload of branch situational awareness —
                             # conflicts vs base, ahead/behind, land freshness,
                             # fetch age, unsynced receipts, worktree greenness
preceipts land <branch> [--onto main] [--no-push]
                             # squash via commit-tree, embed receipt trailers in the
                             # message, ff base ref, push. If base moved: state it,
                             # offer rebase-first or land-anyway. Never blocks.
preceipts sync                # fetch + cat_sort_uniq merge + push of receipt refs
preceipts gc [--keep 30d]     # prune log refs
preceipts init                # write .preceipts/ stub, offer refspec config
--json everywhere            # agents and the future TUI consume the same output
```

Merge-commit trailers make receipts legible in plain `git log`:

```
Receipts: typecheck ✓ 48s · test ✓ 3m12s · lint ✓ 9s
Receipts-Tree: 8f3a…
Receipts-Runner: jokull@mbp.local (claude-code)
```

## Agent workflow (the point of all this)

Agent edits → `preceipts run` → commits (tree now matches, receipts valid) →
pushes branch + receipts refs. Human opens the cockpit (phase 2) or runs
`preceipts status branch`: green table, logs one keystroke away, `land` when
satisfied. "Queue-merge" is the agent looping: rebase → `run` → `land`.

## The cockpit (phase 2 — the product)

The end state is a TUI that feels like a flight deck: the diff is the main
stream, checks run live beside it, and greenlight-to-land is one motion.

**Built as a lumen fork** (this repo; upstream jnsahaj/lumen, MIT, Rust +
ratatui). Decided 2026-07-06 after hands-on evaluation: lumen already ships
the commodity half of the cockpit — GitHub-PR-style split view with file tree,
tree-sitter highlighting, `/` search with n/N, watch mode, stacked commits,
PR integration, and an annotate-with-`i` → export-to-stdout agent loop — and
its ratatui immediate-mode rendering is the same architecture class the
DiffSurface prototype validated (opened a 166-file/41k-line diff instantly).
What we add is the moat: the receipts rail, live check runs, the HUD footer,
and one-motion land — all speaking to `preceipts-engine` via `--json` and
`--events`. The earlier plan (own OpenTUI/DiffSurface pane; before that, a
hunk fork) is superseded; hunk and the DiffSurface prototype remain reference
material for perf work if lumen's rendering ever needs it.

### Hard requirements

- **100k+ line changesets, smooth.** Scroll ticks < 2ms at any scroll distance
  and any stream size; first frame < 500ms regardless of changeset size.
  Immediate-mode rendering (ratatui) satisfies the architecture mandate the
  DiffSurface prototype established (0.26–0.43ms/tick on a 58k-row stream,
  scroll-distance- and size-invariant — hunk fork
  `benchmarks/diff-surface-proto/NOTES.md`); verified on a real 41k-line
  changeset at adoption time. If lumen's load path ever turns O(changeset),
  the lazy per-file parse design (heights up front, parse near the viewport
  halo) is the fix to port.
- **Live CI, inline.** Checks running in the engine stream into the checks
  rail: per-check spinner, elapsed time, last output line inline; full log one
  keystroke away (follow mode while running). Greenlight state = required set
  green for the current tree, recomputed as receipts mint and as the worktree
  changes.
- **View + monitor, never mutate** (revised 2026-07-06, superseding
  "one-motion land"). The cockpit is the coding agent's companion: the human
  reviews the diff and watches receipts; the *agent* lands via
  `preceipts land` (engine CLI). No land keybinding, no AI commands in the
  TUI or `--help` — scope is scroll, search, monitor, run checks.
- **Always current.** Watch mode is the default (`--no-watch` to opt out):
  the diff reloads on file changes — off-thread, so a multi-second reload of
  a big changeset never freezes the UI while an agent is editing. HUD and
  receipt snapshots also refresh on a timer (status ~5s, HUD ~15s), because
  receipts change with no worktree event at all (an agent minting in another
  terminal, a sync, the base moving).
- **PR diff by default, one key to switch.** Bare `preceipts` opens the PR
  view — merge-base(origin/main | main | …/master, HEAD) → working tree —
  because the unit of review is the branch, not the last save. `t` toggles
  to uncommitted-only ("what did the agent just do?") and back.
- **Vim-grammar navigation, `/` search first.** `/` opens incremental content
  search over the *entire* changeset (all files, not just mounted rows —
  search runs against the row plan, so it works with virtualized/lazy-parsed
  content); smartcase; literal substring v1; `n`/`N` next/prev with wrap
  indicator; match count ("3/47") in the status bar; all matches highlighted,
  current match distinct; Esc restores the pre-search scroll position.
  Searching a 100k-line changeset must stay under ~50ms per keystroke.

### Engine ↔ cockpit interface

`preceipts run --events` emits NDJSON on stdout as checks execute:

```
{"event":"check-started","check":"test","tree":"8f3a…","ts":…}
{"event":"output","check":"test","chunk":"PASS src/core/…\n"}
{"event":"check-finished","check":"test","ok":true,"exit":0,"duration_ms":…}
{"event":"receipt-minted","check":"test","tree":"8f3a…","log":"blob:…"}
```

The cockpit (Rust) shells out to `preceipts-engine` and consumes these events
over stdout; `--events` and `--json` are the whole contract, so *any*
front-end — including an agent tailing progress — gets identical truth. The
`preceipts` binary also passes engine subcommands straight through
(`preceipts run` == `preceipts-engine run`), so there is one entry point.

### The footer HUD

A persistent one-line (expandable) footer answering "what happens if I land
right now?" — the situational awareness GitHub's PR page spreads across five
widgets. Engine side it is `preceipts hud [--base main] --json`: one payload,
consumed identically by the TUI footer, shell prompts, and agents.

Fields (all computed read-only; nothing touches the worktree or index):

- **Merge cleanliness** vs the base — `git merge-tree --write-tree` (in-memory
  merge, git ≥ 2.38): clean / conflicted, with the conflicted file list.
  The base is the *remote-tracking* ref when it exists (`origin/main`), local
  branch otherwise — conflicts with where main actually is, not a stale local.
- **Ahead/behind** the base (`rev-list --left-right --count`).
- **Land freshness** — would the squash commit's tree be the proven tree?
  True iff the base head *is* the merge-base (same predicate `land` uses).
- **Fetch age** — how stale is our picture of the remote (FETCH_HEAD mtime;
  null = never fetched). A "clean merge" verdict against a week-old
  origin/main is worth flagging.
- **Unsynced receipts** — count of local receipt lines origin doesn't have
  yet (local notes vs the remote-tracking notes ref). Receipts that never
  synced are receipts teammates can't see.
- **Worktree coherence** — the working-tree hash, dirty flag, and the full
  status table (rows + green) for that tree: do receipts speak to what's on
  disk *right now*?

The HUD never blocks anything (philosophy: inform, don't gate). The TUI
recomputes it on filesystem/ref changes; the fetch itself stays a deliberate
user/agent action — `hud` reports staleness, it doesn't network.

## Out of scope (phase 1)

- Minting receipts for refs other than the current worktree (needs temp
  worktrees; revisit when agents want to prove branches they haven't checked out)
- Path-filtered checks for monorepos (`paths = ["packages/api/**"]`) — v2
- Signing — format reserves an optional `sig` field, nothing more

## Phasing

1. **Engine + CLI** (done): Bun + TypeScript in `engine/`, `bun build
   --compile` single binary `preceipts-engine`. The `--json`/`--events`
   surface is the cockpit contract.
2. **Dogfood** in trip with agents minting receipts for a couple of weeks; the
   trust/workflow model is the real bet, validate it in parallel with cockpit
   work.
3. **Cockpit** (see above): lumen fork at the repo root — add the receipts
   rail, live check runs (`run --events`), log pager, HUD footer, and `land`,
   all via `preceipts-engine`.

## Decisions log

1. **Name**: `preceipts` (PR + receipts).
2. **Check definition**: executable files in `.preceipts/checks/`, bash by
   default, no workflow DSL.
3. **Log retention**: success logs 30d, failure logs 90d (`gc` defaults,
   configurable). Stored logs capped at ~1MB as head 64KB + tail (the failure
   tail is the valuable part). Receipt lines are permanent; they outlive logs.
4. **Stale `land`**: interactive prompt on a TTY; `--allow-stale` proceeds
   non-interactively and records a `Receipts-Stale: base moved <old>→<new>`
   trailer so the history is honest. `--require-fresh` exits non-zero instead,
   for scripts that want failure.
5. **Sync**: explicit (`preceipts sync`); `run --sync` opts into push-after-
   mint; `init` offers to add receipt refspecs to the remote so ordinary
   `git push` carries receipts.
6. **HUD**: read-only and network-free — it reports fetch staleness rather
   than fetching; the base for conflict/freshness questions is the
   remote-tracking ref when present, the local branch otherwise.
7. **Cockpit = lumen fork, one repo, one name** (2026-07-06). This repo is a
   fork of jnsahaj/lumen (github.com/jokull/preceipts) with the engine folded
   in at `engine/` (history preserved via subtree). preceipts is a superset of
   lumen: its diff cockpit plus receipts. Binaries: `preceipts` (Rust cockpit;
   bare invocation on a TTY opens the diff view; engine subcommands exec
   through to the engine) and `preceipts-engine` (Bun-compiled, discovered on
   PATH or via `PRECEIPTS_ENGINE`). The Cargo *package* keeps upstream's name
   to minimize merge friction; only the `[[bin]]` is renamed. Earlier plans —
   hunk fork, then an own OpenTUI DiffSurface pane — are superseded (prototype
   evidence retained in the hunk fork as reference).

8. **Prepare phase** (2026-07-06, from trip dogfooding): format/codegen is
   `[prepare]` — serial commands run before the receipt tree is computed,
   with changed paths reported. Checks that mutate the worktree invalidate
   the run (no receipts minted, honest error pointing at `[prepare]`);
   minting is deferred until the tree is verified stable across the run.
9. **Cockpit scope: view + monitor** (2026-07-06, user decision). The TUI is
   for reviewing the diff and monitoring receipts; landing is the coding
   agent's job through the engine CLI. Lumen's AI commands and flags are
   hidden from `--help` (still compiled, minimizing upstream divergence).
   Defaults: PR diff scope, watch on, async reloads, timer-refreshed
   HUD/status. `t` toggles PR ⇄ uncommitted scope.10. **Desktop app, macOS-native, GPUI** (2026-07-07, user decision). The
    cockpit moves out of the TUI into a macOS desktop app: GPUI shell over a
    UI-agnostic `preceipts-core` crate (gix, imara-diff, tree-sitter, watch);
    engine boundary unchanged. One scroll surface (Zed-style multibuffer, not
    per-file panes). Vim-style keys dropped for macOS conventions (⌘F search,
    ⌘P quick open, ⌘K palette, real menus). The HUD is rethought as app
    chrome (statusbar chips + titlebar), extended with GitHub via OAuth
    device flow + Keychain (PR association, mergeability, CI rollup —
    optional, degrades to local-only). VS Code chrome is the layout
    reference: file tree with actions/filters, bottom receipts panel, easy
    dirty ⇄ branch-diff scope toggle. Foundations research in
    docs/desktop-foundations.md; full design in docs/desktop-app-design.md.
11. **All-native Swift app — no Rust core** (2026-07-07, user decision
    after an architecture rethink). The desktop app must feel native, and
    once the UI is Swift the Rust core inverts from convenience into a
    permanent FFI tax: the heavyweight dependencies (libgit2, tree-sitter)
    are C libraries Swift consumes first-class, and the novel logic
    (similarity pairing, word-level intraline, segment composition, row
    model) is ~500 tested lines that port in a day. So the Swift/AppKit
    app owns UI *and* compute: libgit2 in-process for reads + line diffs
    (patience + indent-heuristic — same xdiff lineage as histogram),
    SwiftTreeSitter for highlighting, Swift ports of the display
    algorithm with the same test suite, FSEvents watching, Keychain +
    URLSession for GitHub. `preceipts-engine` (TS/Bun) is unchanged
    behind its subprocess boundary (receipts/runs/hud as JSON/NDJSON).
    The Rust workspace — TUI cockpit, preceipts-core, and the GPUI app
    (decision 10) — is deprecated reference code until the Swift app
    reaches parity, then removed. An intermediate "Swift shell over a
    Rust-core C FFI" design was considered the same day and dropped
    before any code shipped. Rationale in docs/desktop-app-design.md.
12. **Product pivot: the AI-coding workbench; Rust + GPUI; procpane
    absorbed** (2026-08-22, user decision). preceipts becomes the
    workbench *around* AI coding and explicitly never hosts it — no
    terminal pane, no agent harness; the user drives their own agent.
    **Adoption, not creation, is the primitive:** any worktree of a
    registered project is adopted as a workspace whether it came from
    the CLI (`preceipts new`, the default path — no handoff needed
    because the user is already in a terminal), the app (creates, then
    hands off to the user's terminal and stops caring), or an agent
    running `git worktree add` on its own (detected via FSEvents on
    `.git/worktrees/`). **No capability may depend on a specific harness
    or model vendor**: the full loop runs on observation alone —
    worktree detection, file-write activity, quiet debounce, git, and
    checks in the live env — for harnesses that have never heard of
    preceipts. The CLI verbs (`adopt`, `intent`, `check`, `release`) are
    a public interface anything may call to trade a heuristic for an
    exact signal; harness-specific lifecycle adapters are contributed,
    not designed in. Branch naming is a local slug by default; any model
    call is optional, non-blocking, vendor-neutral, and never shipped
    with a key. **Sandbox rails are pluggable, never written here:** the
    URL fabric treats a service as "an address plus a healthcheck," so
    the runtime is per-project — `native` (default, no VM, isolation by
    port/hostname/DB namespacing), `confined` (Seatbelt profile derived
    from the worktree plus egress allowlist enforced by our own proxy,
    kept behind a trait because `sandbox-exec` is deprecated with no
    replacement), or `container` (delegated to Apple `container` first —
    Apache 2.0, per-container VM, own IP on macOS 26 — then OrbStack and
    Docker/Lima through the same adapter). We do not build a VMM.
    Projects author their environment in one `preceipts.toml` with
    progressive disclosure — zero config by detection, up to trip's
    kitchen sink. The schema owns what the app and receipts must
    understand: services, health, per-service runtime, env **policy**
    (masked dotenvs, Keychain by reference, test-key assertions that
    fail the boot), captured env from service output, a composite
    readiness gate, provider mocks, **drains** (email outbox, analytics
    ledger, local error sink, HTTP transcript), a **fidelity** map
    (`local-real | local-simulated | mocked | disabled |
    remote-required`, adopted from trip), seed bundles and radii, and
    checks. Everything domain-specific is a declared `[actions.*]` verb,
    surfaced automatically as a CLI command, an MCP tool, and a panel
    button. **Receipts record the fidelity map of the environment they
    were minted in** — tree hash plus fidelity is the honest proof.
    Benchmark: trip's sandbox must be expressible as one
    `preceipts.toml` plus fixtures, with Docker optional.
    Scope: tab per project, worktree workspaces carrying a genesis
    prompt, per-workspace dev environments with subdomain URLs, a fast
    diff and tree browser, and receipts as pre-CI signal fired
    automatically when the worktree goes quiet. The PR conversation
    surface (~3.5k LOC) is deleted. Decision 11 is reversed: the UI
    returns to GPUI/gpui-component and the system unifies on Rust — one
    cargo workspace, `preceipts-core` in-process for git/diff/highlight/
    watch, `preceiptsd` for processes, healthchecks, TLS proxy, secrets,
    and check runs, `preceipts` CLI on the same socket. The TS/Bun
    engine is retired: receipt reads move into core, writes into the
    daemon. procpane's `Workspace` is renamed `Project`; every keyed
    resource gains a workspace dimension. Algorithms port forward from
    `PreceiptsKit` (27 XCTests as the conformance suite), not backward
    from the deleted Rust core. The workspace is a **laboratory**: every
    instrument the UI shows (logs, health, URLs, HTTP transcript, diff,
    receipts, process control) is equally available to agents through
    the CLI and an MCP server (open protocol, no vendor assumed), and
    a worktree self-identifies via
    `PRECEIPTS_WORKSPACE` so any harness started inside it locates the
    lab with no configuration. macOS integration is first-class:
    data-protection Keychain with a shared access group across signed
    app and daemon (retiring procpane's open-ACL workaround),
    `SMAppService` LaunchAgent, `/etc/resolver/test` + an in-daemon DNS
    responder replacing `/etc/hosts` (which cannot express per-workspace
    subdomains), per-workspace leaf certs, Touch ID for secret reveal,
    notarized bundle. Accepted cost: GPUI draws its own widgets, so
    native feel — text selection, IME, VoiceOver, scroll physics — is
    built and budgeted rather than inherited from AppKit.
    Full design, and the sequencing it was built to,
    in docs/direction-2026-08.md.
