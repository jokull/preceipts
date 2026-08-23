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
//! Holding is an advisory `flock`, not a pid written to a file. A pid can be
//! reused: the holder dies to `kill -9` or a closed laptop, the number comes
//! back around on some unrelated process, and every later run reads the lock as
//! live forever with only a manual delete to clear it. The kernel drops an
//! `flock` when the holding process dies, whatever killed it, and there is no
//! number left to misread.

use crate::error::{Error, Result};
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "preceipts-run.lock";

/// Held for the duration of a run; releases on drop.
///
/// Drop rather than an explicit release, because the interesting paths are the
/// ones that do not reach the end of the function: a check that fails, a tree
/// that moved, an error anywhere in between. Every one of those must give the
/// lock back.
///
/// `_file` is the lock. `flock` binds to the open file description, so closing
/// this handle *is* the release — dropping it early, or holding only the path,
/// would hand the worktree to a second run mid-prepare.
#[derive(Debug)]
pub struct RunLock {
    _file: File,
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

    // No `.truncate(true)`: truncation happens at open, before we know whether
    // anyone holds the lock, so a refused contender would erase the live
    // holder's pid and the message below would name nobody.
    #[allow(clippy::suspicious_open_options)]
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .map_err(|e| {
            Error::Git(git2::Error::from_str(&format!(
                "taking the run lock at {}: {e}",
                path.display()
            )))
        })?;

    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let errno = std::io::Error::last_os_error();
        if errno.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(Error::Git(git2::Error::from_str(&format!(
                "taking the run lock at {}: {errno}",
                path.display()
            ))));
        }
        // The pid is not what refused us — the kernel did. It is here so the
        // person reading this has something to wait for or kill.
        let holder = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<i32>().ok())
            .unwrap_or(0);
        return Err(Error::Git(git2::Error::from_str(&format!(
            "another preceipts run is already in progress (pid {holder}) — a second \
             run would race prepare and duplicate checks on this worktree; wait for \
             it or kill it"
        ))));
    }

    // Truncate now that the lock is ours, so a short pid written over a longer
    // stale one does not leave the tail behind as digits.
    use std::io::Write;
    let _ = file.set_len(0);
    let _ = write!(&file, "{}", std::process::id());

    Ok(RunLock { _file: file })
}

// The file is deliberately never unlinked, not even on a clean drop. Unlinking
// races: a waiting process has already opened this inode and is about to lock
// it, we remove the name, a third process creates a fresh file at the same path
// and locks that — two holders, two inodes, one worktree. An empty lock file
// left in the git dir costs nothing; the lock it carries is the fd, not the
// name.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::Command;

    /// Env var naming the worktree the helper below should lock.
    const HELPER_ROOT: &str = "PRECEIPTS_RUNLOCK_HELPER_ROOT";

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
            error.contains(&format!("pid {}", std::process::id())),
            "the message names someone to wait for or kill: {error}"
        );
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

    /// Not a test: the child half of the one below, run as its own process so
    /// that killing it is a real death rather than a simulated one. Inert
    /// unless the parent asks for it by name.
    #[test]
    #[ignore = "helper process for a_lock_whose_holder_was_killed_is_acquirable"]
    fn runlock_helper_holds_the_lock_until_killed() {
        let Ok(root) = std::env::var(HELPER_ROOT) else {
            return;
        };
        let _lock = acquire(Path::new(&root)).expect("the helper takes the lock");
        println!("HOLDING {}", std::process::id());
        use std::io::Write;
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    /// The failure mode that matters more than the contention it prevents: a
    /// crash must not leave a worktree permanently unable to run checks — and
    /// with no liveness check left, the pid in the file could belong to
    /// anything by now, including a live process that reused it.
    #[test]
    fn a_lock_whose_holder_was_killed_is_acquirable_without_asking_whether_its_pid_lives() {
        let (_t, dir) = repo();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runlock::tests::runlock_helper_holds_the_lock_until_killed",
                "--ignored",
                "--nocapture",
            ])
            .env(HELPER_ROOT, &dir)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawning the holder");

        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let held = lines
            .find_map(|line| {
                let line = line.ok()?;
                line.strip_prefix("HOLDING ")?.trim().parse::<i32>().ok()
            })
            .expect("the holder announces itself once the lock is its own");

        assert!(
            acquire(&dir).is_err(),
            "while the holder lives, the lock is the holder's"
        );

        // SIGKILL leaves no chance to clean up — no drop, no unlink, nothing
        // written. Exactly the crash the old pid file could not recover from.
        assert_eq!(unsafe { libc::kill(held, libc::SIGKILL) }, 0);
        child
            .wait()
            .expect("the holder is gone, not just signalled");

        let lock = acquire(&dir).expect("the kernel released what the dead process held");
        let path = git_dir(&dir).unwrap().join(LOCK_FILE);
        drop(lock);
        assert!(
            std::fs::read_to_string(&path).unwrap().trim() != held.to_string(),
            "the file was rewritten by the new holder"
        );
    }

    #[test]
    fn a_lock_file_left_behind_with_no_readable_pid_is_still_acquirable() {
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
}
