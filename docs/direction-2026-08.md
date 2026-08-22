# Direction — the AI-coding workbench

*2026-08-22. Supersedes the earlier three-way assessment in this file.
Stack decided by the user: Rust + GPUI, procpane absorbed. This is the
architecture and the north star, not a cost debate.*

## North star

**preceipts is the workbench around AI coding. It never does the coding.**

An agent writes; a human decides. Everything the app does serves one loop:

```
prompt → worktree → live env with real URLs → diff you can read fast
       → receipts → land
```

The app is a **read surface with a control plane, never a host.** It does
not run your agent, does not embed a terminal, does not own a session. It
watches the filesystem the agent is writing to, owns the environment that
code runs in, and holds the evidence that says whether it is safe to land.
Whatever drives the agent — any harness, any model, in your terminal or
someone else's tool — stays outside and stays yours.

That constraint is the product. Every AI-coding tool is racing to own the
harness. The unclaimed ground is everything *around* it: the diff, the
tree, the environment, the URLs, the pre-CI signal. Superset is the closest
neighbour and it complements rather than competes — it orchestrates agents;
this orchestrates the world the agents work in.

The one sentence: *the place you look while an agent works, and the place
you decide from when it stops.*

### The feature that only this app can have

Because the app owns **both** the environment and the receipts, it can do
something neither a diff viewer nor a process runner can: when the agent
stops typing — FSEvents goes quiet for a beat — checks fire automatically
in that workspace's already-warm, already-healthy environment, and the tab
badge turns green or red. Pre-CI signal with no push, no CI queue, no
context switch. That is the north star made concrete, and it is the reason
the merge is a product rather than two tools in a repo.

## The laboratory

The app is two things at once, and they are the same thing seen from two
sides:

- **For the human** — the monitoring surface that sits *beside* the agent
  conversation. Not around it, not containing it. You keep your agent in
  your terminal; preceipts is the other window, showing the diff growing,
  services going healthy, receipts flipping green. Which means glanceable
  is a hard requirement: comfortable at half-screen width, plus a menu bar
  extra carrying workspace status when the window is behind something.
- **For the agent** — the workspace it *knows it is inside*. The lab has
  walls the agent can feel: it can ask what services are up, read its own
  logs since a cursor, replay the HTTP its code just served, check whether
  the tree it produced passes.

**Self-location, no configuration.** A workspace's worktree carries its
own identity (`PRECEIPTS_WORKSPACE` in the environment, a marker file at
the root). Any agent started in that directory — by any harness, in any
terminal — discovers which lab it is standing in without being told. That
is what makes "never own the harness" and "the agent is aware of its
workspace" compatible rather than contradictory.

### Instruments

Everything the human panel shows, the agent can query. One truth, two
faces — never a UI-only capability:

| Instrument | Human sees | Agent calls |
|---|---|---|
| Service logs | env panel tail | `tail`/`grep`/`since <cursor>` |
| Health + URLs | status dots, clickable links | `status`, `wait-for` |
| HTTP transcript | request list | `requests --since` |
| Drains (email, analytics, errors) | counts in status, viewer | `emails`, `events --require`, `errors` |
| Fidelity map | "Stripe: mocked" chips | `status --fidelity` |
| Diff vs base | the diff surface | `diff --json` |
| Receipts | tab badge, drawer | `status`, `run` |
| Process control | restart button | `signal HUP --wait 5s --tail` |

The **HTTP transcript** is the instrument worth building deliberately:
the TLS proxy already sees every request into every service, so the lab
can hand an agent a full request/response log with zero instrumentation
in the app under test. procpane's IDEAS.md gestures at this as
time-travel debugging; scoped to a workspace it is simply "what did my
code just serve, and what did it answer."

**Two client faces, one socket.** The `preceipts` CLI is the scriptable
face; `preceipts mcp` is the same instruments over MCP — an open
protocol, so any MCP-capable harness picks the lab up without us picking
a side. IDEAS.md called this "the MCP server for any local dev runner" —
here it stops being a standards play and becomes the product's own agent
interface.

## Domain model

One noun unifies preceipts and procpane, and it is not "repo":

- **Project** — a registered repo. One tab. Owns base branch, check
  config, service topology.
- **Workspace** — a git worktree of a project. Owns: branch, genesis
  prompt, environment, diff vs base, receipts, lifecycle state. *This is
  the central object.* Everything in the UI is a view onto a workspace.
- **Service** — a process inside a workspace's environment. Has health
  and a URL.
- **Run / Receipt** — a check execution keyed to a git tree hash.

Workspace lifecycle — the app's spine, in core, driven by the daemon,
observed by the UI:

```
draft → creating (worktree add + link caches)
      → booting (procpane up, healthchecks)
      → live (diff streams, receipts fire on quiet)
      → landed | archived (worktree removed, env torn down)
```

**Naming collision to fix on absorption:** procpane's `Workspace` today
means "the turborepo root." That becomes `Project`. Every keyed thing in
procpane — socket path, URL registry, secret scope, proxy route, port
allocation — gains a workspace dimension. That refactor *is* the
absorption; the rest is moving files.

## Process architecture

```
┌─ Preceipts.app (GPUI) ──────────────────────────────┐
│  tabs · workspace list · diff surface · tree ·      │
│  env panel (URLs, health, log tail) · receipts      │
│                                                     │
│  in-process: preceipts-core                         │
│    gix reads · diff · tree-sitter · notify watch    │
│    receipt reads (git notes)                        │
└──────────────────┬──────────────────────────────────┘
                   │ unix socket, JSON
┌──────────────────▼──────────────────────────────────┐
│  preceiptsd — one daemon, all projects              │
│    process supervision (PTY, ring buffers)          │
│    healthcheck graph · port allocator               │
│    TLS proxy :443 · local CA · per-ws certs         │
│    Keychain secrets · URL registry                  │
│    check runner → mints receipts                    │
└─────────────────────────────────────────────────────┘
   ▲
   └── preceipts CLI — same socket, same core.
       Agents and scripts are first-class clients.
```

**Why this split.** The daemon exists because processes must outlive the
window — closing a tab must not kill your dev servers — and because the
proxy and CA are inherently long-lived and machine-global. Everything
*read-only and per-frame* (git, diff, highlight, watch) stays in-process
in the app: no IPC on the hot path, which is what "super fast diff"
actually means. Note the bar to re-earn: the Swift surface is already
virtualized (`NSTableView` row reuse) and loads a 181-file diff in ~0.7s
release. gpui-component's virtual table hands you the scrolling; the
*compute* is the part to port with care.

**Port direction: Swift → Rust, not resurrect.** The Rust `core/` in git
history (`711c348^`, 3.0k LOC) is behind `PreceiptsKit` now — pairing,
intraline, and the raw-C UTF-8 tree-sitter integration all evolved past
it. Port forward from Swift with the 27 XCTests as the conformance suite,
and mine the old Rust only for gix idioms.

**The engine's last polyglot seam.** `preceipts-engine` (TS/Bun, 3.3k LOC)
splits: receipt *reads* — status, log, hud — are gix reads of notes and
belong in core, in-process, no subprocess spawn per refresh. Receipt
*writes* — run, land, sync, gc — move into the daemon and the CLI. The
Bun binary goes away. One language, two artifacts: the `.app` and the
`preceipts` CLI it bundles.

## macOS alignment

Alignment here means **system integration**, not widget provenance. Four
things are load-bearing, and two of them are already-known problems in
procpane that a signed app finally lets us solve properly.

**Keychain, done right.** `secrets.rs` documents the trap it fell into:
`SecItemAdd`'s default ACL binds an item to the creating binary's
codesign hash, so every rebuild reads as a new app and the user drowns in
prompts. The current workaround shells to `/usr/bin/security` to create
items with an *empty trusted-app list* — an open ACL. That is a real
security compromise taken for dev convenience, and shipping a signed,
notarized app is the moment to retire it: put secrets in the
data-protection keychain under a **shared access group**, with the `.app`
and `preceiptsd` signed by the same Team ID and carrying the
`keychain-access-groups` entitlement. Two binaries, one item, no prompts,
real ACLs. Gate reveal-in-UI behind LocalAuthentication (Touch ID).

**The daemon is a LaunchAgent.** Register it with `SMAppService` from the
app rather than hand-rolling a spawn — that is how a daemon survives
logout, restarts on crash, and stays inspectable with the tools the OS
already ships. It also makes "processes outlive the window" a system
guarantee instead of an implementation detail.

**`/etc/hosts` cannot survive per-workspace subdomains.** `pretty_urls.rs`
says it outright: hosts has no wildcards, so it writes explicit entries
for the hostnames declared in `procpane.toml`. That works when hostnames
are static and enumerable. It breaks the moment hostnames are
`api.<workspace>.<project>.test` and workspaces are created on demand —
you would be rewriting a root-owned file on every worktree add. The
macOS-native answer is `/etc/resolver/test`: a resolver file pointing at
a tiny DNS responder inside the daemon on loopback, which answers
anything under `.test` with 127.0.0.1. One privileged install at setup,
never touched again, and hostnames become free. Pair it with per-workspace
leaf certs from the existing CA (`ca.rs::sign_leaf` already takes a
`dns_names` list, so this is a caller change, not new machinery).

**Everything else is table stakes:** FSEvents via `notify`, `NSWorkspace`
for opening URLs and revealing in Finder, user notifications when a check
fails, a proper menu bar extra, Services and `open -a` for the handoff,
notarization and a signed `.app` in the packaging script.

**What GPUI cannot inherit — accept this explicitly.** Widgets are drawn,
not system-provided, so text selection, IME, VoiceOver, scroll physics,
and the standard menu behaviours are Zed's implementation rather than
AppKit's. Zed proves the result can be excellent; it also spent years
getting there. Native *feel* is now a thing this project builds and
budgets for, not a thing it gets for free — which is a fair trade for the
system integration above, but it should be a decision, not a surprise.

## The lightweight environment

Trip's sandbox is the heavy reference — Docker, seeds, provider mocks.
The lightweight version drops the container and keeps the discipline.

- **Hostnames carry the workspace.** `api.fix-checkout.trip.test`.
  *Design detail with teeth:* a wildcard cert matches one label, so
  `*.test` does not cover `api.fix-checkout.test`. Issue a per-workspace
  cert from the local CA at boot (rcgen, milliseconds) rather than
  chasing a mega-SAN cert. One CA install, one Touch ID, ever — the
  promise procpane already makes.
- **The rails exist.** procpane's IDEAS.md already proposes a service URL
  registry (`${tasks.api.url}` injected when a dep goes healthy) and
  profiles. A workspace label is one more dimension on that registry, and
  it is what makes `WEB_URL` correct inside a worktree without anyone
  editing a `.env`.
- **Cheap worktrees.** Share the pnpm store and turbo cache across
  worktrees; APFS `cp -c` clones `node_modules` copy-on-write, so a new
  workspace boots in seconds without a fresh install. This is where
  "as lightweight as possible" is actually won.
- **Steal trip's discipline, not its runtime:** a readiness gate before
  anything is browsed (`wait-for`), status that probes reality instead of
  trusting a stale ports file, warm reuse instead of teardown between
  attempts.

## Sandbox rails

First, split a word that hides two different requirements:

1. **Collision isolation** — three agents working at once must not fight
   over port 3001, the same database, or the same `.next` cache.
2. **Containment** — an agent running unattended must not be able to read
   `~/.ssh`, push to a remote, or exfiltrate a token.

Almost all the product value is (1), and (1) needs no virtual machine at
all. (2) is a security policy, and it is where VMs and Seatbelt live.
Conflating them is how a lightweight tool becomes a slow one.

### The seam that makes the runtime pluggable

We already committed to a URL fabric — a DNS responder answering `.test`
on loopback, per-workspace leaf certs, a TLS proxy in the daemon. That
fabric does not care what is listening on the other end. Which gives the
whole architecture its cleanest abstraction:

> **A service is a thing with an address and a healthcheck.**

Native process, container, VM — the workspace, the URLs, the instruments,
and the receipts are identical either way. So the runtime becomes a
per-project setting rather than a foundational bet:

| Runtime | Cost | Isolation | When |
|---|---|---|---|
| `native` (default) | ~0 | ports, hostnames, DB-per-workspace | The lightweight promise. Parallel agents, full speed. |
| `confined` | ~0 | + Seatbelt profile + egress allowlist | Unattended agents on your own machine. |
| `container` | ~1s boot, VM per service | full | Untrusted work, or a project whose stack is already containerised. |

### Do not write the hypervisor

Rust bindings for Apple's virtualization stack exist — `virtualization-rs`,
`hv`, `applevisor` — and they are not the hard part. A mini-OrbStack means
a VMM *plus* OCI image handling, filesystem sharing, networking, DNS, and
lifecycle management: years of work, and the least differentiated code in
the product. Everyone shipping agent sandboxes has isolation; nobody has
`api.fix-checkout.trip.test` with an HTTP transcript and a tree-keyed
receipt. Build the fabric, delegate the box.

**Delegate to, in order of fit:**

- **Apple `container`** — hit 1.0.0 in June 2026, Apache 2.0, Swift, a
  lightweight VM per container, OCI-compatible. On macOS 26 each container
  gets its own IP and real container-to-container networking, which is
  exactly what our DNS responder wants to point at. Apple-native matches
  the project's macOS alignment, it costs nothing, and it is the obvious
  default container backend. (Not yet installed here; OrbStack is.)
- **OrbStack** — installed on this machine, and trip already relies on its
  per-container domains. Excellent DX, closed source, paid. Support it
  because the user already has it, not as the baseline.
- **Docker / Lima / colima** — same adapter shape, no special casing.

### Containment without a VM

For `confined`, the cheap mechanism is Seatbelt: a profile derived from
the workspace path, restricting writes to the worktree plus its caches.
Child processes inherit it, so confining the shell confines everything the
agent spawns. `/usr/bin/sandbox-exec` is present on this machine (26.5.2)
and is what essentially every agent sandbox uses in practice — including
Claude Code's own runtime, macbox, Agent Safehouse.

**Two honest caveats.** Seatbelt has been formally deprecated for years
with no third-party replacement — App Sandbox needs a signed `.app` and
cannot express this granularity — and there is an open request on
`apple/containerization` asking Apple to clarify the timeline. So treat
confinement as a **policy module behind a trait**, never as a pillar: if
it disappears, `native` and `container` still stand and only the middle
row changes. And an allow-by-default profile is theatre; profiles must
deny first.

**The egress half is already ours.** Claude Code's sandbox pairs Seatbelt
with a network proxy — and we are already building a proxy that sees every
request in and out of a workspace. One mechanism, two features: the same
component that records the **HTTP transcript** for the lab is the natural
place to enforce an egress allowlist. That convergence is worth designing
for deliberately rather than discovering later.

### Prior art to stay honest about

`macbox` (agents in a Seatbelt sandbox, worktree per agent), Dagger's
`container-use` (containerised sandbox + worktree per agent), Melty Labs'
`conductor` — which trip already has a `conductor.json` for — and a stack
of Seatbelt profile projects. This is a populated field. What none of them
combine is the environment, the URL fabric, the instruments, and tree-keyed
receipts in one place; that combination is the thing worth building, and
the isolation mechanism underneath it is a commodity to be borrowed.

## Authoring a sandbox: `preceipts.toml`

One file per project, honest at both ends of the range — four lines for a
Vite app, and trip's kitchen sink without becoming a program. Design rule:
**progressive disclosure. Every rung is optional; you pay only for the rung
you're on.**

Reading trip's `packages/sandbox` changed this section. A service manifest
is not enough. What trip actually built — masked env with test-key
assertions, provider mocks, an email outbox, a PostHog ledger, a local
Sentry sink, a fidelity map, composable seed bundles, a readiness gate
distinct from health — is not trip-specific sprawl. It is the general
shape of "an environment an agent can prove something in," and most of it
belongs in the schema rather than in a project's own scripts.

Today's `procpane.toml` is the right instinct but coupled: task identity is
`<package>#<script>`, so it needs `turbo.json` and a JS workspace.
IDEAS.md already names the fix — decoupling the config schema is what
unlocks the polyglot story. **Services become the primitive**; turbo
becomes an optional reference.

### Rung 0 — no file

Detection. A `package.json` with `dev`, a `Cargo.toml`, a `Procfile` — one
service, health inferred from the first port it binds. `preceipts new`
works in a repo that has never heard of us. The file appears when you
outgrow the default.

### Rung 1 — a service

```toml
[services.web]
run = "pnpm dev"
health.log = "ready in"
```

### Rung 2 — a graph

```toml
[services.db]
image = "postgres:17"
runtime = "container"          # per service, not per project
health.tcp = 5432

[services.api]
run = "pnpm --filter api dev"
needs = ["db"]
health.http = "/health"
host = "api"                   # a label; the fabric composes the FQDN
env = ["@shared", "STRIPE_*"]
```

**Hostnames are labels.** trip writes `hostname = "api.trip.test"` by
hand. Once workspaces exist that is wrong by construction — the fabric
composes `api.<workspace>.<project>.test` from `host = "api"`. Authors
never spell a domain again. trip's Superset mode already proves the
model with `https://admin.<workspace>.trip.local`; this makes it native
instead of an OrbStack label plus an in-container Host-header proxy.

**Runtime is per service.** Containerise the stateful things, run the JS
natively where iteration speed lives. The mixed case is the normal case.

### Rung 3 — env is a policy, not an allowlist

trip's `procpane.toml` is ~100 lines and almost all of it is `env_from`
repetition. But the interesting part is in the sandbox README, not the
manifest: repo `.env`, `.env.*`, and `.dev.vars` are **masked** inside the
sandbox, only curated values are copied in, and a live Stripe key is
**rejected before Docker starts**. That is not an allowlist. That is a
policy with assertions and a fail-closed default.

```toml
[env.shared]
keys = ["ENV", "SECRET", "SENTRY_*", "DATOCMS_*"]

[env.STRIPE_SECRET_KEY]
require_prefix = "sk_test_"    # refuse to boot on a live key
[env.NEXT_PUBLIC_STRIPE_PUBLISHABLE_KEY]
require_prefix = "pk_test_"
```

Defaults that follow from what trip learned the hard way: dotenv files in
the worktree are masked unless named, values come from the Keychain by
reference, per-service allowlists stay (a stray `postinstall` must not see
your Stripe key), and a failed assertion stops the boot rather than
warning. Groups plus globs turn a hundred lines into a dozen.

### Rung 3b — services that mint env for their dependents

trip's `stripe-webhook` task runs `stripe listen`, harvests the minted
`whsec_…` from its own output, and **gates API startup** on it. That is a
general primitive and the natural completion of procpane's URL registry:
a service can export values, not just an address.

```toml
[services.stripe-webhook]
run = "stripe listen --forward-to {{services.api.url}}/stripe/webhook"
capture.STRIPE_WEBHOOK_SECRET = "whsec_\\w+"

[services.api]
needs = ["stripe-webhook"]     # inherits the captured value
```

### Rung 3c — readiness is not health

`pnpm sandbox wait` returns when *the browser, API, and required services
are safe to exercise* — a composite gate above per-service health, and the
thing Superset startup blocks on. Model it explicitly, because "every
process is healthy" and "you may now take a screenshot" are different
claims:

```toml
[ready]
requires = ["api", "next", "seed"]
```

### Rung 4 — mocks: the proxy is already the interception point

trip routes Bokun, Linktivity, and Expedia to a mock proxy and appends
every request to `external-requests.jsonl` with sensitive fields redacted.
We are already building a proxy for TLS and URLs. That one component now
has four jobs, and they reinforce each other:

1. hostname routing and TLS termination
2. the **HTTP transcript** instrument
3. egress allowlist for `confined` workspaces
4. **provider mocking** — upstream swapped for fixtures

```toml
[mocks.bokun]
match = "https://api.bokun.io/**"
fixtures = "sandbox/mocks/bokun"
record = true                  # redacted, into the workspace transcript
```

### Rung 4b — drains: side effects get caught, not lost

This is the primitive I was missing entirely, and it is the heart of what
makes trip's sandbox useful. A workspace captures its own side effects
into named sinks: an email outbox, a PostHog event ledger, a local Sentry
(Urgentry), external requests, media requests. `status` sweeps all of them
and prints counts; each has a query verb.

```toml
[drains.email]
kind = "outbox"
from = "/sandbox-artifacts/emails"

[drains.analytics]
kind = "jsonl"
from = "posthog-events.jsonl"
assert = "event"               # enables `--require "Book Order"`

[drains.errors]
kind = "sentry"                # local sink; nothing leaves the workspace
```

Drains are what turn "the screenshot looks right" into evidence. And
trip's rule is worth encoding in the product, not just the docs: **an
empty drain is a finding.** If a route 500s and the error drain is empty,
that is a bug in error reporting, not a clean run. `status` should say
"errors: 0" loudly enough that nobody reads it as success by default.

### Rung 4c — fidelity: the lab states how real it is

trip's `SandboxBindingFidelity` is exactly the right vocabulary, and I'd
adopt it verbatim: `local-real`, `local-simulated`, `mocked`, `disabled`,
`remote-required`. `status` reports the map so an agent cannot over-claim
— Stripe.js is shimmed, so a browse is route proof and not payment proof.

**This is where sandbox and receipts finally meet.** A receipt should
record the fidelity map of the environment it was minted in. "These checks
passed" and "these checks passed with Stripe mocked and Turnstile
disabled" are different claims, and only one of them is honest. Tree hash
plus fidelity map is a materially stronger proof than either half — and
it is a thing neither a diff viewer nor a process runner could ever
produce.

### Rung 4d — bundles, radii, actions

Seeds compose; radii are shortcuts over bundles, exactly as trip has them:

```toml
[bundles.admin-user]  run = "pnpm seed:bundle admin-user"
[bundles.tour-commerce-basic] run = "pnpm seed:bundle tour-commerce-basic"

[radii]
s = ["admin-user", "customer-user"]
m = ["@s", "trip", "booking"]
l = ["@m", "checkout", "tour-inventory"]
```

Everything else the project needs is a **declared verb**, and we model
none of its domain:

```toml
[actions.purchase-smoke]
run = "pnpm sandbox purchase-smoke"
about = "Whole-funnel checkout proof"

[actions.auth]
run = "pnpm seed:auth {{actor}}"
args = ["actor"]
```

Each action becomes three things for free: a CLI verb, an **MCP tool** so
an agent discovers what this lab can do without being told, and a button
in the workspace panel. trip's `emails`, `provider-contracts`,
`purchase-smoke`, `posthog --require` are config entries, not features in
our repo.

The line between schema and action: **if the app or a receipt needs to
understand it, it is schema** (services, env policy, ready, mocks, drains,
fidelity, bundles). If only the project understands it, it is an action.

### Checks live in the same file

Once one document describes the environment, splitting the proof from the
room it runs in is arbitrary:

```toml
[prepare]
run = ["cargo fmt"]            # decision 8: mutates the tree, runs first

[checks]
required = ["fmt", "engine-test"]
```

Executable checks stay plain files in `.preceipts/checks/`, exit 0 passes.
One `preceipts.toml`, one gitignored `preceipts.local.toml` overlay,
secrets by reference only.

### Ports and artifacts

Each workspace gets a **reserved port block**, lock-protected, stable
across restarts and preserved across teardown so a worktree keeps its
addresses — trip learned this one and it matters more than it sounds.
Artifacts live under a per-workspace root: logs, transcripts, drains,
screenshots. `preceipts env --export` prints the shell env, replacing
`source /tmp/trip-sandbox/<run>/env.sh`.

### Make it authorable

`preceipts init` scaffolds from detection and writes what it found.
`preceipts doctor` validates and explains — an unreachable dependency, a
health check that never passes, a declared key missing from the Keychain,
a live credential where a test key was asserted. Publish a JSON Schema for
editor autocomplete. Version with `schema = 1` from day one.

**The test of this design:** trip's sandbox should be expressible as one
`preceipts.toml` plus its existing scripts — with the Docker container
optional rather than mandatory, and `packages/sandbox`'s ~40 TypeScript
files reduced to fixtures, seed bundles, and mock definitions. If it
can't, the schema is wrong, and trip is the benchmark to check against
before writing the parser.

## Bridging to the harness

An app that won't host a terminal has an obvious dead end: you type a
prompt, a workspace appears — and then what runs the agent? The fix is to
stop treating the app as the origin of work.

**Adoption is the primitive, not creation.** Any worktree of a registered
project becomes a workspace the moment it exists, no matter who made it.
The app is a registry that notices, not a factory that must be used.

Three doors, one registry:

1. **The CLI door — the default, and the one with no handoff at all.**
   `preceipts new "fix the checkout race"` creates the worktree, boots the
   environment, and drops you in it. You were already in a terminal; you
   run `claude` yourself, the way you always do. The prompt is an argument,
   naming is instant, nothing needs bridging because nothing was ever
   separated.
2. **The app door — creation plus an honest handoff.** New Workspace makes
   the worktree and boots the env, then hands off: copy `cd <path> &&
   <your agent command>`, or launch your configured terminal via
   `NSWorkspace`. Not owning a panel doesn't mean refusing to launch a
   program — this is the same move as every "Open in Editor" button. The
   app does not wait on, track, or supervise the agent process. Its job
   ends at the door.
3. **The ambient door — the agent made it, we noticed.** Watch
   `<repo>/.git/worktrees/` with FSEvents: git creates a directory there
   per linked worktree, so `git worktree add` from an agent, a script, or
   your own muscle memory shows up immediately. Reconcile with
   `git worktree list --porcelain` on launch. An agent that spawns its own
   worktrees is a first-class citizen, not an edge case.

**Adopt ≠ boot.** A discovered worktree appears in the list; it does not
silently start a database. Booting is explicit — a click, or
`preceipts up` — with auto-boot as per-project opt-in. Otherwise a stray
worktree costs you a stack.

### The seam is observation, not integration

**Hard rule: no capability may depend on a specific harness or model
vendor.** Anything a harness could tell us must also be inferable from the
filesystem and the environment we already own. Cooperation sharpens
precision; it is never the mechanism. This is what keeps the lab a lab —
a room any tool can walk into — instead of a plugin for whoever is
winning this quarter.

So the bridge is built from things that are true regardless of who is
driving:

| Signal | Inferred from | Needs cooperation? |
|---|---|---|
| A workspace exists | `.git/worktrees/` via FSEvents | No |
| Work is happening | file writes in the worktree | No |
| Work has paused | writes quiet for a debounce window | No |
| What changed | git, in-process | No |
| Whether it's sound | checks in the live env | No |
| Stated intent | branch name, or supplied | No — falls back to the slug |

That table is the whole product working with a harness that has never
heard of us, which is every harness by default.

**Then let anything opt in to precision.** The lab's public interface is
the CLI and its socket — `preceipts adopt`, `intent`, `check`, `release`.
Anything that can run a command can call them: a shell wrapper, a
Makefile, a CI script, a person, or an agent harness that supports
lifecycle hooks. Where a harness does emit lifecycle events, a small
adapter maps them onto those verbs and you get an exact "done" instead of
a debounce guess — worth having, never required, and shipped as a
contributed adapter rather than a designed-in dependency.

Same discipline on the model side. Branch naming is a local slug by
default. If a workspace wants nicer names it can call out, but that path
is optional, non-blocking, and vendor-neutral — the app ships no API key,
assumes no provider, and works fully offline.

### What the prompt is still for

It keeps all three jobs — it just no longer has to be typed in the app:

1. **Name the branch.** Local slug instantly; optionally refine with a
   small model call, non-blocking. Never make someone wait for a worktree.
2. **Be the workspace's recorded intent.** On the card, in the worktree,
   in the land commit trailer. This is what makes eight worktrees legible
   next week.
3. **Arrive from any door** — CLI argument, app field, or
   `UserPromptSubmit` hook.

## Scope cuts

Deleted with the pivot, not migrated: the PR conversation surface —
thread cards, feedback panel, PR panel, GitHub clients, device-flow auth,
avatars, markdown bodies (~3.5k LOC across app and kit). Also not built:
terminal emulation, agent session hosting, anything that owns a model
call beyond branch naming.

**Assumption stated rather than asked:** cutting comments does not
necessarily cut *checks*. "Pre-CI signaling" reads as local receipts
being the signal, so the plan keeps `land` and a lightweight remote-status
chip and drops the checks board with the rest of the GitHub surface. Say
the word if the Actions/Vercel rollup should survive.

## UI shape

- Window: **tab per project** (GPUI tabs; a project registry replaces
  today's `PreceiptsApp <repo>` single-repo launch).
- Left: **workspace list** — worktree cards showing branch, genesis
  prompt, receipt verdict, service health dots.
- Center: **diff surface** — gpui-component's virtual table, one scroll
  surface, tree-sitter highlighting.
- Dock: **tree browser** and **env panel** — services with clickable
  URLs, health, log tail from the daemon's ring buffers.
- ⌘K palette; ⌘F find; no vim keys.

## Draft decision 12 (for PRD.md)

> 12. **Product pivot: the AI-coding workbench; Rust + GPUI; procpane
>     absorbed** (2026-08-22, user decision). preceipts becomes the
>     workbench *around* AI coding and explicitly never hosts it — no
>     terminal pane, no agent harness; the user drives their own agent.
>     **Adoption, not creation, is the primitive:** any worktree of a
>     registered project is adopted as a workspace whether it came from
>     the CLI (`preceipts new`, the default path — no handoff needed
>     because the user is already in a terminal), the app (creates, then
>     hands off to the user's terminal and stops caring), or an agent
>     running `git worktree add` on its own (detected via FSEvents on
>     `.git/worktrees/`). **No capability may depend on a specific harness
>     or model vendor**: the full loop runs on observation alone —
>     worktree detection, file-write activity, quiet debounce, git, and
>     checks in the live env — for harnesses that have never heard of
>     preceipts. The CLI verbs (`adopt`, `intent`, `check`, `release`) are
>     a public interface anything may call to trade a heuristic for an
>     exact signal; harness-specific lifecycle adapters are contributed,
>     not designed in. Branch naming is a local slug by default; any model
>     call is optional, non-blocking, vendor-neutral, and never shipped
>     with a key. **Sandbox rails are pluggable, never written here:** the
>     URL fabric treats a service as "an address plus a healthcheck," so
>     the runtime is per-project — `native` (default, no VM, isolation by
>     port/hostname/DB namespacing), `confined` (Seatbelt profile derived
>     from the worktree plus egress allowlist enforced by our own proxy,
>     kept behind a trait because `sandbox-exec` is deprecated with no
>     replacement), or `container` (delegated to Apple `container` first —
>     Apache 2.0, per-container VM, own IP on macOS 26 — then OrbStack and
>     Docker/Lima through the same adapter). We do not build a VMM.
>     Projects author their environment in one `preceipts.toml` with
>     progressive disclosure — zero config by detection, up to trip's
>     kitchen sink. The schema owns what the app and receipts must
>     understand: services, health, per-service runtime, env **policy**
>     (masked dotenvs, Keychain by reference, test-key assertions that
>     fail the boot), captured env from service output, a composite
>     readiness gate, provider mocks, **drains** (email outbox, analytics
>     ledger, local error sink, HTTP transcript), a **fidelity** map
>     (`local-real | local-simulated | mocked | disabled |
>     remote-required`, adopted from trip), seed bundles and radii, and
>     checks. Everything domain-specific is a declared `[actions.*]` verb,
>     surfaced automatically as a CLI command, an MCP tool, and a panel
>     button. **Receipts record the fidelity map of the environment they
>     were minted in** — tree hash plus fidelity is the honest proof.
>     Benchmark: trip's sandbox must be expressible as one
>     `preceipts.toml` plus fixtures, with Docker optional.
>     Scope: tab per project, worktree workspaces carrying a genesis
>     prompt, per-workspace dev environments with subdomain URLs, a fast
>     diff and tree browser, and receipts as pre-CI signal fired
>     automatically when the worktree goes quiet. The PR conversation
>     surface (~3.5k LOC) is deleted. Decision 11 is reversed: the UI
>     returns to GPUI/gpui-component and the system unifies on Rust — one
>     cargo workspace, `preceipts-core` in-process for git/diff/highlight/
>     watch, `preceiptsd` for processes, healthchecks, TLS proxy, secrets,
>     and check runs, `preceipts` CLI on the same socket. The TS/Bun
>     engine is retired: receipt reads move into core, writes into the
>     daemon. procpane's `Workspace` is renamed `Project`; every keyed
>     resource gains a workspace dimension. Algorithms port forward from
>     `PreceiptsKit` (27 XCTests as the conformance suite), not backward
>     from the deleted Rust core. The workspace is a **laboratory**: every
>     instrument the UI shows (logs, health, URLs, HTTP transcript, diff,
>     receipts, process control) is equally available to agents through
>     the CLI and an MCP server (open protocol, no vendor assumed), and
>     a worktree self-identifies via
>     `PRECEIPTS_WORKSPACE` so any harness started inside it locates the
>     lab with no configuration. macOS integration is first-class:
>     data-protection Keychain with a shared access group across signed
>     app and daemon (retiring procpane's open-ACL workaround),
>     `SMAppService` LaunchAgent, `/etc/resolver/test` + an in-daemon DNS
>     responder replacing `/etc/hosts` (which cannot express per-workspace
>     subdomains), per-workspace leaf certs, Touch ID for secret reveal,
>     notarized bundle. Accepted cost: GPUI draws its own widgets, so
>     native feel — text selection, IME, VoiceOver, scroll physics — is
>     built and budgeted rather than inherited from AppKit.

## Sequencing

The order that keeps a working app at every step:

0. Delete `swift/` and `engine/` in one honest commit, the way the Rust
   workspace went at `711c348`. Same repo, continuous PRD, history keeps
   the reference material.
1. Cargo workspace; procpane moves in under its new `Project` naming.
2. `preceipts-core`: port PreceiptsKit forward, tests first — diff and
   highlight conformance before any UI exists.
3. GPUI shell: one project, one workspace, the diff surface. This is the
   riskiest port; do it early and prove the scroll.
4. Workspace dimension through the daemon: ports, registry, certs, socket
   namespacing. Worktree lifecycle.
5. Env panel + URLs; workspace list. Workspace adoption in all three
   doors — CLI `new` first, FSEvents discovery second, app button last.
6. Retire the Bun engine: reads into core, writes into the daemon.
7. The lab's agent face: CLI parity, then `preceipts mcp`, then the HTTP
   transcript. Ship this before it feels finished — it is the half of the
   product that has no UI to flatter it.
8. Quiet-triggered check runs and tab badges — the north-star feature,
   last because it needs everything above. Build the observation path
   first and ship it complete; lifecycle adapters come after, as
   precision on top of something that already works alone.
