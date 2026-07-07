// Port of the Rust highlight tests (core/src/highlight/mod.rs), plus
// predicate coverage the Rust side got for free from tree-sitter-highlight.

import XCTest

@testable import PreceiptsKit

final class HighlightTests: XCTestCase {
    private func kindName(_ span: HighlightSpan) -> String {
        guard let kind = span.kind, Int(kind) < highlightNames.count else { return "" }
        return highlightNames[Int(kind)]
    }

    func testRustKeywordsAndStrings() throws {
        let code = "fn main() {\n    let msg = \"hi\";\n}\n"
        let highlight = try XCTUnwrap(Highlighter.highlight(code, path: "src/main.rs"))

        let line1 = highlight.line(1).map(kindName)
        XCTAssertTrue(line1.contains("keyword"), "line1 kinds: \(line1)")

        let line2 = highlight.line(2)
        let stringSpan = try XCTUnwrap(
            line2.first { kindName($0) == "string" }, "string literal highlighted")
        let lineText = Array("    let msg = \"hi\";".utf8)
        let text = String(
            decoding: lineText[stringSpan.range.lowerBound..<stringSpan.range.upperBound],
            as: UTF8.self)
        XCTAssertTrue(text.contains("hi"))
    }

    func testMultilineCommentSpansAllLines() throws {
        let code = "/* one\ntwo */\nlet x = 1;\n"
        let highlight = try XCTUnwrap(Highlighter.highlight(code, path: "a.ts"))
        XCTAssertTrue(highlight.line(1).contains { kindName($0) == "comment" })
        XCTAssertTrue(highlight.line(2).contains { kindName($0) == "comment" })
        XCTAssertFalse(highlight.line(3).contains { kindName($0) == "comment" })
    }

    func testUnsupportedExtensionIsNil() {
        XCTAssertNil(Highlighter.highlight("hello", path: "readme.xyz"))
    }

    func testSpanRangesAreLineRelativeAndOrdered() throws {
        let code = "const a = 1;\nconst b = \"two\";\n"
        let highlight = try XCTUnwrap(Highlighter.highlight(code, path: "x.ts"))
        let lines = code.split(separator: "\n").map { Array($0.utf8) }
        for number in 1...2 {
            let lineLength = lines[number - 1].count
            var cursor = 0
            for span in highlight.line(number) {
                XCTAssertGreaterThanOrEqual(span.range.lowerBound, cursor, "overlapping spans")
                XCTAssertLessThanOrEqual(span.range.upperBound, lineLength, "span past line end")
                cursor = span.range.upperBound
            }
        }
    }

    // Elixir's bundled query leans on #any-of?/#match? — the predicate
    // evaluation path the C API doesn't provide.
    func testElixirPredicatesFilterMatches() throws {
        let code = "defmodule Greeter do\n  def hello, do: :world\nend\n"
        let highlight = try XCTUnwrap(Highlighter.highlight(code, path: "lib/greeter.ex"))
        // "defmodule"/"def" are keywords only via (#any-of? @keyword ...).
        let line1 = highlight.line(1)
        let keywordSpan = try XCTUnwrap(line1.first { kindName($0) == "keyword" })
        let lineText = Array("defmodule Greeter do".utf8)
        let text = String(
            decoding: lineText[keywordSpan.range.lowerBound..<keywordSpan.range.upperBound],
            as: UTF8.self)
        XCTAssertEqual(text, "defmodule")
        XCTAssertTrue(highlight.line(2).contains { kindName($0) == "keyword" })
    }

    func testLaterPatternWinsOnSameNode() throws {
        // Same node captured by two patterns: tree-sitter-highlight gives
        // the LATER pattern the win — greet is @property (later catch-all)
        // not @function.method, per the Rust reference implementation.
        let code = "class A {\n  greet() {}\n}\n"
        let highlight = try XCTUnwrap(Highlighter.highlight(code, path: "a.ts"))
        let kinds = highlight.line(2).map(kindName)
        XCTAssertTrue(kinds.contains("property"), "line2 kinds: \(kinds)")
        XCTAssertFalse(kinds.contains("function.method"))
    }
}
