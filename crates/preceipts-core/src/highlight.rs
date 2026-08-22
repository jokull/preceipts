//! Tree-sitter syntax highlighting: whole-file parse, per-line spans.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/Highlight.swift` at
//! fc3643e, with one large simplification. The Swift version drove the
//! tree-sitter C API directly and hand-rolled what `tree-sitter-highlight`
//! does — capture precedence and `#eq?`/`#match?`/`#any-of?` predicate
//! evaluation — because the Swift binding layer worked in UTF-16 and the
//! crate was not available. Its own header called those differences "none
//! observable with our queries", and the capture-precedence test was verified
//! against the crate. Here we use the crate, so that behaviour is inherited
//! rather than reimplemented.
//!
//! The query registry is *not* inherited: these are the same `.scm` sources
//! the Swift app shipped, extracted verbatim from `Queries.swift`. Grammar
//! crates bundle their own `highlights.scm`, but those emit different capture
//! names and different precedence outcomes, which would change what the app
//! renders and quietly invalidate the tests that pin it.

use crate::segments::HighlightSpan;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

/// Capture-kind table; `HighlightSpan::kind` indexes into this. Renderers map
/// names to colors by dot-separated family, so an unknown leaf still themes
/// off its family root.
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "function",
    "function.builtin",
    "function.method",
    "function.macro",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "variable.member",
];

/// Files past this size are rendered plain. A whole-file parse of a vendored
/// bundle costs more than the highlighting is worth.
pub const MAX_BYTES: usize = 512 << 10;

/// Per-line highlight spans for one version of one file. Ranges are UTF-8
/// byte offsets relative to the line, newline excluded.
#[derive(Debug, Clone, Default)]
pub struct FileHighlight {
    lines: Vec<Vec<HighlightSpan>>,
}

impl FileHighlight {
    /// Spans for a 1-based line number; empty when out of range.
    pub fn line(&self, number: usize) -> &[HighlightSpan] {
        if number == 0 {
            return &[];
        }
        self.lines.get(number - 1).map_or(&[], Vec::as_slice)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Keep spans only for the given 1-based line numbers.
    ///
    /// The surface renders hunk lines only, and whole-file span tables
    /// dominate a changeset's retained size when large files carry small
    /// diffs — this is memory discipline, not an optimization.
    pub fn pruned(mut self, keep: &std::collections::HashSet<usize>) -> FileHighlight {
        for (index, spans) in self.lines.iter_mut().enumerate() {
            if !keep.contains(&(index + 1)) {
                spans.clear();
                spans.shrink_to_fit();
            }
        }
        self
    }
}

/// Configurations, plus the extension index into them. Several extensions
/// share one language (`js`/`jsx`, `sh`/`bash`), and a `HighlightConfiguration`
/// is neither cloneable nor cheap, so extensions point at a slot rather than
/// owning a copy.
struct Registry {
    configs: Vec<HighlightConfiguration>,
    by_extension: HashMap<&'static str, usize>,
}

impl Registry {
    fn get(&self, ext: &str) -> Option<&HighlightConfiguration> {
        self.by_extension
            .get(ext)
            .map(|&index| &self.configs[index])
    }

    fn add(
        &mut self,
        exts: &[&'static str],
        name: &str,
        language: tree_sitter::Language,
        query: &str,
    ) {
        let mut config = match HighlightConfiguration::new(language, name, query, "", "") {
            Ok(config) => config,
            Err(error) => {
                // A broken query is a build-time mistake, but it must never
                // take the app down — the file just renders plain.
                debug_assert!(false, "{name} highlight query failed: {error}");
                return;
            }
        };
        config.configure(HIGHLIGHT_NAMES);
        let index = self.configs.len();
        self.configs.push(config);
        for ext in exts {
            self.by_extension.insert(ext, index);
        }
    }
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(build_registry)
}

fn build_registry() -> Registry {
    let mut r = Registry {
        configs: Vec::new(),
        by_extension: HashMap::new(),
    };

    r.add(
        &["ts"],
        "typescript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        include_str!("../queries/typescript.scm"),
    );
    r.add(
        &["tsx"],
        "tsx",
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        include_str!("../queries/tsx.scm"),
    );
    r.add(
        &["js", "jsx"],
        "javascript",
        tree_sitter_javascript::LANGUAGE.into(),
        include_str!("../queries/javascript.scm"),
    );
    r.add(
        &["rs"],
        "rust",
        tree_sitter_rust::LANGUAGE.into(),
        include_str!("../queries/rust.scm"),
    );
    r.add(
        &["json"],
        "json",
        tree_sitter_json::LANGUAGE.into(),
        include_str!("../queries/json.scm"),
    );
    r.add(
        &["py"],
        "python",
        tree_sitter_python::LANGUAGE.into(),
        include_str!("../queries/python.scm"),
    );
    r.add(
        &["go"],
        "go",
        tree_sitter_go::LANGUAGE.into(),
        include_str!("../queries/go.scm"),
    );
    r.add(
        &["css"],
        "css",
        tree_sitter_css::LANGUAGE.into(),
        include_str!("../queries/css.scm"),
    );
    r.add(
        &["html"],
        "html",
        tree_sitter_html::LANGUAGE.into(),
        include_str!("../queries/html.scm"),
    );
    r.add(
        &["sh", "bash"],
        "bash",
        tree_sitter_bash::LANGUAGE.into(),
        include_str!("../queries/bash.scm"),
    );
    r.add(
        &["cs"],
        "c_sharp",
        tree_sitter_c_sharp::LANGUAGE.into(),
        include_str!("../queries/csharp.scm"),
    );
    r.add(
        &["rb"],
        "ruby",
        tree_sitter_ruby::LANGUAGE.into(),
        include_str!("../queries/ruby.scm"),
    );
    r.add(
        &["ex", "exs"],
        "elixir",
        tree_sitter_elixir::LANGUAGE.into(),
        include_str!("../queries/elixir.scm"),
    );

    r
}

/// Highlight a whole file. `None` for unsupported extensions and oversized
/// files — callers render plain text.
///
/// Multi-line constructs are handled by the parse, so a block comment paints
/// on every line it covers rather than only where it opens.
pub fn highlight(source: &str, path: &str) -> Option<FileHighlight> {
    if source.len() > MAX_BYTES {
        return None;
    }
    let ext = path.rsplit('.').next()?;
    let config = registry().get(ext)?;

    let mut highlighter = TsHighlighter::new();
    let events = highlighter
        .highlight(config, source.as_bytes(), None, |_| None)
        .ok()?;

    let line_starts = line_starts(source);
    let mut lines: Vec<Vec<HighlightSpan>> = vec![Vec::new(); line_starts.len()];
    let mut stack: Vec<usize> = Vec::new();

    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(h) => stack.push(h.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                // The innermost active capture wins, which is what painting
                // outer-then-inner produced in the Swift version.
                let Some(&kind) = stack.last() else { continue };
                let Ok(kind) = u8::try_from(kind) else {
                    continue;
                };
                push_span(&mut lines, &line_starts, source, start..end, kind);
            }
        }
    }

    Some(FileHighlight { lines })
}

/// Byte offset where each line begins. A trailing newline does not open a
/// line, matching how the surface counts them.
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' && index + 1 < source.len() {
            starts.push(index + 1);
        }
    }
    starts
}

/// Split a source range across the lines it covers, converting to
/// line-relative offsets and dropping the newline.
fn push_span(
    lines: &mut [Vec<HighlightSpan>],
    line_starts: &[usize],
    source: &str,
    range: Range<usize>,
    kind: u8,
) {
    let first = line_index(line_starts, range.start);
    let last = line_index(line_starts, range.end.saturating_sub(1).max(range.start));

    for index in first..=last.min(lines.len().saturating_sub(1)) {
        let line_start = line_starts[index];
        let line_end = line_starts
            .get(index + 1)
            .map(|next| next - 1)
            .unwrap_or(source.len());
        let start = range.start.max(line_start);
        let end = range.end.min(line_end);
        if start >= end {
            continue;
        }
        lines[index].push(HighlightSpan::new(
            start - line_start..end - line_start,
            Some(kind),
        ));
    }
}

fn line_index(line_starts: &[usize], offset: usize) -> usize {
    match line_starts.binary_search(&offset) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_name(span: &HighlightSpan) -> &'static str {
        span.kind
            .and_then(|k| HIGHLIGHT_NAMES.get(k as usize).copied())
            .unwrap_or("")
    }

    fn kinds(spans: &[HighlightSpan]) -> Vec<&'static str> {
        spans.iter().map(kind_name).collect()
    }

    fn text<'a>(line: &'a str, span: &HighlightSpan) -> &'a str {
        &line[span.range.clone()]
    }

    #[test]
    fn rust_keywords_and_strings() {
        let code = "fn main() {\n    let msg = \"hi\";\n}\n";
        let h = highlight(code, "src/main.rs").expect("rust is supported");

        let line1 = kinds(h.line(1));
        assert!(line1.contains(&"keyword"), "line1 kinds: {line1:?}");

        let line2_text = "    let msg = \"hi\";";
        let string_span = h
            .line(2)
            .iter()
            .find(|s| kind_name(s) == "string")
            .expect("string literal highlighted");
        assert!(text(line2_text, string_span).contains("hi"));
    }

    #[test]
    fn multiline_comment_spans_all_lines() {
        let code = "/* one\ntwo */\nlet x = 1;\n";
        let h = highlight(code, "a.ts").expect("ts is supported");
        assert!(kinds(h.line(1)).contains(&"comment"));
        assert!(kinds(h.line(2)).contains(&"comment"));
        assert!(!kinds(h.line(3)).contains(&"comment"));
    }

    #[test]
    fn unsupported_extension_is_none() {
        assert!(highlight("hello", "readme.xyz").is_none());
    }

    #[test]
    fn oversized_files_are_not_highlighted() {
        let big = "// x\n".repeat(MAX_BYTES / 5 + 1);
        assert!(big.len() > MAX_BYTES);
        assert!(highlight(&big, "a.rs").is_none());
    }

    #[test]
    fn span_ranges_are_line_relative_and_ordered() {
        let code = "const a = 1;\nconst b = \"two\";\n";
        let h = highlight(code, "x.ts").expect("ts is supported");
        let lines: Vec<&str> = code.lines().collect();
        for number in 1..=2 {
            let line_length = lines[number - 1].len();
            let mut cursor = 0;
            for span in h.line(number) {
                assert!(
                    span.range.start >= cursor,
                    "overlapping spans on line {number}"
                );
                assert!(
                    span.range.end <= line_length,
                    "span past end of line {number}"
                );
                cursor = span.range.end;
            }
        }
    }

    // Elixir's query leans on #any-of?/#match? — the predicate evaluation the
    // Swift port had to hand-roll and this one inherits from the crate.
    #[test]
    fn elixir_predicates_filter_matches() {
        let code = "defmodule Greeter do\n  def hello, do: :world\nend\n";
        let h = highlight(code, "lib/greeter.ex").expect("elixir is supported");
        let line1 = "defmodule Greeter do";
        let keyword = h
            .line(1)
            .iter()
            .find(|s| kind_name(s) == "keyword")
            .expect("defmodule is a keyword only via (#any-of? @keyword ...)");
        assert_eq!(text(line1, keyword), "defmodule");
        assert!(kinds(h.line(2)).contains(&"keyword"));
    }

    #[test]
    fn pruning_keeps_only_requested_lines() {
        let code = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let h = highlight(code, "a.rs").expect("rust is supported");
        assert!(!h.line(1).is_empty());
        assert!(!h.line(2).is_empty());

        let keep = std::collections::HashSet::from([2usize]);
        let pruned = h.pruned(&keep);
        assert!(pruned.line(1).is_empty());
        assert!(!pruned.line(2).is_empty());
        assert!(pruned.line(3).is_empty());
    }
    #[test]
    fn later_pattern_wins_on_same_node() {
        // Same node captured by two patterns. tree-sitter-highlight gives the
        // LATER pattern the win: `greet` is @property (the later catch-all),
        // not @function.method. The Swift port encoded this after verifying it
        // against this very crate; here it is inherited rather than imitated.
        let code = "class A {\n  greet() {}\n}\n";
        let h = highlight(code, "a.ts").expect("ts is supported");
        let k = kinds(h.line(2));
        assert!(k.contains(&"property"), "line2 kinds: {k:?}");
        assert!(!k.contains(&"function.method"), "line2 kinds: {k:?}");
    }
}
