//! Intraline (word-level) diff for paired change rows — the fine pass of the
//! display algorithm (line diff → similarity pairing → word LCS).
//!
//! Byte offsets are UTF-8; tokens never split a multi-byte character, because
//! every byte >= 0x80 classifies as `Word` and so joins its neighbours.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/Intraline.swift` at fc3643e.

use std::ops::Range;

/// Beyond this, the O(n·m) LCS table stops being worth it and the whole line
/// is reported as changed.
const MAX_TOKENS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteClass {
    Word,
    Space,
    Punct,
}

fn classify(byte: u8) -> ByteClass {
    if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80 {
        ByteClass::Word
    } else if byte == b' ' || byte == b'\t' {
        ByteClass::Space
    } else {
        ByteClass::Punct
    }
}

/// Words (identifier runs), whitespace runs, and single punctuation
/// characters, as byte ranges.
fn tokenize(bytes: &[u8]) -> Vec<Range<usize>> {
    let mut tokens = Vec::new();
    let mut idx = 0;
    while idx < bytes.len() {
        let current = classify(bytes[idx]);
        let start = idx;
        idx += 1;
        if current == ByteClass::Punct {
            tokens.push(start..idx);
            continue;
        }
        while idx < bytes.len() && classify(bytes[idx]) == current {
            idx += 1;
        }
        tokens.push(start..idx);
    }
    tokens
}

/// Append `range`, merging into the previous range when they touch, so a run
/// of changed tokens paints as one emphasis span.
fn push_merged(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    }
}

/// Byte ranges (into each line's UTF-8) that differ between the two lines.
/// Adjacent ranges merge; identical lines yield none; pathological lines fall
/// back to whole-line ranges.
pub fn word_diff(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();
    let old_tokens = tokenize(old_bytes);
    let new_tokens = tokenize(new_bytes);
    if old_tokens.len() > MAX_TOKENS || new_tokens.len() > MAX_TOKENS {
        // One range covering the whole line, deliberately — the fallback says
        // "all of this changed", not "a range of lines changed".
        #[allow(clippy::single_range_in_vec_init)]
        return (vec![0..old_bytes.len()], vec![0..new_bytes.len()]);
    }

    let n = old_tokens.len();
    let m = new_tokens.len();
    let same = |i: usize, j: usize| -> bool {
        old_bytes[old_tokens[i].clone()] == new_bytes[new_tokens[j].clone()]
    };

    // LCS table over token texts, filled backwards so the walk below can read
    // it forwards.
    let width = m + 1;
    let mut lcs = vec![0u16; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if same(i, j) {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }

    let mut old_changed = Vec::new();
    let mut new_changed = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if same(i, j) {
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            push_merged(&mut old_changed, old_tokens[i].clone());
            i += 1;
        } else {
            push_merged(&mut new_changed, new_tokens[j].clone());
            j += 1;
        }
    }
    while i < n {
        push_merged(&mut old_changed, old_tokens[i].clone());
        i += 1;
    }
    while j < m {
        push_merged(&mut new_changed, new_tokens[j].clone());
        j += 1;
    }

    (old_changed, new_changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slices<'a>(line: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|r| &line[r.clone()]).collect()
    }

    #[test]
    fn single_word_change() {
        let old = "let two = compute(input);";
        let new = "let TWO = compute(input);";
        let (o, n) = word_diff(old, new);
        assert_eq!(slices(old, &o), ["two"]);
        assert_eq!(slices(new, &n), ["TWO"]);
    }

    #[test]
    fn insertion_only_marks_new_side() {
        let (o, n) = word_diff("foo(a, b)", "foo(a, b, c)");
        assert!(o.is_empty());
        assert_eq!(slices("foo(a, b, c)", &n), [", c"]);
    }

    #[test]
    fn adjacent_changed_tokens_merge() {
        let old = "return value;";
        let new = "return other_thing();";
        let (o, n) = word_diff(old, new);
        assert_eq!(slices(old, &o), ["value"]);
        assert_eq!(slices(new, &n), ["other_thing()"]);
    }

    #[test]
    fn identical_lines_yield_nothing() {
        let (o, n) = word_diff("same", "same");
        assert!(o.is_empty() && n.is_empty());
    }

    #[test]
    fn unicode_identifiers_stay_intact() {
        let old = "name = \"Jökull\"";
        let new = "name = \"Sólberg\"";
        let (o, n) = word_diff(old, new);
        assert_eq!(slices(old, &o), ["Jökull"]);
        assert_eq!(slices(new, &n), ["Sólberg"]);
    }
}
