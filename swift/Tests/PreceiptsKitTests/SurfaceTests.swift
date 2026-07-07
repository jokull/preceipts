// Surface building, file-tree folding, and ⌘F matching — the pure models
// behind the app's sidebar, sticky headers, and find bar.

import XCTest

@testable import PreceiptsKit

private func makeFile(
    _ path: String,
    status: FileStatus = .modified,
    added: Int = 1,
    removed: Int = 1,
    hunks: [DiffHunk] = [
        DiffHunk(
            skippedBefore: 3,
            rows: [
                DiffRow(
                    kind: .change,
                    old: LineRef(number: 4, text: "let value = 1"),
                    new: LineRef(number: 4, text: "let value = 2")),
                DiffRow(
                    kind: .addition,
                    old: nil,
                    new: LineRef(number: 5, text: "print(value)")),
            ])
    ]
) -> FileDiff {
    FileDiff(
        path: path, oldPath: nil, status: status, isBinary: false,
        added: added, removed: removed, hunks: hunks)
}

private func makeChangeset(_ files: [FileDiff]) -> Changeset {
    Changeset(
        scope: .branch, baseName: "origin/main", branch: "feature",
        workdir: URL(fileURLWithPath: "/tmp/x"),
        gitDir: URL(fileURLWithPath: "/tmp/x/.git"),
        files: files)
}

final class SurfaceTests: XCTestCase {
    func testBuildFlattensFilesWithAnchorsAndGaps() {
        let changeset = makeChangeset([makeFile("a.swift"), makeFile("b.swift")])
        let surface = Surface.build(changeset)
        XCTAssertEqual(surface.fileAnchors, [0, 4])
        XCTAssertEqual(surface.rows.count, 8)
        XCTAssertEqual(surface.rows[0], .fileHeader(file: 0))
        XCTAssertEqual(surface.rows[1], .gap(skipped: 3))
        XCTAssertEqual(surface.rows[2], .line(file: 0, hunk: 0, row: 0))
        XCTAssertEqual(surface.rows[4], .fileHeader(file: 1))
        XCTAssertEqual(surface.hunkAnchors, [2, 6])
    }

    func testFileIndexAtRowIsTheLastAnchorAtOrAbove() {
        let changeset = makeChangeset([makeFile("a.swift"), makeFile("b.swift")])
        let surface = Surface.build(changeset)
        XCTAssertEqual(surface.fileIndex(atRow: 0), 0)
        XCTAssertEqual(surface.fileIndex(atRow: 3), 0)
        XCTAssertEqual(surface.fileIndex(atRow: 4), 1)
        XCTAssertEqual(surface.fileIndex(atRow: 7), 1)
    }

    func testEmptySurface() {
        let surface = Surface.build(makeChangeset([]))
        XCTAssertTrue(surface.rows.isEmpty)
        XCTAssertNil(surface.fileIndex(atRow: 0))
    }
}

final class FileTreeTests: XCTestCase {
    func testDirectoriesAggregateAndSortBeforeFiles() {
        let roots = FileTree.build([
            makeFile("readme.md", added: 5, removed: 0),
            makeFile("src/main.swift", added: 2, removed: 1),
            makeFile("src/util.swift", added: 3, removed: 4),
        ])
        XCTAssertEqual(roots.map(\.name), ["src", "readme.md"])
        let src = roots[0]
        XCTAssertTrue(src.isDirectory)
        XCTAssertEqual(src.added, 5)
        XCTAssertEqual(src.removed, 5)
        XCTAssertEqual(src.children.map(\.name), ["main.swift", "util.swift"])
    }

    func testSingleChildDirectoryChainsCompact() {
        let roots = FileTree.build([
            makeFile("apps/web/components/button.tsx"),
            makeFile("apps/web/components/input.tsx"),
        ])
        XCTAssertEqual(roots.count, 1)
        XCTAssertEqual(roots[0].name, "apps/web/components")
        XCTAssertEqual(roots[0].path, "apps/web/components")
        XCTAssertEqual(roots[0].children.count, 2)
        XCTAssertFalse(roots[0].children[0].isDirectory)
    }

    func testCompactionStopsAtBranchingDirectories() {
        let roots = FileTree.build([
            makeFile("apps/web/a.ts"),
            makeFile("apps/api/b.ts"),
        ])
        XCTAssertEqual(roots.count, 1)
        XCTAssertEqual(roots[0].name, "apps")
        XCTAssertEqual(roots[0].children.map(\.name), ["api", "web"])
    }

    func testLeavesCarryFileIndexAndStatus() {
        let roots = FileTree.build([makeFile("a.swift", status: .added)])
        XCTAssertEqual(roots[0].fileIndex, 0)
        XCTAssertEqual(roots[0].status, .added)
    }
}

final class FindTests: XCTestCase {
    private var changeset: Changeset {
        makeChangeset([makeFile("src/Value.swift"), makeFile("docs/notes.md")])
    }

    func testMatchesLineTextCaseInsensitive() {
        let surface = Surface.build(changeset)
        let hits = FindMatcher.matches(query: "VALUE", surface: surface, changeset: changeset)
        // File 0's header path, both its line rows, and file 1's line rows.
        XCTAssertEqual(hits, [0, 2, 3, 6, 7])
    }

    func testMatchesHeaderOnPathOnly() {
        let surface = Surface.build(changeset)
        let hits = FindMatcher.matches(query: "notes.md", surface: surface, changeset: changeset)
        XCTAssertEqual(hits, [4])
    }

    func testMatchesOldSideOfChangeRows() {
        let surface = Surface.build(changeset)
        let hits = FindMatcher.matches(query: "= 1", surface: surface, changeset: changeset)
        XCTAssertEqual(hits, [2, 6])
    }

    func testEmptyQueryMatchesNothing() {
        let surface = Surface.build(changeset)
        XCTAssertTrue(
            FindMatcher.matches(query: "", surface: surface, changeset: changeset).isEmpty)
    }

    func testGapsNeverMatch() {
        let surface = Surface.build(changeset)
        let hits = FindMatcher.matches(query: "unchanged", surface: surface, changeset: changeset)
        XCTAssertTrue(hits.isEmpty)
    }
}
