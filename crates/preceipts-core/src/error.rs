//! Errors the core surfaces to its callers.
//!
//! Ported forward from `PreceiptsError` in
//! `swift/Sources/PreceiptsKit/Model.swift` at fc3643e.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),

    #[error("no base branch found (tried origin/main, main, origin/master, master)")]
    NoBase,

    #[error("HEAD does not resolve — unborn branch?")]
    UnbornHead,

    // `transparent` rather than `"{0}"`: with #[from] the inner error is also
    // the source, so a formatted chain would print the same sentence twice.
    #[error(transparent)]
    Git(#[from] git2::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
