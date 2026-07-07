//! The diff row model: what the scroll surface renders.
//!
//! Rows are fixed-height and precomputed — the architecture decision from
//! docs/desktop-foundations.md that makes scroll geometry trivial. Each
//! `Row` pairs an old-side and new-side line (either may be absent);
//! removed/added runs inside a hunk are aligned pairwise, the classic
//! side-by-side layout.

use imara_diff::{Algorithm, Diff, InternedInput};

const CONTEXT_LINES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Context,
    /// Old side removed, new side added (paired change).
    Change,
    Addition,
    Removal,
}

#[derive(Clone, Debug)]
pub struct LineRef {
    /// 1-based line number in its version of the file.
    pub number: u32,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub kind: RowKind,
    pub old: Option<LineRef>,
    pub new: Option<LineRef>,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    /// Unchanged lines skipped since the previous hunk (or file start).
    pub skipped_before: u32,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: crate::changeset::FileStatus,
    pub is_binary: bool,
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
}

/// Compute the side-by-side row model for one file's old/new contents.
pub fn diff_rows(old: &str, new: &str) -> (Vec<Hunk>, usize, usize) {
    let input = InternedInput::new(old, new);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);

    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut hunks: Vec<Hunk> = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    // First unchanged line (0-based) not yet emitted, per side. The gap
    // between cursor and the next change is always unchanged AND aligned,
    // so a gap row's new index is cursor_new + the same offset.
    let mut cursor_old: u32 = 0;
    let mut cursor_new: u32 = 0;

    for change in diff.hunks() {
        added += change.after.len();
        removed += change.before.len();

        let gap = change.before.start - cursor_old;
        if hunks.is_empty() || gap > 2 * CONTEXT_LINES {
            // Close the previous hunk with trailing context, then open a
            // new hunk with leading context; the middle is skipped.
            let trailing = if hunks.is_empty() {
                0
            } else {
                CONTEXT_LINES.min(gap)
            };
            if let Some(last) = hunks.last_mut() {
                for offset in 0..trailing {
                    last.rows.push(context_row(
                        &old_lines,
                        &new_lines,
                        cursor_old + offset,
                        cursor_new + offset,
                    ));
                }
            }
            let leading = CONTEXT_LINES.min(gap - trailing);
            let mut rows = Vec::new();
            for offset in 0..leading {
                let old_idx = change.before.start - leading + offset;
                let new_idx = change.after.start - leading + offset;
                rows.push(context_row(&old_lines, &new_lines, old_idx, new_idx));
            }
            hunks.push(Hunk {
                skipped_before: gap - trailing - leading,
                rows,
            });
        } else {
            // Close enough to the previous hunk: emit the whole gap as
            // context and continue in the same hunk.
            let last = hunks.last_mut().expect("checked non-empty");
            for offset in 0..gap {
                last.rows.push(context_row(
                    &old_lines,
                    &new_lines,
                    cursor_old + offset,
                    cursor_new + offset,
                ));
            }
        }

        // The change block: pair removed/added lines, remainder one-sided.
        let rows = &mut hunks.last_mut().expect("hunk exists").rows;
        let paired = change.before.len().min(change.after.len()) as u32;
        for offset in 0..paired {
            rows.push(Row {
                kind: RowKind::Change,
                old: Some(line_ref(&old_lines, change.before.start + offset)),
                new: Some(line_ref(&new_lines, change.after.start + offset)),
            });
        }
        for old_idx in (change.before.start + paired)..change.before.end {
            rows.push(Row {
                kind: RowKind::Removal,
                old: Some(line_ref(&old_lines, old_idx)),
                new: None,
            });
        }
        for new_idx in (change.after.start + paired)..change.after.end {
            rows.push(Row {
                kind: RowKind::Addition,
                old: None,
                new: Some(line_ref(&new_lines, new_idx)),
            });
        }

        cursor_old = change.before.end;
        cursor_new = change.after.end;
    }

    // Trailing context after the last change.
    if let Some(last) = hunks.last_mut() {
        let remaining = (old_lines.len() as u32 - cursor_old)
            .min(new_lines.len() as u32 - cursor_new)
            .min(CONTEXT_LINES);
        for offset in 0..remaining {
            last.rows.push(context_row(
                &old_lines,
                &new_lines,
                cursor_old + offset,
                cursor_new + offset,
            ));
        }
    }

    (hunks, added, removed)
}

fn line_ref(lines: &[&str], idx: u32) -> LineRef {
    LineRef {
        number: idx + 1,
        text: lines.get(idx as usize).unwrap_or(&"").to_string(),
    }
}

fn context_row(old_lines: &[&str], new_lines: &[&str], old_idx: u32, new_idx: u32) -> Row {
    Row {
        kind: RowKind::Context,
        old: Some(line_ref(old_lines, old_idx)),
        new: Some(line_ref(new_lines, new_idx)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_change_pairs_lines() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\n";
        let (hunks, added, removed) = diff_rows(old, new);
        assert_eq!((added, removed), (1, 1));
        assert_eq!(hunks.len(), 1);
        let rows = &hunks[0].rows;
        let change: Vec<&Row> = rows.iter().filter(|r| r.kind == RowKind::Change).collect();
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].old.as_ref().unwrap().text, "b");
        assert_eq!(change[0].new.as_ref().unwrap().text, "B");
        assert_eq!(change[0].old.as_ref().unwrap().number, 2);
    }

    #[test]
    fn pure_addition_has_empty_old_side() {
        let old = "a\nb\n";
        let new = "a\nx\nb\n";
        let (hunks, added, removed) = diff_rows(old, new);
        assert_eq!((added, removed), (1, 0));
        let rows = &hunks[0].rows;
        let add: Vec<&Row> = rows
            .iter()
            .filter(|r| r.kind == RowKind::Addition)
            .collect();
        assert_eq!(add.len(), 1);
        assert!(add[0].old.is_none());
        assert_eq!(add[0].new.as_ref().unwrap().number, 2);
    }

    #[test]
    fn distant_changes_become_separate_hunks_with_skips() {
        let old: String = (1..=40).map(|i| format!("line{i}\n")).collect();
        let new = old.replace("line5\n", "LINE5\n").replace("line35\n", "LINE35\n");
        let (hunks, added, removed) = diff_rows(&old, &new);
        assert_eq!((added, removed), (2, 2));
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].skipped_before, 1); // lines 1..=1 skipped (5-3-1)
        assert!(hunks[1].skipped_before > 0);
    }

    #[test]
    fn nearby_changes_merge_into_one_hunk() {
        let old: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let new = old.replace("line5\n", "LINE5\n").replace("line8\n", "LINE8\n");
        let (hunks, _, _) = diff_rows(&old, &new);
        assert_eq!(hunks.len(), 1);
    }

    #[test]
    fn identical_contents_yield_no_hunks() {
        let (hunks, added, removed) = diff_rows("a\nb\n", "a\nb\n");
        assert!(hunks.is_empty());
        assert_eq!((added, removed), (0, 0));
    }
}
