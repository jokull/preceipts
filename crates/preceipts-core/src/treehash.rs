//! The working tree's hash — the key a receipt is filed under.
//!
//! Ported forward from `engine/src/lib/tree-hash.ts` at fc3643e, and this one
//! keeps shelling to `git` deliberately. Everywhere else in core, libgit2 is
//! the right call because reads sit on the frame path. Here the requirement is
//! different: the tree hash must be **byte-identical** to what the TypeScript
//! engine produced, or receipts minted before the rewrite stop matching trees
//! minted after it. `git add -A` into a temporary index is the exact operation
//! that produced the existing receipts, so it is the operation we keep.
//!
//! The temp index is seeded by *copying the real index* when one exists: the
//! copy carries git's stat cache, so `add -A` re-hashes only what actually
//! changed (~0.2s on a large monorepo). Seeding with `read-tree HEAD` instead
//! has no stat data and forces a full re-hash of every tracked file, which
//! takes many seconds — so that path is the fallback for fresh repos only.

use crate::error::{Error, Result};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingTree {
    /// Tree sha of the working tree as it stands right now.
    pub tree: String,
    /// Tree sha of HEAD's tree; `None` on an unborn branch.
    pub head_tree: Option<String>,
    /// The working tree differs from HEAD.
    pub dirty: bool,
}

fn git(root: &Path, args: &[&str], index: Option<&Path>) -> Result<std::process::Output> {
    let mut command = Command::new("git");
    command.args(args).current_dir(root);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    command
        .output()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("running git {args:?}: {e}"))))
}

fn git_stdout(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    let output = git(root, args, index)?;
    if !output.status.success() {
        return Err(Error::Git(git2::Error::from_str(&format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Hash the working tree through a temporary index, leaving the real one
/// untouched. Ignored files stay excluded — normal gitignore semantics.
pub fn compute(root: &Path) -> Result<WorkingTree> {
    let scratch = tempfile::tempdir()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("temp dir: {e}"))))?;
    let index_file = scratch.path().join("index");

    let head_tree = git_stdout(root, &["rev-parse", "HEAD^{tree}"], None).ok();

    // Seed from the real index when there is one, for its stat cache.
    let real_index = git_stdout(
        root,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        None,
    )
    .unwrap_or_default();
    let seeded = !real_index.is_empty() && std::fs::copy(&real_index, &index_file).is_ok();

    if !seeded {
        let args: &[&str] = if head_tree.is_some() {
            &["read-tree", "HEAD"]
        } else {
            &["read-tree", "--empty"]
        };
        git_stdout(root, args, Some(&index_file))?;
    }

    git_stdout(root, &["add", "-A"], Some(&index_file))?;
    let tree = git_stdout(root, &["write-tree"], Some(&index_file))?;

    let dirty = head_tree.as_deref() != Some(tree.as_str());
    Ok(WorkingTree {
        tree,
        head_tree,
        dirty,
    })
}

/// Resolve a ref — branch, commit, or tree sha — to its tree.
pub fn resolve_tree(root: &Path, reference: &str) -> Result<String> {
    git_stdout(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{tree}}"),
        ],
        None,
    )
    .map_err(|_| {
        Error::Git(git2::Error::from_str(&format!(
            "cannot resolve \"{reference}\" to a tree — is it a branch, commit, or tree sha?"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        assert!(Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    }

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        sh(&dir, &["git", "init", "-q", "-b", "main"]);
        sh(&dir, &["git", "config", "user.name", "t"]);
        sh(&dir, &["git", "config", "user.email", "t@t.local"]);
        sh(&dir, &["git", "config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "base"]);
        (temp, dir)
    }

    #[test]
    fn a_clean_worktree_hashes_to_heads_tree() {
        let (_t, dir) = scratch();
        let wt = compute(&dir).unwrap();
        assert_eq!(Some(wt.tree.clone()), wt.head_tree);
        assert!(!wt.dirty);
    }

    #[test]
    fn an_edit_changes_the_tree_and_marks_it_dirty() {
        let (_t, dir) = scratch();
        let clean = compute(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        let dirty = compute(&dir).unwrap();
        assert_ne!(clean.tree, dirty.tree);
        assert!(dirty.dirty);
        assert_eq!(dirty.head_tree, clean.head_tree, "HEAD did not move");
    }

    /// Untracked files count. They are part of what a run would exercise, so
    /// they must be part of what the receipt is filed under.
    #[test]
    fn untracked_files_are_included() {
        let (_t, dir) = scratch();
        let before = compute(&dir).unwrap();
        std::fs::write(dir.join("new.txt"), "hello\n").unwrap();
        let after = compute(&dir).unwrap();
        assert_ne!(before.tree, after.tree);
    }

    #[test]
    fn ignored_files_are_excluded() {
        let (_t, dir) = scratch();
        std::fs::write(dir.join(".gitignore"), "junk/\n").unwrap();
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "ignore"]);
        let before = compute(&dir).unwrap();

        std::fs::create_dir(dir.join("junk")).unwrap();
        std::fs::write(dir.join("junk/big.bin"), "noise\n").unwrap();
        let after = compute(&dir).unwrap();
        assert_eq!(
            before.tree, after.tree,
            "ignored files must not move the tree"
        );
    }

    /// The whole point of the temp index: the real one is untouched, so a
    /// receipt run never disturbs a staged-but-uncommitted state.
    #[test]
    fn the_real_index_is_left_alone() {
        let (_t, dir) = scratch();
        std::fs::write(dir.join("staged.txt"), "s\n").unwrap();
        sh(&dir, &["git", "add", "staged.txt"]);
        let before = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dir)
            .output()
            .unwrap();

        compute(&dir).unwrap();

        let after = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(before.stdout, after.stdout, "status changed under us");
    }

    #[test]
    fn the_same_content_hashes_the_same_way_twice() {
        let (_t, dir) = scratch();
        assert_eq!(compute(&dir).unwrap().tree, compute(&dir).unwrap().tree);
    }

    #[test]
    fn resolve_tree_accepts_refs_and_rejects_nonsense() {
        let (_t, dir) = scratch();
        let head = resolve_tree(&dir, "HEAD").unwrap();
        assert_eq!(resolve_tree(&dir, "main").unwrap(), head);
        assert!(resolve_tree(&dir, "no-such-ref").is_err());
    }
}
