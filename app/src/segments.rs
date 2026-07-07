//! Compose a diff line's render segments from two overlays:
//! syntax spans (foreground) and intraline changed ranges (background
//! emphasis). Pure and unit-tested; the renderer maps segments to styled
//! elements 1:1. Byte offsets throughout — segments split only at valid
//! UTF-8 boundaries because both producers emit char-aligned ranges.

use std::ops::Range;

use preceipts_core::HighlightSpan;

#[derive(Debug, PartialEq)]
pub struct Segment {
    pub text: String,
    /// Index into HIGHLIGHT_NAMES (None = default fg).
    pub kind: Option<u8>,
    /// Inside an intraline changed range → emphasized background.
    pub emphasized: bool,
}

/// Cut `text` at every syntax-span and changed-range boundary. Tabs are
/// expanded to 4 spaces (display only) after slicing so offsets stay honest.
pub fn line_segments(
    text: &str,
    syntax: &[HighlightSpan],
    changed: &[Range<u32>],
) -> Vec<Segment> {
    if text.is_empty() {
        return Vec::new();
    }
    let len = text.len() as u32;
    let mut cuts: Vec<u32> = vec![0, len];
    for span in syntax {
        cuts.push(span.range.start.min(len));
        cuts.push(span.range.end.min(len));
    }
    for range in changed {
        cuts.push(range.start.min(len));
        cuts.push(range.end.min(len));
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut segments = Vec::with_capacity(cuts.len());
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start == end {
            continue;
        }
        let kind = syntax
            .iter()
            .find(|s| s.range.start <= start && end <= s.range.end)
            .and_then(|s| s.kind);
        let emphasized = changed
            .iter()
            .any(|r| r.start <= start && end <= r.end);
        segments.push(Segment {
            text: text[start as usize..end as usize].replace('\t', "    "),
            kind,
            emphasized,
        });
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(range: Range<u32>, kind: u8) -> HighlightSpan {
        HighlightSpan {
            range,
            kind: Some(kind),
        }
    }

    #[test]
    fn plain_line_is_one_segment() {
        let segments = line_segments("hello world", &[], &[]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello world");
        assert!(!segments[0].emphasized);
    }

    #[test]
    fn syntax_and_changed_overlays_compose() {
        // "let x = 1;" — syntax: [0..3 keyword], changed: [4..5]
        let segments = line_segments("let x = 1;", &[span(0..3, 7)], &[4..5]);
        let texts: Vec<&str> = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["let", " ", "x", " = 1;"]);
        assert_eq!(segments[0].kind, Some(7));
        assert!(segments[2].emphasized);
        assert!(!segments[3].emphasized);
    }

    #[test]
    fn overlapping_boundaries_never_panic_or_gap() {
        let segments = line_segments("abcdef", &[span(0..4, 1), span(4..6, 2)], &[2..5]);
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
        // "Jökull" — ö is 2 bytes; changed range covers the whole word.
        let text = "name Jökull here";
        let segments = line_segments(text, &[], &[5..12]);
        let joined: String = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, text);
        assert!(segments.iter().any(|s| s.text == "Jökull" && s.emphasized));
    }
}
