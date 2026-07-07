// ⌘F row matching over the scroll surface — the GPUI find bar's semantics
// (case-insensitive substring; headers match on path, line rows on either
// side). Text-range highlighting happens at draw time in the app.

import Foundation

public enum FindMatcher {
    /// Indices into `surface.rows` whose visible text contains `query`,
    /// case-insensitive. Empty query matches nothing.
    public static func matches(
        query: String, surface: Surface, changeset: Changeset
    ) -> [Int] {
        guard !query.isEmpty else { return [] }
        var hits: [Int] = []
        for (index, row) in surface.rows.enumerated() {
            let hit: Bool
            switch row {
            case .fileHeader(let file):
                hit = contains(changeset.files[file].path, query)
            case .gap:
                hit = false
            case .line(let file, let hunk, let rowIndex):
                let diffRow = changeset.files[file].hunks[hunk].rows[rowIndex]
                hit = [diffRow.old, diffRow.new].compactMap { $0 }
                    .contains { contains($0.text, query) }
            }
            if hit {
                hits.append(index)
            }
        }
        return hits
    }

    private static func contains(_ text: String, _ query: String) -> Bool {
        text.range(of: query, options: [.caseInsensitive]) != nil
    }
}
