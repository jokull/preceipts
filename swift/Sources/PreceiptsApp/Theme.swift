// v0 theme: One Dark-adjacent, matching the GPUI prototype. System
// appearance adaptation comes with the design pass.

import AppKit

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
}
