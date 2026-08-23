//! Reading and appending receipts in `refs/notes/receipts`.
//!
//! Ported forward from `engine/src/lib/notes.ts` at fc3643e. The ref names are
//! part of the wire format — notes written by the TypeScript engine live under
//! these exact refs, and so must ours.

use crate::error::{Error, Result};
use crate::receipt::{self, Receipt};
use git2::{Oid, Repository, Signature};
use std::path::Path;

pub const NOTES_REF: &str = "refs/notes/receipts";
pub const NOTES_REMOTE_TRACKING_REF: &str = "refs/notes/remotes/origin/receipts";
/// Log blobs are pinned by a ref each, so `git gc` cannot collect a log that a
/// receipt still points at.
pub const LOG_REF_PREFIX: &str = "refs/receipts/logs/";

/// Raw note text for a tree, or `None` when the tree carries no note.
pub fn read_note(repo: &Repository, tree: &str) -> Option<String> {
    let oid = Oid::from_str(tree).ok()?;
    let note = repo.find_note(Some(NOTES_REF), oid).ok()?;
    note.message().map(str::to_string)
}

/// Every receipt recorded for a tree.
pub fn read_receipts(repo: &Repository, tree: &str) -> Vec<Receipt> {
    read_note(repo, tree)
        .map(|text| receipt::parse_all(&text))
        .unwrap_or_default()
}

/// Every receipt in the notes ref, across all trees. Used by `gc` and by any
/// caller that wants the whole picture rather than one tree's.
pub fn read_all_receipts(repo: &Repository) -> Vec<Receipt> {
    let Ok(notes) = repo.notes(Some(NOTES_REF)) else {
        return Vec::new(); // no notes ref yet is not an error
    };
    let mut out = Vec::new();
    for entry in notes.flatten() {
        let (_note_oid, annotated) = entry;
        if let Ok(note) = repo.find_note(Some(NOTES_REF), annotated) {
            if let Some(message) = note.message() {
                out.extend(receipt::parse_all(message));
            }
        }
    }
    out
}

/// Append one receipt line to a tree's note.
///
/// Matches `git notes append`: existing content, a blank separator line, then
/// the new line. Parsing skips blanks, so the separator is cosmetic — but a
/// note a human opens should look like the ones git itself writes.
pub fn append_receipt(repo: &Repository, tree: &str, line: &str) -> Result<()> {
    let oid = Oid::from_str(tree)?;
    let existing = read_note(repo, tree).unwrap_or_default();

    let mut message = String::new();
    if !existing.trim().is_empty() {
        message.push_str(existing.trim_end());
        message.push_str("\n\n");
    }
    message.push_str(line.trim_end());
    message.push('\n');

    let signature = signature(repo)?;
    repo.note(&signature, &signature, Some(NOTES_REF), oid, &message, true)?;
    Ok(())
}

/// Store a log as a blob and pin it with `refs/receipts/logs/<sha>`.
pub fn store_log(repo: &Repository, content: &[u8]) -> Result<String> {
    let oid = repo.blob(content)?;
    let sha = oid.to_string();
    repo.reference(
        &format!("{LOG_REF_PREFIX}{sha}"),
        oid,
        true,
        "preceipts log",
    )?;
    Ok(sha)
}

/// Read a stored log. `None` when the blob is gone — a pruned-then-collected
/// log is a normal end state, not a failure.
pub fn read_log(repo: &Repository, blob_sha: &str) -> Option<String> {
    let oid = Oid::from_str(blob_sha).ok()?;
    let blob = repo.find_blob(oid).ok()?;
    Some(String::from_utf8_lossy(blob.content()).into_owned())
}

/// Identity to stamp notes with. Falls back to a fixed identity rather than
/// failing: an unconfigured git is a reason to write an anonymous note, not a
/// reason to lose a check result.
fn signature(repo: &Repository) -> Result<Signature<'static>> {
    match repo.signature() {
        Ok(signature) => Ok(signature),
        Err(_) => Ok(Signature::now("preceipts", "preceipts@localhost")?),
    }
}

/// The runner identity recorded in a receipt.
pub fn runner_identity(repo: &Repository) -> crate::receipt::Runner {
    let config = repo.config().ok();
    let get = |key: &str| -> String {
        config
            .as_ref()
            .and_then(|c| c.get_string(key).ok())
            .unwrap_or_default()
    };
    crate::receipt::Runner {
        name: get("user.name"),
        email: get("user.email"),
        host: hostname(),
        // Agents identify themselves; we do not guess.
        agent: std::env::var("PRECEIPTS_AGENT")
            .ok()
            .filter(|a| !a.is_empty()),
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Open the repository at `path` for receipt work.
pub fn open(path: &Path) -> Result<Repository> {
    Repository::discover(path).map_err(|_| crate::error::Error::NotARepo(path.to_path_buf()))
}

/// Watch for receipts arriving, calling `on_change` when the notes ref moves.
///
/// This is the second half of the north-star loop, and it exists because of a
/// deliberate exclusion elsewhere: [`crate::watch`] ignores `.git` outright,
/// since every git command touches it and a `git status` in another terminal
/// would otherwise read as an edit. Receipts are git notes. So a check that
/// passes in a terminal writes into exactly the directory the tree watch
/// throws away, and without this the verdict would sit at "not run" *after*
/// the checks went green — the north-star moment, displayed wrong.
///
/// Blocks, like the other watches. `on_change` returning `false` ends it.
pub fn watch_receipts<F>(worktree: &Path, mut on_change: F) -> Result<()>
where
    F: FnMut() -> bool,
{
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::Duration;

    // Refs are common to every worktree, so this is the *main* git dir even
    // when the pane is looking at a linked one.
    let git_dir = crate::workspace::common_git_dir(worktree)?;
    let notes_dir = git_dir.join("refs/notes");

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let _ = tx.send(());
    })
    .map_err(|e| Error::Git(git2::Error::from_str(&format!("watching: {e}"))))?;

    // Non-recursive on the git dir catches `packed-refs`, which is where the
    // ref lives after a `git gc` — and where it lives *only* until the next
    // write makes it loose again. Both forms are ordinary, so both are
    // watched, and `refs/notes/` is attached separately because on a
    // repository that has never recorded a receipt it does not exist yet.
    watcher
        .watch(&git_dir, RecursiveMode::NonRecursive)
        .map_err(|e| {
            Error::Git(git2::Error::from_str(&format!(
                "watching {}: {e}",
                git_dir.display()
            )))
        })?;
    let mut watching_notes =
        notes_dir.is_dir() && watcher.watch(&notes_dir, RecursiveMode::Recursive).is_ok();

    loop {
        if rx.recv().is_err() {
            return Ok(());
        }
        // `git notes append` writes a lock file, the object, then the ref.
        // Settling means one reload per receipt rather than three.
        while rx.recv_timeout(Duration::from_millis(120)).is_ok() {}

        if !watching_notes && notes_dir.is_dir() {
            watching_notes = watcher.watch(&notes_dir, RecursiveMode::Recursive).is_ok();
        }
        if !on_change() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::parse_line;
    use std::process::Command;

    const REAL: &str = r#"{"v":1,"check":"fmt","cmd":".preceipts/checks/fmt","tree":"2b8f46c30dfbbe65142b0f6aba24ebb95b04d432","ok":true,"exit":0,"started":"2026-07-06T17:26:02Z","duration_ms":284,"runner":{"name":"t","email":"t@t.local","host":"h"},"dirty":false,"log":"blob:e69de29bb2d1d6434b8b29ae775ad8c2e48c5391","check_blob":"abc"}"#;

    fn scratch() -> (tempfile::TempDir, Repository) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        for args in [
            vec!["git", "init", "-q", "-b", "main"],
            vec!["git", "config", "user.name", "t"],
            vec!["git", "config", "user.email", "t@t.local"],
            vec!["git", "config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .status()
                .unwrap()
                .success());
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        for args in [
            vec!["git", "add", "-A"],
            vec!["git", "commit", "-qm", "base"],
        ] {
            assert!(Command::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .status()
                .unwrap()
                .success());
        }
        let repo = Repository::open(&dir).unwrap();
        (temp, repo)
    }

    fn head_tree(repo: &Repository) -> String {
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree_id()
            .to_string()
    }

    #[test]
    fn a_tree_with_no_note_reads_as_empty_not_an_error() {
        let (_t, repo) = scratch();
        let tree = head_tree(&repo);
        assert!(read_note(&repo, &tree).is_none());
        assert!(read_receipts(&repo, &tree).is_empty());
        assert!(read_all_receipts(&repo).is_empty());
    }

    #[test]
    fn receipts_append_rather_than_replace() {
        let (_t, repo) = scratch();
        let tree = head_tree(&repo);

        let mut first = parse_line(REAL).unwrap();
        first.check = "fmt".into();
        first.tree = tree.clone();
        let mut second = parse_line(REAL).unwrap();
        second.check = "test".into();
        second.tree = tree.clone();

        append_receipt(&repo, &tree, &first.encode()).unwrap();
        append_receipt(&repo, &tree, &second.encode()).unwrap();

        let receipts = read_receipts(&repo, &tree);
        assert_eq!(receipts.len(), 2, "the first receipt survived the second");
        let names: Vec<&str> = receipts.iter().map(|r| r.check.as_str()).collect();
        assert_eq!(names, ["fmt", "test"]);
    }

    /// The note must be readable by git itself, not only by us — receipts are
    /// meant to be inspectable with plain git when this tool is not around.
    #[test]
    fn git_can_read_what_we_wrote() {
        let (temp, repo) = scratch();
        let tree = head_tree(&repo);
        let mut receipt = parse_line(REAL).unwrap();
        receipt.tree = tree.clone();
        append_receipt(&repo, &tree, &receipt.encode()).unwrap();

        let out = Command::new("git")
            .args(["notes", &format!("--ref={NOTES_REF}"), "show", &tree])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "git notes show failed");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("\"check\":\"fmt\""), "git read: {text}");
    }

    #[test]
    fn logs_round_trip_and_are_pinned() {
        let (_t, repo) = scratch();
        let sha = store_log(&repo, b"check output\n").unwrap();
        assert_eq!(read_log(&repo, &sha).as_deref(), Some("check output\n"));
        assert!(
            repo.find_reference(&format!("{LOG_REF_PREFIX}{sha}"))
                .is_ok(),
            "the log blob is pinned by a ref so gc cannot take it"
        );
    }

    #[test]
    fn a_missing_log_reads_as_none() {
        let (_t, repo) = scratch();
        assert!(read_log(&repo, "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391").is_none());
        assert!(read_log(&repo, "not-a-sha").is_none());
    }

    #[test]
    fn read_all_spans_every_tree() {
        let (_t, repo) = scratch();
        let tree = head_tree(&repo);
        let mut receipt = parse_line(REAL).unwrap();
        receipt.tree = tree.clone();
        append_receipt(&repo, &tree, &receipt.encode()).unwrap();
        assert_eq!(read_all_receipts(&repo).len(), 1);
    }

    /// Read this repository's own receipts, minted by the TypeScript engine in
    /// July 2026. This is the strongest conformance test available: real notes,
    /// written by the tool being replaced, read by its replacement.
    ///
    /// Skips rather than fails when run outside a checkout that carries them —
    /// a clone without the notes ref is a normal state, not a broken build.
    #[test]
    fn reads_this_repositorys_own_historical_receipts() {
        let Ok(repo) = Repository::discover(env!("CARGO_MANIFEST_DIR")) else {
            return;
        };
        let receipts = read_all_receipts(&repo);
        if receipts.is_empty() {
            return; // notes ref not present in this clone
        }
        assert!(
            receipts.iter().all(|r| r.v == 1 && !r.tree.is_empty()),
            "every historical receipt parses with a tree"
        );
        assert!(
            receipts.iter().any(|r| r.check == "fmt"),
            "the fmt check's receipts are still readable"
        );
    }
}
