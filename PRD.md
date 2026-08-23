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
  <blob-sha>` refs. Receipts reference their log blob. `preceipts gc
  --keep-success 30d --keep-failure 90d` prunes old log refs; receipt lines
  outlive their logs gracefully.
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

## CLI surface

```
preceipts run [check…]        # run in current worktree; mint against working-tree hash;
                             # warn (not fail) on dirty worktree
preceipts watch [check…]      # run them again whenever the worktree goes quiet
preceipts status [--ref R]    # receipt table for a tree vs the required set; exit code
                             # reflects greenness (scriptable) but nothing enforces it
preceipts log                 # every receipt recorded in this repository
preceipts land [branch] [--onto main] [--no-push]
                             # squash via commit-tree, embed receipt trailers in the
                             # message, ff base ref, push. If base moved: state it,
                             # offer rebase-first or land-anyway. Never blocks.
preceipts sync                # fetch + cat_sort_uniq merge + push of receipt refs
preceipts gc [--keep-success 30d] [--keep-failure 90d]
                             # prune log refs
preceipts init                # write .preceipts/ stub, offer refspec config
preceipts doctor              # validate preceipts.toml and say what is wrong with it
preceipts migrate             # convert procpane.toml, move its secrets, clean up
preceipts mcp                 # serve the same instruments over MCP on stdio

preceipts new <intent…>       # worktree + branch named from the intent + the intent kept
preceipts list | ls           # every workspace of this project
preceipts where               # which workspace this directory stands in
preceipts diff [--uncommitted]
                             # files, lines, base for the changeset
preceipts remove [id]         # drop a workspace's worktree and its registration

preceipts up [task…]          # bring the project's services up, healthcheck-gated
preceipts down                # stop this project's daemon
preceipts services            # what is running, and is it healthy
preceipts wait-for <task>     # block until healthy; 0 healthy, 1 failed, 2 timeout
preceipts proc <name> …       # per-service tail, grep, since, signal
preceipts grep <pattern>      # search every service's output at once
preceipts secrets …           # project secrets in the Keychain (never printed by list)
preceipts trust …             # the local CA, and the :443 forwarder
--json on the read commands  # agents and the app consume the same output
```

The environment verbs live on the same binary as the receipt verbs on purpose:
the lab has one door, and an agent should not have to learn which tool owns
which verb.

Merge-commit trailers make receipts legible in plain `git log`:

```
Receipts: typecheck ✓ 48s · test ✓ 3m12s · lint ✓ 9s
Receipts-Tree: 8f3a…
Receipts-Runner: jokull@mbp.local (claude-code)
```

## Agent workflow (the point of all this)

Agent edits → `preceipts run` → commits (tree now matches, receipts valid) →
pushes branch + receipts refs. Human opens the cockpit or runs `preceipts
status --ref branch`: green table, logs one keystroke away, `land` when
satisfied. "Queue-merge" is the agent looping: rebase → `run` → `land`.

## The cockpit (the product)

A desktop app that feels like a flight deck: the diff is the main stream,
checks answer beside it, and the receipt verdict is always on screen.

**Built as a GPUI app** — `crates/preceipts-app`, gpui + gpui-component,
linking `preceipts-core` directly. Nothing is shelled out to: loading a
changeset, building the row surface, and reading receipts are function calls,
so no process boundary sits on the scroll path. The three binaries are
`preceipts` (the CLI), `preceipts-app` (this), and `preceiptsd` (processes,
health, proxy, secrets) — none of them a check engine.

Two earlier plans are retired and recorded in the decisions log rather than
here: a lumen fork (Rust + ratatui) speaking to a `preceipts-engine` binary
over `--json`/`--events` (decisions 7 and 9), and the all-Swift app that
replaced it (decision 11). Decision 12 settled on GPUI and deleted the engine.
The hunk fork and the DiffSurface prototype survive only as perf reference.

### Hard requirements

- **100k+ line changesets, smooth.** Scroll ticks < 2ms at any scroll distance
  and any stream size; first frame < 500ms regardless of changeset size. The
  surface is one virtualized list (`uniform_list`): fixed-height rows, only
  the visible window built per frame, so a tick costs the viewport rather than
  the changeset. The mandate comes from the DiffSurface prototype
  (0.26–0.43ms/tick on a 58k-row stream, scroll-distance- and size-invariant
  — hunk fork `benchmarks/diff-surface-proto/NOTES.md`); `preceipts-app
  --stats` is how the load path is timed against it without opening a window.
- **Live CI, inline.** Checks run in `preceipts-core`, so the app watches the
  same runs an agent starts from the CLI: per-check state, elapsed time, full
  log one keystroke away. Greenlight state = required set green for the
  current tree, recomputed as receipts mint and as the worktree changes.
  Built today: the verdict in the HUD. The per-check rail is not.
- **View + monitor, never mutate** (revised 2026-07-06, superseding
  "one-motion land"). The cockpit is the coding agent's companion: the human
  reviews the diff and watches receipts; the *agent* lands via
  `preceipts land`. No land keybinding — the app's scope is scroll, search,
  monitor, and watch checks run.
- **Always current.** The diff reloads on file changes, off-thread, so a
  multi-second reload of a big changeset never freezes the UI while an agent
  is editing — `preceipts-core`'s watcher is the same quiet detector
  `preceipts watch` fires checks from. Receipt snapshots also refresh on a
  timer, because receipts change with no worktree event at all (an agent
  minting in another terminal, a sync, the base moving). The CLI watches
  today; the app does not wire the watcher yet.
- **Branch diff by default, one key to switch.** The app opens the branch view
  — merge-base(origin/main | main | …/master, HEAD) → working tree — because
  the unit of review is the branch, not the last save. A toggle to
  uncommitted-only ("what did the agent just do?") and back; the CLI spells
  the same two scopes `preceipts diff` and `preceipts diff --uncommitted`.
- **⌘F search over the whole changeset**, not just mounted rows: the index in
  `preceipts-core` folds case once per changeset, off the UI thread, and each
  keystroke is one sweep over a contiguous buffer. Literal substring,
  next/prev with a wrap indicator, match count, all matches highlighted with
  the current one distinct. Under ~50ms per keystroke on a 100k-line
  changeset. The index is ported; the app does not bind ⌘F to it yet.
  Vim-grammar navigation was dropped with the TUI (decision 10): the app
  follows macOS conventions.

### App ↔ core interface

There is no engine process to talk to. Receipts, status, and check runs all
live in `preceipts-core`; the app links it and reads in-process, the CLI links
it and prints. Agents get the same truth out of `preceipts run --json` and
`preceipts status --json`, which is what keeps the two front-ends honest about
each other. (The NDJSON
`--events` stream and the `preceipts-engine` pass-through this section once
specified went out with the TS/Bun engine — decision 12.)

### The status HUD

A one-line footer answering "where does this workspace stand?" It is app
chrome (decision 10), not a verb: `render_hud` in the cockpit draws the base
ref, the file and line counts of the changeset, and the receipt verdict —
green, or the required checks that are not.

Still design intent, none of it built: merge cleanliness vs the base
(`git merge-tree --write-tree`, conflicted file list), ahead/behind, land
freshness (is the base head still the merge-base?), fetch age, and the count
of local receipts origin has never seen. Each is a question you otherwise
answer by leaving the app.

Whatever it grows into, the HUD stays read-only, never networks of its own
accord, and blocks nothing (philosophy: inform, don't gate).

## Out of scope

- Minting receipts for refs other than the current worktree (needs temp
  worktrees; revisit when agents want to prove branches they haven't checked out)
- Path-filtered checks for monorepos (`paths = ["packages/api/**"]`) — v2
- Signing — format reserves an optional `sig` field, nothing more

## Phasing

1. **Engine + CLI** (done, then retired): the first `preceipts-engine` was Bun
   + TypeScript in `engine/`, and its `--json` surface is what the CLI's read
   commands still owe their shape to. Decision 12 deleted it; receipts,
   checks, and diff live in `preceipts-core`, spoken to by the `preceipts`
   binary.
2. **Dogfood** in trip with agents minting receipts; the trust/workflow model
   is the real bet, validated in parallel with cockpit work.
3. **Cockpit** (see above): the GPUI app over the same core — diff surface
   first (the riskiest port), then the workspace list and HUD, then the
   per-check rail and log pager.
4. **The lab's agent face**: CLI parity, `preceipts mcp`, and the environment
   verbs, so anything an agent needs is reachable without the app.
5. **Quiet-triggered runs** (`preceipts watch`, and the same detector in the
   app): the north-star feature, last because it needs everything above.

Step-by-step sequencing, and what remains of it, is in
docs/direction-2026-08.md.

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
   `git push` carries receipts. *(The `--sync` flag never shipped: pushing
   after minting stayed the standalone `sync` verb. The rest of the decision
   holds.)*
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
   evidence retained in the hunk fork as reference). *(Superseded whole by
   decisions 10-12: no fork, no TUI, no second binary. The app links the
   core.)*

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
   HUD/status. `t` toggles PR ⇄ uncommitted scope. *(The scope rule and the
   view-only stance survive into the app; the TUI and the engine CLI it
   landed through do not — decisions 10-12.)*

10. **Desktop app, macOS-native, GPUI** (2026-07-07, user decision). The
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
    `SMAppService` LaunchAgent, per-workspace leaf certs, Touch ID for
    secret reveal, notarized bundle. *(The DNS clause of this decision was
    superseded by decision 13.)* Accepted cost: GPUI draws its own widgets, so
    native feel — text selection, IME, VoiceOver, scroll physics — is
    built and budgeted rather than inherited from AppKit.
    Full design, and the sequencing it was built to,
    in docs/direction-2026-08.md.

13. **The environment layer, decided by measurement** (2026-08-23, four
    user directives). Four things settle here, and the first one deletes
    a subsystem.

    **Subdomain routing needs no DNS.** Decision 12 specified
    `/etc/resolver/test` plus a DNS responder in the daemon, inheriting
    procpane's `/etc/hosts` problem. Two findings retired that design.
    macOS 26 has mDNSResponder intercept every TLD absent from the IANA
    root zone — `.test`, `.internal`, `.lan`, `.home.arpa` — and answer
    it as multicast DNS, never consulting the nameserver named in
    `/etc/resolver/`; the recipe every local dev tool has used for a
    decade does not work on the OS we target. And `*.localhost` already
    resolves to loopback at arbitrary depth through `getaddrinfo` itself,
    verified on macOS 26.5.2 (`deep.sub.localhost`, and a live HTTP
    request to `api.fix-checkout.preceipts.localhost`) — so curl, Node,
    Rust, Safari, and every subprocess an agent spawns agree without a
    line of configuration. **The scheme becomes
    `<service>.<workspace>.<project>.localhost` and the DNS layer is
    deleted**: no responder, no resolver file, no `/etc/hosts` block,
    nothing installed and nothing left behind. Two constraints stay
    written down: a wildcard cert matches one label, so leaves carry
    concrete SANs; and inside a container `*.localhost` is the guest's
    loopback, so cross-service addresses are injected, not resolved. The
    only privileged surface left is the `:443` bind — a root-owned byte
    forwarder in front of the unprivileged TLS proxy on `:8443`,
    registered with `SMAppService.daemon` from inside the signed bundle
    rather than by `sudo`-writing a plist, and deliberately given no XPC
    surface.

    **procpane is dissolved, not vendored.** The crate and its binary are
    deleted; its jobs move into `preceiptsd` (process supervision,
    healthcheck graph, port allocator, TLS proxy, local CA, Keychain
    secrets, URL registry). `preceipts-core` does not grow a process
    supervisor — its charter is the frame path. No dependency edge
    between them survives because there is no second crate.

    **Env carries two orthogonal declarations**: where a value comes from
    (keychain, dotenv, captured, literal) and whether it is **hashed**
    into a build-cache key or merely **passed through** — mirroring
    Turborepo's `env`/`globalEnv` versus `passThroughEnv`. A secret must
    never be hashed: it busts a shared remote cache on every machine and
    puts its value in a key that travels. `doctor` reconciles the
    manifest against `turbo.json` and can report both failure modes —
    the secret poisoning the cache, and behaviour-changing config
    declared nowhere that silently produces a wrong cache *hit*.

    **Fidelity is an optional receipt field.** Optional is the load-
    bearing word: the receipt suite pins real receipts this repo minted
    in July 2026 and asserts byte-identical re-encoding, so the format
    grows additively or not at all. A receipt without the field means
    what it always meant; with it, it distinguishes "checks passed" from
    "checks passed with Stripe mocked and Turnstile disabled".

    Implemented on branch `rust`: hostnames, the deleted DNS layer, the
    dissolution, the env split with turbo reconciliation, and the fidelity
    field. Since then `preceipts migrate` converts `procpane.toml` into
    `preceipts.toml` and `preceipts up` boots a workspace's environment
    through the daemon. Still ahead of the decision: the signed bundle that
    `SMAppService` registration and the shared Keychain access group both
    wait on.

    Surveyed alongside: **ABox** (libkrun microVM per agent session on
    Hypervisor.framework). Adopted from it — libkrun as the rail that
    needs no installed container runtime, golden images cloned per
    workspace with APFS copy-on-write, and its refusal to call unproven
    isolation verified. Refused — the harness itself, which owns the
    agent loop and is this project's stated anti-goal.
