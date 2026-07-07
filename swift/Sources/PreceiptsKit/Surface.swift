// The scroll-surface row model: every changed file flattened into one
// virtualized list (the Zed multibuffer shape, docs/desktop-app-design.md).
// Pure data — built from a Changeset, consumed by the AppKit surface table.

import Foundation

public enum SurfaceRow: Equatable, Sendable {
    case fileHeader(file: Int)
    case gap(skipped: Int)
    case line(file: Int, hunk: Int, row: Int)
}

public struct Surface: Sendable {
    public let rows: [SurfaceRow]
    /// Index into `rows` of each file's header, by file index.
    public let fileAnchors: [Int]
    /// Index into `rows` of each hunk's first line, in surface order.
    public let hunkAnchors: [Int]

    public static let empty = Surface(rows: [], fileAnchors: [], hunkAnchors: [])

    public init(rows: [SurfaceRow], fileAnchors: [Int], hunkAnchors: [Int]) {
        self.rows = rows
        self.fileAnchors = fileAnchors
        self.hunkAnchors = hunkAnchors
    }

    public static func build(_ changeset: Changeset) -> Surface {
        var rows: [SurfaceRow] = []
        var fileAnchors: [Int] = []
        var hunkAnchors: [Int] = []
        for (fileIndex, file) in changeset.files.enumerated() {
            fileAnchors.append(rows.count)
            rows.append(.fileHeader(file: fileIndex))
            for (hunkIndex, hunk) in file.hunks.enumerated() {
                if hunk.skippedBefore > 0 {
                    rows.append(.gap(skipped: hunk.skippedBefore))
                }
                if !hunk.rows.isEmpty {
                    hunkAnchors.append(rows.count)
                }
                for rowIndex in hunk.rows.indices {
                    rows.append(.line(file: fileIndex, hunk: hunkIndex, row: rowIndex))
                }
            }
        }
        return Surface(rows: rows, fileAnchors: fileAnchors, hunkAnchors: hunkAnchors)
    }

    /// Resolve a comment anchor (path + 1-based line + side) to its surface
    /// row. Nil when the file isn't in the changeset or the line isn't in
    /// any rendered hunk — the comment is outdated or outside the diff.
    public func anchorRow(
        path: String, line: Int, side: CommentSide, changeset: Changeset
    ) -> Int? {
        guard let fileIndex = changeset.files.firstIndex(where: { $0.path == path })
        else { return nil }
        let start = fileAnchors[fileIndex]
        let end = fileIndex + 1 < fileAnchors.count ? fileAnchors[fileIndex + 1] : rows.count
        let file = changeset.files[fileIndex]
        for index in start..<end {
            guard case .line(_, let hunk, let row) = rows[index] else { continue }
            let diffRow = file.hunks[hunk].rows[row]
            let candidate = side == .new ? diffRow.new : diffRow.old
            if candidate?.number == line {
                return index
            }
        }
        return nil
    }

    /// The file whose section contains `row` — the last anchor at or above
    /// it. Nil when the surface is empty or `row` precedes the first header.
    public func fileIndex(atRow row: Int) -> Int? {
        var low = 0
        var high = fileAnchors.count - 1
        var best: Int?
        while low <= high {
            let mid = (low + high) / 2
            if fileAnchors[mid] <= row {
                best = mid
                low = mid + 1
            } else {
                high = mid - 1
            }
        }
        return best
    }
}
