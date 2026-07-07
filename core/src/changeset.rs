//! The changeset: which files differ between the scope base and the working
//! tree, and their full diff row models.
//!
//! Reads go through libgit2 in-process (the GitUp/Sublime-Merge lesson from
//! docs/desktop-foundations.md: never block the UI on a `git` subprocess).

use std::path::{Path, PathBuf};

use git2::{Delta, DiffFindOptions, DiffOptions, Repository};

use crate::error::CoreError;
use crate::rows::{diff_rows, FileDiff};
use crate::scope::{self, DiffScope, ScopeInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl FileStatus {
    pub fn glyph(&self) -> &'static str {
        match self {
            FileStatus::Added => "A",
            FileStatus::Modified => "M",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
}

pub struct Changeset {
    pub info: ScopeInfo,
    pub files: Vec<FileDiff>,
    pub workdir: PathBuf,
}

impl Changeset {
    /// Load the full changeset for a repo path and scope. This is the
    /// expensive call — run it off the UI thread and cache the result.
    pub fn load(repo_path: &Path, scope: DiffScope) -> Result<Changeset, CoreError> {
        let repo = Repository::discover(repo_path)
            .map_err(|_| CoreError::NotARepo(repo_path.to_path_buf()))?;
        let workdir = repo
            .workdir()
            .ok_or_else(|| CoreError::NotARepo(repo_path.to_path_buf()))?
            .to_path_buf();
        let info = scope::resolve(&repo, scope)?;
        let entries = changed_files(&repo, &info)?;

        let base_tree = repo.find_commit(info.base_commit)?.tree()?;
        let mut files = Vec::with_capacity(entries.len());
        for entry in entries {
            let old_source = entry.old_path.as_deref().unwrap_or(&entry.path);
            let (old_content, old_binary) = match entry.status {
                FileStatus::Added => (String::new(), false),
                _ => blob_content(&repo, &base_tree, old_source),
            };
            let (new_content, new_binary) = match entry.status {
                FileStatus::Deleted => (String::new(), false),
                _ => worktree_content(&workdir, &entry.path),
            };
            let is_binary = old_binary || new_binary;
            let (hunks, added, removed) = if is_binary {
                (Vec::new(), 0, 0)
            } else {
                diff_rows(&old_content, &new_content)
            };
            // A file can appear changed by stat but diff clean (e.g. touch).
            if hunks.is_empty() && matches!(entry.status, FileStatus::Modified) {
                continue;
            }
            files.push(FileDiff {
                path: entry.path,
                old_path: entry.old_path,
                status: entry.status,
                is_binary,
                hunks,
                added,
                removed,
            });
        }
        Ok(Changeset {
            info,
            files,
            workdir,
        })
    }

    pub fn total_added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }

    pub fn total_removed(&self) -> usize {
        self.files.iter().map(|f| f.removed).sum()
    }
}

/// List changed files between the scope base tree and the working tree
/// (untracked files included — they are part of what a run would mint).
fn changed_files(repo: &Repository, info: &ScopeInfo) -> Result<Vec<FileEntry>, CoreError> {
    let base_tree = repo.find_commit(info.base_commit)?.tree()?;
    let mut opts = DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_typechange(true);
    let mut diff = repo.diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut opts))?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;

    let mut entries = Vec::new();
    for delta in diff.deltas() {
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        let (status, path, old) = match delta.status() {
            Delta::Added | Delta::Untracked => (FileStatus::Added, new_path, None),
            Delta::Deleted => (FileStatus::Deleted, old_path, None),
            Delta::Renamed => (FileStatus::Renamed, new_path, old_path),
            Delta::Modified | Delta::Typechange => (FileStatus::Modified, new_path, None),
            _ => continue,
        };
        let Some(path) = path else { continue };
        entries.push(FileEntry {
            path,
            old_path: old,
            status,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
    Ok(entries)
}

fn blob_content(repo: &Repository, tree: &git2::Tree, path: &str) -> (String, bool) {
    let Ok(entry) = tree.get_path(Path::new(path)) else {
        return (String::new(), false);
    };
    let Ok(blob) = repo.find_blob(entry.id()) else {
        return (String::new(), false);
    };
    if blob.is_binary() {
        return (String::new(), true);
    }
    (String::from_utf8_lossy(blob.content()).into_owned(), false)
}

fn worktree_content(workdir: &Path, path: &str) -> (String, bool) {
    let Ok(bytes) = std::fs::read(workdir.join(path)) else {
        return (String::new(), false);
    };
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return (String::new(), true);
    }
    (String::from_utf8_lossy(&bytes).into_owned(), false)
}
