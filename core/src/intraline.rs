//! Intraline (word-level) diff for paired Change rows — the second pass of
//! the classic coarse-lines-then-fine-words scheme. Lines are short, so a
//! plain LCS DP is fast, exact, and dependency-free; pathological lines
//! fall back to whole-line ranges.

use std::ops::Range;

const MAX_TOKENS: usize = 300;

/// Byte ranges (into each line's text) that differ between the two lines.
/// Adjacent/overlapping ranges are merged; identical lines yield none.
pub fn word_diff(old: &str, new: &str) -> (Vec<Range<u32>>, Vec<Range<u32>>) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    if old_tokens.len() > MAX_TOKENS || new_tokens.len() > MAX_TOKENS {
        // Too long to be worth a DP — highlight both lines wholly.
        return (
            vec![0..old.len() as u32],
            vec![0..new.len() as u32],
        );
    }

    // LCS table over token texts.
    let n = old_tokens.len();
    let m = new_tokens.len();
    let mut lcs = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[at(i, j)] = if token_text(old, &old_tokens[i]) == token_text(new, &new_tokens[j]) {
                lcs[at(i + 1, j + 1)] + 1
            } else {
                lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
            };
        }
    }

    // Walk the table: tokens not on a common path are changed.
    let mut old_changed = Vec::new();
    let mut new_changed = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if token_text(old, &old_tokens[i]) == token_text(new, &new_tokens[j]) {
            i += 1;
            j += 1;
        } else if lcs[at(i + 1, j)] >= lcs[at(i, j + 1)] {
            push_merged(&mut old_changed, old_tokens[i].clone());
            i += 1;
        } else {
            push_merged(&mut new_changed, new_tokens[j].clone());
            j += 1;
        }
    }
    for token in &old_tokens[i..] {
        push_merged(&mut old_changed, token.clone());
    }
    for token in &new_tokens[j..] {
        push_merged(&mut new_changed, token.clone());
    }

    (old_changed, new_changed)
}

fn token_text<'a>(line: &'a str, range: &Range<u32>) -> &'a str {
    &line[range.start as usize..range.end as usize]
}

fn push_merged(ranges: &mut Vec<Range<u32>>, range: Range<u32>) {
    if let Some(last) = ranges.last_mut() {
        if last.end == range.start {
            last.end = range.end;
            return;
        }
    }
    ranges.push(range);
}

/// Words (identifier runs), whitespace runs, and single punctuation chars.
fn tokenize(line: &str) -> Vec<Range<u32>> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Punct,
    }
    let class = |byte: u8| {
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80 {
            Class::Word
        } else if byte == b' ' || byte == b'\t' {
            Class::Space
        } else {
            Class::Punct
        }
    };
    while idx < bytes.len() {
        let current = class(bytes[idx]);
        start = idx;
        idx += 1;
        if current == Class::Punct {
            tokens.push(start as u32..idx as u32);
            continue;
        }
        while idx < bytes.len() && class(bytes[idx]) == current {
            idx += 1;
        }
        tokens.push(start as u32..idx as u32);
    }
    let _ = start;
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slices<'a>(line: &'a str, ranges: &[Range<u32>]) -> Vec<&'a str> {
        ranges
            .iter()
            .map(|r| &line[r.start as usize..r.end as usize])
            .collect()
    }

    #[test]
    fn single_word_change() {
        let old = "let two = compute(input);";
        let new = "let TWO = compute(input);";
        let (o, n) = word_diff(old, new);
        assert_eq!(slices(old, &o), vec!["two"]);
        assert_eq!(slices(new, &n), vec!["TWO"]);
    }

    #[test]
    fn insertion_only_marks_new_side() {
        let old = "foo(a, b)";
        let new = "foo(a, b, c)";
        let (o, n) = word_diff(old, new);
        assert!(o.is_empty());
        assert_eq!(slices(new, &n), vec![", c"]);
    }

    #[test]
    fn adjacent_changed_tokens_merge() {
        let old = "return value;";
        let new = "return other_thing();";
        let (o, n) = word_diff(old, new);
        assert_eq!(slices(old, &o), vec!["value"]);
        assert_eq!(slices(new, &n), vec!["other_thing()"]);
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
        assert_eq!(slices(old, &o), vec!["Jökull"]);
        assert_eq!(slices(new, &n), vec!["Sólberg"]);
    }
}
