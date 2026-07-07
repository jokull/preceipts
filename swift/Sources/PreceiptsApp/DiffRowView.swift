// One fixed-height row of the diff surface, custom-drawn: two synced
// halves (old/new), gutter line numbers, per-segment styling. Custom
// drawing over precomputed rows is the native fast path
// (docs/desktop-foundations.md).

import AppKit
import PreceiptsKit

enum SurfaceRow {
    case fileHeader(file: Int)
    case gap(skipped: Int)
    case line(file: Int, hunk: Int, row: Int)
}

final class DiffRowView: NSView {
    var surfaceRow: SurfaceRow?
    var changeset: Changeset?

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
        drawText(text, x: 12)
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
        let textRect = NSRect(
            x: rect.minX + Self.gutterWidth,
            y: 0,
            width: rect.width - Self.gutterWidth - 4,
            height: bounds.height
        )
        drawText(text, x: textRect.minX, clipTo: textRect)
    }
}
