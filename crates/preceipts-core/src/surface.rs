//! The scroll-surface row model: every changed file flattened into one
//! virtualized list (the Zed multibuffer shape).
//!
//! Pure data — built from a `Changeset`, consumed by whatever draws rows. The
//! surface is what makes the diff fast: rows are fixed-height and precomputed,
//! so the view never measures and never walks the changeset to paint a frame.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/Surface.swift` at fc3643e.

use crate::model::Changeset;

/// Which version of a file a line number refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRow {
    FileHeader {
        file: usize,
    },
    /// Unchanged lines elided between hunks.
    Gap {
        skipped: usize,
    },
    Line {
        file: usize,
        hunk: usize,
        row: usize,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Surface {
    pub rows: Vec<SurfaceRow>,
    /// Index into `rows` of each file's header, by file index.
    pub file_anchors: Vec<usize>,
    /// Index into `rows` of each hunk's first line, in surface order.
    pub hunk_anchors: Vec<usize>,
}

impl Surface {
    pub fn build(changeset: &Changeset) -> Surface {
        let mut rows = Vec::new();
        let mut file_anchors = Vec::with_capacity(changeset.files.len());
        let mut hunk_anchors = Vec::new();

        for (file_index, file) in changeset.files.iter().enumerate() {
            file_anchors.push(rows.len());
            rows.push(SurfaceRow::FileHeader { file: file_index });
            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                if hunk.skipped_before > 0 {
                    rows.push(SurfaceRow::Gap {
                        skipped: hunk.skipped_before,
                    });
                }
                if !hunk.rows.is_empty() {
                    hunk_anchors.push(rows.len());
                }
                for row_index in 0..hunk.rows.len() {
                    rows.push(SurfaceRow::Line {
                        file: file_index,
                        hunk: hunk_index,
                        row: row_index,
                    });
                }
            }
        }

        Surface {
            rows,
            file_anchors,
            hunk_anchors,
        }
    }

    /// Resolve a path + 1-based line + side to its surface row. `None` when
    /// the file isn't in the changeset or the line isn't inside any rendered
    /// hunk — the target is outside the diff.
    pub fn anchor_row(
        &self,
        path: &str,
        line: usize,
        side: Side,
        changeset: &Changeset,
    ) -> Option<usize> {
        let file_index = changeset.files.iter().position(|f| f.path == path)?;
        let start = self.file_anchors[file_index];
        let end = self
            .file_anchors
            .get(file_index + 1)
            .copied()
            .unwrap_or(self.rows.len());
        let file = &changeset.files[file_index];

        (start..end).find(|&index| {
            let SurfaceRow::Line { hunk, row, .. } = self.rows[index] else {
                return false;
            };
            let diff_row = &file.hunks[hunk].rows[row];
            let candidate = match side {
                Side::Old => diff_row.old.as_ref(),
                Side::New => diff_row.new.as_ref(),
            };
            candidate.is_some_and(|c| c.number == line)
        })
    }

    /// The file whose section contains `row` — the last anchor at or above it.
    /// `None` when the surface is empty or `row` precedes the first header.
    pub fn file_index_at_row(&self, row: usize) -> Option<usize> {
        let mut best = None;
        let (mut low, mut high) = (0usize, self.file_anchors.len());
        while low < high {
            let mid = (low + high) / 2;
            if self.file_anchors[mid] <= row {
                best = Some(mid);
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{changeset, file};

    #[test]
    fn build_flattens_files_with_anchors_and_gaps() {
        let cs = changeset(vec![file("a.rs"), file("b.rs")]);
        let surface = Surface::build(&cs);
        assert_eq!(surface.file_anchors, [0, 4]);
        assert_eq!(surface.rows.len(), 8);
        assert_eq!(surface.rows[0], SurfaceRow::FileHeader { file: 0 });
        assert_eq!(surface.rows[1], SurfaceRow::Gap { skipped: 3 });
        assert_eq!(
            surface.rows[2],
            SurfaceRow::Line {
                file: 0,
                hunk: 0,
                row: 0
            }
        );
        assert_eq!(surface.rows[4], SurfaceRow::FileHeader { file: 1 });
        assert_eq!(surface.hunk_anchors, [2, 6]);
    }

    #[test]
    fn file_index_at_row_is_the_last_anchor_at_or_above() {
        let cs = changeset(vec![file("a.rs"), file("b.rs")]);
        let surface = Surface::build(&cs);
        assert_eq!(surface.file_index_at_row(0), Some(0));
        assert_eq!(surface.file_index_at_row(3), Some(0));
        assert_eq!(surface.file_index_at_row(4), Some(1));
        assert_eq!(surface.file_index_at_row(7), Some(1));
    }

    #[test]
    fn empty_surface() {
        let cs = changeset(vec![]);
        let surface = Surface::build(&cs);
        assert!(surface.rows.is_empty());
        assert_eq!(surface.file_index_at_row(0), None);
    }

    #[test]
    fn anchor_row_resolves_by_path_line_and_side() {
        let cs = changeset(vec![file("a.rs"), file("b.rs")]);
        let surface = Surface::build(&cs);
        // b.rs's change row: old line 4, new line 4; its addition: new line 5.
        assert_eq!(surface.anchor_row("b.rs", 4, Side::New, &cs), Some(6));
        assert_eq!(surface.anchor_row("b.rs", 5, Side::New, &cs), Some(7));
        assert_eq!(surface.anchor_row("a.rs", 4, Side::Old, &cs), Some(2));
        // Addition rows have no old side.
        assert_eq!(surface.anchor_row("a.rs", 5, Side::Old, &cs), None);
        assert_eq!(surface.anchor_row("missing.rs", 1, Side::New, &cs), None);
        assert_eq!(surface.anchor_row("a.rs", 99, Side::New, &cs), None);
    }
}
