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
