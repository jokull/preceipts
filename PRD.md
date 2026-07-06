# receipts — local CI, proven by trees, merged by humans

*Working title: `receipts` (CLI: `receipts`). PRD v1 — 2026-07-06.*

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
  <blob-sha>` refs. Receipts reference their log blob. `receipts gc --keep 30d`
  prunes old log refs; receipt lines outlive their logs gracefully.
- **Sync** is `git push`/`fetch` of those refs. `receipts sync` wraps
  fetch + `notes merge` (cat_sort_uniq) + push; `receipts init` offers to add
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
  "config_hash": "d41d…"
}
```

`config_hash` fingerprints the check's definition so a changed command visibly
invalidates old receipts. `runner.agent` distinguishes human runs from agent
runs — receipts from agents are first-class, labeled, never privileged.

## Config: `.receipts.toml` (committed)

```toml
[check.typecheck]
run = "pnpm typecheck"
timeout = "10m"

[check.test]
run = "pnpm test"

[check.lint]
run = "pnpm exec oxlint"

[required]
checks = ["typecheck", "test", "lint"]   # what "green" means for `status`/`land`
```

## CLI surface (phase 1)

```
receipts run [check…]        # run in current worktree; mint against working-tree hash;
                             # warn (not fail) on dirty worktree; stream output live
receipts status [ref]        # receipt table for ref's tree vs required set; exit code
                             # reflects greenness (scriptable) but nothing enforces it
receipts log <check> [ref]   # stored log for that check/tree (tail of failures first)
receipts land <branch> [--onto main] [--no-push]
                             # squash via commit-tree, embed receipt trailers in the
                             # message, ff base ref, push. If base moved: state it,
                             # offer rebase-first or land-anyway. Never blocks.
receipts sync                # fetch + cat_sort_uniq merge + push of receipt refs
receipts gc [--keep 30d]     # prune log refs
receipts init                # write .receipts.toml stub, offer refspec config
--json everywhere            # agents and the future TUI consume the same output
```

Merge-commit trailers make receipts legible in plain `git log`:

```
Receipts: typecheck ✓ 48s · test ✓ 3m12s · lint ✓ 9s
Receipts-Tree: 8f3a…
Receipts-Runner: jokull@mbp.local (claude-code)
```

## Agent workflow (the point of all this)

Agent edits → `receipts run` → commits (tree now matches, receipts valid) →
pushes branch + receipts refs. Human opens the cockpit (phase 2) or runs
`receipts status branch`: green table, logs one keystroke away, `land` when
satisfied. "Queue-merge" is the agent looping: rebase → `run` → `land`.

## Out of scope (phase 1)

- Minting receipts for refs other than the current worktree (needs temp
  worktrees; revisit when agents want to prove branches they haven't checked out)
- Path-filtered checks for monorepos (`paths = ["packages/api/**"]`) — v2
- Signing — format reserves an optional `sig` field, nothing more
- The TUI cockpit — phase 2, lands in the hunk fork; the engine exposes
  `--json` and (later) a watch/daemon mode for it

## Phasing

1. **Engine + CLI** (this PRD): Bun + TypeScript, `bun build --compile` single
   binary — same stack as the hunk fork so the cockpit imports the engine as a
   library, not a subprocess.
2. **Dogfood** in trip with agents minting receipts for a couple of weeks; the
   trust/workflow model is the real bet, validate it before building UI.
3. **Cockpit**: hunk fork grows a checks rail, log pager, and `land` action
   wired to this engine.

## Unresolved questions

1. Name. `receipts` reads well as a concept ("show me the receipts") and as a
   CLI. Alternatives: `vouch`, `landed`, `greenlight`.
2. Log retention default (30d? size-capped?) and whether failure logs get
   longer retention than success logs (probably yes).
3. `land` when base moved: is "land anyway" allowed silently with a trailer
   noting staleness, or always interactive? (Agents will want a
   `--allow-stale` flag either way.)
4. Should `receipts run` auto-`sync` after minting, or is sync always explicit?
