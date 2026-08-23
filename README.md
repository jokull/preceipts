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
preceipts sync                          # share receipts with origin, merged losslessly
preceipts gc                            # prune old logs; receipts themselves are permanent
```

One run per worktree at a time. `[prepare]` writes to the working tree, so a
quiet-triggered run and a `preceipts run` you typed would corrupt each other's
evidence — the second one fails fast and says whose pid holds the lock. A lock
left by a crash is taken over rather than requiring cleanup.

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
| What did my code just serve? | `requests` |

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
task = "api#dev"               # a task your runner already defines
needs = ["db"]
health.http = "/health"
host = "api"                   # a label; the fabric composes the FQDN
env = ["@shared", "STRIPE_SECRET_KEY"]

[env.STRIPE_SECRET_KEY]
from = "keychain"              # so never hashed into a build-cache key
require_prefix = "sk_test_"    # a live key refuses to boot

[env.NEXT_PUBLIC_SITE_URL]
from = "literal"
hash = true                    # changes behaviour, so it must bust the cache
```

Every value carries two facts: **where it comes from**, and **whether it
belongs in a hash** — turbo's `env` versus `passThroughEnv`, in the same file
that already knows the services. That lets `preceipts doctor` say a sentence
neither file can say alone:

```
! STRIPE_SECRET_KEY is a keychain secret but turbo.json hashes it — every
  machine's value differs, so this misses cache everywhere, and the value
  itself becomes part of a key that travels to your remote cache.
! NEXT_PUBLIC_SITE_URL changes behaviour but turbo.json does not declare it at
  all — changing it does not bust the cache, so a build with the old value
  will be reused and look green.
```

The second one is the dangerous direction: a cache *miss* is slow, a wrong
cache *hit* is green.

`task` points at something the repository's own runner already defines, so a
monorepo keeps one definition of how a package starts; `run` is the plain
command for everything else. A project needs no turbo at all — four lines of
`preceipts.toml` and `preceipts up` is a complete setup.

The rule: **if the app or a receipt must understand it, it is schema; if only
the project understands it, it is an action.** So services, env policy,
readiness, mocks, drains, and fidelity are modelled — and everything
domain-specific is a declared verb. `preceipts doctor` validates and explains.

`crates/preceipts-core/tests/trip_benchmark.rs` holds trip's real stack as the
design's own acceptance test, including its actual `turbo.json` — which is how
we learned that turbo names root tasks `//#format`, indistinguishable from a
comment to any JSONC parser that is not string-aware.

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
  secrets scoped per project.

Coming from `procpane`? `preceipts migrate` converts its manifest into a
`preceipts.toml`, moves its Keychain secrets into the new namespace, and
migrates its CA — `--dry-run` first if you want to read the result before
anything is written. Nothing reads the old file after that; the name survives
only in the code that retires it.

Every verb lives on `preceipts` — `up`, `down`, `services`, `proc`, `grep`,
`secrets`, `trust` came across with the daemon — because the lab has one door
and an agent should not have to learn which tool owns which verb. `preceiptsd`
answers only to launchd and to `preceipts up`.

Two worktrees of one project run at the same time without knowing about each
other. Each workspace holds a reserved block of 16 ports — stable across
restarts, so a bookmark keeps working — and its own TLS proxy and leaf cert,
under its own hostname:

```
web.trip.localhost
web.add-checkout-flow.trip.localhost
```

Both on the same port, because a router owns it and splices by hostname to the
workspace that owns the name. It holds no keys and terminates no TLS — the
server name in a ClientHello is in the clear, so routing means reading a few
dozen bytes and getting out of the way. Each workspace keeps its own
certificate and its own transcript.

`preceipts requests` reads the HTTP transcript: every request the proxy carried
and what answered it, with nothing instrumented in your app, because the proxy
is already in the path. Heads only — a recorded body is a recorded password.

A service with an `image` runs as a foreground container, so the supervisor that
owns your dev servers owns it too: same log buffer, same health gating, same
stop signal. Delegated to whatever is installed — we do not write a VMM.

`packaging/package.sh` assembles `Preceipts.app`; `sign.sh` signs it with a
Developer ID identity. Signed, a secret's ACL names our own binaries by their
designated requirement, so nothing else on the machine can read it and a
rebuild does not re-prompt. Both halves happen in-process: shelling out to
read would present `/usr/bin/security` as the reader, and any ACL written
against that protects nothing — and an item created by `security` is
partitioned to Apple's own tools, which can shut our binaries out of an item
their ACL names. Unsigned, there is no stable
identity to name, so secrets fall back to an open ACL and `preceipts trust
status` says so out loud: that trade is defensible, hiding it is not.

Still ahead: the env panel and project tabs, and registering the forwarder
through `SMAppService` instead of `sudo` — a signed bundle is eligible for it,
but the call is not written yet. Service URLs are
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
changeset in ~0.7s release; this branch against origin/main is 189 files,
57,015 surface rows, in **331ms** — reads, diff, highlighting, surface build,
workspace discovery, and a receipt status read.

## History

preceipts started as a TUI built on [lumen](https://github.com/jnsahaj/lumen) by
[@jnsahaj](https://github.com/jnsahaj) (MIT), grew a Rust core and a GPUI
prototype, spent a stretch as an all-native Swift app (decisions 10–11), and
returned to Rust when the product became the workbench rather than the PR viewer
(decision 12). Every era is in the history; nothing was thrown away.
