//! preceipts-core — everything the app needs per frame, in-process.
//!
//! Git reads, the diff display algorithm, syntax highlighting, and filesystem
//! watching live here rather than behind the daemon socket: they are on the
//! scroll path, and IPC is not something a fast diff can afford. The daemon
//! owns what must outlive the window (processes, proxy, checks); this crate
//! owns what must keep up with it.
//!
//! The display algorithm runs in three passes, each its own module:
//!
//! 1. [`linediff`] — libgit2 xdiff (patience + indent heuristic) into hunks
//! 2. [`pairing`] — similarity-gated alignment of a block's removals/additions
//! 3. [`intraline`] — word-level LCS inside each paired row
//!
//! [`segments`] then composes a row's syntax spans and changed ranges into
//! render-ready runs, and [`surface`] flattens every file into the single
//! virtualized scroll list the view draws — with [`filetree`] and [`find`]
//! reading off the same model.
//!
//! All of it is ported forward from `PreceiptsKit` (Swift) at fc3643e, tests
//! included — those tests are the conformance suite for the rewrite, not new
//! work. See decision 12 in PRD.md.

pub mod error;
pub mod filetree;
pub mod find;
pub mod gitreader;
pub mod intraline;
pub mod linediff;
pub mod loader;
pub mod model;
pub mod pairing;
pub mod segments;
pub mod surface;

#[cfg(test)]
mod testing;

pub use error::{Error, Result};
pub use filetree::FileTreeNode;
pub use find::FindIndex;
pub use gitreader::{worktree_content, ChangedFile, GitReader};
pub use intraline::word_diff;
pub use linediff::{diff_rows, LineDiff};
pub use loader::load;
pub use model::{Changeset, DiffHunk, DiffRow, DiffScope, FileDiff, FileStatus, LineRef, RowKind};
pub use pairing::{pair_block, similarity, Pairing};
pub use segments::{line_segments, HighlightSpan, Segment};
pub use surface::{Side, Surface, SurfaceRow};
