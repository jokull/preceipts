//! libgit2 reads, in-process.
//!
//! The GitUp / Sublime Merge lesson: never block a frame on a `git`
//! subprocess. Every read here is a library call, which is also why this lives
//! in core rather than behind the daemon socket.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/GitReader.swift` at fc3643e.

use crate::error::{Error, Result};
use crate::model::FileStatus;
use git2::{Delta, DiffFindOptions, DiffOptions, Oid, Repository};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
}

/// One opened repository. Cheap to hold, not `Sync` — libgit2 objects are not
/// thread-safe, so use one per load.
pub struct GitReader {
    repo: Repository,
}

impl GitReader {
    /// Open the repository containing `path`, searching upward.
    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::discover(path).map_err(|_| Error::NotARepo(path.to_path_buf()))?;
        Ok(Self { repo })
    }

    pub fn workdir(&self) -> Result<PathBuf> {
        self.repo
            .workdir()
            .map(Path::to_path_buf)
            .ok_or_else(|| Error::NotARepo(self.repo.path().to_path_buf()))
    }

    pub fn git_dir(&self) -> PathBuf {
        self.repo.path().to_path_buf()
    }

    /// The current branch's shorthand name and commit. The name is `None` on
    /// a detached HEAD.
    pub fn head_branch(&self) -> Result<(Option<String>, Oid)> {
        let head = self.repo.head()?;
        let oid = head.target().ok_or(Error::UnbornHead)?;
        Ok((head.shorthand().map(str::to_string), oid))
    }

    /// First base candidate that resolves, as (short name, commit id).
    pub fn resolve_base(&self) -> Result<(String, Oid)> {
        const CANDIDATES: [(&str, &str); 4] = [
            ("origin/main", "refs/remotes/origin/main"),
            ("main", "refs/heads/main"),
            ("origin/master", "refs/remotes/origin/master"),
            ("master", "refs/heads/master"),
        ];
        for (short, full) in CANDIDATES {
            if let Ok(reference) = self.repo.find_reference(full) {
                if let Ok(resolved) = reference.resolve() {
                    if let Some(target) = resolved.target() {
                        return Ok((short.to_string(), target));
                    }
                }
            }
        }
        Err(Error::NoBase)
    }

    pub fn merge_base(&self, a: Oid, b: Oid) -> Result<Oid> {
        Ok(self.repo.merge_base(a, b)?)
    }

    /// Changed files between a base commit's tree and the working tree.
    /// Untracked files are included: they are part of what a run would mint.
    pub fn changed_files(&self, base_commit: Oid) -> Result<Vec<ChangedFile>> {
        let tree = self.repo.find_commit(base_commit)?.tree()?;

        let mut options = DiffOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_typechange(true);

        let mut diff = self
            .repo
            .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut options))?;

        let mut find_options = DiffFindOptions::new();
        find_options.renames(true);
        diff.find_similar(Some(&mut find_options))?;

        let mut files = Vec::new();
        for delta in diff.deltas() {
            let new_path = delta.new_file().path().map(path_string);
            let old_path = delta.old_file().path().map(path_string);
            let entry = match delta.status() {
                Delta::Added | Delta::Untracked => new_path.map(|path| ChangedFile {
                    path,
                    old_path: None,
                    status: FileStatus::Added,
                }),
                Delta::Deleted => old_path.map(|path| ChangedFile {
                    path,
                    old_path: None,
                    status: FileStatus::Deleted,
                }),
                Delta::Renamed => new_path.map(|path| ChangedFile {
                    path,
                    old_path,
                    status: FileStatus::Renamed,
                }),
                Delta::Modified | Delta::Typechange => new_path.map(|path| ChangedFile {
                    path,
                    old_path: None,
                    status: FileStatus::Modified,
                }),
                _ => None,
            };
            if let Some(entry) = entry {
                files.push(entry);
            }
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut seen = HashSet::new();
        files.retain(|f| seen.insert(f.path.clone()));
        Ok(files)
    }

    /// Content of a blob at `path` in the base commit's tree, and whether it
    /// is binary. A path missing from the tree reads as empty, not an error —
    /// that is how an added file looks from the base side.
    pub fn blob_content(&self, base_commit: Oid, path: &str) -> Result<(String, bool)> {
        let tree = self.repo.find_commit(base_commit)?.tree()?;
        let Ok(entry) = tree.get_path(Path::new(path)) else {
            return Ok((String::new(), false));
        };
        let Ok(blob) = self.repo.find_blob(entry.id()) else {
            return Ok((String::new(), false));
        };
        if blob.is_binary() {
            return Ok((String::new(), true));
        }
        Ok((String::from_utf8_lossy(blob.content()).into_owned(), false))
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Content of a worktree file, and whether it is binary. Unreadable files read
/// as empty: a file that vanished between the diff and the read is not an
/// error worth failing a whole changeset over.
pub fn worktree_content(workdir: &Path, path: &str) -> (String, bool) {
    let Ok(bytes) = std::fs::read(workdir.join(path)) else {
        return (String::new(), false);
    };
    // Same heuristic libgit2 uses: a NUL early in the file means binary.
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return (String::new(), true);
    }
    (String::from_utf8_lossy(&bytes).into_owned(), false)
}
