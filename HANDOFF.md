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
2. ~~⌘F find + NSOutlineView file tree + sticky file headers + modern
   chrome (NSSplitViewController sidebar, unified NSToolbar, semantic
   chrome colors, SF Symbols)~~ **done 2026-07-07**. Surface/FileTree/
   FindMatcher are Kit models with tests; chrome is semantic, the diff
   surface keeps One Dark (two-regime split per modern-macos-feel.md).
3. ~~Engine integration~~ **done 2026-07-07**: statusbar HUD chips
   (SwiftUI island, glass gated macOS 26; hud --json single-flight on
   watch events + 45s timer), receipts panel ⌘J (run --events NDJSON
   streaming into disclosure rows), Run Checks ⌘R (Reload moved to
   ⌘⇧R). Deferred: chip click actions for fetch/sync.
4. ~~Feedback viewer + draft comments~~ **done 2026-07-07**: Kit ports
   of comments.rs/feedback.rs (CommentStore, clipboard digest,
   parseFeedback) with tests; gh-CLI transport (GhClient, OAuth
   replaces it in step 5); feedback inspector pane ⌘⇧J (filters,
   copy/copy-all, click-to-anchor); drafts via row selection + ⌘⇧M
   popover; gutter comment badges. Deferred: inline chips under
   anchored rows (badges + panel for now), file filter in panel.
5. ~~GitHub OAuth device flow + Keychain~~ **done 2026-07-07**:
   GitHubAuth (device flow, no wrapper deps — researched, see design
   doc), Keychain token, GitHubClient (GraphQL PR status + REST
   feedback), Settings ⌘, (client ID + sign in/out), toolbar PR chip
   (60s + focus polling), gh-CLI fallback when signed out. NOTE: user
   must create a GitHub OAuth app with device flow enabled and paste
   its client ID into Settings to sign in.
6. ~~App bundle/icon/vendored libgit2~~ **done 2026-07-07** via the
   vendored packaging skill: `swift/Scripts/package_app.sh` (bundles
   Preceipts.app, vendors the libgit2→llhttp/libssh2→openssl dylib
   chain to @rpath, ad-hoc signs by default), `build_icon.sh` +
   `generate_icon.swift` (programmatic One Dark diff/claw icon →
   Icon.icns, committed), `compile_and_run.sh`, and the template
   `setup_dev_signing.sh` / `sign-and-notarize.sh` (notarization needs
   a Developer ID + App Store Connect key — not run). Finder launches
   get an NSOpenPanel repo chooser. `swift/version.env` holds versions.

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
