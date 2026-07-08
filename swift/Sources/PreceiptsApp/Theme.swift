// Two regimes (docs/modern-macos-feel.md): the diff surface keeps its own
// One Dark palette (content is ours); all chrome — sidebar, toolbar,
// statusbar, find bar — uses semantic system colors so it tracks
// light/dark/accent and Tahoe materials.

import AppKit
import PreceiptsKit

/// Chrome layout tokens — 4pt grid, no arbitrary values inline.
enum Metrics {
    static let unit: CGFloat = 4
    static let padding: CGFloat = 8
    static let paddingWide: CGFloat = 12
    /// Pane-edge padding (inspector/sidebar content regions).
    static let paddingXL: CGFloat = 16
    static let statusBarHeight: CGFloat = 28
    static let findBarHeight: CGFloat = 36
    static let receiptsPanelHeight: CGFloat = 240
    static let prPanelHeight: CGFloat = 280
    static let sidebarMinWidth: CGFloat = 200
    static let sidebarInitialWidth: CGFloat = 260
    static let surfaceMinWidth: CGFloat = 480
}

enum Theme {
    static let bg = NSColor(srgbRed: 0.118, green: 0.133, blue: 0.153, alpha: 1)
    static let bgPanel = NSColor(srgbRed: 0.137, green: 0.153, blue: 0.180, alpha: 1)
    static let bgHeader = NSColor(srgbRed: 0.173, green: 0.192, blue: 0.227, alpha: 1)
    static let fg = NSColor(srgbRed: 0.784, green: 0.800, blue: 0.831, alpha: 1)
    static let fgMuted = NSColor(srgbRed: 0.498, green: 0.518, blue: 0.557, alpha: 1)
    static let addBg = NSColor(srgbRed: 0.125, green: 0.208, blue: 0.153, alpha: 1)
    static let addBgWord = NSColor(srgbRed: 0.180, green: 0.341, blue: 0.220, alpha: 1)
    static let delBg = NSColor(srgbRed: 0.227, green: 0.149, blue: 0.165, alpha: 1)
    static let delBgWord = NSColor(srgbRed: 0.361, green: 0.196, blue: 0.220, alpha: 1)
    static let emptyBg = NSColor(srgbRed: 0.133, green: 0.149, blue: 0.169, alpha: 1)
    static let accent = NSColor(srgbRed: 0.380, green: 0.686, blue: 0.937, alpha: 1)
    static let green = NSColor(srgbRed: 0.596, green: 0.765, blue: 0.475, alpha: 1)
    static let red = NSColor(srgbRed: 0.878, green: 0.424, blue: 0.459, alpha: 1)
    static let yellow = NSColor(srgbRed: 0.898, green: 0.753, blue: 0.482, alpha: 1)

    static let font = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
    static let rowHeight: CGFloat = 20

    static func statusColor(_ status: String) -> NSColor {
        switch status {
        case "A": return green
        case "D": return red
        default: return yellow
        }
    }

    // One Dark syntax palette, ported from the GPUI prototype
    // (app/src/theme.rs, git history). Kind indexes highlightNames; families map to
    // colors, with variable.* refined by full name.
    static func syntaxColor(_ kind: UInt8?) -> NSColor {
        guard let kind, Int(kind) < highlightNames.count else { return fg }
        let name = highlightNames[Int(kind)]
        let family = name.split(separator: ".").first.map(String.init) ?? name
        switch family {
        case "keyword": return rgb(0xc678dd)
        case "string": return rgb(0x98c379)
        case "comment": return rgb(0x5c6370)
        case "function", "constructor": return rgb(0x61afef)
        case "type", "module", "label": return rgb(0xe5c07b)
        case "number", "constant": return rgb(0xd19a66)
        case "property", "attribute", "tag": return rgb(0xe06c75)
        case "operator": return rgb(0x56b6c2)
        case "punctuation": return rgb(0x9da5b4)
        case "variable":
            switch name {
            case "variable.builtin": return rgb(0xe06c75)
            case "variable.parameter", "variable.member": return rgb(0xd19a66)
            default: return fg
            }
        default: return fg
        }
    }

    private static func rgb(_ value: UInt32) -> NSColor {
        NSColor(
            srgbRed: CGFloat((value >> 16) & 0xFF) / 255,
            green: CGFloat((value >> 8) & 0xFF) / 255,
            blue: CGFloat(value & 0xFF) / 255,
            alpha: 1
        )
    }
}
