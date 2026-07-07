pub mod changeset;
pub mod comments;
pub mod error;
pub mod feedback;
pub mod highlight;
pub mod intraline;
pub mod pairing;
pub mod rows;
pub mod scope;

pub use changeset::{Changeset, FileEntry, FileStatus};
pub use comments::{CommentContext, CommentStore, LocalComment, Side};
pub use error::CoreError;
pub use feedback::{fetch_pr_feedback, FeedbackComment, FeedbackKind, PrFeedback};
pub use highlight::{FileHighlight, HighlightSpan, HIGHLIGHT_NAMES};
pub use rows::{FileDiff, Hunk, LineRef, Row, RowKind};
pub use scope::{DiffScope, ScopeInfo};
