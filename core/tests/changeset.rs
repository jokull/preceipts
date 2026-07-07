//! Integration: Changeset::load against a scratch repo, both scopes.

use std::path::Path;
use std::process::Command;

use preceipts_core::{Changeset, DiffScope, FileStatus};

fn sh(dir: &Path, args: &[&str]) {
    let output = Command::new(args[0])
        .args(&args[1..])
        .current_dir(dir)
        .output()
        .expect("spawn");
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(dir: &Path, path: &str, content: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

/// main has a.txt + src/keep.rs; feature branch modifies a.txt, adds new.rs,
/// deletes src/keep.rs (committed), plus uncommitted edits on top.
fn scratch_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["git", "init", "-q", "-b", "main"]);
    sh(p, &["git", "config", "user.name", "t"]);
    sh(p, &["git", "config", "user.email", "t@t.local"]);
    sh(p, &["git", "config", "commit.gpgsign", "false"]);
    write(p, "a.txt", "one\ntwo\nthree\n");
    write(p, "src/keep.rs", "fn keep() {}\n");
    sh(p, &["git", "add", "-A"]);
    sh(p, &["git", "commit", "-qm", "base"]);
    sh(p, &["git", "checkout", "-qb", "feature"]);
    write(p, "a.txt", "one\nTWO\nthree\n");
    write(p, "new.rs", "fn new_thing() {}\n");
    sh(p, &["git", "rm", "-q", "src/keep.rs"]);
    sh(p, &["git", "add", "-A"]);
    sh(p, &["git", "commit", "-qm", "feature work"]);
    // Uncommitted on top: edit a.txt again + an untracked file.
    write(p, "a.txt", "one\nTWO\nthree\nfour\n");
    write(p, "untracked.md", "# notes\n");
    dir
}

#[test]
fn branch_scope_spans_commits_and_working_tree() {
    let dir = scratch_repo();
    let cs = Changeset::load(dir.path(), DiffScope::Branch).unwrap();

    assert_eq!(cs.info.branch.as_deref(), Some("feature"));
    assert!(cs.info.base_name.starts_with("main"));

    let paths: Vec<(&str, FileStatus)> = cs
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.status))
        .collect();
    assert!(paths.contains(&("a.txt", FileStatus::Modified)));
    assert!(paths.contains(&("new.rs", FileStatus::Added)));
    assert!(paths.contains(&("src/keep.rs", FileStatus::Deleted)));
    assert!(paths.contains(&("untracked.md", FileStatus::Added)));

    // a.txt: TWO changed + four added, relative to main's merge-base.
    let a = cs.files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!(a.added, 2);
    assert_eq!(a.removed, 1);

    // Deleted file renders removal rows.
    let keep = cs.files.iter().find(|f| f.path == "src/keep.rs").unwrap();
    assert_eq!(keep.removed, 1);
    assert_eq!(keep.added, 0);
}

#[test]
fn uncommitted_scope_sees_only_working_tree_changes() {
    let dir = scratch_repo();
    let cs = Changeset::load(dir.path(), DiffScope::Uncommitted).unwrap();

    let paths: Vec<&str> = cs.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"a.txt")); // the uncommitted "four" line
    assert!(paths.contains(&"untracked.md"));
    assert!(!paths.contains(&"new.rs")); // committed — not in this scope
    assert!(!paths.contains(&"src/keep.rs"));

    let a = cs.files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!((a.added, a.removed), (1, 0));
}

#[test]
fn binary_files_are_flagged_not_diffed() {
    let dir = scratch_repo();
    std::fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();
    let cs = Changeset::load(dir.path(), DiffScope::Uncommitted).unwrap();
    let bin = cs.files.iter().find(|f| f.path == "blob.bin").unwrap();
    assert!(bin.is_binary);
    assert!(bin.hunks.is_empty());
}
