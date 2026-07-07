use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("not a git repository (or any parent): {0}")]
    NotARepo(PathBuf),
    #[error("no base branch found (tried origin/main, main, origin/master, master)")]
    NoBase,
    #[error("HEAD does not resolve — unborn branch?")]
    UnbornHead,
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Feedback(String),
}
