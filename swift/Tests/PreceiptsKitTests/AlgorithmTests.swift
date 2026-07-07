// Ports of the Rust core's algorithm tests — the quality contract carried
// across the rewrite (core/src/{intraline,pairing}.rs, app/src/segments.rs).

import XCTest

@testable import PreceiptsKit

final class IntralineTests: XCTestCase {
    private func slices(_ line: String, _ ranges: [Range<Int>]) -> [String] {
        let bytes = Array(line.utf8)
        return ranges.map { String(decoding: bytes[$0], as: UTF8.self) }
    }

    func testSingleWordChange() {
        let old = "let two = compute(input);"
        let new = "let TWO = compute(input);"
        let (o, n) = wordDiff(old: old, new: new)
        XCTAssertEqual(slices(old, o), ["two"])
        XCTAssertEqual(slices(new, n), ["TWO"])
    }

    func testInsertionOnlyMarksNewSide() {
        let (o, n) = wordDiff(old: "foo(a, b)", new: "foo(a, b, c)")
        XCTAssertTrue(o.isEmpty)
        XCTAssertEqual(slices("foo(a, b, c)", n), [", c"])
    }

    func testAdjacentChangedTokensMerge() {
        let old = "return value;"
        let new = "return other_thing();"
        let (o, n) = wordDiff(old: old, new: new)
        XCTAssertEqual(slices(old, o), ["value"])
        XCTAssertEqual(slices(new, n), ["other_thing()"])
    }

    func testIdenticalLinesYieldNothing() {
        let (o, n) = wordDiff(old: "same", new: "same")
        XCTAssertTrue(o.isEmpty && n.isEmpty)
    }

    func testUnicodeIdentifiersStayIntact() {
        let old = "name = \"Jökull\""
        let new = "name = \"Sólberg\""
        let (o, n) = wordDiff(old: old, new: new)
        XCTAssertEqual(slices(old, o), ["Jökull"])
        XCTAssertEqual(slices(new, n), ["Sólberg"])
    }
}

final class PairingTests: XCTestCase {
    private func kinds(_ pairings: [Pairing]) -> String {
        pairings.map { pairing in
            switch pairing {
            case .pair: return "P"
            case .removed: return "R"
            case .added: return "A"
            }
        }.joined()
    }

    func testEditInPlacePairs() {
        let p = pairBlock(oldLines: ["let x = compute(a);"], newLines: ["let x = compute(a, b);"])
        XCTAssertEqual(kinds(p), "P")
    }

    func testUnrelatedReplacementDoesNotPair() {
        let p = pairBlock(
            oldLines: ["return legacy_path();"], newLines: ["#[cfg(feature = \"v2\")]"])
        XCTAssertEqual(kinds(p), "RA")
    }

    func testSkewedBlockPairsTheMatchingLine() {
        let p = pairBlock(
            oldLines: ["fn handle(req: Request) {"],
            newLines: ["/// Handles one request.", "fn handle(req: &Request) {"]
        )
        XCTAssertEqual(kinds(p), "AP")
        XCTAssertEqual(p[1], .pair(0, 1))
    }

    func testOversizedBlocksFallBackPositionally() {
        let old = (0..<80).map { "old \($0)" }
        let new = (0..<80).map { "new \($0)" }
        let p = pairBlock(oldLines: old, newLines: new)
        XCTAssertEqual(p.count, 80)
        XCTAssertEqual(p[0], .pair(0, 0))
    }
}

final class SegmentsTests: XCTestCase {
    func testPlainLineIsOneSegment() {
        let segments = lineSegments("hello world", syntax: [], changed: [])
        XCTAssertEqual(segments.count, 1)
        XCTAssertEqual(segments[0].text, "hello world")
        XCTAssertFalse(segments[0].emphasized)
    }

    func testSyntaxAndChangedOverlaysCompose() {
        let segments = lineSegments(
            "let x = 1;",
            syntax: [HighlightSpan(range: 0..<3, kind: 7)],
            changed: [4..<5]
        )
        XCTAssertEqual(segments.map(\.text), ["let", " ", "x", " = 1;"])
        XCTAssertEqual(segments[0].kind, 7)
        XCTAssertTrue(segments[2].emphasized)
        XCTAssertFalse(segments[3].emphasized)
    }

    func testOverlappingBoundariesNeverGap() {
        let segments = lineSegments(
            "abcdef",
            syntax: [
                HighlightSpan(range: 0..<4, kind: 1), HighlightSpan(range: 4..<6, kind: 2),
            ],
            changed: [2..<5]
        )
        XCTAssertEqual(segments.map(\.text).joined(), "abcdef")
    }

    func testTabsExpandForDisplay() {
        let segments = lineSegments("\tindent", syntax: [], changed: [])
        XCTAssertEqual(segments[0].text, "    indent")
    }

    func testMultibyteBoundariesSliceCleanly() {
        let text = "name Jökull here"
        let segments = lineSegments(text, syntax: [], changed: [5..<12])
        XCTAssertEqual(segments.map(\.text).joined(), text)
        XCTAssertTrue(segments.contains { $0.text == "Jökull" && $0.emphasized })
    }
}

final class DiffRowsTests: XCTestCase {
    func testSimpleChangePairsLinesWithIntraline() {
        let (hunks, added, removed) = diffRows(
            old: "a\nlet b = 1;\nc\n", new: "a\nlet B = 1;\nc\n")
        XCTAssertEqual(added, 1)
        XCTAssertEqual(removed, 1)
        XCTAssertEqual(hunks.count, 1)
        let changes = hunks[0].rows.filter { $0.kind == .change }
        XCTAssertEqual(changes.count, 1)
        XCTAssertEqual(changes[0].old?.text, "let b = 1;")
        XCTAssertEqual(changes[0].new?.text, "let B = 1;")
        XCTAssertEqual(changes[0].old?.number, 2)
        let oldBytes = Array("let b = 1;".utf8)
        XCTAssertEqual(
            String(decoding: oldBytes[changes[0].oldChanged[0]], as: UTF8.self), "b")
    }

    func testDissimilarReplacementRendersRemovalPlusAddition() {
        let (hunks, _, _) = diffRows(
            old: "a\nreturn legacy_path();\nc\n", new: "a\ntodo!(\"rewrite\")\nc\n")
        let kinds = Set(hunks[0].rows.map(\.kind))
        XCTAssertTrue(kinds.contains(.removal))
        XCTAssertTrue(kinds.contains(.addition))
        XCTAssertFalse(kinds.contains(.change))
    }

    func testDistantChangesBecomeSeparateHunksWithSkips() {
        let old = (1...40).map { "line\($0)\n" }.joined()
        let new = old
            .replacingOccurrences(of: "line5\n", with: "LINE5\n")
            .replacingOccurrences(of: "line35\n", with: "LINE35\n")
        let (hunks, added, removed) = diffRows(old: old, new: new)
        XCTAssertEqual(added, 2)
        XCTAssertEqual(removed, 2)
        XCTAssertEqual(hunks.count, 2)
        XCTAssertEqual(hunks[0].skippedBefore, 1)
        XCTAssertGreaterThan(hunks[1].skippedBefore, 0)
    }

    func testIdenticalContentsYieldNoHunks() {
        let (hunks, added, removed) = diffRows(old: "a\nb\n", new: "a\nb\n")
        XCTAssertTrue(hunks.isEmpty)
        XCTAssertEqual(added + removed, 0)
    }
}
