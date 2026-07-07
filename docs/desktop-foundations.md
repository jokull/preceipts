# Desktop foundations research — moving the cockpit out of the TUI

*2026-07-07. Four parallel research tracks: (1) known macOS diff apps and their
tech, (2) native macOS text foundations, (3) GPU UI engines, (4) web-stack
honesty check + open-source fork candidates. Sublime Text is the spirit
animal: instant startup, buttery scroll, keyboard-first, one purpose.*

## The five convergent findings

**1. The "Sublime feel" has only ever shipped on native or fully custom
rendering.** Sublime Merge: custom C++ toolkit ("skyline"), Skia CPU raster +
in-house OpenGL batching (52ms → 3ms per text frame,
[first-party writeup](https://www.sublimetext.com/blog/articles/hardware-accelerated-rendering),
[Tristan Hume's teardown](https://thume.ca/2016/12/03/disassembling-sublime-text/)).
GitUp: ObjC/AppKit custom views over libgit2, 40k commits rendered <1s.
Kaleidoscope, Fork, Tower: AppKit. Zed: GPUI/Metal. Every webview app
(GitKraken, GitButler's UI) is the cautionary tale, never the benchmark.
GitHub's own [diff-lines post-mortem](https://github.blog/engineering/architecture-optimization/the-uphill-climb-of-making-diff-lines-performant/)
and [Pierre's rendering essay](https://pierre.computer/writing/on-rendering-diffs)
show heroic engineering landing at "responsive," not "instant."

**2. Own the git read path.** The two supernaturally fast apps (Sublime Merge,
GitUp) both bypass the `git` binary for reads — custom ODB parser / libgit2
into an in-memory model — and shell out only for mutations, with optimistic
UI. Never block a keystroke on a subprocess.

**3. A diff view is not a text editor.** Read-only + line-oriented kills
everything hard about text views (editing, IME, height re-estimation, undo).
Fixed-height precomputed rows make scroll geometry over 100k+ lines trivial
*by construction*. TextKit 2 is distrusted by its heaviest users
([STTextView author's 22+ radars](https://blog.krzyzanowskim.com/2025/08/14/textkit-2-the-promised-land/));
Nova, CodeEdit, Runestone all wrote custom Core Text line layout. Kaleidoscope
is read-only *because* editing needs a different machine. This matches our own
hunk-fork experiment (React row tree too slow → immediate-mode rasterization).

**4. The niche is empty.** No open-source "PR-diff cockpit desktop app" exists
to fork (the way lumen was forked for the TUI). Closest artifacts: hunk (TUI,
right interaction model, MIT), Zed's project-diff multibuffer (right
architecture, GPL, inside an editor), GitUpKit (right macOS design decisions,
GPL, 10-year-old ObjC), `@pierre/diffs` (right web renderer, MIT). Empty niche
= opportunity + no shortcut.

**5. The compute layer is settled, whatever the UI.**
- Diff: **imara-diff** (histogram, the gitoxide/Helix engine; two-pass
  coarse line → fine word/char for intraline). difftastic only as an opt-in
  per-file lens (blows up on large changes). Steal VS Code's word-matching
  heuristics ([vscode-diff](https://github.com/micnil/vscode-diff)).
- Reads: **gix (gitoxide)** in-process for trees/ODB/status.
- Highlighting: **tree-sitter** (incremental — matches watch-mode live
  updates), highlight only visible hunks lazily on background threads.
- Watching: Helix's pattern — background diff task per buffer, UI never blocks.

## The three candidate stacks

### 1. GPUI (Zed's framework) — recommended

- **Apache-2.0, on crates.io** (0.2.x since Oct 2025); macOS rendering is
  Metal + CoreText, native menus/IME proven daily in Zed.
- **The use case has existence proofs**: Zed's split-diffs/multibuffers were
  built for multi-hundred-file changesets
  ([blog](https://zed.dev/blog/split-diffs));
  [gpui-component](https://github.com/longbridge/gpui-component) (Apache-2.0)
  ships virtualized lists/tables, dock layouts, and a 200K-line tree-sitter
  code editor component, shipped in production (Longbridge Pro); a third-party
  app (Arbor) is already building a git-worktree/diff cockpit on GPUI.
- **Agent leverage is maximal**: plain Rust, Tailwind-like builder API, and
  the entire Zed codebase as a greppable idiom corpus. Cockpit stays Rust —
  RunEvent/HUD/status models port straight from the TUI.
- **Sharp edges, accepted knowingly**: pre-1.0 churn tied to Zed's release
  train (pin + budget upgrade PRs); ~zero screen-reader support today; open
  GPL-transitive-dep issue
  ([zed#55470](https://github.com/zed-industries/zed/issues/55470)) to watch
  if we ever ship proprietary binaries.

### 2. Swift/AppKit custom row renderer — the maximal-polish alternative

Custom line-based diff surface (Core Text rows, layer-backed, fixed line
height, two panes over one aligned row model with phantom rows) + **Neon +
SwiftTreeSitter** (BSD/MIT) for highlighting. GitUpKit and CodeEditTextView
(MIT, custom Core Text layout) as references. Gets native menus, IME,
accessibility, and scrolling physics *for free* — the things GPUI makes you
accept as gaps. Estimated 2–4 weeks to a fast two-pane MVP, +2–4 for polish
(selection, find, a11y). Costs: hand-rolled selection/search, a second
language next to the Rust/TS codebase, no Zed-sized idiom corpus, and the git
layer needs bridging (gix has no Swift bindings; libgit2 does).

### 3. Tauri 2 + `@pierre/diffs` — the pragmatic 90%

Rust core (gix + imara-diff), per-hunk view models over binary IPC, Pierre's
MIT renderer (worker-thread Shiki, DOM pooling, proven on 700MB patches —
it's also hunk's engine, so this is "hunk, but a window"). Real numbers:
<0.5s launch, ~40MB idle. Ceiling: WKWebView scroll feel — GitHub-v2/Pierre
responsiveness, not Sublime feel. Falls short of the spirit animal by
exactly the margin that motivated leaving the TUI.

## Decision

**Build on GPUI (#1).** It is the only stack where the exact workload —
Metal-smooth, multi-hundred-file side-by-side diffs with tree-sitter — is
already proven, in Rust we already write, with the strongest AI-agent
leverage. Keep #2 honestly on the table as the escape hatch if GPUI churn or
the a11y gap becomes intolerable — the Ghostty lesson makes that pivot cheap:

**Architecture invariant (survives any engine choice):** keep the
preceipts-engine (TS/Bun: receipts, runs, notes, sync) behind the existing
process/NDJSON boundary, and put all new read-path/diff/highlight compute in
a **UI-agnostic Rust crate** (`preceipts-core`: gix reads, imara-diff,
tree-sitter, watch). The GPUI app is then a thin shell over core — and so
would a Swift shell be, via C ABI, if we ever want maximal-native.

**Product notes from the survey** (steal list):
- Repo-wide instant search *including diff contents* (Sublime Merge, GitUp).
- Draggable hunk context boundaries (Sublime Merge).
- Changeset browser with filters (Kaleidoscope).
- Optimistic UI on every git mutation.
- Command palette as the keyboard spine.
- Pricing, eventually: perpetual license + N years of updates (Sublime's
  model — the goodwill-maximizing structure in this exact niche; both
  subscription pivots in this market caused visible exoduses).

## Open questions

1. Does the cockpit stay a lumen descendant in spirit (bare `preceipts`
   opens the window) with the TUI kept as a fallback (`preceipts --tui`)?
2. gix vs shelling to git for the *initial* read path — gix all-in from day
   one, or port incrementally from the TUI's plumbing?
3. Is Zed's multibuffer (one scroll surface spanning all files) the right
   diff model for preceipts, vs per-file panes with a file tree (the
   TUI/Kaleidoscope model)? Multibuffer is the bolder, more keyboard-native
   answer and what GPUI is best at.
