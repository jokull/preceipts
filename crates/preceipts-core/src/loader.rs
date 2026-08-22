//! Assemble the full changeset for a repo path and scope.
//!
//! Expensive — call it off the UI thread and cache the result.
//!
//! Ported forward from `swift/Sources/PreceiptsKit/ChangesetLoader.swift` at
//! fc3643e, minus the GitHub remote association (cut with the PR conversation
//! surface) and, for now, the parallel highlight pass: highlighting lands with
//! the tree-sitter port, and the loader is shaped to take it.

use crate::error::{Error, Result};
use crate::gitreader::{worktree_content, GitReader};
use crate::linediff::diff_rows;
use crate::model::{Changeset, DiffScope, FileDiff, FileStatus};
use std::path::Path;

pub fn load(repo_path: &Path, scope: DiffScope) -> Result<Changeset> {
    let reader = GitReader::open(repo_path)?;
    let workdir = reader.workdir()?;
    let (branch, head_oid) = reader.head_branch()?;

    // The base branch name is wanted in both scopes: "origin/main" and "main"
    // both mean `--base main` to the check runner.
    let resolved_base = reader.resolve_base().ok();
    let base_branch = resolved_base.as_ref().map(|(name, _)| {
        name.strip_prefix("origin/")
            .unwrap_or(name.as_str())
            .to_string()
    });

    let (base_name, base_commit) = match scope {
        DiffScope::Uncommitted => ("HEAD".to_string(), head_oid),
        DiffScope::Branch => {
            let (name, oid) = resolved_base.clone().ok_or(Error::NoBase)?;
            // No range-syntax ellipsis: in a status label it reads as a
            // truncated string rather than as merge-base semantics.
            (name, reader.merge_base(oid, head_oid)?)
        }
    };

    let mut files = Vec::new();
    for entry in reader.changed_files(base_commit)? {
        let old_source = entry.old_path.clone().unwrap_or_else(|| entry.path.clone());

        let (old_content, old_binary) = match entry.status {
            FileStatus::Added => (String::new(), false),
            _ => reader.blob_content(base_commit, &old_source)?,
        };
        let (new_content, new_binary) = match entry.status {
            FileStatus::Deleted => (String::new(), false),
            _ => worktree_content(&workdir, &entry.path),
        };

        let is_binary = old_binary || new_binary;
        let diff = if is_binary {
            None
        } else {
            Some(diff_rows(&old_content, &new_content))
        };
        let (hunks, added, removed) = match diff {
            Some(d) => (d.hunks, d.added, d.removed),
            None => (Vec::new(), 0, 0),
        };

        // A file can look changed by stat yet diff clean — `touch` is enough.
        if hunks.is_empty() && entry.status == FileStatus::Modified {
            continue;
        }

        files.push(FileDiff {
            path: entry.path,
            old_path: entry.old_path,
            status: entry.status,
            is_binary,
            added,
            removed,
            hunks,
        });
    }

    Ok(Changeset {
        scope,
        base_name,
        base_branch,
        branch,
        workdir,
        git_dir: reader.git_dir(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::process::Command;

    fn sh(dir: &Path, args: &[&str]) {
        let status = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .status()
            .unwrap_or_else(|e| panic!("{args:?} failed to spawn: {e}"));
        assert!(status.success(), "{args:?} failed");
    }

    fn write(dir: &Path, path: &str, content: &str) {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    /// main has a.txt + src/keep.rs. feature modifies a.txt, adds new.rs, and
    /// deletes src/keep.rs (all committed), then leaves uncommitted edits on
    /// top: one more line in a.txt and an untracked file.
    fn scratch_repo() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        sh(&dir, &["git", "init", "-q", "-b", "main"]);
        sh(&dir, &["git", "config", "user.name", "t"]);
        sh(&dir, &["git", "config", "user.email", "t@t.local"]);
        sh(&dir, &["git", "config", "commit.gpgsign", "false"]);
        write(&dir, "a.txt", "one\ntwo\nthree\n");
        write(&dir, "src/keep.rs", "fn keep() {}\n");
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "base"]);
        sh(&dir, &["git", "checkout", "-qb", "feature"]);
        write(&dir, "a.txt", "one\nTWO\nthree\n");
        write(&dir, "new.rs", "fn new_thing() {}\n");
        sh(&dir, &["git", "rm", "-q", "src/keep.rs"]);
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "feature work"]);
        write(&dir, "a.txt", "one\nTWO\nthree\nfour\n");
        write(&dir, "untracked.md", "# notes\n");
        (temp, dir)
    }

    fn by_path(changeset: &Changeset) -> HashMap<&str, &FileDiff> {
        changeset
            .files
            .iter()
            .map(|f| (f.path.as_str(), f))
            .collect()
    }

    #[test]
    fn branch_scope_spans_commits_and_working_tree() {
        let (_temp, dir) = scratch_repo();
        let changeset = load(&dir, DiffScope::Branch).unwrap();

        assert_eq!(changeset.branch.as_deref(), Some("feature"));
        assert!(changeset.base_name.starts_with("main"));

        let files = by_path(&changeset);
        assert_eq!(files["a.txt"].status, FileStatus::Modified);
        assert_eq!(files["new.rs"].status, FileStatus::Added);
        assert_eq!(files["src/keep.rs"].status, FileStatus::Deleted);
        assert_eq!(files["untracked.md"].status, FileStatus::Added);

        // Two added lines across the commit and the uncommitted edit.
        assert_eq!(files["a.txt"].added, 2);
        assert_eq!(files["a.txt"].removed, 1);
        assert_eq!(files["src/keep.rs"].removed, 1);
        assert_eq!(files["src/keep.rs"].added, 0);
    }

    #[test]
    fn uncommitted_scope_sees_only_working_tree_changes() {
        let (_temp, dir) = scratch_repo();
        let changeset = load(&dir, DiffScope::Uncommitted).unwrap();

        let paths: HashSet<&str> = changeset.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains("a.txt"));
        assert!(paths.contains("untracked.md"));
        assert!(!paths.contains("new.rs"));
        assert!(!paths.contains("src/keep.rs"));

        let files = by_path(&changeset);
        assert_eq!(files["a.txt"].added, 1);
        assert_eq!(files["a.txt"].removed, 0);
    }

    #[test]
    fn binary_files_are_flagged_not_diffed() {
        let (_temp, dir) = scratch_repo();
        std::fs::write(dir.join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();
        let changeset = load(&dir, DiffScope::Uncommitted).unwrap();
        let files = by_path(&changeset);
        let bin = files["blob.bin"];
        assert!(bin.is_binary);
        assert!(bin.hunks.is_empty());
    }

    #[test]
    fn totals_sum_across_files() {
        let (_temp, dir) = scratch_repo();
        let changeset = load(&dir, DiffScope::Branch).unwrap();
        let added: usize = changeset.files.iter().map(|f| f.added).sum();
        let removed: usize = changeset.files.iter().map(|f| f.removed).sum();
        assert_eq!(changeset.total_added(), added);
        assert_eq!(changeset.total_removed(), removed);
    }
}
