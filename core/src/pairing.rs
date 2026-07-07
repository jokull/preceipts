//! Similarity-gated pairing of removed/added lines inside a change block —
//! the third piece of the chosen display algorithm (histogram lines →
//! similarity pairing → word-level intraline). Positional pairing paints
//! nonsense intraline highlights whenever block shapes differ; pairing by
//! content similarity (order-preserving alignment, GitHub's approach) only
//! marks lines as "changed into each other" when they plausibly were.
//!
//! difftastic-style AST diffing was evaluated and rejected as the default:
//! structural graph search degrades catastrophically on large changes, and
//! this surface recomputes on every watch event across monorepo-scale
//! diffs. See docs/desktop-foundations.md.

use std::ops::Range;

use crate::intraline;

/// Below this token-overlap ratio two lines are not "the same line edited",
/// they are a removal plus an addition.
const PAIR_THRESHOLD: f32 = 0.3;
/// DP alignment is O(n·m); beyond this block size fall back to positional.
const MAX_BLOCK: usize = 64;

/// How one aligned row of a change block should render.
pub enum Pairing {
    /// old index + new index (both relative to block start) edited in place.
    Pair(usize, usize),
    Removed(usize),
    Added(usize),
}

/// Align a change block's removed lines with its added lines, preserving
/// order. Returns render-ordered pairings.
pub fn pair_block(old_lines: &[&str], new_lines: &[&str]) -> Vec<Pairing> {
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

    // Needleman-Wunsch-style alignment maximizing summed similarity;
    // pairs below threshold contribute nothing and are never taken.
    let mut score = vec![0f32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let mut best = score[at(i + 1, j)].max(score[at(i, j + 1)]);
            let sim = similarity(old_lines[i], new_lines[j]);
            if sim >= PAIR_THRESHOLD {
                best = best.max(score[at(i + 1, j + 1)] + sim);
            }
            score[at(i, j)] = best;
        }
    }

    let mut out = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        let sim = similarity(old_lines[i], new_lines[j]);
        let take_pair = sim >= PAIR_THRESHOLD
            && (score[at(i, j)] - (score[at(i + 1, j + 1)] + sim)).abs() < f32::EPSILON;
        if take_pair {
            out.push(Pairing::Pair(i, j));
            i += 1;
            j += 1;
        } else if score[at(i + 1, j)] >= score[at(i, j + 1)] {
            out.push(Pairing::Removed(i));
            i += 1;
        } else {
            out.push(Pairing::Added(j));
            j += 1;
        }
    }
    out.extend((i..n).map(Pairing::Removed));
    out.extend((j..m).map(Pairing::Added));
    out
}

fn positional(n: usize, m: usize) -> Vec<Pairing> {
    let paired = n.min(m);
    let mut out: Vec<Pairing> = (0..paired).map(|k| Pairing::Pair(k, k)).collect();
    out.extend((paired..n).map(Pairing::Removed));
    out.extend((paired..m).map(Pairing::Added));
    out
}

/// Token-overlap ratio in [0, 1]: 2·|common| / (|a| + |b|), order-aware via
/// LCS so `foo(bar)` vs `bar(foo)` scores lower than a rename.
fn similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let a_trim = a.trim();
    let b_trim = b.trim();
    if a_trim.is_empty() || b_trim.is_empty() {
        return 0.0;
    }
    // Reuse the intraline word differ: unchanged bytes ≈ common content.
    let (old_changed, new_changed) = intraline::word_diff(a_trim, b_trim);
    let changed_old: u32 = old_changed.iter().map(Range::len).map(|l| l as u32).sum();
    let changed_new: u32 = new_changed.iter().map(Range::len).map(|l| l as u32).sum();
    let total = (a_trim.len() + b_trim.len()) as f32;
    let common = total - (changed_old + changed_new) as f32;
    (common / total).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(pairings: &[Pairing]) -> String {
        pairings
            .iter()
            .map(|p| match p {
                Pairing::Pair(_, _) => 'P',
                Pairing::Removed(_) => 'R',
                Pairing::Added(_) => 'A',
            })
            .collect()
    }

    #[test]
    fn edit_in_place_pairs() {
        let p = pair_block(&["let x = compute(a);"], &["let x = compute(a, b);"]);
        assert_eq!(kinds(&p), "P");
    }

    #[test]
    fn unrelated_replacement_does_not_pair() {
        let p = pair_block(&["return legacy_path();"], &["#[cfg(feature = \"v2\")]"]);
        assert_eq!(kinds(&p), "RA");
    }

    #[test]
    fn skewed_block_pairs_the_matching_line() {
        // Old line matches the SECOND new line; positional pairing would
        // pair it with the first and produce garbage intraline highlights.
        let p = pair_block(
            &["fn handle(req: Request) {"],
            &["/// Handles one request.", "fn handle(req: &Request) {"],
        );
        assert_eq!(kinds(&p), "AP");
        match p[1] {
            Pairing::Pair(0, 1) => {}
            _ => panic!("expected old 0 paired with new 1"),
        }
    }

    #[test]
    fn oversized_blocks_fall_back_positionally() {
        let old: Vec<String> = (0..80).map(|i| format!("old {i}")).collect();
        let new: Vec<String> = (0..80).map(|i| format!("new {i}")).collect();
        let old_refs: Vec<&str> = old.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new.iter().map(String::as_str).collect();
        let p = pair_block(&old_refs, &new_refs);
        assert_eq!(p.len(), 80);
        assert!(matches!(p[0], Pairing::Pair(0, 0)));
    }
}
