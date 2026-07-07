// The diff row model — Swift port of the (deprecated) Rust core's model.
// Rows are fixed-height and precomputed; see docs/desktop-foundations.md.

import Foundation

public enum DiffScope: Sendable {
    case branch
    case uncommitted
}

public enum FileStatus: String, Sendable {
    case added = "A"
    case modified = "M"
    case deleted = "D"
    case renamed = "R"
}

public enum RowKind: Sendable {
    case context
    /// Old side removed, new side added (similarity-paired edit).
    case change
    case addition
    case removal
}

public struct LineRef: Sendable {
    /// 1-based line number in its version of the file.
    public let number: Int
    public let text: String

    public init(number: Int, text: String) {
        self.number = number
        self.text = text
    }
}

public struct DiffRow: Sendable {
    public let kind: RowKind
    public let old: LineRef?
    public let new: LineRef?
    /// Word-level changed byte ranges (change rows only), UTF-8 offsets.
    public let oldChanged: [Range<Int>]
    public let newChanged: [Range<Int>]

    public init(
        kind: RowKind,
        old: LineRef?,
        new: LineRef?,
        oldChanged: [Range<Int>] = [],
        newChanged: [Range<Int>] = []
    ) {
        self.kind = kind
        self.old = old
        self.new = new
        self.oldChanged = oldChanged
        self.newChanged = newChanged
    }
}

public struct DiffHunk: Sendable {
    /// Unchanged lines skipped since the previous hunk (or file start).
    public let skippedBefore: Int
    public var rows: [DiffRow]

    public init(skippedBefore: Int, rows: [DiffRow]) {
        self.skippedBefore = skippedBefore
        self.rows = rows
    }
}

public struct FileDiff: Sendable {
    public let path: String
    public let oldPath: String?
    public let status: FileStatus
    public let isBinary: Bool
    public let added: Int
    public let removed: Int
    public let hunks: [DiffHunk]

    public init(
        path: String,
        oldPath: String?,
        status: FileStatus,
        isBinary: Bool,
        added: Int,
        removed: Int,
        hunks: [DiffHunk]
    ) {
        self.path = path
        self.oldPath = oldPath
        self.status = status
        self.isBinary = isBinary
        self.added = added
        self.removed = removed
        self.hunks = hunks
    }
}

public struct Changeset: Sendable {
    public let scope: DiffScope
    /// Human name of the comparison base ("origin/main…" or "HEAD").
    public let baseName: String
    public let branch: String?
    public let workdir: URL
    public let gitDir: URL
    public let files: [FileDiff]

    public var totalAdded: Int { files.reduce(0) { $0 + $1.added } }
    public var totalRemoved: Int { files.reduce(0) { $0 + $1.removed } }

    public init(
        scope: DiffScope,
        baseName: String,
        branch: String?,
        workdir: URL,
        gitDir: URL,
        files: [FileDiff]
    ) {
        self.scope = scope
        self.baseName = baseName
        self.branch = branch
        self.workdir = workdir
        self.gitDir = gitDir
        self.files = files
    }
}

public enum PreceiptsError: Error, LocalizedError {
    case notARepo(String)
    case noBase
    case unbornHead
    case git(String)

    public var errorDescription: String? {
        switch self {
        case .notARepo(let path): return "not a git repository: \(path)"
        case .noBase:
            return "no base branch found (tried origin/main, main, origin/master, master)"
        case .unbornHead: return "HEAD does not resolve — unborn branch?"
        case .git(let message): return message
        }
    }
}
