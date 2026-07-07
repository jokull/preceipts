# Modern macOS feel (Tahoe+) — resource audit + recommendations

*2026-07-07. Source: [twostraws/swift-agent-skills](https://github.com/twostraws/swift-agent-skills)
(a link directory, not skills itself) — every listed repo was inventoried,
most pruned, the survivors read in full. This doc feeds the design pass in
[desktop-app-design.md](./desktop-app-design.md).*

## The prune

**Cut (iOS or wrong layer for a macOS diff cockpit):** SwiftData ×2,
Core Data, Widgets, App Intents, App Store ×4 (changelog/ASO/review),
Background Execution, Focus Engine (tvOS), iOS Simulator, Figma→SwiftUI,
iOS Code Audit, Swift Security (generic checklists), Swift FormatStyle,
SwiftData/CoreData experts. The accessibility skills are respectable but
iOS-shaped; revisit at the polish stage.

**Kept — vendored into `.claude/skills/` (all MIT, licenses included):**

1. **`swiftui-liquid-glass`** (Thomas Ricouard) — the only Tahoe-specific
   skill in the whole directory. Concrete API guidance: `glassEffect()`,
   `GlassEffectContainer`, `glassEffectID` morphing, `.buttonStyle(.glass)`,
   availability gating, review checklist. SwiftUI-flavored but the
   concepts map 1:1 to AppKit's `NSGlassEffectView` /
   `NSGlassEffectContainerView`.
2. **`macos-spm-app-packaging`** (Thomas Ricouard) — exactly our step 6:
   SwiftPM macOS app without Xcode — `.app` bundle assembly scripts,
   dev signing, notarytool + stapling, Sparkle appcast, Icon Composer
   icns. Includes a failure-mode table for notarization. Ricouard builds
   his macOS apps this way; the templates are battle-tested.
3. **`writing-for-interfaces`** (Andrew Gleave) — HIG-grounded interface
   copy. A cockpit is dense with microcopy (empty states, error states,
   receipt verdicts, confirmation affordances); this is the difference
   between "dev tool" and "product" feel.

**Noted, not vendored:**

- **swiftui-design-principles** (Arjit Jaiswal) — the best
  general-purpose design discipline in the list: 4pt spacing grid,
  hierarchy through weight not size, semantic system colors over
  hardcoded values, proportional component sizing, system patterns over
  custom cards. Its rules inform the design tokens below; vendor it if
  chrome moves to SwiftUI.
- **swiftui-pro** (Paul Hudson) + **SwiftAgents AGENTS.md** — solid
  review baselines, SwiftUI/iOS-centric. Pull in when (if) chrome goes
  SwiftUI.
- **swift-concurrency-expert** (Ricouard) — not look-and-feel, but the
  reference we want when the tools-6 / strict-concurrency migration
  lands (see constraint below).

## Recommendations for preceipts.app

The app today is pre-Tahoe by construction: hand-rolled `NSView` chrome,
hard-coded One Dark colors everywhere, no toolbar, a plain `NSSplitView`.
Tahoe's Liquid Glass restyling flows through the **standard chrome
primitives** — apps that hand-roll chrome get none of it. So the moves,
in order of feel-per-effort:

1. **Adopt standard chrome, get Tahoe for free.**
   - `NSSplitViewController` with a `.sidebar`-behavior split item
     (full-height sidebar material) instead of the raw `NSSplitView`.
   - A real `NSToolbar` (unified style) carrying scope toggle, reload,
     search — instead of menu-only actions. Toolbar items get glass
     treatment automatically on Tahoe.
   - `NSVisualEffectView` materials (`.sidebar`, `.headerView`) or —
     gated `#available(macOS 26, *)` — `NSGlassEffectView` for the
     statusbar/HUD chips. Never custom-blur.
2. **Split the theme into two regimes.** The diff surface keeps its own
   palette (One Dark today; content is ours). The **chrome must switch to
   semantic system colors** (`.windowBackgroundColor`, `.labelColor`,
   `.secondaryLabelColor`, accent-tint) so it tracks light/dark/accent
   and Tahoe materials. Hard-coded chrome colors are what makes an app
   feel "ported".
3. **SF Symbols for every glyph** (status letters A/M/D/R chips, receipt
   verdict icons, toolbar). Text-as-icon reads as TUI residue.
4. **Design tokens** (from swiftui-design-principles): one constants
   enum — 4pt spacing grid, two or three type sizes with weight-based
   hierarchy, consistent corner radii. No arbitrary values inline.
5. **Deployment strategy:** keep SPM at `.v14`/tools-5.9 for now and gate
   Tahoe API at runtime (`#available(macOS 26, *)`) — the Xcode 26 SDK we
   build with has the symbols. Declaring a `.v26` platform needs
   swift-tools ≥6, which drags strict concurrency across the package —
   do that as its own migration (with swift-concurrency-expert loaded),
   not as a side effect of styling.
6. **Chrome in SwiftUI where it pays, surface stays AppKit.** The
   feedback panel, HUD chips, and find bar are small stateful panels —
   `NSHostingView` islands let us use `.glassEffect()` and modern
   controls directly. The diff surface remains the custom fixed-row
   AppKit view; that decision is unaffected (foundations doc).
7. **Microcopy pass with writing-for-interfaces** once the panels exist:
   empty states, unproven/failed receipt wording, error surfaces.
8. **Step 6 (bundle/notarize) uses `macos-spm-app-packaging` verbatim** —
   its scripts replace what we'd have written from scratch, including
   Sparkle if we want auto-updates.
