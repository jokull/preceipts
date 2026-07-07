// Similarity-gated pairing of removed/added lines inside a change block —
// the middle pass of the display algorithm. Direct port of the tested
// Rust original (core/src/pairing.rs). Positional pairing paints nonsense
// intraline highlights whenever block shapes differ; order-preserving
// alignment only pairs lines that plausibly are "the same line edited".

import Foundation

/// Below this content-overlap ratio two lines are a removal plus an
/// addition, not an edit.
private let pairThreshold: Float = 0.3
/// DP alignment is O(n·m); beyond this block size fall back to positional.
private let maxBlock = 64

public enum Pairing: Equatable {
    /// old index + new index (relative to block start) edited in place.
    case pair(Int, Int)
    case removed(Int)
    case added(Int)
}

/// Align a change block's removed lines with its added lines, preserving
/// order. Returns render-ordered pairings.
public func pairBlock(oldLines: [String], newLines: [String]) -> [Pairing] {
    let n = oldLines.count
    let m = newLines.count
    if n == 0 { return (0..<m).map(Pairing.added) }
    if m == 0 { return (0..<n).map(Pairing.removed) }
    if n > maxBlock || m > maxBlock {
        return positional(n: n, m: m)
    }

    var sims = [Float](repeating: 0, count: n * m)
    for i in 0..<n {
        for j in 0..<m {
            sims[i * m + j] = similarity(oldLines[i], newLines[j])
        }
    }

    // Needleman-Wunsch-style alignment maximizing summed similarity;
    // pairs below threshold contribute nothing and are never taken.
    let width = m + 1
    var score = [Float](repeating: 0, count: (n + 1) * (m + 1))
    for i in stride(from: n - 1, through: 0, by: -1) {
        for j in stride(from: m - 1, through: 0, by: -1) {
            var best = max(score[(i + 1) * width + j], score[i * width + j + 1])
            let sim = sims[i * m + j]
            if sim >= pairThreshold {
                best = max(best, score[(i + 1) * width + j + 1] + sim)
            }
            score[i * width + j] = best
        }
    }

    var out: [Pairing] = []
    out.reserveCapacity(max(n, m))
    var i = 0
    var j = 0
    while i < n && j < m {
        let sim = sims[i * m + j]
        let takePair =
            sim >= pairThreshold
            && abs(score[i * width + j] - (score[(i + 1) * width + j + 1] + sim))
                < Float.ulpOfOne
        if takePair {
            out.append(.pair(i, j))
            i += 1
            j += 1
        } else if score[(i + 1) * width + j] >= score[i * width + j + 1] {
            out.append(.removed(i))
            i += 1
        } else {
            out.append(.added(j))
            j += 1
        }
    }
    while i < n {
        out.append(.removed(i))
        i += 1
    }
    while j < m {
        out.append(.added(j))
        j += 1
    }
    return out
}

private func positional(n: Int, m: Int) -> [Pairing] {
    let paired = min(n, m)
    var out: [Pairing] = (0..<paired).map { .pair($0, $0) }
    out.append(contentsOf: (paired..<n).map(Pairing.removed))
    out.append(contentsOf: (paired..<m).map(Pairing.added))
    return out
}

/// Content-overlap ratio in [0, 1]: unchanged bytes over total bytes,
/// via the intraline word differ (order-aware).
func similarity(_ a: String, _ b: String) -> Float {
    if a == b { return 1.0 }
    let aTrim = a.trimmingCharacters(in: .whitespaces)
    let bTrim = b.trimmingCharacters(in: .whitespaces)
    if aTrim.isEmpty || bTrim.isEmpty { return 0.0 }
    let (oldChanged, newChanged) = wordDiff(old: aTrim, new: bTrim)
    let changedOld = oldChanged.reduce(0) { $0 + $1.count }
    let changedNew = newChanged.reduce(0) { $0 + $1.count }
    let total = Float(aTrim.utf8.count + bTrim.utf8.count)
    let common = total - Float(changedOld + changedNew)
    return min(max(common / total, 0.0), 1.0)
}
