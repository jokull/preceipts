//! Quiet detection: fire when the agent stops typing.
//!
//! This is the north-star feature of docs/direction-2026-08.md, and it is only
//! possible because one thing owns both the environment and the receipts. The
//! agent goes quiet, checks run in the already-warm workspace, and the answer
//! arrives without a push, a CI queue, or a context switch.
//!
//! It is deliberately built on **observation, not integration**. No harness has
//! to tell us anything: a worktree that stops changing is a worktree that
//! stopped being worked on, and that is true of every agent, every editor, and
//! every person. Where a harness *can* report "I am done", it should call the
//! CLI and trade this heuristic for an exact signal — but nothing depends on
//! its doing so.

use crate::error::{Error, Result};
use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long the tree must hold still before it counts as quiet.
///
/// Long enough to sit through a formatter or a multi-file edit, short enough
/// that the answer feels like a reaction rather than a cron job.
pub const DEFAULT_QUIET: Duration = Duration::from_secs(2);

/// Paths that change constantly and mean nothing about the work.
///
/// `.git` is the important one: every git command touches it, so watching it
/// means a `git status` in another terminal looks like an edit. Build output is
/// excluded for the same reason a receipt excludes it — it is derived.
fn is_noise(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    relative.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git" | "target" | "node_modules" | ".next" | "dist" | ".turbo" | ".preceipts"
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The tree changed and the quiet timer restarted.
    Busy,
    /// The tree has held still. This is the moment to run checks.
    Quiet,
}

/// Watch a worktree, calling `on_event` as it goes busy and quiet.
///
/// Blocks. `on_event` returning `false` stops the watch, which is how a caller
/// bows out after one cycle or on a signal.
pub fn watch<F>(root: &Path, quiet_after: Duration, mut on_event: F) -> Result<()>
where
    F: FnMut(Event) -> bool,
{
    let (tx, rx) = mpsc::channel();
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let watch_root = root.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        // Access-only events (a read, a stat) are not work.
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        if event.paths.iter().all(|p| is_noise(p, &watch_root)) {
            return;
        }
        let _ = tx.send(());
    })
    .map_err(|e| Error::Git(git2::Error::from_str(&format!("watching: {e}"))))?;

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| {
            Error::Git(git2::Error::from_str(&format!(
                "watching {}: {e}",
                root.display()
            )))
        })?;

    // `None` means "already quiet, nothing pending" — so a watch that starts on
    // a still tree does not immediately claim it just went quiet.
    let mut pending: Option<Instant> = None;

    loop {
        let timeout = match pending {
            Some(since) => quiet_after.saturating_sub(since.elapsed()),
            None => Duration::from_secs(60),
        };

        match rx.recv_timeout(timeout) {
            Ok(()) => {
                // Drain the burst: a single save is many events, and each one
                // should push the deadline out rather than queue a cycle.
                while rx.try_recv().is_ok() {}
                if pending.is_none() && !on_event(Event::Busy) {
                    return Ok(());
                }
                pending = Some(Instant::now());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(since) = pending {
                    if since.elapsed() >= quiet_after {
                        pending = None;
                        if !on_event(Event::Quiet) {
                            return Ok(());
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Paths under `root` that a watch would care about — exposed so callers can
/// explain themselves ("watching 412 files") without duplicating the rules.
pub fn watched_paths(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_noise(&path, root) {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn git_internals_and_build_output_are_not_work() {
        let root = Path::new("/repo");
        assert!(is_noise(Path::new("/repo/.git/index"), root));
        assert!(is_noise(Path::new("/repo/target/debug/thing"), root));
        assert!(is_noise(
            Path::new("/repo/apps/web/node_modules/x/y.js"),
            root
        ));
        assert!(is_noise(Path::new("/repo/.preceipts/workspace.toml"), root));
        assert!(!is_noise(Path::new("/repo/src/main.rs"), root));
        assert!(!is_noise(Path::new("/repo/apps/web/src/app.tsx"), root));
    }

    #[test]
    fn a_path_outside_the_root_is_ignored() {
        assert!(is_noise(
            Path::new("/elsewhere/file.rs"),
            Path::new("/repo")
        ));
    }

    #[test]
    fn watched_paths_skips_the_noise() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("target/debug/out"), "binary").unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: x").unwrap();

        let paths = watched_paths(root);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("src/main.rs"));
    }

    /// Wait for `predicate` to hold, or give up.
    ///
    /// Every assertion here is about a *thread* observing a filesystem event,
    /// which is the one thing a test cannot make happen on demand: FSEvents
    /// arms asynchronously, and on a loaded machine that takes longer than
    /// any sleep you would be willing to write. Polling with a deadline is
    /// the only honest shape.
    fn within(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    /// The behaviour that matters: an edit makes it busy, and quiet arrives
    /// only after the tree has actually held still.
    ///
    /// The watch thread is deliberately never joined. If the watcher somehow
    /// never fires, joining would hang the whole test binary until something
    /// outside kills it — which is exactly what happened before this was
    /// written this way: the suite ran until `preceipts run` timed out at ten
    /// minutes and reported a red `test` receipt with no failure in it. A
    /// leaked thread dies with the process; a joined one takes the process
    /// with it.
    #[test]
    fn an_edit_goes_busy_then_quiet() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "one").unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let writer_root = root.clone();

        std::thread::spawn(move || {
            watch(&root, Duration::from_millis(300), move |event| {
                seen.lock().unwrap().push(event.clone());
                // Stop once we have seen a full cycle.
                event != Event::Quiet
            })
        });

        // Keep touching the file until the watcher notices. One write after a
        // fixed sleep assumes the watcher armed in time, and when it has not,
        // the event is simply lost and nothing ever happens.
        let saw_busy = within(Duration::from_secs(20), || {
            let _ = std::fs::write(writer_root.join("a.txt"), "two");
            events.lock().unwrap().contains(&Event::Busy)
        });
        assert!(saw_busy, "the watcher never reported the edit");

        // Now stop touching it, and quiet must follow on its own.
        let saw_quiet = within(Duration::from_secs(20), || {
            events.lock().unwrap().contains(&Event::Quiet)
        });
        assert!(saw_quiet, "the tree went still but never went quiet");

        let events = events.lock().unwrap();
        assert_eq!(events[0], Event::Busy, "busy comes first: {events:?}");
        assert_eq!(
            events.last(),
            Some(&Event::Quiet),
            "and quiet ends the cycle: {events:?}"
        );
    }

    #[test]
    fn a_still_tree_never_claims_to_have_gone_quiet() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "one").unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);

        // Same reason as above: never joined.
        std::thread::spawn(move || {
            watch(&root, Duration::from_millis(100), move |event| {
                seen.lock().unwrap().push(event);
                true
            })
        });

        // A tree nobody touches, for well past the quiet threshold.
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            events.lock().unwrap().is_empty(),
            "a watch that starts on a still tree has nothing to report: {:?}",
            events.lock().unwrap()
        );
    }
}
