// Comment bodies are GitHub-flavored markdown, and bot comments wrap it
// in HTML scaffolding (badges, <details> prompt blobs, hidden markers).
// Parsing is swift-markdown (Apple's cmark-gfm binding — real nested
// lists, tables, strikethrough, entities); rendering stays native via
// the visitor below. HTML is deliberately condensed, not rendered:
// this is a triage surface, not a browser.

import AppKit
import Markdown

/// One rendered piece of a comment body. Text segments are attributed
/// strings; images become native views (GIFs animate) — no webviews.
enum BodySegment {
    case text(NSAttributedString)
    case image(url: URL, alt: String)

    /// Collapse accounting: how many "lines" this segment occupies.
    var lineWeight: Int {
        switch self {
        case .text(let text):
            return text.string.components(separatedBy: "\n").count
        case .image:
            return 8
        }
    }
}

enum MarkdownBody {
    static func render(_ raw: String) -> [BodySegment] {
        let document = Document(parsing: sanitize(raw))
        var renderer = AttributedRenderer()
        var segments: [BodySegment] = []
        let current = NSMutableAttributedString()

        func flush() {
            trimBlankEdges(current)
            if current.length > 0 {
                segments.append(.text(NSAttributedString(attributedString: current)))
            }
            current.setAttributedString(NSAttributedString())
        }

        for block in document.blockChildren {
            if let images = standaloneImages(block) {
                flush()
                for (url, alt) in images {
                    segments.append(.image(url: url, alt: alt))
                }
            } else {
                current.append(renderer.visit(block))
            }
        }
        flush()
        return segments
    }

    /// A paragraph that is only images (possibly link-wrapped — GitHub
    /// screenshots) renders as native image views.
    private static func standaloneImages(_ block: BlockMarkup) -> [(URL, String)]? {
        guard let paragraph = block as? Paragraph else { return nil }
        var images: [(URL, String)] = []
        for child in paragraph.inlineChildren {
            switch child {
            case let image as Image:
                guard let url = renderableURL(image) else { return nil }
                images.append((url, image.plainText))
            case let link as Link:
                let inner = Array(link.children)
                guard inner.count == 1, let image = inner[0] as? Image,
                    let url = renderableURL(image)
                else { return nil }
                images.append((url, image.plainText))
            case let text as Text:
                guard text.string.trimmingCharacters(in: .whitespaces).isEmpty else {
                    return nil
                }
            case is SoftBreak, is LineBreak, is InlineHTML:
                continue
            default:
                return nil
            }
        }
        return images.isEmpty ? nil : images
    }

    /// Raster images (png/jpg/gif/webp) render; SVGs are almost always
    /// status badges — those stay as dim [alt] text.
    private static func renderableURL(_ image: Image) -> URL? {
        guard let source = image.source, let url = URL(string: source),
            url.scheme == "https" || url.scheme == "http",
            !url.path.lowercased().hasSuffix(".svg")
        else { return nil }
        return url
    }

    // ------------------------------------------------------------------
    // Sanitize: condense the HTML scaffolding before parsing. cmark
    // handles entities/reference definitions; we only remove what we
    // never want structure for.

    private static func sanitize(_ raw: String) -> String {
        var text = raw.replacingOccurrences(of: "\r\n", with: "\n")
        // Hidden markers (<!-- kami:review-progress -->).
        text = regexReplace(text, pattern: "<!--[\\s\\S]*?-->", with: "")
        // <details> unwrap to summary caption + full content — the
        // card-level "Show more" collapse contains the length.
        text = regexReplace(
            text,
            pattern:
                "<details[^>]*>\\s*<summary>([\\s\\S]*?)</summary>([\\s\\S]*?)</details>",
            with: "\u{25b8} $1\n\n$2")
        // <picture> badges (responsive light/dark status SVGs).
        text = regexReplace(text, pattern: "<picture>[\\s\\S]*?</picture>", with: "")
        // Raw <img> tags → markdown images so real screenshots/GIFs
        // survive tag stripping and render natively.
        text = convertImgTags(text)
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func convertImgTags(_ text: String) -> String {
        guard
            let regex = try? NSRegularExpression(
                pattern: "<img[^>]*>", options: [.caseInsensitive])
        else { return text }
        let ns = text as NSString
        var result = ""
        var last = 0
        for match in regex.matches(in: text, range: NSRange(location: 0, length: ns.length)) {
            result += ns.substring(with: NSRange(location: last, length: match.range.location - last))
            let tag = ns.substring(with: match.range)
            if let src = attribute(tag, "src") {
                let alt = attribute(tag, "alt") ?? ""
                result += "![\(alt)](\(src))"
            }
            last = match.range.location + match.range.length
        }
        result += ns.substring(from: last)
        return result
    }

    private static func attribute(_ tag: String, _ name: String) -> String? {
        guard
            let regex = try? NSRegularExpression(
                pattern: "\(name)=\"([^\"]*)\"", options: [.caseInsensitive]),
            let match = regex.firstMatch(in: tag, range: NSRange(tag.startIndex..., in: tag))
        else { return nil }
        return (tag as NSString).substring(with: match.range(at: 1))
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
