//! Tree-sitter syntax highlighting: whole-file parse, per-line spans.
//!
//! This is the canonical copy of the language registry (config/queries);
//! the TUI keeps a private copy until it is retired. UI-agnostic: spans
//! carry an index into [`HIGHLIGHT_NAMES`], the renderer maps names to
//! colors. Compute per file, off the UI thread, cache by file.

pub mod config;
mod queries;

use std::ops::Range;

use tree_sitter_highlight::{HighlightEvent, Highlighter};

pub use config::HIGHLIGHT_NAMES;

/// One highlighted run within a single line. `kind` indexes
/// [`HIGHLIGHT_NAMES`]; `None` means default text.
#[derive(Clone, Debug)]
pub struct HighlightSpan {
    /// Byte range into the line's text.
    pub range: Range<u32>,
    pub kind: Option<u8>,
}

/// Per-line highlight spans for one version of one file.
pub struct FileHighlight {
    lines: Vec<Vec<HighlightSpan>>,
}

impl FileHighlight {
    /// Highlight a whole file. Returns `None` for unsupported extensions —
    /// callers render plain text. Multi-line constructs (block comments,
    /// template strings) highlight correctly because the parse is global.
    pub fn new(content: &str, path: &str) -> Option<FileHighlight> {
        let ext = path.rsplit('.').next().unwrap_or_default();
        let config = config::CONFIGS.iter().find(|(e, _)| *e == ext)?;

        let mut highlighter = Highlighter::new();
        let events = highlighter
            .highlight(&config.1.config, content.as_bytes(), None, |_| None)
            .ok()?;

        // Split the event stream at newlines into per-line spans.
        let mut lines: Vec<Vec<HighlightSpan>> = vec![Vec::new()];
        // Line start byte offset of the current line.
        let mut line_start: usize = 0;
        let mut stack: Vec<u8> = Vec::new();
        for event in events {
            match event.ok()? {
                HighlightEvent::HighlightStart(h) => stack.push(h.0 as u8),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let kind = stack.last().copied();
                    let mut seg_start = start;
                    for (offset, byte) in content.as_bytes()[start..end].iter().enumerate() {
                        if *byte == b'\n' {
                            let abs = start + offset;
                            push_span(&mut lines, line_start, seg_start, abs, kind);
                            lines.push(Vec::new());
                            line_start = abs + 1;
                            seg_start = abs + 1;
                        }
                    }
                    push_span(&mut lines, line_start, seg_start, end, kind);
                }
            }
        }
        Some(FileHighlight { lines })
    }

    /// Spans for a 1-based line number; empty for out-of-range or plain.
    pub fn line(&self, number: u32) -> &[HighlightSpan] {
        self.lines
            .get(number.saturating_sub(1) as usize)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

fn push_span(
    lines: &mut [Vec<HighlightSpan>],
    line_start: usize,
    seg_start: usize,
    seg_end: usize,
    kind: Option<u8>,
) {
    if seg_end <= seg_start {
        return;
    }
    let line = lines.last_mut().expect("lines never empty");
    line.push(HighlightSpan {
        range: (seg_start - line_start) as u32..(seg_end - line_start) as u32,
        kind,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_name(span: &HighlightSpan) -> &'static str {
        span.kind
            .and_then(|k| HIGHLIGHT_NAMES.get(k as usize).copied())
            .unwrap_or("")
    }

    #[test]
    fn rust_keywords_and_strings() {
        let code = "fn main() {\n    let msg = \"hi\";\n}\n";
        let hl = FileHighlight::new(code, "src/main.rs").expect("rust supported");

        let line1: Vec<&str> = hl.line(1).iter().map(kind_name).collect();
        assert!(line1.contains(&"keyword"), "line1 kinds: {line1:?}");

        let line2 = hl.line(2);
        let string_span = line2
            .iter()
            .find(|s| kind_name(s) == "string")
            .expect("string literal highlighted");
        let text = &code.lines().nth(1).unwrap()
            [string_span.range.start as usize..string_span.range.end as usize];
        assert!(text.contains("hi"));
    }

    #[test]
    fn multiline_comment_spans_all_lines() {
        let code = "/* one\ntwo */\nlet x = 1;\n";
        let hl = FileHighlight::new(code, "a.ts").expect("ts supported");
        assert!(hl.line(1).iter().any(|s| kind_name(s) == "comment"));
        assert!(hl.line(2).iter().any(|s| kind_name(s) == "comment"));
        assert!(!hl.line(3).iter().any(|s| kind_name(s) == "comment"));
    }

    #[test]
    fn unsupported_extension_is_none() {
        assert!(FileHighlight::new("hello", "readme.xyz").is_none());
    }

    #[test]
    fn span_ranges_are_line_relative_and_ordered() {
        let code = "const a = 1;\nconst b = \"two\";\n";
        let hl = FileHighlight::new(code, "x.ts").unwrap();
        for number in 1..=2u32 {
            let line_len = code.lines().nth(number as usize - 1).unwrap().len() as u32;
            let mut cursor = 0;
            for span in hl.line(number) {
                assert!(span.range.start >= cursor, "overlapping spans");
                assert!(span.range.end <= line_len, "span past line end");
                cursor = span.range.end;
            }
        }
    }
}
