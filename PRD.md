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

**Built in this repo**, not as a hunk fork. Hunk (MIT) is a code quarry and a
reference implementation: we harvest its diff stack (Pierre integration, the
row-planning layer, theme model) and its session-daemon/agent-protocol ideas,
but the cockpit's information architecture (checks rail, log pager, land flow)
is its own product and owes hunk's "review-first diff viewer" definition
nothing. The perf work already proven in the fork (60fps pacing, DiffSurface
prototype, interaction-aware scheduling lessons) ports over as designs, not
as merge commits.

### Hard requirements

- **100k+ line changesets, smooth.** Scroll ticks < 2ms at any scroll distance
  and any stream size; first frame < 500ms regardless of changeset size.
  This mandates the immediate-mode `DiffSurface` architecture (prototype
  validated 2026-07-06: 0.26–0.43ms/tick on a 58k-row stream, scroll-distance-
  and size-invariant — see hunk fork `benchmarks/diff-surface-proto/NOTES.md`)
  plus **lazy per-file parse**: compute only per-file row *heights* up front
  (cheap patch-text line counting for exact spacers + scrollbar), Pierre-parse
  a file only when it approaches the viewport halo. Load is O(viewport),
  never O(changeset).
- **Live CI, inline.** Checks running in the engine stream into the checks
  rail: per-check spinner, elapsed time, last output line inline; full log one
  keystroke away (follow mode while running). Greenlight state = required set
  green for the current tree, recomputed as receipts mint and as the worktree
  changes.
- **One-motion land.** Green board → `l` → staleness dialog only if base
  moved → squash + trailers + push. The cockpit never blocks; it informs.
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

The cockpit imports the engine as a library (same Bun process) and consumes
the same event objects; `--events` exists so *any* front-end — including an
agent tailing progress — gets identical truth.

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

1. **Engine + CLI** (this PRD): Bun + TypeScript, `bun build --compile` single
   binary — same stack as the hunk fork so the cockpit imports the engine as a
   library, not a subprocess.
2. **Dogfood** in trip with agents minting receipts for a couple of weeks; the
   trust/workflow model is the real bet, validate it in parallel with cockpit
   work.
3. **Cockpit** (see above): built here — DiffSurface diff pane, checks rail,
   log pager, `land` action wired to this engine.

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
