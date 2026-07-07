# preceipts

**Local CI, proven by trees, merged by humans.**

preceipts replaces the GitHub PR treadmill for teams that trust each other:
checks run on dev machines, results ("receipts") are keyed to git **tree
hashes** and stored in git notes, and landing a branch is a human/agent
decision informed by receipts — never gated by a server.

Two halves, one repo:

- **`engine/`** — the receipts engine (TypeScript/Bun, zero runtime deps,
  git plumbing only). Ships as the `preceipts-engine` binary:
  `init · run · status · log · hud · land · sync · gc`, everything scriptable
  via `--json`, check runs streamable via `--events` (NDJSON).
- **repo root** — the cockpit (Rust), a fork of the excellent
  [lumen](https://github.com/jnsahaj/lumen) diff TUI. Ships as the
  `preceipts` binary: bare invocation opens the diff cockpit; engine
  subcommands pass through to `preceipts-engine`. On top of lumen's review
  surface we add the receipts rail, live check runs, a branch HUD footer,
  and one-motion land.

## Why trees, not commits?

A receipt attached to a *tree hash* survives history rewrites: rebase,
reword, squash — if the content is identical, the proof still stands. And a
squash-land via `git commit-tree` produces a commit whose tree **is** the
proven tree, so "are we landing what we tested?" is a hash comparison, not a
policy.

Read [`PRD.md`](PRD.md) for the full design and decisions log.

## Quickstart

```sh
# engine (receipts, runs, land)
cd engine && bun install && bun run build     # → engine/dist/preceipts-engine

# desktop cockpit (macOS)
cd swift && swift build                       # requires: brew install libgit2
swift run PreceiptsApp /path/to/repo

# in your repo
preceipts-engine init         # scaffold .preceipts/
preceipts-engine run          # run checks, mint receipts
preceipts-engine status       # receipt table for the working tree
preceipts-engine hud          # conflicts/freshness/sync at a glance
preceipts-engine land my-branch   # squash + trailers + push, receipts willing
```

Checks are plain executable files in `.preceipts/checks/` — exit 0 means
pass. Required checks are listed in `.preceipts/config.toml`.

## History

preceipts started as a TUI built on [lumen](https://github.com/jnsahaj/lumen)
by [@jnsahaj](https://github.com/jnsahaj) (MIT), then grew a Rust core and a
GPUI prototype before landing on the all-native Swift app (PRD decisions
10–11). The Rust workspace was removed from the working tree once its
algorithms and tests were ported — mine it via git history if needed.
