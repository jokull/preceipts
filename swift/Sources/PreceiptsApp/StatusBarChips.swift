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
    /// "181 files  +1200 −340" — set by the surface on each changeset.
    @Published var filesSummary = ""
    /// Load failure text; replaces the chip row while present.
    @Published var message: String?
    var onReceiptsTap: (() -> Void)?
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
            Text(model.filesSummary)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.horizontal, 12)
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
