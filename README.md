<p align="center">
  <img src="docs/assets/icon.png" width="128" alt="preceipts icon">
</p>

<h1 align="center">preceipts</h1>

<p align="center"><strong>The workbench around AI coding. It never does the coding.</strong></p>

An agent writes; a human decides. Everything here serves one loop:

```
prompt → worktree → live env with real URLs → diff you can read fast
       → receipts → land
```

preceipts is a read surface with a control plane, never a host. It does not run
your agent, embed a terminal, or own a session. It watches the files the agent
is writing, owns the environment that code runs in, and holds the evidence that
says whether it is safe to land. Whatever drives the agent — any harness, any
model — stays outside and stays yours.

> **Status: mid-rewrite** (decision 12). The receipts loop, workspaces, the
> sandbox schema, the MCP server, and quiet-triggered checks all work today and
> this repository runs them on itself. The GPUI cockpit builds and loads but its
> *rendering* has not been reviewed by human eyes. Plan:
> [`docs/direction-2026-08.md`](docs/direction-2026-08.md). Pitch:
> [`docs/pitch.md`](docs/pitch.md).

## The loop, today

```sh
preceipts new "fix the checkout race"   # worktree + branch + recorded intent
cd "$(preceipts new "…")"               # the path is stdout, alone, on purpose
preceipts run                           # checks run, receipts minted
preceipts status                        # ✓ green — tree 2a7db2504869
preceipts land                          # squash, trailers, push — receipts willing
preceipts watch                         # or: fire checks whenever the tree goes quiet
```

Every read command takes `--json`, because the CLI is the agent-facing surface
and a verb that only prints for humans would have to be rewritten to serve one.

## The workspace is a laboratory

Everything the UI shows, an agent can query — no UI-only capabilities.
`preceipts mcp` serves the same instruments over MCP, an open protocol, so any
harness picks the lab up without a plugin and without us naming a vendor:

| Instrument | Tool |
|---|---|
| Which lab am I in? | `where` |
| What changed? | `diff` |
| Is it sound? | `status`, `run`, `receipts` |
| What is around me? | `workspaces`, `environment` |

A project's own `[actions.*]` become tools automatically, with their declared
arguments — so `auth <actor>` and `purchase-smoke` reach an agent without a line
of project-specific code here.

## Why trees, not commits?

A receipt attached to a *tree hash* survives history rewrites: rebase, reword,
squash — if the content is identical, the proof still stands. And `land` builds
its commit with `git commit-tree <proven-tree>`, so the landed commit's tree
**is** the proven tree. "Are we landing what we tested?" is a hash comparison,
not a policy.

Receipts stay inspectable with plain git:
`git notes --ref=refs/notes/receipts show <tree>`.

Read [`PRD.md`](PRD.md) for the full design and decisions log.

## Describing a sandbox

One `preceipts.toml`, with progressive disclosure — four lines for a Vite app,
and trip's kitchen sink without becoming a program:

```toml
[services.db]
image = "postgres:17"          # an image implies a container
health.tcp = 5432

[services.api]
run = "pnpm --filter api dev"  # native, where iteration speed lives
needs = ["db"]
health.http = "/health"
host = "api"                   # a label; the fabric composes the FQDN
env = ["@shared", "STRIPE_SECRET_KEY"]

[env.STRIPE_SECRET_KEY]
require_prefix = "sk_test_"    # a live key refuses to boot
```

The rule: **if the app or a receipt must understand it, it is schema; if only
the project understands it, it is an action.** So services, env policy,
readiness, mocks, drains, and fidelity are modelled — and everything
domain-specific is a declared verb. `preceipts doctor` validates and explains.

`crates/preceipts-core/tests/trip_benchmark.rs` holds trip's real stack as the
design's own acceptance test.

## Layout

One cargo workspace:

- **`crates/preceipts-core`** — everything on the frame path, in-process: the
  three-pass diff algorithm, the row model, surface/tree/find, tree-sitter
  highlighting, git reads, workspaces, port blocks, receipts, land, the sandbox
  schema, and quiet detection.
- **`crates/preceipts-cli`** — `preceipts`: the terminal door and the agent's,
  including `mcp`.
- **`crates/preceipts-app`** — the GPUI cockpit: workspace list, virtualized
  diff surface, status HUD.
- **`crates/preceiptsd`** — the daemon: healthcheck-gated orchestration, PTY
  supervision with queryable ring buffers, a local CA and TLS proxy, Keychain
  secrets scoped per project. This was `procpane`; it is dissolved rather than
  vendored, so there is no second product and no dependency edge between two
  halves of one system.

Every verb lives on `preceipts` — `up`, `down`, `services`, `proc`, `grep`,
`secrets`, `trust` came across with the daemon — because the lab has one door
and an agent should not have to learn which tool owns which verb. `preceiptsd`
answers only to launchd and to `preceipts up`.

Still ahead: converging `procpane.toml` into `preceipts.toml` (two schemas, one
job), per-workspace certs behind the `:443` forwarder, the HTTP transcript, the
env panel and project tabs, and `sync`/`gc`. Service URLs are
`<service>.<workspace>.<project>.localhost` — the system resolves `*.localhost` to
loopback at any depth, so there is no DNS to install (decision 13).

## Build

```sh
cargo build
cargo test
cargo run -p preceipts-app -- /path/to/repo          # the cockpit
cargo run -p preceipts-app -- --stats /path/to/repo  # headless
```

Building the app needs Apple's Metal toolchain, which Xcode 26 ships separately.
If the build fails compiling gpui's shaders:

```sh
xcodebuild -downloadComponent MetalToolchain
```

Note that `gpui-component` pins tree-sitter for the whole workspace — only one
crate in a cargo graph may link the native library — so core's grammar ABI
follows whatever the app requires.

## Quality contract

`cargo test` is the whole story, and this repository mints receipts on itself:
`clippy`, `fmt`, and `test` are its required checks.

The diff, surface, and highlight tests came across from Swift unchanged in
meaning; the receipt and trailer tests came across from the TypeScript engine
the same way, and the receipt suite reads *real receipts this repo minted in
July 2026*. They pinned those implementations and now pin this one, which is the
only reason rewriting tested code is safe to attempt.

The performance contract came across too. The Swift cockpit loaded a 181-file
changeset in ~0.7s release; this branch against origin/main is also 181 files,
52,608 surface rows, in **366ms** — reads, diff, highlighting, surface build,
workspace discovery, and a receipt status read.

## History

preceipts started as a TUI built on [lumen](https://github.com/jnsahaj/lumen) by
[@jnsahaj](https://github.com/jnsahaj) (MIT), grew a Rust core and a GPUI
prototype, spent a stretch as an all-native Swift app (decisions 10–11), and
returned to Rust when the product became the workbench rather than the PR viewer
(decision 12). Every era is in the history; nothing was thrown away.
