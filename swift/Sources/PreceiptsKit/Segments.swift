// Compose a diff line's render segments from two overlays: syntax spans
// (foreground kind) and intraline changed ranges (background emphasis).
// Port of the tested Rust original (app/src/segments.rs). Byte offsets
// are UTF-8; both producers emit character-aligned ranges so slices are
// always valid.

import Foundation

/// One highlighted run within a line. `kind` indexes the highlight-name
/// table (tree-sitter); nil = default text.
public struct HighlightSpan: Sendable {
    public let range: Range<Int>
    public let kind: UInt8?

    public init(range: Range<Int>, kind: UInt8?) {
        self.range = range
        self.kind = kind
    }
}

public struct Segment: Equatable, Sendable {
    public let text: String
    public let kind: UInt8?
    /// Inside an intraline changed range → emphasized background.
    public let emphasized: Bool

    public init(text: String, kind: UInt8?, emphasized: Bool) {
        self.text = text
        self.kind = kind
        self.emphasized = emphasized
    }
}

/// Cut `text` at every syntax-span and changed-range boundary. Tabs are
/// expanded to 4 spaces (display only) after slicing so offsets stay
/// honest.
public func lineSegments(
    _ text: String,
    syntax: [HighlightSpan],
    changed: [Range<Int>]
) -> [Segment] {
    if text.isEmpty { return [] }
    let bytes = Array(text.utf8)
    let len = bytes.count

    var cuts: Set<Int> = [0, len]
    for span in syntax {
        cuts.insert(min(span.range.lowerBound, len))
        cuts.insert(min(span.range.upperBound, len))
    }
    for range in changed {
        cuts.insert(min(range.lowerBound, len))
        cuts.insert(min(range.upperBound, len))
    }
    let sorted = cuts.sorted()

    var segments: [Segment] = []
    segments.reserveCapacity(sorted.count)
    for pair in zip(sorted, sorted.dropFirst()) {
        let (start, end) = pair
        if start == end { continue }
        let kind = syntax.first {
            $0.range.lowerBound <= start && end <= $0.range.upperBound
        }?.kind
        let emphasized = changed.contains {
            $0.lowerBound <= start && end <= $0.upperBound
        }
        let slice = String(decoding: bytes[start..<end], as: UTF8.self)
        segments.append(
            Segment(
                text: slice.replacingOccurrences(of: "\t", with: "    "),
                kind: kind,
                emphasized: emphasized
            )
        )
    }
    return segments
}
