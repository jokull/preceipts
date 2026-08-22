//! ⌘F row matching over the scroll surface: case-insensitive substring, where
//! headers match on path and line rows match on either side.
//!
//! Built as an index so case folding is paid once per changeset (off the UI
//! thread) and each keystroke is one `memmem` sweep over a contiguous buffer —
//! fast enough to run synchronously while typing.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/Find.swift` at fc3643e.

use crate::model::Changeset;
use crate::surface::{Surface, SurfaceRow};

pub struct FindIndex {
    /// Every row's text, lowercased, joined with `\n`. Row texts never contain
    /// a newline themselves (paths and single lines), so a newline-free needle
    /// cannot match across a row boundary.
    buffer: Vec<u8>,
    /// Buffer offset where each row's text begins; length is rows + 1.
    row_starts: Vec<usize>,
}

impl FindIndex {
    pub fn new(surface: &Surface, changeset: &Changeset) -> Self {
        let mut buffer: Vec<u8> = Vec::new();
        let mut row_starts = Vec::with_capacity(surface.rows.len() + 1);
        row_starts.push(0);

        for row in &surface.rows {
            match *row {
                SurfaceRow::FileHeader { file } => {
                    buffer.extend_from_slice(changeset.files[file].path.to_lowercase().as_bytes());
                }
                SurfaceRow::Gap { .. } => {}
                SurfaceRow::Line { file, hunk, row } => {
                    let diff_row = &changeset.files[file].hunks[hunk].rows[row];
                    if let Some(old) = &diff_row.old {
                        buffer.extend_from_slice(old.text.to_lowercase().as_bytes());
                    }
                    if let Some(new) = &diff_row.new {
                        if diff_row.old.is_some() {
                            buffer.push(b'\n');
                        }
                        buffer.extend_from_slice(new.text.to_lowercase().as_bytes());
                    }
                }
            }
            buffer.push(b'\n');
            row_starts.push(buffer.len());
        }

        Self { buffer, row_starts }
    }

    /// Surface rows whose text contains `query`, case-insensitively. Empty
    /// queries — and queries containing a newline — match nothing.
    pub fn matches(&self, query: &str) -> Vec<usize> {
        let needle = query.to_lowercase().into_bytes();
        if needle.is_empty() || needle.contains(&b'\n') || needle.len() > self.buffer.len() {
            return Vec::new();
        }

        let finder = memchr::memmem::Finder::new(&needle);
        let mut hits = Vec::new();
        let mut position = 0;
        while position + needle.len() <= self.buffer.len() {
            let Some(found) = finder.find(&self.buffer[position..]) else {
                break;
            };
            let row = self.row_containing(position + found);
            hits.push(row);
            // One hit per row is enough — skip to the next row.
            position = self.row_starts[row + 1];
        }
        hits
    }

    /// The row whose `[start, next_start)` span contains `offset` — the last
    /// row start at or below it. Empty rows share a start, but a match can
    /// only land inside a non-empty span.
    fn row_containing(&self, offset: usize) -> usize {
        let mut best = 0;
        let (mut low, mut high) = (0usize, self.row_starts.len() - 1);
        while low < high {
            let mid = (low + high) / 2;
            if self.row_starts[mid] <= offset {
                best = mid;
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

    fn fixture() -> (Surface, Changeset) {
        let cs = changeset(vec![file("src/Value.rs"), file("docs/notes.md")]);
        (Surface::build(&cs), cs)
    }

    #[test]
    fn matches_line_text_case_insensitive() {
        let (surface, cs) = fixture();
        let hits = FindIndex::new(&surface, &cs).matches("VALUE");
        // File 0's header path, both its line rows, and file 1's line rows.
        assert_eq!(hits, [0, 2, 3, 6, 7]);
    }

    #[test]
    fn matches_header_on_path_only() {
        let (surface, cs) = fixture();
        assert_eq!(FindIndex::new(&surface, &cs).matches("notes.md"), [4]);
    }

    #[test]
    fn matches_old_side_of_change_rows() {
        let (surface, cs) = fixture();
        assert_eq!(FindIndex::new(&surface, &cs).matches("= 1"), [2, 6]);
    }

    #[test]
    fn empty_query_matches_nothing() {
        let (surface, cs) = fixture();
        assert!(FindIndex::new(&surface, &cs).matches("").is_empty());
    }

    #[test]
    fn gaps_never_match() {
        let (surface, cs) = fixture();
        assert!(FindIndex::new(&surface, &cs)
            .matches("unchanged")
            .is_empty());
    }
}
