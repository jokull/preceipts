// Comment bodies are GitHub-flavored markdown, and bot comments are full
// of HTML scaffolding (badges, <details> prompt blobs, hidden markers).
// This is a deliberately small renderer: sanitize the HTML noise, style
// the markdown people actually read (headings, bold/italic/code, links,
// quotes, lists, fences) into an NSAttributedString. Not a browser — a
// triage surface.

import AppKit

enum MarkdownBody {
    static func render(_ raw: String) -> NSAttributedString {
        let cleaned = sanitize(raw)
        let out = NSMutableAttributedString()
        var inFence = false
        var first = true
        for line in cleaned.split(separator: "\n", omittingEmptySubsequences: false) {
            if !first {
                out.append(NSAttributedString(string: "\n"))
            }
            first = false
            let text = String(line)
            if text.trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                inFence.toggle()
                continue
            }
            if inFence {
                out.append(
                    NSAttributedString(
                        string: text,
                        attributes: [
                            .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
                            .foregroundColor: NSColor.secondaryLabelColor,
                        ]))
                continue
            }
            out.append(block(text))
        }
        trimBlankEdges(out)
        return out
    }

    /// Line count of the rendered body — drives the collapse affordance.
    static func lineCount(_ raw: String) -> Int {
        sanitize(raw).split(separator: "\n", omittingEmptySubsequences: false).count
    }

    // ------------------------------------------------------------------
    // Sanitize: strip the HTML scaffolding bots wrap around content.

    private static func sanitize(_ raw: String) -> String {
        var text = raw.replacingOccurrences(of: "\r\n", with: "\n")
        // Hidden markers (<!-- kami:review-progress -->).
        text = regexReplace(text, pattern: "<!--[\\s\\S]*?-->", with: "")
        // <details> blobs (AI-fix prompts, file lists) → just the summary.
        text = regexReplace(
            text,
            pattern: "<details[^>]*>\\s*<summary>([\\s\\S]*?)</summary>[\\s\\S]*?</details>",
            with: "\u{25b8} $1")
        // Badge images and bare images carry no text.
        text = regexReplace(
            text, pattern: "<a[^>]*>\\s*(?:<picture>[\\s\\S]*?</picture>|<img[^>]*/?>)\\s*</a>",
            with: "")
        text = regexReplace(text, pattern: "<picture>[\\s\\S]*?</picture>", with: "")
        text = regexReplace(text, pattern: "<img[^>]*/?>", with: "")
        text = regexReplace(text, pattern: "!\\[[^\\]]*\\]\\([^)]*\\)", with: "")
        // Known tags only — never touch `Array<T>` in prose or code.
        text = regexReplace(
            text,
            pattern: "</?(?:a|p|br|div|span|source|summary|details|b|i|em|strong|code|pre|ul"
                + "|ol|li|table|thead|tbody|tr|td|th|sub|sup|kbd|blockquote|h[1-6]|hr)\\b[^>]*>",
            with: "")
        // Reference-style link definitions ([vc]: #base64blob…).
        text = regexReplace(
            text, pattern: "(?m)^\\s*\\[[^\\]]+\\]:\\s+\\S.*$", with: "")
        // Entities the tag strip leaves behind.
        for (entity, plain) in [
            ("&nbsp;", " "), ("&lt;", "<"), ("&gt;", ">"),
            ("&quot;", "\""), ("&#39;", "'"), ("&amp;", "&"),
        ] {
            text = text.replacingOccurrences(of: entity, with: plain)
        }
        // Tables → dot-joined rows; alignment rows and rules vanish.
        text = regexReplace(
            text, pattern: "(?m)^\\s*\\|?[\\s|:-]*-[\\s|:-]*\\|?\\s*$", with: "")
        text = flattenTableRows(text)
        // Collapse the blank-line runs the stripping leaves behind.
        text = regexReplace(text, pattern: "\n{3,}", with: "\n\n")
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// "| a | b | c |" → "a · b · c" — a triage surface, not a grid.
    private static func flattenTableRows(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> String in
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                guard trimmed.hasPrefix("|"), trimmed.hasSuffix("|"), trimmed.count > 2
                else { return String(line) }
                let cells = trimmed.dropFirst().dropLast()
                    .split(separator: "|")
                    .map { $0.trimmingCharacters(in: .whitespaces) }
                    .filter { !$0.isEmpty }
                return cells.joined(separator: " \u{00b7} ")
            }
            .joined(separator: "\n")
    }

    // ------------------------------------------------------------------
    // Block styling

    private static func block(_ line: String) -> NSAttributedString {
        // Headings: bold, body-size — hierarchy without shouting.
        if let match = firstMatch(line, pattern: "^(#{1,6})\\s+(.*)$") {
            return inline(
                match[2],
                font: NSFont.systemFont(ofSize: 13, weight: .semibold),
                color: .labelColor)
        }
        // Blockquotes: dim.
        if let match = firstMatch(line, pattern: "^>\\s?(.*)$") {
            return inline(match[1], font: ThreadStyle.bodyFont, color: .secondaryLabelColor)
        }
        // Bullets: typographic dot, preserved indent.
        if let match = firstMatch(line, pattern: "^(\\s*)[-*+]\\s+(.*)$") {
            let out = NSMutableAttributedString(
                string: match[1] + "\u{2022} ",
                attributes: [.font: ThreadStyle.bodyFont, .foregroundColor: NSColor.labelColor])
            out.append(inline(match[2], font: ThreadStyle.bodyFont, color: .labelColor))
            return out
        }
        return inline(line, font: ThreadStyle.bodyFont, color: .labelColor)
    }

    // ------------------------------------------------------------------
    // Inline styling: code, bold, italic, links — earliest match wins,
    // then recurse on the remainder.

    private static let inlinePatterns: [(NSRegularExpression, Style)] = {
        func regex(_ pattern: String) -> NSRegularExpression {
            try! NSRegularExpression(pattern: pattern)
        }
        return [
            (regex("`([^`]+)`"), .code),
            (regex("\\*\\*([^*]+)\\*\\*"), .bold),
            (regex("__([^_]+)__"), .bold),
            (regex("\\*([^*\\s][^*]*)\\*"), .italic),
            (regex("\\[([^\\]]+)\\]\\(([^)\\s]+)\\)"), .link),
        ]
    }()

    private enum Style {
        case code, bold, italic, link
    }

    private static func inline(
        _ text: String, font: NSFont, color: NSColor
    ) -> NSAttributedString {
        let out = NSMutableAttributedString()
        var remainder = text
        while !remainder.isEmpty {
            let ns = remainder as NSString
            let full = NSRange(location: 0, length: ns.length)
            var earliest: (NSTextCheckingResult, Style)?
            for (regex, style) in inlinePatterns {
                guard let match = regex.firstMatch(in: remainder, range: full) else {
                    continue
                }
                if earliest == nil || match.range.location < earliest!.0.range.location {
                    earliest = (match, style)
                }
            }
            guard let (match, style) = earliest else {
                out.append(
                    NSAttributedString(
                        string: remainder,
                        attributes: [.font: font, .foregroundColor: color]))
                break
            }
            let prefix = ns.substring(to: match.range.location)
            if !prefix.isEmpty {
                out.append(
                    NSAttributedString(
                        string: prefix, attributes: [.font: font, .foregroundColor: color]))
            }
            let content = ns.substring(with: match.range(at: 1))
            switch style {
            case .code:
                out.append(
                    NSAttributedString(
                        string: content,
                        attributes: [
                            .font: NSFont.monospacedSystemFont(
                                ofSize: font.pointSize - 1.5, weight: .regular),
                            .foregroundColor: color,
                            .backgroundColor: NSColor.quaternarySystemFill,
                        ]))
            case .bold:
                out.append(
                    NSAttributedString(
                        string: content,
                        attributes: [
                            .font: NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask),
                            .foregroundColor: color,
                        ]))
            case .italic:
                out.append(
                    NSAttributedString(
                        string: content,
                        attributes: [
                            .font: NSFontManager.shared.convert(
                                font, toHaveTrait: .italicFontMask),
                            .foregroundColor: color,
                        ]))
            case .link:
                var attributes: [NSAttributedString.Key: Any] = [
                    .font: font,
                    .foregroundColor: NSColor.linkColor,
                ]
                if match.numberOfRanges > 2,
                    let url = URL(string: ns.substring(with: match.range(at: 2)))
                {
                    attributes[.link] = url
                }
                out.append(NSAttributedString(string: content, attributes: attributes))
            }
            remainder = ns.substring(from: match.range.location + match.range.length)
        }
        return out
    }

    // ------------------------------------------------------------------

    private static func regexReplace(
        _ text: String, pattern: String, with template: String
    ) -> String {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return text }
        return regex.stringByReplacingMatches(
            in: text, range: NSRange(text.startIndex..., in: text), withTemplate: template)
    }

    private static func firstMatch(_ text: String, pattern: String) -> [String]? {
        guard let regex = try? NSRegularExpression(pattern: pattern),
            let match = regex.firstMatch(
                in: text, range: NSRange(text.startIndex..., in: text))
        else { return nil }
        let ns = text as NSString
        return (0..<match.numberOfRanges).map {
            match.range(at: $0).location == NSNotFound ? "" : ns.substring(with: match.range(at: $0))
        }
    }

    private static func trimBlankEdges(_ text: NSMutableAttributedString) {
        while text.string.hasSuffix("\n") {
            text.deleteCharacters(in: NSRange(location: text.length - 1, length: 1))
        }
        while text.string.hasPrefix("\n") {
            text.deleteCharacters(in: NSRange(location: 0, length: 1))
        }
    }
}
