//! Line diff via libgit2's xdiff (patience + indent heuristic — the same
//! lineage as git's histogram default), then the display algorithm's
//! remaining passes: similarity-gated pairing and word-level intraline.
//! Produces the fixed-height `DiffHunk`/`DiffRow` model directly.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/LineDiff.swift` at fc3643e.
//!
//! Why `git2` and not `gix`: the Swift port drove libgit2 directly, and the
//! conformance tests below encode *that* implementation's output. gix ships a
//! different diff implementation, so porting onto it would mean the tests no
//! longer test what they were written to pin. Same C library, same results.

use crate::intraline::word_diff;
use crate::model::{DiffHunk, DiffRow, LineRef, RowKind};
use crate::pairing::{pair_block, Pairing};

const CONTEXT_LINES: u32 = 3;

/// One content line reported by libgit2, kept until its block is flushed.
struct Event {
    origin: char,
    old_line: usize,
    new_line: usize,
    text: String,
}

struct RawHunk {
    old_start: usize,
    old_lines: usize,
    events: Vec<Event>,
}

pub struct LineDiff {
    pub hunks: Vec<DiffHunk>,
    pub added: usize,
    pub removed: usize,
}

/// Diff two file contents into render-ready hunks.
pub fn diff_rows(old: &str, new: &str) -> LineDiff {
    let mut options = git2::DiffOptions::new();
    options
        .context_lines(CONTEXT_LINES)
        .indent_heuristic(true)
        .patience(true);

    let mut raw_hunks: Vec<RawHunk> = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;

    // `Patch` walks hunks and lines by index, which avoids threading a payload
    // pointer through callbacks the way the Swift port had to.
    let patch = match git2::Patch::from_buffers(
        old.as_bytes(),
        None,
        new.as_bytes(),
        None,
        Some(&mut options),
    ) {
        Ok(patch) => patch,
        // A diff that libgit2 refuses (binary content, allocation failure) has
        // no rows to show; an empty diff is the honest rendering.
        Err(_) => {
            return LineDiff {
                hunks: Vec::new(),
                added: 0,
                removed: 0,
            }
        }
    };

    for hunk_idx in 0..patch.num_hunks() {
        let Ok((hunk, line_count)) = patch.hunk(hunk_idx) else {
            continue;
        };
        let mut raw = RawHunk {
            old_start: hunk.old_start() as usize,
            old_lines: hunk.old_lines() as usize,
            events: Vec::new(),
        };
        for line_idx in 0..line_count {
            let Ok(line) = patch.line_in_hunk(hunk_idx, line_idx) else {
                continue;
            };
            let origin = line.origin();
            // Content lines only; skip EOFNL markers and headers.
            if origin != ' ' && origin != '+' && origin != '-' {
                continue;
            }
            let mut text = String::from_utf8_lossy(line.content()).into_owned();
            if text.ends_with('\n') {
                text.pop();
            }
            if text.ends_with('\r') {
                text.pop();
            }
            match origin {
                '+' => added += 1,
                '-' => removed += 1,
                _ => {}
            }
            raw.events.push(Event {
                origin,
                old_line: line.old_lineno().unwrap_or(0) as usize,
                new_line: line.new_lineno().unwrap_or(0) as usize,
                text,
            });
        }
        raw_hunks.push(raw);
    }

    // Assemble rows: runs of -/+ inside a hunk form change blocks, which go
    // through similarity pairing and then intraline.
    let mut hunks = Vec::with_capacity(raw_hunks.len());
    let mut previous_old_end = 1usize;
    for raw in &raw_hunks {
        let mut rows: Vec<DiffRow> = Vec::new();
        let mut pending_old: Vec<&Event> = Vec::new();
        let mut pending_new: Vec<&Event> = Vec::new();

        for event in &raw.events {
            match event.origin {
                '-' => pending_old.push(event),
                '+' => pending_new.push(event),
                _ => {
                    flush_block(&mut rows, &mut pending_old, &mut pending_new);
                    rows.push(DiffRow::new(
                        RowKind::Context,
                        Some(LineRef::new(event.old_line, event.text.clone())),
                        Some(LineRef::new(event.new_line, event.text.clone())),
                    ));
                }
            }
        }
        flush_block(&mut rows, &mut pending_old, &mut pending_new);

        hunks.push(DiffHunk {
            skipped_before: raw.old_start.saturating_sub(previous_old_end),
            rows,
        });
        previous_old_end = raw.old_start + raw.old_lines;
    }

    LineDiff {
        hunks,
        added,
        removed,
    }
}

fn flush_block(
    rows: &mut Vec<DiffRow>,
    pending_old: &mut Vec<&Event>,
    pending_new: &mut Vec<&Event>,
) {
    if pending_old.is_empty() && pending_new.is_empty() {
        return;
    }
    let old_texts: Vec<String> = pending_old.iter().map(|e| e.text.clone()).collect();
    let new_texts: Vec<String> = pending_new.iter().map(|e| e.text.clone()).collect();
    for pairing in pair_block(&old_texts, &new_texts) {
        match pairing {
            Pairing::Pair(o, n) => {
                let old_event = pending_old[o];
                let new_event = pending_new[n];
                let (old_changed, new_changed) = word_diff(&old_event.text, &new_event.text);
                rows.push(DiffRow {
                    kind: RowKind::Change,
                    old: Some(LineRef::new(old_event.old_line, old_event.text.clone())),
                    new: Some(LineRef::new(new_event.new_line, new_event.text.clone())),
                    old_changed,
                    new_changed,
                });
            }
            Pairing::Removed(o) => {
                let event = pending_old[o];
                rows.push(DiffRow::new(
                    RowKind::Removal,
                    Some(LineRef::new(event.old_line, event.text.clone())),
                    None,
                ));
            }
            Pairing::Added(n) => {
                let event = pending_new[n];
                rows.push(DiffRow::new(
                    RowKind::Addition,
                    None,
                    Some(LineRef::new(event.new_line, event.text.clone())),
                ));
            }
        }
    }
    pending_old.clear();
    pending_new.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn simple_change_pairs_lines_with_intraline() {
        let d = diff_rows("a\nlet b = 1;\nc\n", "a\nlet B = 1;\nc\n");
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
        assert_eq!(d.hunks.len(), 1);
        let changes: Vec<&DiffRow> = d.hunks[0]
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Change)
            .collect();
        assert_eq!(changes.len(), 1);
        let old = changes[0].old.as_ref().unwrap();
        let new = changes[0].new.as_ref().unwrap();
        assert_eq!(old.text, "let b = 1;");
        assert_eq!(new.text, "let B = 1;");
        assert_eq!(old.number, 2);
        assert_eq!(&old.text[changes[0].old_changed[0].clone()], "b");
    }

    #[test]
    fn dissimilar_replacement_renders_removal_plus_addition() {
        let d = diff_rows(
            "a\nreturn legacy_path();\nc\n",
            "a\ntodo!(\"rewrite\")\nc\n",
        );
        let kinds: HashSet<RowKind> = d.hunks[0].rows.iter().map(|r| r.kind).collect();
        assert!(kinds.contains(&RowKind::Removal));
        assert!(kinds.contains(&RowKind::Addition));
        assert!(!kinds.contains(&RowKind::Change));
    }

    #[test]
    fn distant_changes_become_separate_hunks_with_skips() {
        let old: String = (1..=40).map(|i| format!("line{i}\n")).collect();
        let new = old
            .replace("line5\n", "LINE5\n")
            .replace("line35\n", "LINE35\n");
        let d = diff_rows(&old, &new);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 2);
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.hunks[0].skipped_before, 1);
        assert!(d.hunks[1].skipped_before > 0);
    }

    #[test]
    fn identical_contents_yield_no_hunks() {
        let d = diff_rows("a\nb\n", "a\nb\n");
        assert!(d.hunks.is_empty());
        assert_eq!(d.added + d.removed, 0);
    }
}
