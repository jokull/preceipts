// Tree-sitter syntax highlighting: whole-file parse, per-line spans.
// Port of the Rust registry (core/src/highlight/, now in git history only), directly on the
// tree-sitter C API so byte offsets stay UTF-8 (the Swift wrapper layer
// works in UTF-16). Compute per file, off the UI thread.
//
// Differences from the tree-sitter-highlight crate, none observable with
// our queries: captures are flattened by painting (outer-then-inner)
// instead of an event stream, and only the text predicates our queries
// use are evaluated (#eq?/#match?/#any-of? and negations; #set! ignored).

import Foundation
import TreeSitter
import TreeSitterBash
import TreeSitterCSS
import TreeSitterCSharp
import TreeSitterElixir
import TreeSitterGo
import TreeSitterHTML
import TreeSitterJSON
import TreeSitterJavaScript
import TreeSitterMarkdown
import TreeSitterPython
import TreeSitterRuby
import TreeSitterRust
import TreeSitterTOML
import TreeSitterTSX
import TreeSitterTypeScript

/// Capture-kind table; `HighlightSpan.kind` indexes into this. Renderers
/// map names to colors by dot-separated family.
public let highlightNames: [String] = [
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "function",
    "function.builtin",
    "function.method",
    "function.macro",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "variable.member",
]

/// Per-line highlight spans for one version of one file. Ranges are
/// UTF-8 byte offsets relative to the line (newline excluded).
public struct FileHighlight: Sendable {
    let lines: [[HighlightSpan]]

    /// Spans for a 1-based line number; empty for out-of-range.
    public func line(_ number: Int) -> [HighlightSpan] {
        let index = number - 1
        guard index >= 0, index < lines.count else { return [] }
        return lines[index]
    }

    /// Keep spans only for the given 1-based line numbers. The surface
    /// renders hunk lines only, and whole-file span tables dominate a
    /// changeset's retained size when large files carry small diffs.
    public func pruned(keeping keep: Set<Int>) -> FileHighlight {
        FileHighlight(
            lines: lines.enumerated().map { index, spans in
                keep.contains(index + 1) ? spans : []
            })
    }
}

public enum Highlighter {
    /// Files past this size render plain — they are generated artifacts
    /// (lockfiles, snapshots, .d.ts bundles) where highlighting is noise
    /// and parse time dominates the whole changeset load.
    public static let maxBytes = 512 << 10

    /// Highlight a whole file. Returns nil for unsupported extensions and
    /// oversized files — callers render plain text. Multi-line constructs
    /// (block comments, template strings) highlight correctly because the
    /// parse is global.
    public static func highlight(_ content: String, path: String) -> FileHighlight? {
        guard content.utf8.count <= maxBytes else { return nil }
        let ext = path.split(separator: ".").last.map(String.init) ?? ""
        guard let language = registry[ext] else { return nil }
        return language.highlight(content)
    }
}

// ------------------------------------------------------------------
// Language registry

private let registry: [String: HighlightLanguage] = {
    var map: [String: HighlightLanguage] = [:]
    func add(_ exts: [String], _ name: String, _ language: OpaquePointer?, _ query: String) {
        guard let entry = HighlightLanguage(language: language, querySource: query) else {
            #if DEBUG
                FileHandle.standardError.write(
                    Data("[WARN] failed to load \(name) highlight query\n".utf8))
            #endif
            return
        }
        for ext in exts { map[ext] = entry }
    }
    add(["ts"], "typescript", tree_sitter_typescript(), HighlightQueries.typescript)
    add(["tsx"], "tsx", tree_sitter_tsx(), HighlightQueries.tsx)
    add(["js", "jsx"], "javascript", tree_sitter_javascript(), HighlightQueries.javascript)
    add(["rs"], "rust", tree_sitter_rust(), HighlightQueries.rust)
    add(["json"], "json", tree_sitter_json(), HighlightQueries.json)
    add(["py"], "python", tree_sitter_python(), HighlightQueries.python)
    add(["go"], "go", tree_sitter_go(), HighlightQueries.go)
    add(["css"], "css", tree_sitter_css(), HighlightQueries.css)
    add(["html"], "html", tree_sitter_html(), HighlightQueries.html)
    add(["toml"], "toml", tree_sitter_toml(), HighlightQueries.toml)
    add(["sh", "bash"], "bash", tree_sitter_bash(), HighlightQueries.bash)
    add(["md", "mdx"], "markdown", tree_sitter_markdown(), HighlightQueries.markdown)
    add(["cs"], "c_sharp", tree_sitter_c_sharp(), HighlightQueries.csharp)
    add(["rb"], "ruby", tree_sitter_ruby(), HighlightQueries.ruby)
    add(["ex", "exs"], "elixir", tree_sitter_elixir(), HighlightQueries.elixir)
    return map
}()

private final class HighlightLanguage {
    let language: OpaquePointer
    let query: OpaquePointer
    /// Capture index → kind (index into highlightNames), nil = uncolored.
    let captureKinds: [UInt8?]
    /// Pattern index → text predicates that must all pass per match.
    let patternPredicates: [[TextPredicate]]

    init?(language: OpaquePointer?, querySource: String) {
        guard let language else { return nil }
        var errorOffset: UInt32 = 0
        var errorType = TSQueryErrorNone
        let query = querySource.withCString { source in
            ts_query_new(
                language, source, UInt32(querySource.utf8.count), &errorOffset, &errorType)
        }
        guard let query else { return nil }
        self.language = language
        self.query = query

        var kinds: [UInt8?] = []
        for id in 0..<ts_query_capture_count(query) {
            var length: UInt32 = 0
            let name = ts_query_capture_name_for_id(query, id, &length).map {
                String(decoding: UnsafeRawBufferPointer(start: $0, count: Int(length)), as: UTF8.self)
            }
            kinds.append(name.flatMap(Self.kind(for:)))
        }
        self.captureKinds = kinds

        var predicates: [[TextPredicate]] = []
        for pattern in 0..<ts_query_pattern_count(query) {
            predicates.append(TextPredicate.forPattern(query: query, pattern: pattern))
        }
        self.patternPredicates = predicates
    }

    deinit {
        ts_query_delete(query)
    }

    /// Longest dot-separated prefix of `name` present in highlightNames.
    private static func kind(for name: String) -> UInt8? {
        var parts = name.split(separator: ".")
        while !parts.isEmpty {
            let candidate = parts.joined(separator: ".")
            if let index = highlightNames.firstIndex(of: candidate) {
                return UInt8(index)
            }
            parts.removeLast()
        }
        return nil
    }

    func highlight(_ content: String) -> FileHighlight {
        let bytes = Array(content.utf8)
        var spans: [(start: Int, end: Int, kind: UInt8, pattern: Int)] = []

        if !bytes.isEmpty, let parser = ts_parser_new() {
            defer { ts_parser_delete(parser) }
            ts_parser_set_language(parser, language)
            let tree = bytes.withUnsafeBufferPointer { buffer in
                buffer.baseAddress!.withMemoryRebound(to: CChar.self, capacity: buffer.count) {
                    ts_parser_parse_string(parser, nil, $0, UInt32(buffer.count))
                }
            }
            if let tree {
                defer { ts_tree_delete(tree) }
                spans = collectSpans(tree: tree, bytes: bytes)
            }
        }
        return FileHighlight(lines: Self.splitLines(spans: spans, bytes: bytes))
    }

    private func collectSpans(
        tree: OpaquePointer, bytes: [UInt8]
    ) -> [(start: Int, end: Int, kind: UInt8, pattern: Int)] {
        guard let cursor = ts_query_cursor_new() else { return [] }
        defer { ts_query_cursor_delete(cursor) }
        ts_query_cursor_exec(cursor, query, ts_tree_root_node(tree))

        var spans: [(start: Int, end: Int, kind: UInt8, pattern: Int)] = []
        var match = TSQueryMatch()
        while ts_query_cursor_next_match(cursor, &match) {
            let pattern = Int(match.pattern_index)
            let passes = patternPredicates[pattern].allSatisfy { $0.passes(match, bytes: bytes) }
            guard passes else { continue }
            for i in 0..<Int(match.capture_count) {
                let capture = match.captures[i]
                guard let kind = captureKinds[Int(capture.index)] else { continue }
                let start = Int(ts_node_start_byte(capture.node))
                let end = Int(ts_node_end_byte(capture.node))
                if start < end, end <= bytes.count {
                    spans.append((start, end, kind, pattern))
                }
            }
        }
        return spans
    }

    /// Flatten possibly-nested spans and split at newlines. Painting in
    /// (start asc, outer-before-inner, pattern asc) order with
    /// last-painted-wins reproduces tree-sitter-highlight: inner nodes
    /// override outer ones (the event stack), and for identical ranges
    /// the later pattern wins (verified against the Rust reference —
    /// elixir "defmodule" is keyword, TS method names are property).
    private static func splitLines(
        spans: [(start: Int, end: Int, kind: UInt8, pattern: Int)], bytes: [UInt8]
    ) -> [[HighlightSpan]] {
        let none: UInt8 = 255
        var paint = [UInt8](repeating: none, count: bytes.count)
        let ordered = spans.sorted {
            if $0.start != $1.start { return $0.start < $1.start }
            if $0.end != $1.end { return $0.end > $1.end }
            return $0.pattern < $1.pattern
        }
        for span in ordered {
            for i in span.start..<span.end { paint[i] = span.kind }
        }

        var lines: [[HighlightSpan]] = []
        var lineStart = 0
        func flushLine(_ end: Int) {
            var spans: [HighlightSpan] = []
            var i = lineStart
            while i < end {
                let kind = paint[i]
                var j = i + 1
                while j < end && paint[j] == kind { j += 1 }
                if kind != none {
                    spans.append(HighlightSpan(range: (i - lineStart)..<(j - lineStart), kind: kind))
                }
                i = j
            }
            lines.append(spans)
        }
        for (index, byte) in bytes.enumerated() where byte == 0x0A {
            flushLine(index)
            lineStart = index + 1
        }
        flushLine(bytes.count)
        return lines
    }
}

// ------------------------------------------------------------------
// Text predicates

private enum TextPredicate {
    case eq(capture: UInt32, other: Argument, negate: Bool)
    case match(capture: UInt32, regex: NSRegularExpression, negate: Bool)
    case anyOf(capture: UInt32, values: Set<String>, negate: Bool)

    enum Argument {
        case capture(UInt32)
        case literal(String)
    }

    static func forPattern(query: OpaquePointer, pattern: UInt32) -> [TextPredicate] {
        var stepCount: UInt32 = 0
        guard let steps = ts_query_predicates_for_pattern(query, pattern, &stepCount) else {
            return []
        }

        func stringValue(_ id: UInt32) -> String {
            var length: UInt32 = 0
            guard let value = ts_query_string_value_for_id(query, id, &length) else { return "" }
            return String(
                decoding: UnsafeRawBufferPointer(start: value, count: Int(length)), as: UTF8.self)
        }

        var predicates: [TextPredicate] = []
        var arguments: [Argument] = []
        var op: String? = nil
        for i in 0..<Int(stepCount) {
            let step = steps[i]
            switch step.type {
            case TSQueryPredicateStepTypeString:
                if op == nil {
                    op = stringValue(step.value_id)
                } else {
                    arguments.append(.literal(stringValue(step.value_id)))
                }
            case TSQueryPredicateStepTypeCapture:
                arguments.append(.capture(step.value_id))
            default:  // Done — one predicate finished
                if let op, let predicate = build(op: op, arguments: arguments) {
                    predicates.append(predicate)
                }
                op = nil
                arguments = []
            }
        }
        return predicates
    }

    private static func build(op: String, arguments: [Argument]) -> TextPredicate? {
        guard case .capture(let capture)? = arguments.first else { return nil }
        let rest = arguments.dropFirst()
        switch op {
        case "eq?", "not-eq?":
            guard let other = rest.first else { return nil }
            return .eq(capture: capture, other: other, negate: op.hasPrefix("not-"))
        case "match?", "not-match?":
            guard case .literal(let source)? = rest.first,
                let regex = try? NSRegularExpression(pattern: source)
            else { return nil }
            return .match(capture: capture, regex: regex, negate: op.hasPrefix("not-"))
        case "any-of?", "not-any-of?":
            var values: Set<String> = []
            for argument in rest {
                guard case .literal(let value) = argument else { return nil }
                values.insert(value)
            }
            return .anyOf(capture: capture, values: values, negate: op.hasPrefix("not-"))
        default:
            // #set! and other non-text predicates don't filter matches.
            return nil
        }
    }

    func passes(_ match: TSQueryMatch, bytes: [UInt8]) -> Bool {
        func text(_ captureIndex: UInt32) -> String? {
            for i in 0..<Int(match.capture_count) where match.captures[i].index == captureIndex {
                let node = match.captures[i].node
                let start = Int(ts_node_start_byte(node))
                let end = Int(ts_node_end_byte(node))
                guard start <= end, end <= bytes.count else { return nil }
                return String(decoding: bytes[start..<end], as: UTF8.self)
            }
            return nil
        }

        switch self {
        case .eq(let capture, let other, let negate):
            guard let left = text(capture) else { return negate }
            let right: String?
            switch other {
            case .capture(let index): right = text(index)
            case .literal(let value): right = value
            }
            return (left == right) != negate
        case .match(let capture, let regex, let negate):
            guard let value = text(capture) else { return negate }
            let range = NSRange(value.startIndex..., in: value)
            return (regex.firstMatch(in: value, range: range) != nil) != negate
        case .anyOf(let capture, let values, let negate):
            guard let value = text(capture) else { return negate }
            return values.contains(value) != negate
        }
    }
}
