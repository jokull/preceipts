// Review comments: local drafts (notes-to-agent, never posted) and the
// clipboard formats that feed them to a coding agent. Port of the Rust
// core's comments.rs.
//
// Local comments live under `.git/preceipts/comments.json`, keyed by
// branch — review state, not repo content; they survive the app closing
// but not intentional cleanup. GitHub feedback renders through the same
// CommentContext so every comment shares the Copy to Clipboard affordance.

import Foundation

public enum CommentSide: String, Codable, Sendable {
    case old
    case new
}

public struct LocalComment: Codable, Sendable, Identifiable {
    public let id: UInt64
    public let path: String
    /// 1-based line number on `side` of the diff.
    public let line: Int
    public let side: CommentSide
    /// The diff line's text when the comment was written — quoted in the
    /// clipboard format and used to flag the comment as outdated later.
    public let lineText: String
    public var body: String
    /// Unix seconds; display formatting is the UI's job.
    public let createdAt: UInt64

    enum CodingKeys: String, CodingKey {
        case id, path, line, side, body
        case lineText = "line_text"
        case createdAt = "created_at"
    }
}

/// Everything needed to render one comment to the clipboard, regardless
/// of origin (local draft or GitHub).
public struct CommentContext {
    public let path: String
    public let line: Int
    public let lineText: String
    public let body: String
    /// Nil for local drafts; "octocat" / "coderabbit[bot]" for GitHub.
    public let author: String?

    public init(path: String, line: Int, lineText: String, body: String, author: String?) {
        self.path = path
        self.line = line
        self.lineText = lineText
        self.body = body
        self.author = author
    }
}

/// One comment as a markdown block with file:line context:
///
///     apps/next/components/foo.tsx:123
///     > const x = useMemo(...)
///     This memo is unnecessary — props are primitives.
///
public func formatComment(_ comment: CommentContext) -> String {
    var out = "\(comment.path):\(comment.line)\n"
    let quoted = comment.lineText.trimmingTrailingWhitespace()
    if !quoted.trimmingCharacters(in: .whitespaces).isEmpty {
        out += "> \(quoted)\n"
    }
    if let author = comment.author {
        out += "\u{2014} \(author): "
    }
    out += comment.body.trimmingCharacters(in: .whitespacesAndNewlines)
    out += "\n"
    return out
}

/// Many comments as one digest, grouped by file — a paste-ready worklist
/// for a coding agent.
public func formatDigest(_ comments: [CommentContext]) -> String {
    var byFile: [String: [String]] = [:]
    for comment in comments {
        byFile[comment.path, default: []].append(formatComment(comment))
    }
    var out = ""
    for path in byFile.keys.sorted() {
        out += "## \(path)\n\n"
        for block in byFile[path]! {
            out += block
            out += "\n"
        }
    }
    return out.trimmingTrailingWhitespace() + "\n"
}

// ------------------------------------------------------------------
// Store

public final class CommentStore {
    private struct StoreFile: Codable {
        /// branch name → comments. Drafts are review state for a branch's PR.
        var branches: [String: [LocalComment]] = [:]
        var nextId: UInt64 = 0

        enum CodingKeys: String, CodingKey {
            case branches
            case nextId = "next_id"
        }
    }

    private let file: URL
    private let branch: String
    private var data: StoreFile

    /// Open (or create) the store for a repo's git dir and current branch.
    public init(gitDir: URL, branch: String) throws {
        let dir = gitDir.appendingPathComponent("preceipts")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        self.file = dir.appendingPathComponent("comments.json")
        self.branch = branch
        if let bytes = try? Data(contentsOf: file),
            let decoded = try? JSONDecoder().decode(StoreFile.self, from: bytes)
        {
            self.data = decoded
        } else {
            self.data = StoreFile()
        }
    }

    public var comments: [LocalComment] {
        data.branches[branch] ?? []
    }

    @discardableResult
    public func add(
        path: String, line: Int, side: CommentSide, lineText: String, body: String
    ) throws -> LocalComment {
        data.nextId += 1
        let comment = LocalComment(
            id: data.nextId,
            path: path,
            line: line,
            side: side,
            lineText: lineText,
            body: body,
            createdAt: UInt64(Date().timeIntervalSince1970))
        data.branches[branch, default: []].append(comment)
        try save()
        return comment
    }

    public func remove(id: UInt64) throws {
        data.branches[branch]?.removeAll { $0.id == id }
        try save()
    }

    public func updateBody(id: UInt64, body: String) throws {
        guard var list = data.branches[branch],
            let index = list.firstIndex(where: { $0.id == id })
        else { return }
        list[index].body = body
        data.branches[branch] = list
        try save()
    }

    /// Digest of every draft on this branch — the "Copy all" affordance.
    public func digest() -> String {
        formatDigest(
            comments.map {
                CommentContext(
                    path: $0.path, line: $0.line, lineText: $0.lineText,
                    body: $0.body, author: nil)
            })
    }

    private func save() throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(data).write(to: file)
    }
}

extension String {
    fileprivate func trimmingTrailingWhitespace() -> String {
        var view = self[...]
        while let last = view.last, last.isWhitespace {
            view = view.dropLast()
        }
        return String(view)
    }
}
