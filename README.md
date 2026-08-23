<p align="center">
  <img src="docs/assets/icon.png" width="128" alt="preceipts icon">
</p>

<h1 align="center">preceipts</h1>

<p align="center"><strong>The workbench around AI coding. It never does the coding.</strong></p>

> **Status: mid-rewrite.** Decision 12 pivoted preceipts from a PR companion
> to an AI-coding workbench, and moved the whole system to Rust. The plan is
> [`docs/direction-2026-08.md`](docs/direction-2026-08.md); the pitch is
> [`docs/pitch.md`](docs/pitch.md). Steps 0–3 of its sequencing are done:
> the diff surface scrolls a real changeset in a GPUI window. The workspace
> dimension, the sandbox schema, and the agent-facing lab are still ahead.
> The Swift cockpit and the TypeScript engine it replaced are mineable from
> history at `fc3643e`.

An agent writes; a human decides. Everything here serves one loop:

```
prompt → worktree → live env with real URLs → diff you can read fast
       → receipts → land
```

preceipts is a read surface with a control plane, never a host. It does not
run your agent, embed a terminal, or own a session. It watches the files the
agent is writing, owns the environment that code runs in, and holds the
evidence that says whether it is safe to land. Whatever drives the agent —
any harness, any model — stays outside and stays yours.

The workspace is a **laboratory**: everything the UI shows (logs, health,
URLs, HTTP transcript, drains, diff, receipts) is equally available to agents
through the CLI and an MCP server, and a worktree self-identifies so any
harness started inside it finds the lab with no configuration.

## Why trees, not commits?

A receipt attached to a *tree hash* survives history rewrites: rebase,
reword, squash — if the content is identical, the proof still stands. And a
squash-land via `git commit-tree` produces a commit whose tree **is** the
proven tree, so "are we landing what we tested?" is a hash comparison, not a
policy.

Read [`PRD.md`](PRD.md) for the full design and the decisions log.

## Layout

One cargo workspace:

- **`crates/preceipts-core`** — everything on the frame path, in-process: the
  three-pass diff display algorithm (libgit2 xdiff → similarity pairing →
  word-level intraline), the row model, and segment composition. Ported
  forward from the Swift `PreceiptsKit` with its tests, which are the
  conformance suite for the rewrite.
- **`crates/preceipts-app`** — the cockpit, on GPUI: the virtualized diff
  surface over one repository. `--stats` loads and reports headlessly.
- **`crates/procpane`** — the absorbed process runner: healthcheck-gated
  orchestration, PTY supervision with queryable ring buffers, a local CA and
  TLS proxy for `https://*.test`, and Keychain-backed secrets with per-task
  allowlists.

Still to land, in order: the workspace dimension through the daemon
(worktrees, port blocks, per-workspace certs and hostnames), the workspace
list and env panel, the `preceipts.toml` sandbox schema, the agent-facing CLI
and MCP server, and receipts firing when the worktree goes quiet.

## Build

```sh
cargo build
cargo test
cargo run -p preceipts-app -- /path/to/repo          # the cockpit
cargo run -p preceipts-app -- --stats /path/to/repo  # headless
```

Building the app needs Apple's Metal toolchain, which Xcode 26 ships
separately. If the build fails compiling gpui's shaders:

```sh
xcodebuild -downloadComponent MetalToolchain
```

Note that `gpui-component` pins tree-sitter for the whole workspace — only
one crate in a cargo graph may link the native library — so core's grammar
ABI follows whatever the app requires.

## Quality contract

`cargo test` is the whole story — 59 tests today. The diff, surface, and
highlight tests came across from Swift unchanged in meaning: they pinned that
implementation and now pin this one, which is the only reason rewriting
tested code is safe to attempt.

The performance contract came across too. The Swift cockpit loaded a 181-file
changeset in ~0.7s release; this branch against itself is 165 files, 46,304
surface rows, in ~0.25s — reads, diff, highlighting, and surface build.

## History

preceipts started as a TUI built on [lumen](https://github.com/jnsahaj/lumen)
by [@jnsahaj](https://github.com/jnsahaj) (MIT), grew a Rust core and a GPUI
prototype, spent a stretch as an all-native Swift app (decisions 10–11), and
returned to Rust when the product became the workbench rather than the PR
viewer (decision 12). Every era is in the history; nothing was thrown away.
