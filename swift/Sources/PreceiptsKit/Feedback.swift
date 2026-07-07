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
    public let author: String
    public let isBot: Bool
    public let body: String
    /// Anchor, when the comment is line-anchored.
    public let path: String?
    public let line: Int?
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
            lineText: lineText,
            body: body,
            author: author)
    }
}

public struct PrFeedback: Sendable {
    public let number: Int
    public let title: String
    public let url: String
    public let comments: [FeedbackComment]

    public init(number: Int, title: String, url: String, comments: [FeedbackComment]) {
        self.number = number
        self.title = title
        self.url = url
        self.comments = comments
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
        out.append(
            FeedbackComment(
                kind: .reviewComment,
                author: authorLogin(item),
                isBot: isBot(item),
                body: string(item, "body"),
                path: item["path"] as? String,
                line: line ?? originalLine,
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
                author: authorLogin(item),
                isBot: isBot(item),
                body: body,
                path: nil,
                line: nil,
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
                author: authorLogin(item),
                isBot: isBot(item),
                body: string(item, "body"),
                path: nil,
                line: nil,
                lineText: "",
                outdated: false,
                createdAt: string(item, "created_at"),
                url: string(item, "html_url"),
                state: nil))
    }

    return out.sorted { $0.createdAt < $1.createdAt }
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
