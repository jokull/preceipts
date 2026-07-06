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

1. ~~Name~~ — resolved: `preceipts` (PR + receipts).
2. ~~Check definition~~ — resolved: executable files in `.preceipts/checks/`,
   bash by default, no workflow DSL.
3. Log retention default (30d? size-capped?) and whether failure logs get
   longer retention than success logs (probably yes).
4. `land` when base moved: is "land anyway" allowed silently with a trailer
   noting staleness, or always interactive? (Agents will want a
   `--allow-stale` flag either way.)
5. Should `preceipts run` auto-`sync` after minting, or is sync always explicit?
