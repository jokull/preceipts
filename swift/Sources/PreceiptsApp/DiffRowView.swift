// One fixed-height row of the diff surface, custom-drawn: two synced
// halves (old/new), gutter line numbers, per-segment styling. Custom
// drawing over precomputed rows is the native fast path
// (docs/desktop-foundations.md).

import AppKit
import PreceiptsKit

final class DiffRowView: NSView {
    var surfaceRow: SurfaceRow?
    var changeset: Changeset?
    /// Active ⌘F query — occurrences get a highlight background.
    var findQuery: String?
    var isCurrentFindMatch = false
    /// Row selection (for comment anchoring) — drawn as an accent edge bar.
    var isRowSelected = false
    /// Comments anchored to this row — drawn as a trailing badge.
    var commentBadge = 0

    /// This row's slice of the open thread's claw — the accent bracket
    /// hugging the anchored line range on the commented side.
    enum ClawSegment {
        case single, top, middle, bottom
    }
    var claw: (segment: ClawSegment, side: CommentSide)?

    override var isFlipped: Bool { true }

    private static let gutterWidth: CGFloat = 46

    override func draw(_ dirtyRect: NSRect) {
        guard let surfaceRow, let changeset else {
            Theme.bg.setFill()
            bounds.fill()
            return
        }
        switch surfaceRow {
        case .fileHeader(let file):
            drawFileHeader(changeset.files[file])
        case .gap(let skipped):
            drawGap(skipped)
        case .line(let file, let hunk, let row):
            let fileDiff = changeset.files[file]
            drawLine(fileDiff.hunks[hunk].rows[row], file: fileDiff)
        }
        if commentBadge > 0 {
            drawCommentBadge()
        }
        if let claw {
            drawClaw(claw.segment, side: claw.side)
        }
        if isRowSelected {
            NSColor.controlAccentColor.setFill()
            NSRect(x: 0, y: 0, width: 3, height: bounds.height).fill()
        }
    }

    /// A 2px accent bracket at the commented half's edge: nubs at the
    /// range ends, a straight rail between — a claw around the lines.
    private func drawClaw(_ segment: ClawSegment, side: CommentSide) {
        let x = side == .old ? 3.0 : bounds.width / 2 + 3.0
        let width = 2.0
        let nub = 6.0
        // Wash the commented half so the range reads even mid-scroll.
        NSColor.controlAccentColor.withAlphaComponent(0.07).setFill()
        let half = NSRect(
            x: side == .old ? 0 : bounds.width / 2, y: 0,
            width: bounds.width / 2, height: bounds.height)
        half.fill()

        NSColor.controlAccentColor.setFill()
        NSRect(x: x, y: 0, width: width, height: bounds.height).fill()
        if segment == .top || segment == .single {
            NSRect(x: x, y: 0, width: nub, height: width).fill()
        }
        if segment == .bottom || segment == .single {
            NSRect(x: x, y: bounds.height - width, width: nub, height: width).fill()
        }
    }

    private func drawCommentBadge() {
        let text = NSAttributedString(
            string: "\(commentBadge)",
            attributes: [
                .font: NSFont.systemFont(ofSize: 9, weight: .semibold),
                .foregroundColor: NSColor.white,
            ])
        let textSize = text.size()
        let iconSide: CGFloat = 9
        let badge = NSRect(
            x: bounds.width - textSize.width - iconSide - 26,
            y: (bounds.height - 14) / 2,
            width: textSize.width + iconSide + 16,
            height: 14)
        let path = NSBezierPath(
            roundedRect: badge, xRadius: badge.height / 2, yRadius: badge.height / 2)
        NSColor.controlAccentColor.withAlphaComponent(0.85).setFill()
        path.fill()

        let config = NSImage.SymbolConfiguration(pointSize: iconSide, weight: .medium)
            .applying(.init(paletteColors: [.white]))
        if let icon = NSImage(systemSymbolName: "text.bubble.fill", accessibilityDescription: nil)?
            .withSymbolConfiguration(config)
        {
            icon.draw(
                in: NSRect(
                    x: badge.minX + 6, y: badge.midY - iconSide / 2,
                    width: iconSide, height: iconSide))
        }
        text.draw(
            at: NSPoint(
                x: badge.minX + iconSide + 9, y: badge.midY - textSize.height / 2))
    }

    private func baseline(_ font: NSFont) -> CGFloat {
        (bounds.height - font.capHeight) / 2 - font.descender / 2
    }

    private func drawText(_ attributed: NSAttributedString, x: CGFloat, clipTo: NSRect? = nil) {
        NSGraphicsContext.current?.saveGraphicsState()
        if let clipTo {
            clipTo.clip()
        }
        let y = (bounds.height - attributed.size().height) / 2
        attributed.draw(at: NSPoint(x: x, y: y))
        NSGraphicsContext.current?.restoreGraphicsState()
    }

    private func drawFileHeader(_ file: FileDiff) {
        Theme.bgHeader.setFill()
        bounds.fill()
        let text = NSMutableAttributedString()
        text.append(
            NSAttributedString(
                string: file.status.rawValue + "  ",
                attributes: [
                    .font: Theme.font, .foregroundColor: Theme.statusColor(file.status.rawValue),
                ]
            ))
        var title = file.path
        if let old = file.oldPath {
            title += " \u{2190} \(old)"
        }
        text.append(
            NSAttributedString(
                string: title + "  ",
                attributes: [.font: Theme.font, .foregroundColor: Theme.fg]
            ))
        text.append(
            NSAttributedString(
                string: "+\(file.added) ",
                attributes: [.font: Theme.font, .foregroundColor: Theme.green]
            ))
        text.append(
            NSAttributedString(
                string: "\u{2212}\(file.removed)",
                attributes: [.font: Theme.font, .foregroundColor: Theme.red]
            ))
        if file.isBinary {
            text.append(
                NSAttributedString(
                    string: "  binary",
                    attributes: [.font: Theme.font, .foregroundColor: Theme.fgMuted]
                ))
        }
        applyFindHighlight(text)
        drawText(text, x: 12)
    }

    /// Paint ⌘F occurrences over an already-composed row string. Character
    /// content matches the source text, so NSString range search lines up.
    private func applyFindHighlight(_ text: NSMutableAttributedString) {
        guard let findQuery, !findQuery.isEmpty else { return }
        let haystack = text.string as NSString
        var searchRange = NSRange(location: 0, length: haystack.length)
        while searchRange.length > 0 {
            let found = haystack.range(of: findQuery, options: [.caseInsensitive], range: searchRange)
            guard found.location != NSNotFound else { break }
            if isCurrentFindMatch {
                text.addAttributes(
                    [.backgroundColor: NSColor.findHighlightColor, .foregroundColor: NSColor.black],
                    range: found)
            } else {
                text.addAttribute(
                    .backgroundColor,
                    value: NSColor.findHighlightColor.withAlphaComponent(0.35),
                    range: found)
            }
            let next = found.location + found.length
            searchRange = NSRange(location: next, length: haystack.length - next)
        }
    }

    private func drawGap(_ skipped: Int) {
        Theme.bgPanel.setFill()
        bounds.fill()
        let text = NSAttributedString(
            string: "\u{22ef} \(skipped) unchanged lines",
            attributes: [.font: Theme.font, .foregroundColor: Theme.fgMuted]
        )
        drawText(text, x: 12)
    }

    private func drawLine(_ row: DiffRow, file: FileDiff) {
        let half = bounds.width / 2
        drawSide(
            row, file: file, old: true,
            rect: NSRect(x: 0, y: 0, width: half, height: bounds.height))
        drawSide(
            row, file: file, old: false,
            rect: NSRect(x: half, y: 0, width: bounds.width - half, height: bounds.height))
    }

    private func drawSide(_ row: DiffRow, file: FileDiff, old: Bool, rect: NSRect) {
        let line = old ? row.old : row.new
        let (lineBg, wordBg): (NSColor, NSColor)
        switch (row.kind, old, line != nil) {
        case (.context, _, _): (lineBg, wordBg) = (Theme.bg, Theme.bg)
        case (_, _, false): (lineBg, wordBg) = (Theme.emptyBg, Theme.emptyBg)
        case (.removal, true, true), (.change, true, true):
            (lineBg, wordBg) = (Theme.delBg, Theme.delBgWord)
        case (.addition, false, true), (.change, false, true):
            (lineBg, wordBg) = (Theme.addBg, Theme.addBgWord)
        default: (lineBg, wordBg) = (Theme.bg, Theme.bg)
        }
        lineBg.setFill()
        rect.fill()

        guard let line else { return }

        let number = NSAttributedString(
            string: String(line.number),
            attributes: [.font: Theme.font, .foregroundColor: Theme.fgMuted]
        )
        let numberX = rect.minX + Self.gutterWidth - 8 - number.size().width
        drawText(number, x: max(rect.minX + 2, numberX))

        let changed = old ? row.oldChanged : row.newChanged
        let highlight = old ? file.oldHighlight : file.newHighlight
        let syntax = highlight?.line(line.number) ?? []
        let text = NSMutableAttributedString()
        for segment in lineSegments(line.text, syntax: syntax, changed: changed) {
            var attributes: [NSAttributedString.Key: Any] = [
                .font: Theme.font,
                .foregroundColor: Theme.syntaxColor(segment.kind),
            ]
            if segment.emphasized {
                attributes[.backgroundColor] = wordBg
            }
            text.append(NSAttributedString(string: segment.text, attributes: attributes))
        }
        applyFindHighlight(text)
        let textRect = NSRect(
            x: rect.minX + Self.gutterWidth,
            y: 0,
            width: rect.width - Self.gutterWidth - 4,
            height: bounds.height
        )
        drawText(text, x: textRect.minX, clipTo: textRect)
    }
}
