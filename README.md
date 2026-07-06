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
# engine
cd engine && bun install && bun run build   # → engine/dist/preceipts-engine

# cockpit
cargo build --release                        # → target/release/preceipts

# in your repo
preceipts init                               # scaffold .preceipts/
preceipts run                                # run checks, mint receipts
preceipts status                             # receipt table for the working tree
preceipts hud                                # conflicts/freshness/sync at a glance
preceipts land my-branch                     # squash + trailers + push, receipts willing
preceipts                                    # open the cockpit
```

Checks are plain executable files in `.preceipts/checks/` — exit 0 means
pass. Required checks are listed in `.preceipts/config.toml`.

## Credit

The cockpit is built on [lumen](https://github.com/jnsahaj/lumen) by
[@jnsahaj](https://github.com/jnsahaj) (MIT), whose upstream README is
preserved at [`docs/LUMEN-README.md`](docs/LUMEN-README.md). We track
upstream and intend to keep the diff-viewer core mergeable.
