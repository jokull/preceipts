// ⌘F row matching over the scroll surface — the GPUI find bar's semantics
// (case-insensitive substring; headers match on path, line rows on either
// side). Built as an index: case folding is paid once per changeset (off
// the main thread), so per-keystroke matching is one memmem sweep over a
// contiguous buffer — fast enough to run synchronously while typing.

import Foundation

public struct FindIndex: Sendable {
    /// All row texts, lowercased, concatenated with "\n" separators.
    /// Row texts never contain "\n" themselves (paths and single lines),
    /// so a newline-free needle can't match across row boundaries.
    private let buffer: [UInt8]
    /// buffer offset where each row's text begins; count = rows + 1.
    private let rowStarts: [Int]

    public init(surface: Surface, changeset: Changeset) {
        var buffer: [UInt8] = []
        var rowStarts: [Int] = [0]
        rowStarts.reserveCapacity(surface.rows.count + 1)
        for row in surface.rows {
            switch row {
            case .fileHeader(let file):
                buffer.append(contentsOf: changeset.files[file].path.lowercased().utf8)
            case .gap:
                break
            case .line(let file, let hunk, let rowIndex):
                let diffRow = changeset.files[file].hunks[hunk].rows[rowIndex]
                if let old = diffRow.old {
                    buffer.append(contentsOf: old.text.lowercased().utf8)
                }
                if let new = diffRow.new {
                    if diffRow.old != nil {
                        buffer.append(0x0A)
                    }
                    buffer.append(contentsOf: new.text.lowercased().utf8)
                }
            }
            buffer.append(0x0A)
            rowStarts.append(buffer.count)
        }
        self.buffer = buffer
        self.rowStarts = rowStarts
    }

    /// Indices of surface rows whose text contains `query`, case-insensitive.
    /// Empty queries (and queries containing newlines) match nothing.
    public func matches(query: String) -> [Int] {
        let needle = Array(query.lowercased().utf8)
        guard !needle.isEmpty, !needle.contains(0x0A), needle.count <= buffer.count else {
            return []
        }
        var hits: [Int] = []
        buffer.withUnsafeBytes { haystack in
            needle.withUnsafeBytes { needle in
                var position = 0
                while position + needle.count <= haystack.count {
                    guard
                        let found = memmem(
                            haystack.baseAddress! + position, haystack.count - position,
                            needle.baseAddress!, needle.count)
                    else { break }
                    let offset = UnsafeRawPointer(found) - haystack.baseAddress!
                    let row = rowIndex(containing: offset)
                    hits.append(row)
                    // One hit per row is enough — skip to the next row.
                    position = rowStarts[row + 1]
                }
            }
        }
        return hits
    }

    /// The row whose [start, nextStart) span contains `offset` — the last
    /// rowStart at or below it (empty rows share starts; matches can only
    /// land in non-empty spans).
    private func rowIndex(containing offset: Int) -> Int {
        var low = 0
        var high = rowStarts.count - 2
        var best = 0
        while low <= high {
            let mid = (low + high) / 2
            if rowStarts[mid] <= offset {
                best = mid
                low = mid + 1
            } else {
                high = mid - 1
            }
        }
        return best
    }
}
