//! Fixtures shared by the model tests.
//!
//! The Swift suite declared these as private helpers inside its test file;
//! here they are a `cfg(test)` module so surface, filetree, and find can all
//! build the same two-row file without repeating it.

use crate::model::{
    Changeset, DiffHunk, DiffRow, DiffScope, FileDiff, FileStatus, LineRef, RowKind,
};
use std::path::PathBuf;

/// A modified file with one hunk: a change row (old/new line 4) followed by an
/// addition (new line 5), three lines skipped before it.
pub fn file(path: &str) -> FileDiff {
    file_with(path, FileStatus::Modified, 1, 1)
}

pub fn file_with(path: &str, status: FileStatus, added: usize, removed: usize) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        old_path: None,
        status,
        is_binary: false,
        added,
        removed,
        hunks: vec![DiffHunk {
            skipped_before: 3,
            rows: vec![
                DiffRow::new(
                    RowKind::Change,
                    Some(LineRef::new(4, "let value = 1")),
                    Some(LineRef::new(4, "let value = 2")),
                ),
                DiffRow::new(
                    RowKind::Addition,
                    None,
                    Some(LineRef::new(5, "print(value)")),
                ),
            ],
        }],
        old_highlight: None,
        new_highlight: None,
    }
}

pub fn changeset(files: Vec<FileDiff>) -> Changeset {
    Changeset {
        scope: DiffScope::Branch,
        base_name: "origin/main".to_string(),
        base_branch: Some("main".to_string()),
        branch: Some("feature".to_string()),
        workdir: PathBuf::from("/tmp/x"),
        git_dir: PathBuf::from("/tmp/x/.git"),
        files,
    }
}
