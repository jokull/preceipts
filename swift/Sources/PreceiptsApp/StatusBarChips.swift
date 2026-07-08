// The statusbar HUD: the TUI footer dissolved into quiet chips
// (docs/desktop-app-design.md "HUD → app chrome"). A SwiftUI island so
// Liquid Glass can be gated in on Tahoe; chrome regime — semantic colors,
// SF Symbols, capsule shapes throughout.

import AppKit
import PreceiptsKit
import SwiftUI

final class StatusBarModel: ObservableObject {
    @Published var hud: EngineHud?
    @Published var hudError: String?
    @Published var runInProgress = false
    /// Diff stats — set by the surface on each changeset.
    @Published var filesCount = 0
    @Published var added = 0
    @Published var removed = 0
    @Published var scopeLabel = ""
    /// The branch's open PR — identity + review/CI state, click for the
    /// drawer. Nil hides the component (local-only degradation).
    @Published var pr: PrStatus?
    /// Load failure text; replaces the chip row while present.
    @Published var message: String?
    var onReceiptsTap: (() -> Void)?
    var onPrTap: (() -> Void)?
    var onReloadTap: (() -> Void)?
}

struct StatusBarChips: View {
    @ObservedObject var model: StatusBarModel

    var body: some View {
        HStack(spacing: 8) {
            if let message = model.message {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            } else {
                chipRow
            }
            Spacer(minLength: 12)
            // Trailing cluster never truncates — the chips yield instead.
            HStack(spacing: 10) {
                if model.pr != nil {
                    prChip
                }
                statsText
                    .font(.caption.monospacedDigit())
                reloadButton
            }
            .fixedSize()
            .layoutPriority(1)
        }
        .padding(.horizontal, 12)
    }

    /// "#2548 title · review · CI" — identity lives in the HUD with the
    /// rest of the repo state; click opens the PR drawer.
    @ViewBuilder private var prChip: some View {
        if let pr = model.pr {
            let label = HStack(spacing: 5) {
                Text(verbatim: "#\(pr.number)")
                    .fontWeight(.semibold)
                Text(pr.title)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: 260, alignment: .leading)
                    .foregroundStyle(.secondary)
                if let review = pr.reviewDecision {
                    reviewIcon(review)
                }
                if let ci = pr.ciState {
                    ciIcon(ci)
                }
            }
            .font(.caption)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .contentShape(Capsule())

            Button(action: { model.onPrTap?() }) {
                if #available(macOS 26, *) {
                    label.glassEffect(.regular.interactive(), in: .capsule)
                } else {
                    label.background(Capsule().fill(Color.secondary.opacity(0.12)))
                }
            }
            .buttonStyle(.plain)
            .help("\(pr.title) \u{2014} pull request panel")
        }
    }

    private var statsText: Text {
        Text(verbatim: "\(model.filesCount) files ").foregroundColor(.secondary)
            + Text(verbatim: "+\(model.added)").foregroundColor(.green)
            + Text(verbatim: " \u{2212}\(model.removed)").foregroundColor(.red)
            + Text(verbatim: "  \u{00b7}  \(model.scopeLabel)").foregroundColor(.secondary)
    }

    private var reloadButton: some View {
        Button(action: { model.onReloadTap?() }) {
            Image(systemName: "arrow.clockwise")
                .foregroundStyle(.secondary)
        }
        .buttonStyle(.plain)
        .help("Reload the diff (\u{2318}\u{21e7}R)")
    }

    @ViewBuilder private func reviewIcon(_ decision: PrStatus.ReviewDecision) -> some View {
        switch decision {
        case .approved:
            Image(systemName: "checkmark.seal.fill")
                .foregroundStyle(.green)
                .help("Approved")
        case .changesRequested:
            Image(systemName: "exclamationmark.octagon.fill")
                .foregroundStyle(.red)
                .help("Changes requested")
        case .reviewRequired:
            Image(systemName: "hourglass.circle")
                .foregroundStyle(.secondary)
                .help("Review required")
        }
    }

    @ViewBuilder private func ciIcon(_ state: PrStatus.CiState) -> some View {
        switch state {
        case .success:
            Image(systemName: "checkmark.circle.fill")
                .foregroundStyle(.green)
                .help("CI green")
        case .failure, .error:
            Image(systemName: "xmark.circle.fill")
                .foregroundStyle(.red)
                .help("CI failing")
        case .pending, .expected:
            Image(systemName: "clock.badge.questionmark")
                .foregroundStyle(.orange)
                .help("CI running")
        }
    }

    @ViewBuilder private var chipRow: some View {
        if #available(macOS 26, *) {
            GlassEffectContainer(spacing: 8) { chips }
        } else {
            chips
        }
    }

    @ViewBuilder private var chips: some View {
        HStack(spacing: 8) {
            receiptsChip
            if let hud = model.hud {
                baseChip(hud)
                mergeChip(hud)
                if hud.dirty {
                    Chip(symbol: "pencil.circle", text: "Dirty", tint: .secondary)
                        .help("The working tree has uncommitted changes")
                }
                if let unsynced = hud.unsyncedReceipts, unsynced > 0 {
                    Chip(
                        symbol: "arrow.up.circle", text: "\(unsynced) unsynced",
                        tint: .secondary
                    )
                    .help("Receipt lines origin doesn't have yet — preceipts-engine sync")
                }
            } else if let error = model.hudError {
                Chip(symbol: "exclamationmark.triangle", text: error, tint: .orange)
                    .lineLimit(1)
            }
        }
    }

    // Receipts verdict — the one interactive chip; click opens the panel.
    @ViewBuilder private var receiptsChip: some View {
        let (symbol, text, tint) = receiptsVerdict
        // Tinted icon, primary-color text: the verdict reads in the glyph
        // and the label stays legible on any capsule/glass fill.
        let label = HStack(spacing: 4) {
            if model.runInProgress {
                ProgressView()
                    .controlSize(.mini)
            } else {
                Image(systemName: symbol)
                    .foregroundStyle(tint)
            }
            Text(text)
                .foregroundStyle(.primary)
        }
        .font(.caption)
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .contentShape(Capsule())

        Button(action: { model.onReceiptsTap?() }) {
            // Glass belongs on the element itself, not on a background
            // shape — a glass background composites as its own layer and
            // can occlude the label.
            if #available(macOS 26, *) {
                label.glassEffect(
                    .regular.tint(tint.opacity(0.2)).interactive(), in: .capsule)
            } else {
                label.background(Capsule().fill(tint.opacity(0.12)))
            }
        }
        .buttonStyle(.plain)
        .help("Receipts for the working tree — click for the panel (\u{2318}J)")
    }

    private var receiptsVerdict: (String, String, Color) {
        guard let hud = model.hud else {
            return ("circle.dotted", model.runInProgress ? "Running\u{2026}" : "Receipts", .secondary)
        }
        if model.runInProgress {
            return ("circle.dotted", "Running\u{2026}", .secondary)
        }
        if hud.green {
            return ("checkmark.circle.fill", "Receipts", .green)
        }
        let required = hud.status.rows.filter(\.required)
        let failed = required.filter { $0.state == .fail }.count
        if failed > 0 {
            return ("xmark.circle.fill", "\(failed) failed", .red)
        }
        return ("circle.dashed", "Unproven", .secondary)
    }

    @ViewBuilder private func baseChip(_ hud: EngineHud) -> some View {
        Chip(
            symbol: hud.behind > 0 ? "arrow.triangle.branch" : "checkmark",
            text: "\(hud.base) \u{2191}\(hud.ahead) \u{2193}\(hud.behind)",
            tint: hud.behind > 0 ? .orange : .secondary
        )
        .help(
            hud.landFresh
                ? "Base is the merge-base — a land now is proven"
                : "Base moved — the squash tree would be unproven")
    }

    @ViewBuilder private func mergeChip(_ hud: EngineHud) -> some View {
        if hud.mergeClean == false {
            Chip(
                symbol: "exclamationmark.triangle.fill",
                text: "\(hud.conflictFiles.count) conflicts",
                tint: .red
            )
            .help(hud.conflictFiles.prefix(8).joined(separator: "\n"))
        } else if hud.mergeClean == true {
            Chip(symbol: "arrow.triangle.merge", text: "Merges clean", tint: .secondary)
        }
    }

}

/// A quiet, non-interactive statusbar chip: tinted glyph, legible label.
private struct Chip: View {
    let symbol: String
    let text: String
    let tint: Color

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: symbol)
                .foregroundStyle(tint)
            Text(text)
                .foregroundStyle(.primary)
        }
        .font(.caption)
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(Capsule().fill(tint.opacity(0.10)))
    }
}
