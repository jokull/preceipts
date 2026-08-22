//! Compose a diff line's render segments from two overlays: syntax spans
//! (foreground kind) and intraline changed ranges (background emphasis).
//!
//! Byte offsets are UTF-8; both producers emit character-aligned ranges, so
//! slices are always valid.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/Segments.swift` at fc3643e.

use std::collections::BTreeSet;
use std::ops::Range;

/// One highlighted run within a line. `kind` indexes the highlight-name table
/// (tree-sitter); `None` means default text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: Option<u8>,
}

impl HighlightSpan {
    pub fn new(range: Range<usize>, kind: Option<u8>) -> Self {
        Self { range, kind }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub kind: Option<u8>,
    /// Inside an intraline changed range → emphasized background.
    pub emphasized: bool,
}

/// Cut `text` at every syntax-span and changed-range boundary. Tabs expand to
/// four spaces for display *after* slicing, so the offsets stay honest.
pub fn line_segments(
    text: &str,
    syntax: &[HighlightSpan],
    changed: &[Range<usize>],
) -> Vec<Segment> {
    if text.is_empty() {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let len = bytes.len();

    let mut cuts: BTreeSet<usize> = BTreeSet::new();
    cuts.insert(0);
    cuts.insert(len);
    for span in syntax {
        cuts.insert(span.range.start.min(len));
        cuts.insert(span.range.end.min(len));
    }
    for range in changed {
        cuts.insert(range.start.min(len));
        cuts.insert(range.end.min(len));
    }
    let sorted: Vec<usize> = cuts.into_iter().collect();

    let mut segments = Vec::with_capacity(sorted.len());
    for pair in sorted.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start == end {
            continue;
        }
        let kind = syntax
            .iter()
            .find(|s| s.range.start <= start && end <= s.range.end)
            .and_then(|s| s.kind);
        let emphasized = changed.iter().any(|r| r.start <= start && end <= r.end);
        segments.push(Segment {
            text: text[start..end].replace('\t', "    "),
            kind,
            emphasized,
        });
    }
    segments
}

#[cfg(test)]
mod tests {
    // Single-element range slices are the point here: one changed range over a
    // line, not a range of indices.
    #![allow(clippy::single_range_in_vec_init)]

    use super::*;

    #[test]
    fn plain_line_is_one_segment() {
        let segments = line_segments("hello world", &[], &[]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello world");
        assert!(!segments[0].emphasized);
    }

    #[test]
    fn syntax_and_changed_overlays_compose() {
        let segments = line_segments("let x = 1;", &[HighlightSpan::new(0..3, Some(7))], &[4..5]);
        let texts: Vec<&str> = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["let", " ", "x", " = 1;"]);
        assert_eq!(segments[0].kind, Some(7));
        assert!(segments[2].emphasized);
        assert!(!segments[3].emphasized);
    }

    #[test]
    fn overlapping_boundaries_never_gap() {
        let segments = line_segments(
            "abcdef",
            &[
                HighlightSpan::new(0..4, Some(1)),
                HighlightSpan::new(4..6, Some(2)),
            ],
            &[2..5],
        );
        let joined: String = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "abcdef");
    }

    #[test]
    fn tabs_expand_for_display() {
        let segments = line_segments("\tindent", &[], &[]);
        assert_eq!(segments[0].text, "    indent");
    }

    #[test]
    fn multibyte_boundaries_slice_cleanly() {
        let text = "name Jökull here";
        let segments = line_segments(text, &[], &[5..12]);
        let joined: String = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, text);
        assert!(segments.iter().any(|s| s.text == "Jökull" && s.emphasized));
    }
}
