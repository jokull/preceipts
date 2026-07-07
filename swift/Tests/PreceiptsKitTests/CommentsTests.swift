// Ports of the Rust core's comments.rs + feedback.rs tests — clipboard
// formats, the branch-keyed draft store, and GitHub payload parsing.

import XCTest

@testable import PreceiptsKit

final class CommentFormatTests: XCTestCase {
    func testFormatCommentIncludesContext() {
        let text = formatComment(
            CommentContext(
                path: "src/foo.rs", line: 42, lineText: "let x = 1;  ",
                body: "  rename this  ", author: nil))
        XCTAssertEqual(text, "src/foo.rs:42\n> let x = 1;\nrename this\n")
    }

    func testFormatCommentWithAuthor() {
        let text = formatComment(
            CommentContext(
                path: "a.ts", line: 7, lineText: "const x = 1",
                body: "why const?", author: "octocat"))
        XCTAssertEqual(text, "a.ts:7\n> const x = 1\n\u{2014} octocat: why const?\n")
    }

    func testFormatCommentSkipsBlankLineText() {
        let text = formatComment(
            CommentContext(path: "a.ts", line: 1, lineText: "   ", body: "note", author: nil))
        XCTAssertEqual(text, "a.ts:1\nnote\n")
    }

    func testDigestGroupsByFile() {
        let digest = formatDigest([
            CommentContext(path: "b.ts", line: 2, lineText: "y", body: "second", author: nil),
            CommentContext(path: "a.ts", line: 1, lineText: "x", body: "first", author: nil),
            CommentContext(path: "b.ts", line: 9, lineText: "z", body: "third", author: nil),
        ])
        XCTAssertTrue(digest.hasPrefix("## a.ts\n\na.ts:1\n"))
        XCTAssertTrue(digest.contains("## b.ts\n\nb.ts:2\n"))
        XCTAssertTrue(digest.contains("b.ts:9\n"))
        XCTAssertTrue(digest.hasSuffix("third\n"))
    }
}

final class CommentStoreTests: XCTestCase {
    private func makeGitDir() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("preceipts-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    func testAddPersistsAcrossReopen() throws {
        let gitDir = try makeGitDir()
        defer { try? FileManager.default.removeItem(at: gitDir) }

        let store = try CommentStore(gitDir: gitDir, branch: "feature")
        try store.add(
            path: "src/a.swift", line: 12, side: .new,
            lineText: "let x = 1", body: "rename this")
        XCTAssertEqual(store.comments.count, 1)

        let reopened = try CommentStore(gitDir: gitDir, branch: "feature")
        XCTAssertEqual(reopened.comments.count, 1)
        XCTAssertEqual(reopened.comments[0].path, "src/a.swift")
        XCTAssertEqual(reopened.comments[0].side, .new)

        // Drafts are branch-scoped review state.
        let other = try CommentStore(gitDir: gitDir, branch: "main")
        XCTAssertTrue(other.comments.isEmpty)
    }

    func testRemoveAndUpdate() throws {
        let gitDir = try makeGitDir()
        defer { try? FileManager.default.removeItem(at: gitDir) }

        let store = try CommentStore(gitDir: gitDir, branch: "b")
        let first = try store.add(
            path: "a", line: 1, side: .old, lineText: "t", body: "one")
        try store.add(path: "a", line: 2, side: .new, lineText: "u", body: "two")

        try store.updateBody(id: first.id, body: "one, edited")
        XCTAssertEqual(store.comments[0].body, "one, edited")

        try store.remove(id: first.id)
        XCTAssertEqual(store.comments.map(\.body), ["two"])

        // IDs keep advancing after removal.
        let third = try store.add(path: "a", line: 3, side: .new, lineText: "v", body: "three")
        XCTAssertGreaterThan(third.id, first.id + 1)
    }

    func testDigestUsesDrafts() throws {
        let gitDir = try makeGitDir()
        defer { try? FileManager.default.removeItem(at: gitDir) }
        let store = try CommentStore(gitDir: gitDir, branch: "b")
        try store.add(
            path: "src/hot.swift", line: 88, side: .new,
            lineText: "for x in xs {", body: "quadratic")
        let digest = store.digest()
        XCTAssertTrue(digest.contains("## src/hot.swift"))
        XCTAssertTrue(digest.contains("src/hot.swift:88"))
        XCTAssertTrue(digest.contains("> for x in xs {"))
    }
}

final class FeedbackParseTests: XCTestCase {
    private let empty = Data("[]".utf8)

    func testMapsReviewCommentsWithBotAndOutdatedDetection() throws {
        let reviewComments = Data(
            """
            [
              {"user": {"login": "jokull", "type": "User"},
               "body": "rename this", "path": "src/a.rs",
               "line": 12, "original_line": 12,
               "diff_hunk": "@@ -1,3 +1,3 @@\\n context\\n+let x = 1;",
               "created_at": "2026-07-07T00:00:00Z",
               "html_url": "https://github.com/o/r/pull/1#discussion_r1"},
              {"user": {"login": "coderabbitai[bot]", "type": "Bot"},
               "body": "possible null deref", "path": "src/b.rs",
               "line": null, "original_line": 30,
               "diff_hunk": "@@ -1 +1 @@\\n-old()",
               "created_at": "2026-07-07T01:00:00Z",
               "html_url": "https://github.com/o/r/pull/1#discussion_r2"}
            ]
            """.utf8)
        let comments = try parseFeedback(
            reviewComments: reviewComments, reviews: empty, conversation: empty)
        XCTAssertEqual(comments.count, 2)

        XCTAssertEqual(comments[0].author, "jokull")
        XCTAssertFalse(comments[0].isBot)
        XCTAssertFalse(comments[0].outdated)
        XCTAssertEqual(comments[0].line, 12)
        XCTAssertEqual(comments[0].lineText, "let x = 1;")

        XCTAssertTrue(comments[1].isBot)
        XCTAssertTrue(comments[1].outdated)
        XCTAssertEqual(comments[1].line, 30)  // falls back to original_line
        XCTAssertEqual(comments[1].lineText, "old()")
    }

    func testSkipsEmptyCommentedReviewShellsKeepsVerdicts() throws {
        let reviews = Data(
            """
            [
              {"user": {"login": "a"}, "body": "", "state": "COMMENTED",
               "submitted_at": "2026-07-07T00:00:00Z", "html_url": "u1"},
              {"user": {"login": "b"}, "body": "LGTM", "state": "APPROVED",
               "submitted_at": "2026-07-07T01:00:00Z", "html_url": "u2"}
            ]
            """.utf8)
        let comments = try parseFeedback(
            reviewComments: empty, reviews: reviews, conversation: empty)
        XCTAssertEqual(comments.count, 1)
        XCTAssertEqual(comments[0].state, "APPROVED")
        XCTAssertEqual(comments[0].kind, .review)
    }

    func testDigestIncludesAuthorsAndAnchors() throws {
        let reviewComments = Data(
            """
            [{"user": {"login": "codex[bot]"},
              "body": "this loop is O(n^2)", "path": "src/hot.rs", "line": 88,
              "diff_hunk": "@@ @@\\n+for x in xs { for y in ys {} }",
              "created_at": "2026-07-07T00:00:00Z", "html_url": "u"}]
            """.utf8)
        let comments = try parseFeedback(
            reviewComments: reviewComments, reviews: empty, conversation: empty)
        let digest = PrFeedback.digest(comments)
        XCTAssertTrue(digest.contains("## src/hot.rs"))
        XCTAssertTrue(digest.contains("src/hot.rs:88"))
        XCTAssertTrue(digest.contains("> for x in xs { for y in ys {} }"))
        XCTAssertTrue(digest.contains("\u{2014} codex[bot]: this loop is O(n^2)"))
    }

    func testConversationCommentsAndSorting() throws {
        let conversation = Data(
            """
            [{"user": {"login": "z"}, "body": "later",
              "created_at": "2026-07-07T02:00:00Z", "html_url": "u3"}]
            """.utf8)
        let reviews = Data(
            """
            [{"user": {"login": "a"}, "body": "first", "state": "APPROVED",
              "submitted_at": "2026-07-07T01:00:00Z", "html_url": "u1"}]
            """.utf8)
        let comments = try parseFeedback(
            reviewComments: empty, reviews: reviews, conversation: conversation)
        XCTAssertEqual(comments.map(\.author), ["a", "z"])
        XCTAssertEqual(comments[1].kind, .conversation)
    }

    func testLastHunkLineTrailingNewline() {
        XCTAssertEqual(lastHunkLine("@@\n+added\n"), "added")
        XCTAssertEqual(lastHunkLine(""), "")
        XCTAssertEqual(lastHunkLine("@@\n context line"), "context line")
    }
}
