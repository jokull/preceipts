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

    func testFormatMultiLineRangeQuotesEveryLine() {
        let text = formatComment(
            CommentContext(
                path: "a.ts", line: 107, startLine: 104,
                lineText: "for (const x of xs) {\n  total += x\n}",
                body: "move to reducer", author: nil))
        XCTAssertEqual(
            text,
            """
            a.ts:104\u{2013}107
            > for (const x of xs) {
            >   total += x
            > }
            move to reducer

            """)
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

    func testMultiLineDraftDigestQuotesTheBlock() throws {
        let gitDir = try makeGitDir()
        defer { try? FileManager.default.removeItem(at: gitDir) }
        let store = try CommentStore(gitDir: gitDir, branch: "b")
        try store.add(
            path: "a.tsx", line: 107, startLine: 104, side: .new,
            lineText: "}",
            quote: "function f() {\n  work()\n}",
            body: "extract this")
        let digest = store.digest()
        XCTAssertTrue(digest.contains("a.tsx:104\u{2013}107"))
        XCTAssertTrue(digest.contains("> function f() {"))
        XCTAssertTrue(digest.contains(">   work()"))
        XCTAssertTrue(digest.contains("> }"))

        // And the quote survives a store reopen.
        let reopened = try CommentStore(gitDir: gitDir, branch: "b")
        XCTAssertEqual(reopened.comments[0].quote, "function f() {\n  work()\n}")
        XCTAssertEqual(reopened.comments[0].lineRange, 104...107)
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

    func testThreadGroupingByInReplyTo() throws {
        let reviewComments = Data(
            """
            [
              {"id": 1, "user": {"login": "codex[bot]"}, "body": "root",
               "path": "a.ts", "line": 12, "start_line": 8, "side": "RIGHT",
               "diff_hunk": "@@\\n+x", "created_at": "2026-07-07T00:00:00Z",
               "html_url": "u1"},
              {"id": 2, "in_reply_to_id": 1, "user": {"login": "jokull"},
               "body": "reply", "path": "a.ts", "line": 12, "side": "RIGHT",
               "diff_hunk": "@@\\n+x", "created_at": "2026-07-07T01:00:00Z",
               "html_url": "u2"},
              {"id": 3, "user": {"login": "jokull"}, "body": "old side",
               "path": "b.ts", "line": 4, "side": "LEFT",
               "diff_hunk": "@@\\n-y", "created_at": "2026-07-07T02:00:00Z",
               "html_url": "u3"}
            ]
            """.utf8)
        let comments = try parseFeedback(
            reviewComments: reviewComments, reviews: empty, conversation: empty)
        let threads = FeedbackThread.group(comments)
        XCTAssertEqual(threads.count, 2)
        XCTAssertEqual(threads[0].root.id, 1)
        XCTAssertEqual(threads[0].replies.map(\.id), [2])
        XCTAssertEqual(threads[0].lineRange, 8...12)
        XCTAssertEqual(threads[0].root.side, .new)
        XCTAssertEqual(threads[1].root.side, .old)
        XCTAssertEqual(threads[1].lineRange, 4...4)
    }

    func testOrphanedRepliesBecomeRoots() throws {
        let reviewComments = Data(
            """
            [{"id": 9, "in_reply_to_id": 999, "user": {"login": "a"},
              "body": "orphan", "path": "a.ts", "line": 1, "side": "RIGHT",
              "diff_hunk": "@@\\n+x", "created_at": "2026-07-07T00:00:00Z",
              "html_url": "u"}]
            """.utf8)
        let comments = try parseFeedback(
            reviewComments: reviewComments, reviews: empty, conversation: empty)
        let threads = FeedbackThread.group(comments)
        XCTAssertEqual(threads.count, 1)
        XCTAssertEqual(threads[0].root.id, 9)
        XCTAssertTrue(threads[0].replies.isEmpty)
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

    func testParseReviewThreadMeta() throws {
        let json = Data(
            """
            {"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": [
              {"id": "RT_1", "isResolved": true,
               "comments": {"nodes": [{"databaseId": 11}]}},
              {"id": "RT_2", "isResolved": false,
               "comments": {"nodes": [{"databaseId": 22}]}},
              {"id": "RT_3", "isResolved": true,
               "comments": {"nodes": [{"databaseId": 33}]}}
            ]}}}}}
            """.utf8)
        let meta = try parseReviewThreadMeta(json)
        XCTAssertEqual(meta[11], ReviewThreadMeta(nodeId: "RT_1", isResolved: true))
        XCTAssertEqual(meta[22], ReviewThreadMeta(nodeId: "RT_2", isResolved: false))
        XCTAssertEqual(meta[33]?.isResolved, true)
        XCTAssertEqual(try parseReviewThreadMeta(Data("{}".utf8)), [:])

        let feedback = PrFeedback(
            number: 1, title: "t", url: "u", comments: [], threadMeta: meta)
        XCTAssertEqual(feedback.resolvedRootIds, [11, 33])
    }

    func testFeedbackFilter() throws {
        let reviewComments = Data(
            """
            [
              {"id": 1, "user": {"login": "codex[bot]", "type": "Bot"}, "body": "b",
               "path": "a.ts", "line": 1, "side": "RIGHT", "diff_hunk": "@@\\n+x",
               "created_at": "2026-07-07T00:00:00Z", "html_url": "u1"},
              {"id": 2, "user": {"login": "jokull"}, "body": "h",
               "path": "b.ts", "line": 2, "side": "RIGHT", "diff_hunk": "@@\\n+y",
               "created_at": "2026-07-07T01:00:00Z", "html_url": "u2"},
              {"id": 3, "user": {"login": "jokull"}, "body": "moved on",
               "path": "c.ts", "line": null, "original_line": 9, "side": "RIGHT",
               "diff_hunk": "@@\\n+z", "created_at": "2026-07-07T02:00:00Z",
               "html_url": "u3"}
            ]
            """.utf8)
        let threads = FeedbackThread.group(
            try parseFeedback(reviewComments: reviewComments, reviews: empty, conversation: empty))
        let bot = threads[0]
        let human = threads[1]
        let outdated = threads[2]

        XCTAssertTrue(FeedbackFilter().includes(bot, resolved: false))
        XCTAssertFalse(FeedbackFilter().includes(bot, resolved: true))
        XCTAssertTrue(
            FeedbackFilter(showResolved: true).includes(bot, resolved: true))
        XCTAssertFalse(FeedbackFilter(authors: .humans).includes(bot, resolved: false))
        XCTAssertTrue(FeedbackFilter(authors: .humans).includes(human, resolved: false))
        XCTAssertTrue(FeedbackFilter(authors: .bots).includes(bot, resolved: false))
        XCTAssertFalse(FeedbackFilter(authors: .bots).includes(human, resolved: false))

        // Outdated threads hide by default, like GitHub.
        XCTAssertTrue(outdated.root.outdated)
        XCTAssertFalse(FeedbackFilter().includes(outdated, resolved: false))
        XCTAssertTrue(
            FeedbackFilter(showOutdated: true).includes(outdated, resolved: false))
        XCTAssertFalse(
            FeedbackFilter(showOutdated: true).includes(outdated, resolved: true))
    }

    func testParseAvatarUrls() throws {
        let reviewComments = Data(
            """
            [{"user": {"login": "codex[bot]", "type": "Bot",
               "avatar_url": "https://avatars.githubusercontent.com/in/1?v=4"},
              "body": "b", "path": "a.ts", "line": 1, "side": "RIGHT",
              "diff_hunk": "@@\\n+x", "created_at": "2026-07-07T00:00:00Z",
              "html_url": "u"}]
            """.utf8)
        let comments = try parseFeedback(
            reviewComments: reviewComments, reviews: empty, conversation: empty)
        XCTAssertEqual(
            comments[0].avatarUrl, "https://avatars.githubusercontent.com/in/1?v=4")
    }

    func testParsePrOverviewGraphQL() throws {
        let json = Data(
            """
            {"data": {"repository": {"pullRequests": {"nodes": [{
              "number": 2548, "title": "feat: chat", "body": "The body",
              "url": "https://github.com/o/r/pull/2548",
              "reviewDecision": "REVIEW_REQUIRED",
              "history": {"nodes": [
                {"commit": {"abbreviatedOid": "abc1234",
                  "messageHeadline": "first", "committedDate": "2026-07-01T00:00:00Z",
                  "author": {"name": "J", "user": {"login": "jokull"}}}},
                {"commit": {"abbreviatedOid": "def5678",
                  "messageHeadline": "second", "committedDate": "2026-07-02T00:00:00Z",
                  "author": {"name": "Codex", "user": null}}}
              ]},
              "head": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [
                {"context": "Vercel", "state": "SUCCESS",
                 "targetUrl": "https://vercel.com/d/1", "description": "Deployed"},
                {"name": "test", "status": "COMPLETED", "conclusion": "FAILURE",
                 "detailsUrl": "https://github.com/checks/1", "title": "2 failed"},
                {"name": "build", "status": "IN_PROGRESS", "conclusion": null,
                 "detailsUrl": null, "title": null}
              ]}}}}]}
            }]}}}}
            """.utf8)
        let overview = try XCTUnwrap(parsePrOverview(json))
        XCTAssertEqual(overview.number, 2548)
        XCTAssertEqual(overview.body, "The body")
        // Newest commit first; login preferred over name.
        XCTAssertEqual(overview.commits.map(\.sha), ["def5678", "abc1234"])
        XCTAssertEqual(overview.commits[1].author, "jokull")
        XCTAssertEqual(overview.commits[0].author, "Codex")
        XCTAssertEqual(overview.checks.count, 3)
        XCTAssertEqual(overview.checks[0].name, "Vercel")
        XCTAssertEqual(overview.checks[0].state, .success)
        XCTAssertEqual(overview.checks[0].detailsUrl, "https://vercel.com/d/1")
        XCTAssertEqual(overview.checks[1].state, .failure)
        XCTAssertEqual(overview.checks[2].state, .pending)

        XCTAssertNil(try parsePrOverview(Data("{}".utf8)))
    }

    func testParsePrOverviewGh() throws {
        let json = Data(
            """
            {"number": 7, "title": "t", "body": "b", "url": "u",
             "reviewDecision": "",
             "commits": [
               {"oid": "abcdef0123456789", "messageHeadline": "one",
                "committedDate": "2026-07-01T00:00:00Z",
                "authors": [{"login": "jokull", "name": "J"}]}
             ],
             "statusCheckRollup": [
               {"context": "Cloudflare Pages", "state": "PENDING",
                "targetUrl": "https://dash.cloudflare.com/x"},
               {"name": "i18n", "status": "COMPLETED", "conclusion": "SUCCESS",
                "detailsUrl": "d"}
             ]}
            """.utf8)
        let overview = try XCTUnwrap(parsePrOverviewGh(json))
        XCTAssertEqual(overview.number, 7)
        XCTAssertNil(overview.reviewDecision)
        XCTAssertEqual(overview.commits[0].sha, "abcdef0")
        XCTAssertEqual(overview.commits[0].author, "jokull")
        XCTAssertEqual(overview.checks[0].state, .pending)
        XCTAssertEqual(overview.checks[1].state, .success)
    }

    func testLastHunkLineTrailingNewline() {
        XCTAssertEqual(lastHunkLine("@@\n+added\n"), "added")
        XCTAssertEqual(lastHunkLine(""), "")
        XCTAssertEqual(lastHunkLine("@@\n context line"), "context line")
    }
}
