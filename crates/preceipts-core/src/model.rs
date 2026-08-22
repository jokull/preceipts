//! The diff row model. Rows are fixed-height and precomputed so the surface
//! can virtualize without measuring.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/Model.swift` at fc3643e.

use std::ops::Range;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffScope {
    Branch,
    Uncommitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl FileStatus {
    /// The single-letter code used in the file tree and by the engine wire
    /// format.
    pub fn code(self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Modified => 'M',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKind {
    Context,
    /// Old side removed, new side added — a similarity-paired edit.
    Change,
    Addition,
    Removal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRef {
    /// 1-based line number in its version of the file.
    pub number: usize,
    pub text: String,
}

impl LineRef {
    pub fn new(number: usize, text: impl Into<String>) -> Self {
        Self {
            number,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    pub old: Option<LineRef>,
    pub new: Option<LineRef>,
    /// Word-level changed byte ranges (change rows only), UTF-8 offsets.
    pub old_changed: Vec<Range<usize>>,
    pub new_changed: Vec<Range<usize>>,
}

impl DiffRow {
    pub fn new(kind: RowKind, old: Option<LineRef>, new: Option<LineRef>) -> Self {
        Self {
            kind,
            old,
            new,
            old_changed: Vec::new(),
            new_changed: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// Unchanged lines skipped since the previous hunk (or file start).
    pub skipped_before: usize,
    pub rows: Vec<DiffRow>,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub is_binary: bool,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone)]
pub struct Changeset {
    pub scope: DiffScope,
    /// Human name of the comparison base ("origin/main…" or "HEAD").
    pub base_name: String,
    /// Resolved base branch (e.g. "main"), independent of scope. `None` when
    /// no base candidate resolves.
    pub base_branch: Option<String>,
    pub branch: Option<String>,
    pub workdir: PathBuf,
    pub git_dir: PathBuf,
    pub files: Vec<FileDiff>,
}

impl Changeset {
    pub fn total_added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }

    pub fn total_removed(&self) -> usize {
        self.files.iter().map(|f| f.removed).sum()
    }
}
