// Intraline (word-level) diff for paired change rows — the fine pass of
// the display algorithm (line diff → similarity pairing → word LCS).
// Direct port of the tested Rust original (core/src/intraline.rs).
// Byte offsets are UTF-8; tokens never split multi-byte characters.

import Foundation

private let maxTokens = 300

enum ByteClass {
    case word
    case space
    case punct
}

private func classify(_ byte: UInt8) -> ByteClass {
    if byte.isASCIIAlphanumeric || byte == UInt8(ascii: "_") || byte >= 0x80 {
        return .word
    }
    if byte == UInt8(ascii: " ") || byte == UInt8(ascii: "\t") {
        return .space
    }
    return .punct
}

extension UInt8 {
    fileprivate var isASCIIAlphanumeric: Bool {
        (UInt8(ascii: "a")...UInt8(ascii: "z")).contains(self)
            || (UInt8(ascii: "A")...UInt8(ascii: "Z")).contains(self)
            || (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(self)
    }
}

/// Words (identifier runs), whitespace runs, and single punctuation chars,
/// as byte ranges.
func tokenize(_ bytes: [UInt8]) -> [Range<Int>] {
    var tokens: [Range<Int>] = []
    var idx = 0
    while idx < bytes.count {
        let current = classify(bytes[idx])
        let start = idx
        idx += 1
        if current == .punct {
            tokens.append(start..<idx)
            continue
        }
        while idx < bytes.count, classify(bytes[idx]) == current {
            idx += 1
        }
        tokens.append(start..<idx)
    }
    return tokens
}

/// Byte ranges (into each line's UTF-8) that differ between the two lines.
/// Adjacent ranges merge; identical lines yield none; pathological lines
/// fall back to whole-line ranges.
public func wordDiff(old: String, new: String) -> ([Range<Int>], [Range<Int>]) {
    if old == new {
        return ([], [])
    }
    let oldBytes = Array(old.utf8)
    let newBytes = Array(new.utf8)
    let oldTokens = tokenize(oldBytes)
    let newTokens = tokenize(newBytes)
    if oldTokens.count > maxTokens || newTokens.count > maxTokens {
        return ([0..<oldBytes.count], [0..<newBytes.count])
    }

    let n = oldTokens.count
    let m = newTokens.count
    func same(_ i: Int, _ j: Int) -> Bool {
        let a = oldTokens[i]
        let b = newTokens[j]
        guard a.count == b.count else { return false }
        return oldBytes[a].elementsEqual(newBytes[b])
    }

    // LCS table over token texts.
    var lcs = [UInt16](repeating: 0, count: (n + 1) * (m + 1))
    let width = m + 1
    if n > 0 && m > 0 {
        for i in stride(from: n - 1, through: 0, by: -1) {
            for j in stride(from: m - 1, through: 0, by: -1) {
                lcs[i * width + j] =
                    same(i, j)
                    ? lcs[(i + 1) * width + j + 1] + 1
                    : max(lcs[(i + 1) * width + j], lcs[i * width + j + 1])
            }
        }
    }

    var oldChanged: [Range<Int>] = []
    var newChanged: [Range<Int>] = []
    func pushMerged(_ ranges: inout [Range<Int>], _ range: Range<Int>) {
        if let last = ranges.last, last.upperBound == range.lowerBound {
            ranges[ranges.count - 1] = last.lowerBound..<range.upperBound
        } else {
            ranges.append(range)
        }
    }

    var i = 0
    var j = 0
    while i < n && j < m {
        if same(i, j) {
            i += 1
            j += 1
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            pushMerged(&oldChanged, oldTokens[i])
            i += 1
        } else {
            pushMerged(&newChanged, newTokens[j])
            j += 1
        }
    }
    while i < n {
        pushMerged(&oldChanged, oldTokens[i])
        i += 1
    }
    while j < m {
        pushMerged(&newChanged, newTokens[j])
        j += 1
    }

    return (oldChanged, newChanged)
}
