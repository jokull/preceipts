//! One run per worktree at a time.
//!
//! Two concurrent runs race each other's `[prepare]` mutations and duplicate
//! every check. That is not a theoretical hazard now that `watch` exists: a
//! quiet-triggered run and a person typing `preceipts run` are exactly the two
//! processes that meet, and prepare *writes to the worktree*. The second one
//! has to fail fast and say why.
//!
//! Ported from `engine/src/lib/runner.ts` at fc3643e, semantics intact.
//!
//! The lock lives in the worktree's own git dir, not the repository's, so
//! parallel worktrees of one repo stay independent — running checks in two
//! workspaces at once is the normal case, not a conflict.
//!
//! A lock left behind by a dead process — `kill -9`, a crash, a closed laptop
//! — is detected by pid and taken over. A stale lock file that could only be
//! cleared by hand would turn one crash into a permanently broken worktree.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "preceipts-run.lock";

/// Held for the duration of a run; releases on drop.
///
/// Drop rather than an explicit release, because the interesting paths are the
/// ones that do not reach the end of the function: a check that fails, a tree
/// that moved, an error anywhere in between. Every one of those must give the
/// lock back.
#[derive(Debug)]
pub struct RunLock {
    path: PathBuf,
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Is a process with this pid running?
///
/// Signal 0 performs the permission and existence checks without delivering
/// anything. `EPERM` means it exists and belongs to someone else, which still
/// counts as alive — treating it as dead would let two runs proceed.
fn is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The worktree's own git directory — `.git/worktrees/<name>` for a linked
/// worktree, `.git` for the primary one.
fn git_dir(root: &Path) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--absolute-git-dir"])
        .current_dir(root)
        .output()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("running git: {e}"))))?;
    if !output.status.success() {
        return Err(Error::NotARepo(root.to_path_buf()));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

/// Take the run lock for `root`, or explain who has it.
pub fn acquire(root: &Path) -> Result<RunLock> {
    let path = git_dir(root)?.join(LOCK_FILE);

    // Two attempts: the second exists only to take over a lock the first
    // found stale. A loop would spin against a live run rather than report it.
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                let _ = write!(file, "{}", std::process::id());
                return Ok(RunLock { path });
            }
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(Error::Git(git2::Error::from_str(&format!(
                    "taking the run lock at {}: {e}",
                    path.display()
                ))));
            }
            Err(_) => {}
        }

        let holder = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<i32>().ok())
            .unwrap_or(0);

        if is_alive(holder) {
            return Err(Error::Git(git2::Error::from_str(&format!(
                "another preceipts run is already in progress (pid {holder}) — a second \
                 run would race prepare and duplicate checks on this worktree; wait for \
                 it or kill it"
            ))));
        }
        // Stale: the holder is gone, or the file never held a readable pid.
        let _ = std::fs::remove_file(&path);
    }

    Err(Error::Git(git2::Error::from_str(
        "could not acquire the run lock",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        for args in [
            &["git", "init", "-q", "-b", "main"][..],
            &["git", "config", "user.name", "t"],
            &["git", "config", "user.email", "t@t.local"],
        ] {
            assert!(Command::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .status()
                .unwrap()
                .success());
        }
        (temp, dir)
    }

    #[test]
    fn a_second_run_is_refused_while_the_first_holds_the_lock() {
        let (_t, dir) = repo();
        let _first = acquire(&dir).expect("the first run takes the lock");
        let error = acquire(&dir)
            .expect_err("the second is refused")
            .to_string();
        assert!(error.contains("already in progress"), "{error}");
        assert!(
            error.contains("race prepare"),
            "the message says why it matters, not just that it happened: {error}"
        );
    }

    #[test]
    fn the_lock_is_released_when_the_run_ends_however_it_ends() {
        let (_t, dir) = repo();
        {
            let _lock = acquire(&dir).unwrap();
        }
        assert!(
            acquire(&dir).is_ok(),
            "a dropped lock is a released lock, including on the error paths"
        );
    }

    /// The failure mode that matters more than the contention it prevents: a
    /// crash must not leave a worktree permanently unable to run checks.
    #[test]
    fn a_lock_left_by_a_dead_process_is_taken_over() {
        let (_t, dir) = repo();
        let path = git_dir(&dir).unwrap().join(LOCK_FILE);
        // A pid that is not running. 999999 is above the default pid_max on
        // macOS and Linux alike, so it cannot collide with a live process.
        std::fs::write(&path, "999999").unwrap();
        assert!(
            acquire(&dir).is_ok(),
            "a stale lock is taken over rather than requiring manual cleanup"
        );
    }

    #[test]
    fn an_unreadable_lock_file_is_treated_as_stale() {
        let (_t, dir) = repo();
        let path = git_dir(&dir).unwrap().join(LOCK_FILE);
        std::fs::write(&path, "not a pid at all").unwrap();
        assert!(acquire(&dir).is_ok());
    }

    /// Two workspaces of one project run checks at the same time constantly —
    /// that is the point of workspaces — so their locks must not be the same.
    #[test]
    fn parallel_worktrees_of_one_repo_do_not_share_a_lock() {
        let (_t, dir) = repo();
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        assert!(Command::new("git")
            .args(["add", "-A"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["commit", "-qm", "base"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success());
        let workspace = crate::workspace::create(&dir, "other", None, None).unwrap();

        let _primary = acquire(&dir).expect("the primary takes its lock");
        assert!(
            acquire(&workspace.path).is_ok(),
            "a sibling worktree is not blocked by the primary's run"
        );
    }

    #[test]
    fn our_own_pid_is_alive_and_a_free_one_is_not() {
        assert!(is_alive(std::process::id() as i32));
        assert!(!is_alive(999_999));
        assert!(!is_alive(0), "pid 0 is not a process we can ask about");
    }
}
