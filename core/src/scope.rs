//! Diff scope: what the working tree is compared against.
//!
//! `Branch` is the PR view — merge-base of the auto-detected base branch vs
//! the working tree. `Uncommitted` compares against HEAD. Both scopes diff
//! against the *working tree* (what `preceipts run` mints against), never
//! just the index.

use git2::{Oid, Repository};

use crate::error::CoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffScope {
    /// merge-base(base, HEAD) → working tree (the PR diff). Default.
    Branch,
    /// HEAD → working tree.
    Uncommitted,
}

#[derive(Clone, Debug)]
pub struct ScopeInfo {
    pub scope: DiffScope,
    /// Human name of the comparison base ("origin/main…" or "HEAD").
    pub base_name: String,
    /// The commit the diff runs from (merge-base for Branch, HEAD otherwise).
    pub base_commit: Oid,
    pub branch: Option<String>,
}

const BASE_CANDIDATES: &[&str] = &[
    "refs/remotes/origin/main",
    "refs/heads/main",
    "refs/remotes/origin/master",
    "refs/heads/master",
];

/// First base candidate that resolves, as (short name, commit id).
pub fn resolve_base(repo: &Repository) -> Result<(String, Oid), CoreError> {
    for candidate in BASE_CANDIDATES {
        if let Ok(reference) = repo.find_reference(candidate) {
            if let Some(oid) = reference.target() {
                let short = candidate
                    .trim_start_matches("refs/remotes/")
                    .trim_start_matches("refs/heads/");
                return Ok((short.to_string(), oid));
            }
        }
    }
    Err(CoreError::NoBase)
}

pub fn resolve(repo: &Repository, scope: DiffScope) -> Result<ScopeInfo, CoreError> {
    let head = repo.head().map_err(|_| CoreError::UnbornHead)?;
    let head_oid = head.target().ok_or(CoreError::UnbornHead)?;
    let branch = head.shorthand().map(str::to_string);

    match scope {
        DiffScope::Uncommitted => Ok(ScopeInfo {
            scope,
            base_name: "HEAD".to_string(),
            base_commit: head_oid,
            branch,
        }),
        DiffScope::Branch => {
            let (base_name, base_oid) = resolve_base(repo)?;
            let merge_base = repo.merge_base(base_oid, head_oid)?;
            Ok(ScopeInfo {
                scope,
                base_name: format!("{base_name}\u{2026}"),
                base_commit: merge_base,
                branch,
            })
        }
    }
}
