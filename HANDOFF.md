# preceipts handoff — 2026-07-07 (post Rust-workspace deletion)

## What preceipts is

The coding-agent PR companion that replaces the GitHub PR UI: local CI
receipts keyed to git tree hashes (notes on tree objects) + a native
macOS diff cockpit. Repo: `~/Code/preceipts` (github.com/jokull/preceipts,
default branch `preceipts`). Everything committed and pushed.

## Read these first (authoritative, don't duplicate)

- `PRD.md` — product spec + decisions log 1–11. Decision 11 = current
  architecture: **all-native Swift app, no Rust core**.
- `docs/desktop-app-design.md` — desktop design: layout, keyboard map,
  GitHub integration, PR feedback viewer, diff algorithm, sequencing.
- `docs/desktop-foundations.md` — the research grounding the native path.
- `docs/modern-macos-feel.md` — Tahoe+ (macOS 26) audit of
  twostraws/swift-agent-skills + concrete recommendations (standard
  chrome primitives, Liquid Glass gating, semantic colors, SwiftUI
  islands). Feeds the design pass.

## Current state

### Swift app (`swift/`) — THE codebase
- SPM, macOS 14, tools 5.9. Do NOT bump platforms to `.v15+` (needs
  tools 6 → strict concurrency); gate Tahoe APIs with
  `#available(macOS 26, *)` instead.
- Requires `brew install libgit2` (Clibgit2 system-library target).
- PreceiptsKit: GitReader (libgit2), LineDiff (patience+indent-heuristic
  → similarity pairing → word intraline), **tree-sitter highlighting**
  (raw C API — NOT SwiftTreeSitter, which is UTF-16; ours stays UTF-8.
  14 languages + elixir, queries in Queries.swift, text predicates
  #eq?/#match?/#any-of? evaluated in Swift, 512KB cap, parallel pass in
  loader). Same-node capture precedence: LATER pattern wins (verified
  against tree-sitter-highlight; encoded in tests).
- PreceiptsKit builds `-O` even in debug (Package.swift unsafeFlags) —
  -Onone made highlighting ~9x slower. Trip 181-file diff loads ~1.4s
  debug / 0.7s release.
- **27 XCTests green** — the quality contract. `cd swift && swift build
  && swift test`; run: `swift run PreceiptsApp <repo>`.
- Grammar SPM deps pinned to the versions the queries target.

### Rust workspace — DELETED (2026-07-07)
- TUI + core + GPUI app removed from the working tree; last present at
  commit `023b5a0`. Mine references with `git show 023b5a0:<path>`.
  Still-unported references for upcoming steps:
  `core/src/feedback.rs`, `core/src/comments.rs`, GPUI UI in `app/src/`
  (find bar, feedback panel, draft comments ⌘⇧M).
- `~/bin/preceipts` symlink removed. The TUI no longer exists; don't
  resurrect it.

### Engine (`engine/`, TS/Bun) — stable
- `~/bin/preceipts-engine` (bun compile). Run lock, prepare phase,
  three-state verdict (green/FAILED/unproven). **56 bun tests green.**
  Commands: init/run/status/log/hud/land/sync/gc.

### Vendored skills (`.claude/skills/`, all MIT)
- `swiftui-liquid-glass` — Tahoe glass APIs; load for any chrome work.
- `macos-spm-app-packaging` — bundle/sign/notarize/appcast scripts; use
  verbatim for step 6.
- `writing-for-interfaces` — load when writing user-facing copy.

## Next work (design-doc sequencing)

1. ~~tree-sitter highlighting~~ **done**.
2. ⌘F find + NSOutlineView file tree + sticky file headers — while
   adopting the modern-macos-feel.md chrome recommendations
   (NSSplitViewController sidebar, NSToolbar, semantic colors for
   chrome, SF Symbols).
3. Engine integration: statusbar HUD chips (`preceipts-engine hud
   --json`), receipts panel (`run --events` NDJSON), single-flight
   discipline (already the pattern in CockpitViewController).
4. Feedback viewer + draft comments (port from git history, above).
5. GitHub OAuth device flow + Keychain.
6. App bundle/icon/vendored libgit2/notarization via the vendored
   packaging skill.

## Gotchas / environment

- SourceKit diagnostics show false "No such module" errors for SPM
  targets in this harness — trust `swift build`.
- brew libgit2 links with a harmless "built for newer version" warning.
- Queries.swift is generated-then-hand-owned now: edit query bodies in
  place (the Rust source it was converted from is deleted).
- User's global gitattributes has `* merge=mergiraf` — engine's
  merge-tree probe disables it via `core.attributesFile`; don't
  reintroduce raw `git merge-tree`.
- Default shell fish — wrap multi-statement Bash tool commands in
  `bash -c '...'`.
- User preferences: never `--amend`, stack commits, push after commit.
- Screen capture from this harness lacks screen-recording permission —
  can't screenshot the app window; verify via tests + logs, or ask user
  to look.

## trip repo context (secondary)

`~/Code/triptojapan/trip` branch `support-chat-auth-upgrade-main-merge`
is the dogfood repo (`.preceipts/config.toml`: required check/test/i18n;
prepare format/sync-prompt). A codex agent works there in parallel —
coordinate via receipts, don't fight over the worktree.

## Suggested skills

- **tmux** (user global) for running/inspecting long-lived processes.
- Vendored repo skills above (`swiftui-liquid-glass`,
  `macos-spm-app-packaging`, `writing-for-interfaces`) as the work
  reaches them.
