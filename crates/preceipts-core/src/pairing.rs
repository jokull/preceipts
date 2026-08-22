//! Similarity-gated pairing of removed/added lines inside a change block —
//! the middle pass of the display algorithm.
//!
//! Positional pairing paints nonsense intraline highlights whenever the two
//! sides of a block have different shapes; order-preserving alignment only
//! pairs lines that plausibly are "the same line, edited".
//!
//! Ported forward from `swift/Sources/PreceiptsKit/Pairing.swift` at fc3643e.

use crate::intraline::word_diff;

/// Below this content-overlap ratio, two lines are a removal plus an
/// addition — not an edit.
const PAIR_THRESHOLD: f32 = 0.3;

/// DP alignment is O(n·m); beyond this block size fall back to positional.
const MAX_BLOCK: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pairing {
    /// Old index and new index (relative to block start) edited in place.
    Pair(usize, usize),
    Removed(usize),
    Added(usize),
}

/// Align a change block's removed lines with its added lines, preserving
/// order. Returns render-ordered pairings.
pub fn pair_block(old_lines: &[String], new_lines: &[String]) -> Vec<Pairing> {
    let n = old_lines.len();
    let m = new_lines.len();
    if n == 0 {
        return (0..m).map(Pairing::Added).collect();
    }
    if m == 0 {
        return (0..n).map(Pairing::Removed).collect();
    }
    if n > MAX_BLOCK || m > MAX_BLOCK {
        return positional(n, m);
    }

    let mut sims = vec![0f32; n * m];
    for i in 0..n {
        for j in 0..m {
            sims[i * m + j] = similarity(&old_lines[i], &new_lines[j]);
        }
    }

    // Needleman–Wunsch-style alignment maximizing summed similarity. Pairs
    // below the threshold contribute nothing and are never taken.
    let width = m + 1;
    let mut score = vec![0f32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let mut best = score[(i + 1) * width + j].max(score[i * width + j + 1]);
            let sim = sims[i * m + j];
            if sim >= PAIR_THRESHOLD {
                best = best.max(score[(i + 1) * width + j + 1] + sim);
            }
            score[i * width + j] = best;
        }
    }

    let mut out = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        let sim = sims[i * m + j];
        let take_pair = sim >= PAIR_THRESHOLD
            && (score[i * width + j] - (score[(i + 1) * width + j + 1] + sim)).abs() < f32::EPSILON;
        if take_pair {
            out.push(Pairing::Pair(i, j));
            i += 1;
            j += 1;
        } else if score[(i + 1) * width + j] >= score[i * width + j + 1] {
            out.push(Pairing::Removed(i));
            i += 1;
        } else {
            out.push(Pairing::Added(j));
            j += 1;
        }
    }
    while i < n {
        out.push(Pairing::Removed(i));
        i += 1;
    }
    while j < m {
        out.push(Pairing::Added(j));
        j += 1;
    }
    out
}

fn positional(n: usize, m: usize) -> Vec<Pairing> {
    let paired = n.min(m);
    let mut out: Vec<Pairing> = (0..paired).map(|i| Pairing::Pair(i, i)).collect();
    out.extend((paired..n).map(Pairing::Removed));
    out.extend((paired..m).map(Pairing::Added));
    out
}

/// Content-overlap ratio in [0, 1]: unchanged bytes over total bytes, via the
/// intraline word differ (order-aware).
pub fn similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let a_trim = a.trim_matches(|c: char| c == ' ' || c == '\t');
    let b_trim = b.trim_matches(|c: char| c == ' ' || c == '\t');
    if a_trim.is_empty() || b_trim.is_empty() {
        return 0.0;
    }
    let (old_changed, new_changed) = word_diff(a_trim, b_trim);
    let changed: usize = old_changed.iter().map(|r| r.len()).sum::<usize>()
        + new_changed.iter().map(|r| r.len()).sum::<usize>();
    let total = (a_trim.len() + b_trim.len()) as f32;
    let common = total - changed as f32;
    (common / total).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn kinds(pairings: &[Pairing]) -> String {
        pairings
            .iter()
            .map(|p| match p {
                Pairing::Pair(..) => 'P',
                Pairing::Removed(_) => 'R',
                Pairing::Added(_) => 'A',
            })
            .collect()
    }

    #[test]
    fn edit_in_place_pairs() {
        let p = pair_block(
            &lines(&["let x = compute(a);"]),
            &lines(&["let x = compute(a, b);"]),
        );
        assert_eq!(kinds(&p), "P");
    }

    #[test]
    fn unrelated_replacement_does_not_pair() {
        let p = pair_block(
            &lines(&["return legacy_path();"]),
            &lines(&["#[cfg(feature = \"v2\")]"]),
        );
        assert_eq!(kinds(&p), "RA");
    }

    #[test]
    fn skewed_block_pairs_the_matching_line() {
        let p = pair_block(
            &lines(&["fn handle(req: Request) {"]),
            &lines(&["/// Handles one request.", "fn handle(req: &Request) {"]),
        );
        assert_eq!(kinds(&p), "AP");
        assert_eq!(p[1], Pairing::Pair(0, 1));
    }

    #[test]
    fn oversized_blocks_fall_back_positionally() {
        let old: Vec<String> = (0..80).map(|i| format!("old {i}")).collect();
        let new: Vec<String> = (0..80).map(|i| format!("new {i}")).collect();
        let p = pair_block(&old, &new);
        assert_eq!(p.len(), 80);
        assert_eq!(p[0], Pairing::Pair(0, 0));
    }
}
