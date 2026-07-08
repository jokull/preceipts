// Comment bodies are GitHub-flavored markdown, and bot comments wrap it
// in HTML scaffolding (badges, <details> prompt blobs, hidden markers).
// Parsing is swift-markdown (Apple's cmark-gfm binding — real nested
// lists, tables, strikethrough, entities); rendering stays native via
// the visitor below. HTML is deliberately condensed, not rendered:
// this is a triage surface, not a browser.

import AppKit
import Markdown

enum MarkdownBody {
    static func render(_ raw: String) -> NSAttributedString {
        let document = Document(parsing: sanitize(raw))
        var renderer = AttributedRenderer()
        let out = NSMutableAttributedString(attributedString: renderer.visit(document))
        trimBlankEdges(out)
        return out
    }

    // ------------------------------------------------------------------
    // Sanitize: condense the HTML scaffolding before parsing. cmark
    // handles entities/reference definitions; we only remove what we
    // never want structure for.

    private static func sanitize(_ raw: String) -> String {
        var text = raw.replacingOccurrences(of: "\r\n", with: "\n")
        // Hidden markers (<!-- kami:review-progress -->).
        text = regexReplace(text, pattern: "<!--[\\s\\S]*?-->", with: "")
        // <details> blobs (AI-fix prompts, file lists) → just the summary.
        text = regexReplace(
            text,
            pattern: "<details[^>]*>\\s*<summary>([\\s\\S]*?)</summary>[\\s\\S]*?</details>",
            with: "\u{25b8} $1")
        // Badge images carry no text; drop them anchor and all.
        text = regexReplace(
            text, pattern: "<a[^>]*>\\s*(?:<picture>[\\s\\S]*?</picture>|<img[^>]*/?>)\\s*</a>",
            with: "")
        text = regexReplace(text, pattern: "<picture>[\\s\\S]*?</picture>", with: "")
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func regexReplace(
        _ text: String, pattern: String, with template: String
    ) -> String {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return text }
        return regex.stringByReplacingMatches(
            in: text, range: NSRange(text.startIndex..., in: text), withTemplate: template)
    }

    private static func trimBlankEdges(_ text: NSMutableAttributedString) {
        while text.string.hasSuffix("\n") || text.string.hasSuffix(" ") {
            text.deleteCharacters(in: NSRange(location: text.length - 1, length: 1))
        }
        while text.string.hasPrefix("\n") {
            text.deleteCharacters(in: NSRange(location: 0, length: 1))
        }
    }
}

// ------------------------------------------------------------------
// Markup → NSAttributedString. Blocks end with "\n"; paragraph spacing
// comes from paragraph styles, not blank lines.

private struct AttributedRenderer: MarkupVisitor {
    typealias Result = NSAttributedString

    private var listDepth = 0
    private var quoteDepth = 0

    // ---- helpers

    private var baseColor: NSColor {
        quoteDepth > 0 ? .secondaryLabelColor : .labelColor
    }

    private func paragraphStyle(indent: CGFloat = 0) -> NSParagraphStyle {
        let style = NSMutableParagraphStyle()
        style.paragraphSpacing = 5
        style.headIndent = indent
        style.firstLineHeadIndent = indent
        return style
    }

    private func baseAttributes(_ font: NSFont) -> [NSAttributedString.Key: Any] {
        [
            .font: font,
            .foregroundColor: baseColor,
            .paragraphStyle: paragraphStyle(indent: CGFloat(listDepth) * 14),
        ]
    }

    private mutating func children(_ markup: Markup) -> NSMutableAttributedString {
        let out = NSMutableAttributedString()
        for child in markup.children {
            out.append(visit(child))
        }
        return out
    }

    /// Re-style every run in `text` (bold/italic/strike wrap arbitrary
    /// inline content — merge, don't clobber inline-code fonts).
    private func transformFonts(
        _ text: NSMutableAttributedString, _ transform: (NSFont) -> NSFont
    ) {
        text.enumerateAttribute(
            .font, in: NSRange(location: 0, length: text.length)
        ) { value, range, _ in
            if let font = value as? NSFont {
                text.addAttribute(.font, value: transform(font), range: range)
            }
        }
    }

    private func block(_ text: NSAttributedString) -> NSAttributedString {
        let out = NSMutableAttributedString(attributedString: text)
        out.append(
            NSAttributedString(string: "\n", attributes: baseAttributes(ThreadStyle.bodyFont)))
        return out
    }

    // ---- structure

    mutating func defaultVisit(_ markup: Markup) -> NSAttributedString {
        children(markup)
    }

    mutating func visitDocument(_ document: Document) -> NSAttributedString {
        children(document)
    }

    mutating func visitParagraph(_ paragraph: Paragraph) -> NSAttributedString {
        block(children(paragraph))
    }

    mutating func visitHeading(_ heading: Heading) -> NSAttributedString {
        let size: CGFloat = heading.level <= 2 ? 14 : 13
        let text = children(heading)
        text.setAttributes(
            [
                .font: NSFont.systemFont(ofSize: size, weight: .semibold),
                .foregroundColor: baseColor,
                .paragraphStyle: paragraphStyle(),
            ],
            range: NSRange(location: 0, length: text.length))
        return block(text)
    }

    mutating func visitBlockQuote(_ blockQuote: BlockQuote) -> NSAttributedString {
        quoteDepth += 1
        defer { quoteDepth -= 1 }
        let inner = children(blockQuote)
        inner.addAttribute(
            .foregroundColor, value: NSColor.secondaryLabelColor,
            range: NSRange(location: 0, length: inner.length))
        return inner
    }

    mutating func visitCodeBlock(_ codeBlock: CodeBlock) -> NSAttributedString {
        var code = codeBlock.code
        while code.hasSuffix("\n") {
            code.removeLast()
        }
        return block(
            NSAttributedString(
                string: code,
                attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
                    .foregroundColor: NSColor.secondaryLabelColor,
                    .paragraphStyle: paragraphStyle(indent: CGFloat(listDepth) * 14 + 8),
                ]))
    }

    mutating func visitThematicBreak(_ thematicBreak: ThematicBreak) -> NSAttributedString {
        NSAttributedString()
    }

    // ---- lists

    mutating func visitUnorderedList(_ list: UnorderedList) -> NSAttributedString {
        renderList(list) { item, _ in
            if let checkbox = item.checkbox {
                return checkbox == .checked ? "\u{2611} " : "\u{2610} "
            }
            return "\u{2022} "
        }
    }

    mutating func visitOrderedList(_ list: OrderedList) -> NSAttributedString {
        let start = Int(list.startIndex)
        return renderList(list) { _, index in "\(start + index). " }
    }

    private mutating func renderList(
        _ list: Markup, marker: (ListItem, Int) -> String
    ) -> NSAttributedString {
        listDepth += 1
        defer { listDepth -= 1 }
        let out = NSMutableAttributedString()
        for (index, child) in list.children.enumerated() {
            guard let item = child as? ListItem else { continue }
            out.append(
                NSAttributedString(
                    string: marker(item, index),
                    attributes: baseAttributes(ThreadStyle.bodyFont)))
            out.append(children(item))
        }
        return out
    }

    // ---- tables (dot-joined rows: triage, not a grid)

    mutating func visitTable(_ table: Table) -> NSAttributedString {
        let out = NSMutableAttributedString()
        let head = renderRow(table.head.cells)
        transformFonts(head) { font in
            NSFont.systemFont(ofSize: font.pointSize, weight: .semibold)
        }
        out.append(block(head))
        for row in table.body.rows {
            out.append(block(renderRow(row.cells)))
        }
        return out
    }

    private mutating func renderRow(
        _ cells: some Sequence<Table.Cell>
    ) -> NSMutableAttributedString {
        let out = NSMutableAttributedString()
        var first = true
        for cell in cells {
            let content = children(cell)
            guard content.length > 0 else { continue }
            if !first {
                out.append(
                    NSAttributedString(
                        string: " \u{00b7} ",
                        attributes: [
                            .font: ThreadStyle.bodyFont,
                            .foregroundColor: NSColor.tertiaryLabelColor,
                        ]))
            }
            first = false
            out.append(content)
        }
        return out
    }

    // ---- inline

    mutating func visitText(_ text: Text) -> NSAttributedString {
        NSAttributedString(string: text.string, attributes: baseAttributes(ThreadStyle.bodyFont))
    }

    mutating func visitSoftBreak(_ softBreak: SoftBreak) -> NSAttributedString {
        NSAttributedString(string: " ", attributes: baseAttributes(ThreadStyle.bodyFont))
    }

    mutating func visitLineBreak(_ lineBreak: LineBreak) -> NSAttributedString {
        NSAttributedString(string: "\n", attributes: baseAttributes(ThreadStyle.bodyFont))
    }

    mutating func visitEmphasis(_ emphasis: Emphasis) -> NSAttributedString {
        let out = children(emphasis)
        transformFonts(out) { NSFontManager.shared.convert($0, toHaveTrait: .italicFontMask) }
        return out
    }

    mutating func visitStrong(_ strong: Strong) -> NSAttributedString {
        let out = children(strong)
        transformFonts(out) { NSFontManager.shared.convert($0, toHaveTrait: .boldFontMask) }
        return out
    }

    mutating func visitStrikethrough(_ strikethrough: Strikethrough) -> NSAttributedString {
        let out = children(strikethrough)
        out.addAttribute(
            .strikethroughStyle, value: NSUnderlineStyle.single.rawValue,
            range: NSRange(location: 0, length: out.length))
        return out
    }

    mutating func visitInlineCode(_ inlineCode: InlineCode) -> NSAttributedString {
        NSAttributedString(
            string: inlineCode.code,
            attributes: [
                .font: NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular),
                .foregroundColor: baseColor,
                .backgroundColor: NSColor.quaternarySystemFill,
                .paragraphStyle: paragraphStyle(indent: CGFloat(listDepth) * 14),
            ])
    }

    mutating func visitLink(_ link: Link) -> NSAttributedString {
        let out = children(link)
        var attributes: [NSAttributedString.Key: Any] = [
            .foregroundColor: NSColor.linkColor
        ]
        if let url = link.destination.flatMap(URL.init(string:)) {
            attributes[.link] = url
        }
        out.addAttributes(attributes, range: NSRange(location: 0, length: out.length))
        return out
    }

    mutating func visitImage(_ image: Image) -> NSAttributedString {
        // No inline images (yet) — show the alt text, dimmed, so the
        // reader knows something was here.
        let alt = image.plainText
        guard !alt.isEmpty else { return NSAttributedString() }
        return NSAttributedString(
            string: "[\(alt)]",
            attributes: [
                .font: ThreadStyle.metaFont,
                .foregroundColor: NSColor.tertiaryLabelColor,
            ])
    }

    mutating func visitInlineHTML(_ inlineHTML: InlineHTML) -> NSAttributedString {
        // Tags vanish; their inner text arrives as sibling Text nodes.
        NSAttributedString()
    }

    mutating func visitHTMLBlock(_ html: HTMLBlock) -> NSAttributedString {
        // Whatever survived sanitize() as an HTML block is scaffolding.
        NSAttributedString()
    }
}
