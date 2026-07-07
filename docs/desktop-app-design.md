# preceipts.app — desktop design

*2026-07-07. Successor to the TUI cockpit (PRD decisions 10 + 11). Stack:
**all-native Swift/AppKit** — the app owns UI *and* compute. libgit2 (C,
in-process) provides git reads and line diffs (patience + indent-heuristic
flags, the same xdiff lineage as git's histogram); tree-sitter (C runtime
+ grammar SPM packages, used directly so byte offsets stay UTF-8 — the
SwiftTreeSitter wrapper works in UTF-16) provides highlighting, with the
Rust registry's queries ported verbatim and text predicates
(#eq?/#match?/#any-of?) evaluated in Swift; the display algorithm's novel
passes (similarity-gated line
pairing → word-level intraline → segment composition) are Swift ports of
the tested Rust originals; FSEvents drives watch-mode reloads
(single-flight, coalesced); Keychain + URLSession handle GitHub. The diff
surface is a custom fixed-row-height Core Text view — the native path from
[desktop-foundations.md](./desktop-foundations.md), proven by
GitUp/Nova/Kaleidoscope. The TS/Bun `preceipts-engine` is unchanged behind
its subprocess boundary. Sublime spirit, macOS body: Cmd-key conventions,
native menus, system feel throughout. VS Code's chrome is the layout
reference, not its weight.

Architecture history, honestly: a GPUI implementation (decision 10,
`app/`) validated the surface model, algorithm stack, watch discipline,
and feedback/comments model in a day — its features port 1:1. A "Swift
shell over a Rust-core C FFI" middle ground was considered and dropped the
same day: the heavyweight dependencies are C libraries Swift consumes
directly, so a Rust layer would be a permanent two-toolchain FFI tax
guarding ~500 lines of portable logic. The Rust workspace (TUI, core, GPUI
app) is deprecated reference code until this app reaches parity.*

## Layout

```
┌──────────────────────────────────────────────────────────────────┐
│ titlebar: repo · branch  [Branch diff ⌄ | Uncommitted]  PR #123 ✓│
├──────────────┬───────────────────────────────────────────────────┤
│ file tree    │  one scroll surface (multibuffer)                 │
│  filter ⌕    │  ┌ sticky file header: path · +12 −4 · ☑ viewed ┐ │
│  ☑ M A D R   │  │ side-by-side rows, synced, virtualized        │ │
│  ○ hide      │  │ hunk gap: ⌄ expand context ⌃                  │ │
│    viewed    │  └───────────────────────────────────────────────┘ │
│              │  ┌ next file header … ┐                            │
├──────────────┴───────────────────────────────────────────────────┤
│ receipts panel (toggle ⌘J): check table + live run output        │
├──────────────────────────────────────────────────────────────────┤
│ statusbar: ✓ receipts · origin/main ↑79↓7 · merge ✓ · CI 11/12  │
└──────────────────────────────────────────────────────────────────┘
```

### One scroll surface (decided)

Every changed file in a single virtualized scroll — the Zed multibuffer
model, not per-file panes. Sticky file headers (path, ±stats, viewed
checkbox, collapse, overflow menu). Files collapse; lockfiles/generated
files collapsed by default with a "show diff" affordance. Hunk gaps show
skipped-line counts and expand context on click (Sublime Merge's draggable
boundaries as a later refinement). Side-by-side with word-level intraline
highlights; rows are fixed-height and materialized only when visible —
diff + highlight computed lazily per hunk on background threads.

### Scope toggle (decided: easy + visible)

Segmented control in the titlebar: **Branch diff** (merge-base of the base
branch vs working tree — the PR view, default) | **Uncommitted**. The base
is auto-detected (origin/main → main → origin/master → master) with a
dropdown to pick another ref. Switching scopes preserves scroll position by
file where possible. Shortcut: ⌘⇧D cycles, ⌘1/⌘2 select directly.

### HUD → app chrome (decided)

The TUI footer HUD dissolves into native chrome:

- **Statusbar (persistent, left→right)**: receipts verdict chip
  (✓ green / ✗ N failed / ∅ unproven — click opens receipts panel) ·
  base ↑ahead↓behind (click = fetch) · local merge verdict (conflicts N
  lists files on click) · dirty · unsynced receipts ⇡ (click = sync).
- **Titlebar (identity + GitHub)**: repo · branch · scope control · PR
  association (#123 title, review state, CI rollup) when signed in.
- Everything remains visible-but-quiet: chips, not banners. Timer +
  watch-event refresh semantics carry over from the TUI (single-flight).

### Receipts panel (⌘J)

The TUI rail, grown up: table of prepare steps + checks (state, duration,
runner, agent), live streaming output per row (disclosure triangle), run
button (⌘R), and the run-level note line (normalized / invalidated / lock
refusal). Statusbar chip is its summary; the panel is its detail.

## Diff display algorithm (decided 2026-07-07)

Three-pass, the GitHub/VS Code lineage: **histogram** line diff
(imara-diff, git's default algorithm) → **similarity-gated line pairing**
inside change blocks (order-preserving alignment; only lines that are
plausibly "the same line edited" pair up — below the threshold they render
as removal + addition) → **word-level intraline** ranges on paired lines
(LCS over word tokens). All exact, all linear-ish, recomputed on every
watch event without breaking a sweat.

**difftastic (AST-aware) was evaluated and rejected as the engine**: its
structural graph search degrades catastrophically on large changes
(documented multi-GB/multi-minute cases; its own manual concedes the
scaling limits and falls back to line diffs), which is fatal for a surface
that recomputes 200-file monorepo diffs on watch events. If a per-file
structural lens ever proves worth it, it can be an opt-in view — never the
default path.

## GitHub integration (new)

- **Auth**: OAuth device flow with a public client ID (native app, no
  secret). Token stored in the **macOS Keychain** (`security-framework`
  crate), never on disk. Sign-in lives in Settings and is prompted
  contextually ("Sign in to see PR status"), never required.
- **Data (GraphQL)**: PR associated with the current branch (number, title,
  URL, review decision, mergeability, `statusCheckRollup`), base-branch
  movement. Poll politely (ETag/If-None-Match, ~60s), refresh on window
  focus.
- **Degradation**: without auth (or offline) the app is exactly the local
  tool — HUD chrome shows local-only facts, GitHub chips simply absent.
  GitHub data never gates any local feature.
- Out of scope for v1: posting reviews/comments, merging from the app
  (landing stays the agent's job via the engine).

## PR feedback viewer (new)

Review feedback is part of the cockpit, not a browser tab:

- **Sources**: PR review comments (line-anchored), review bodies, and
  conversation comments — across **users and GitHub Apps** (codex,
  CodeRabbit, CI bots…). Fetched with the same GitHub auth; interim
  implementation may shell to `gh api` before OAuth lands.
- **Two views of one model**: inline anchors in the scroll surface
  (collapsed chips under the anchored row — expand to read thread), and a
  **Feedback panel** listing all comments with filters: author, app/bot vs
  human, file, resolved, outdated. Clicking a comment scrolls the surface
  to its anchor. Comments whose anchor no longer matches the current tree
  are marked *outdated* but still listed.
- **Local comments (drafts, never posted)**: add a comment on any diff row
  (gutter “+” or ⌘⇧M). Stored locally per repo (`.git/preceipts/`), scoped
  to the branch. These are notes-to-agent, not GitHub state.
- **Copy to Clipboard affordances** (the point — feeding coding agents):
  - Per comment: copies a markdown block with full context —
    ```
    apps/next/components/foo.tsx:123
    > const x = useMemo(() => props.a + props.b, [props.a, props.b])
    This memo is unnecessary — props are primitives.
    ```
  - **Copy all** (per filter selection): one digest of every visible
    comment, grouped by file, same format — paste straight into an agent
    session as a worklist.
  - GitHub comments get the same affordance (author attribution included).

## File tree

- **Filters**: fuzzy path filter field (focus: ⌘⇧F… no — see keys: ⌥⌘F),
  change-type chips (M/A/D/R), "hide viewed" toggle.
- **Views**: tree ⇄ flat list toggle (VS Code SCM-style); ±stats per file,
  aggregated per directory; viewed progress (n/m) at the top.
- **Actions** (context menu + shortcuts): open in editor (⌘↩, honors
  $EDITOR/Zed at the current line), reveal in Finder, copy path, copy file
  diff, mark viewed/unviewed (Space), collapse in surface.
- Selection follows scroll (current file highlights as the surface moves)
  and clicking a file scrolls the surface — two views of one model.

## Keyboard map (macOS-native; no modal keys)

| Key | Action |
|---|---|
| ⌘F | Find in diff — find bar overlay, live count, scrollbar tick marks |
| ⌘G / ⌘⇧G | Next / previous match |
| ⌘P | Quick open — fuzzy switcher over changed files |
| ⌘K | Command palette (every action lives here) |
| ⌘B | Toggle file tree sidebar |
| ⌘J | Toggle receipts panel |
| ⌘R | Run checks (engine `run --events`) |
| ⌘⇧D / ⌘1 ⌘2 | Toggle / select diff scope |
| ⌃⌘↓ / ⌃⌘↑ | Next / previous file header |
| ⌥↓ / ⌥↑ | Next / previous hunk |
| Space | Toggle viewed on current file |
| ⌘↩ | Open current file in editor at line |
| ⌥⌘F | Focus file-tree filter |
| ⌘, | Settings (GitHub sign-in, base branch, editor, theme) |

Arrows/PageUp/PageDown/Home/End scroll natively. All bindings appear in
real menu-bar menus (File, View, Go, Run) so they're discoverable and
remappable the macOS way. `n/p/j/k/t/R` and friends are gone.

## Non-negotiables carried from the TUI

- View + monitor only: the app never mutates the repo (no staging, no
  landing). Engine runs are the one exception, triggered explicitly.
- Always current: watch-driven, single-flight refreshes; the diff follows
  the working tree while an agent edits.
- Instant startup, no blocking subprocesses on the input path (gix reads
  in-process; optimistic/async everything).
- Engine boundary unchanged: NDJSON `run --events`, `hud --json`,
  `status --json` from `preceipts-engine`.

## Build shape

`swift/` — SPM package, the app:
- `Clibgit2`: system-library target over Homebrew libgit2 (pkg-config);
  static/vendored libgit2 comes with app-bundle packaging.
- `PreceiptsKit`: the brains — GitReader (libgit2 reads), LineDiff
  (libgit2 xdiff + pairing + intraline → DiffRow model), ChangesetLoader,
  Segments. Fully covered by the test suite ported from the Rust core.
- `PreceiptsApp`: AppKit shell — window, sidebar, fixed-row diff surface
  (custom-drawn rows), statusbar, FSEvents watcher, menus.
- Dev loop: `swift build && swift test` in `swift/`;
  `swift run PreceiptsApp <repo>` to launch. App bundle + notarization
  come later.

`engine/` — TS/Bun, unchanged (receipts, runs, hud, land, sync, gc).

Rust workspace (root TUI + `core/` + `app/` GPUI) — deprecated reference;
delete when the Swift app reaches parity on the hard requirements.

Sequencing from here: 1) ~~tree-sitter highlighting~~ **done** (raw C
API; per-file parallel in the loader; 512KB cap so generated giants
render plain; same-node precedence verified against the Rust reference)
· 2) ⌘F find
+ NSOutlineView file tree + sticky headers · 3) engine integration
(receipts panel, statusbar HUD chips via `hud --json` / `run --events`)
· 4) feedback viewer + draft comments port (models exist in the Rust
reference) · 5) GitHub OAuth + Keychain · 6) app bundle, icon, vendored
libgit2, notarization.
