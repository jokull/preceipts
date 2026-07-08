// PR feedback: review comments, review bodies, and conversation comments
// from GitHub — across human users and GitHub Apps (bots). Port of the
// Rust core's feedback.rs.
//
// The transport (gh CLI now, OAuth later) lives in the app; this is the
// pure mapping from the three GitHub payloads to the model, so it stays
// unit-tested while the network edge stays thin. Every comment renders to
// the clipboard through the same CommentContext as local drafts.

import Foundation

public enum FeedbackKind: Sendable, Equatable {
    /// Line-anchored review comment on the diff.
    case reviewComment
    /// A review's top-level body (APPROVED / CHANGES_REQUESTED / COMMENTED).
    case review
    /// PR conversation (issue) comment.
    case conversation
}

public struct FeedbackComment: Sendable, Equatable {
    public let kind: FeedbackKind
    /// GitHub id; 0 for kinds that never thread.
    public let id: Int
    /// Root comment this replies to — review-comment threads.
    public let inReplyTo: Int?
    public let author: String
    public let isBot: Bool
    /// GitHub `user.avatar_url`; nil for payloads without a user object.
    public let avatarUrl: String?
    public let body: String
    /// Anchor, when the comment is line-anchored.
    public let path: String?
    public let line: Int?
    /// First line of a multi-line anchor; nil for single-line comments.
    public let startLine: Int?
    /// Which version of the file the anchor points at (GitHub LEFT/RIGHT).
    public let side: CommentSide
    /// The anchored diff line's text (tail of GitHub's diff_hunk).
    public let lineText: String
    /// Line-anchored but the diff moved on — still shown, flagged.
    public let outdated: Bool
    public let createdAt: String
    public let url: String
    /// Review state for `review` kind ("APPROVED", "CHANGES_REQUESTED", …).
    public let state: String?

    public var context: CommentContext {
        CommentContext(
            path: path ?? "(conversation)",
            line: line ?? 0,
            startLine: startLine,
            lineText: lineText,
            body: body,
            author: author)
    }
}

/// GraphQL-side identity + lifecycle of a review thread, keyed off its
/// root comment's REST id. The node id is what the resolve/unresolve
/// mutations take; REST exposes neither.
public struct ReviewThreadMeta: Sendable, Equatable {
    public let nodeId: String
    public let isResolved: Bool

    public init(nodeId: String, isResolved: Bool) {
        self.nodeId = nodeId
        self.isResolved = isResolved
    }
}

public struct PrFeedback: Sendable {
    public let number: Int
    public let title: String
    public let url: String
    public let comments: [FeedbackComment]
    /// Root comment id → thread meta (GraphQL-only data; empty when the
    /// transport couldn't fetch it).
    public let threadMeta: [Int: ReviewThreadMeta]

    /// Root comment ids of resolved review threads.
    public var resolvedRootIds: Set<Int> {
        Set(threadMeta.filter { $0.value.isResolved }.keys)
    }

    public init(
        number: Int, title: String, url: String, comments: [FeedbackComment],
        threadMeta: [Int: ReviewThreadMeta] = [:]
    ) {
        self.number = number
        self.title = title
        self.url = url
        self.comments = comments
        self.threadMeta = threadMeta
    }

    /// "Copy all" for whatever subset the UI filtered down to.
    public static func digest(_ comments: [FeedbackComment]) -> String {
        formatDigest(comments.map(\.context))
    }
}

/// Pure mapping from the three GitHub payloads (each a JSON array as
/// returned by `pulls/N/comments`, `pulls/N/reviews`,
/// `issues/N/comments`) to the model, sorted by creation time.
public func parseFeedback(
    reviewComments: Data, reviews: Data, conversation: Data
) throws -> [FeedbackComment] {
    var out: [FeedbackComment] = []

    for item in try jsonArray(reviewComments) {
        let line = item["line"] as? Int
        let originalLine = item["original_line"] as? Int
        let startLine = item["start_line"] as? Int ?? item["original_start_line"] as? Int
        out.append(
            FeedbackComment(
                kind: .reviewComment,
                id: item["id"] as? Int ?? 0,
                inReplyTo: item["in_reply_to_id"] as? Int,
                author: authorLogin(item),
                isBot: isBot(item),
                avatarUrl: avatarUrl(item),
                body: string(item, "body"),
                path: item["path"] as? String,
                line: line ?? originalLine,
                startLine: startLine,
                side: string(item, "side") == "LEFT" ? .old : .new,
                lineText: lastHunkLine(item["diff_hunk"] as? String ?? ""),
                // GitHub nulls `line`/`position` when the diff has moved on.
                outdated: line == nil,
                createdAt: string(item, "created_at"),
                url: string(item, "html_url"),
                state: nil))
    }

    for item in try jsonArray(reviews) {
        let body = string(item, "body")
        let state = string(item, "state")
        // Empty COMMENTED review bodies are shells around line comments.
        if body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            state == "COMMENTED"
        {
            continue
        }
        out.append(
            FeedbackComment(
                kind: .review,
                id: item["id"] as? Int ?? 0,
                inReplyTo: nil,
                author: authorLogin(item),
                isBot: isBot(item),
                avatarUrl: avatarUrl(item),
                body: body,
                path: nil,
                line: nil,
                startLine: nil,
                side: .new,
                lineText: "",
                outdated: false,
                createdAt: string(item, "submitted_at"),
                url: string(item, "html_url"),
                state: state))
    }

    for item in try jsonArray(conversation) {
        out.append(
            FeedbackComment(
                kind: .conversation,
                id: item["id"] as? Int ?? 0,
                inReplyTo: nil,
                author: authorLogin(item),
                isBot: isBot(item),
                avatarUrl: avatarUrl(item),
                body: string(item, "body"),
                path: nil,
                line: nil,
                startLine: nil,
                side: .new,
                lineText: "",
                outdated: false,
                createdAt: string(item, "created_at"),
                url: string(item, "html_url"),
                state: nil))
    }

    return out.sorted { $0.createdAt < $1.createdAt }
}

/// Review-thread metadata from the GraphQL
/// `reviewThreads { id isResolved comments(first: 1) { databaseId } }`
/// response (REST has neither resolution nor thread node ids).
/// Transport-agnostic: both the signed-in client and `gh api graphql`
/// return this exact shape.
public func parseReviewThreadMeta(_ data: Data) throws -> [Int: ReviewThreadMeta] {
    guard
        let root = try JSONSerialization.jsonObject(with: data) as? [String: Any],
        let repository = ((root["data"] as? [String: Any])?["repository"]) as? [String: Any],
        let threads = (((repository["pullRequest"] as? [String: Any])?["reviewThreads"])
            as? [String: Any])?["nodes"] as? [[String: Any]]
    else {
        return [:]
    }
    var meta: [Int: ReviewThreadMeta] = [:]
    for thread in threads {
        guard let nodeId = thread["id"] as? String,
            let comments = (thread["comments"] as? [String: Any])?["nodes"]
                as? [[String: Any]],
            let rootId = comments.first?["databaseId"] as? Int
        else { continue }
        meta[rootId] = ReviewThreadMeta(
            nodeId: nodeId, isResolved: thread["isResolved"] as? Bool == true)
    }
    return meta
}

// ------------------------------------------------------------------
// Filtering — the sidebar's thread scope control. What passes here is
// what exists downstream: bubbles, badges, navigation.

public struct FeedbackFilter: Equatable, Sendable {
    public enum Authors: String, Sendable {
        case all, humans, bots
    }

    public var authors: Authors
    public var showResolved: Bool
    public var showOutdated: Bool

    /// Defaults mirror GitHub: unresolved, current-diff threads only.
    public init(
        authors: Authors = .all, showResolved: Bool = false, showOutdated: Bool = false
    ) {
        self.authors = authors
        self.showResolved = showResolved
        self.showOutdated = showOutdated
    }

    public func includes(_ thread: FeedbackThread, resolved: Bool) -> Bool {
        switch authors {
        case .all:
            break
        case .humans:
            if thread.root.isBot { return false }
        case .bots:
            if !thread.root.isBot { return false }
        }
        if !showOutdated, thread.root.outdated {
            return false
        }
        return showResolved || !resolved
    }
}

// ------------------------------------------------------------------
// Threads

/// One review-comment conversation: a root and its replies, in time
/// order. Reviews and conversation comments are single-comment threads.
public struct FeedbackThread: Sendable, Equatable {
    public let root: FeedbackComment
    public let replies: [FeedbackComment]

    public var comments: [FeedbackComment] { [root] + replies }

    /// Anchored line span (start…end on `root.side`); nil when unanchored.
    public var lineRange: ClosedRange<Int>? {
        guard let line = root.line else { return nil }
        let start = root.startLine ?? line
        return min(start, line)...max(start, line)
    }

    public init(root: FeedbackComment, replies: [FeedbackComment]) {
        self.root = root
        self.replies = replies
    }

    /// Fold a flat, time-sorted comment list into threads. Replies whose
    /// root is missing (rare pagination edge) become their own roots.
    public static func group(_ comments: [FeedbackComment]) -> [FeedbackThread] {
        var repliesByRoot: [Int: [FeedbackComment]] = [:]
        var roots: [FeedbackComment] = []
        for comment in comments {
            if let parent = comment.inReplyTo {
                repliesByRoot[parent, default: []].append(comment)
            } else {
                roots.append(comment)
            }
        }
        // Orphaned replies (root not fetched) surface as roots.
        let rootIds = Set(roots.map(\.id))
        for (parent, orphans) in repliesByRoot where !rootIds.contains(parent) {
            roots.append(contentsOf: orphans)
            repliesByRoot[parent] = nil
        }
        return roots
            .sorted { $0.createdAt < $1.createdAt }
            .map { FeedbackThread(root: $0, replies: repliesByRoot[$0.id] ?? []) }
    }
}

private func jsonArray(_ data: Data) throws -> [[String: Any]] {
    guard !data.isEmpty else { return [] }
    let parsed = try JSONSerialization.jsonObject(with: data)
    return parsed as? [[String: Any]] ?? []
}

private func string(_ item: [String: Any], _ field: String) -> String {
    item[field] as? String ?? ""
}

private func authorLogin(_ item: [String: Any]) -> String {
    let user = item["user"] as? [String: Any]
    return user?["login"] as? String ?? "(unknown)"
}

private func avatarUrl(_ item: [String: Any]) -> String? {
    (item["user"] as? [String: Any])?["avatar_url"] as? String
}

private func isBot(_ item: [String: Any]) -> Bool {
    let user = item["user"] as? [String: Any]
    if user?["type"] as? String == "Bot" {
        return true
    }
    return (user?["login"] as? String)?.hasSuffix("[bot]") ?? false
}

/// The last line of a diff_hunk is the line the comment anchors to;
/// strip the +/-/space diff marker. Trailing-newline handling matches
/// Rust's `str::lines` (one trailing empty segment dropped).
func lastHunkLine(_ hunk: String) -> String {
    var lines = hunk.split(separator: "\n", omittingEmptySubsequences: false)
    if lines.last == "" {
        lines.removeLast()
    }
    guard let last = lines.last else { return "" }
    let marker = last.first
    if marker == "+" || marker == "-" || marker == " " {
        return String(last.dropFirst())
    }
    return String(last)
}
