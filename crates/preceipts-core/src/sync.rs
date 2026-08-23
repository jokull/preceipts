//! `sync` and `gc`: sharing receipts, and letting their logs age out.
//!
//! Ported from `engine/src/lib/sync.ts` and `gc.ts` at fc3643e.
//!
//! Both shell out to `git` rather than driving libgit2. `notes merge
//! --strategy=cat_sort_uniq` has no libgit2 equivalent, and it is the whole
//! reason sync is safe: two people minting receipts for the same tree produce
//! a note each, and cat_sort_uniq keeps both lines instead of picking a
//! winner. Reimplementing that merge would be reimplementing the one part
//! nobody should have to trust us about.

use crate::error::{Error, Result};
use crate::notes::{LOG_REF_PREFIX, NOTES_REF, NOTES_REMOTE_TRACKING_REF};
use std::path::Path;
use std::process::Command;

/// Logs of passing checks: nobody reads them, and they are the bulk.
pub const DEFAULT_KEEP_SUCCESS: u64 = 30 * 86_400;
/// Logs of failing checks live longer, because those are the ones someone
/// comes back to.
pub const DEFAULT_KEEP_FAILURE: u64 = 90 * 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub merged: bool,
    pub pushed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Log blobs whose refs were removed.
    pub deleted: Vec<String>,
    pub kept: usize,
    /// Log refs no receipt references. Left alone — see `gc`.
    pub orphans: usize,
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("running git {args:?}: {e}"))))
}

fn git_ok(root: &Path, args: &[&str]) -> Result<String> {
    let output = git(root, args)?;
    if !output.status.success() {
        return Err(Error::Git(git2::Error::from_str(&format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn rev_parse(root: &Path, reference: &str) -> Option<String> {
    let output = git(root, &["rev-parse", "--verify", "--quiet", reference]).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn log_refs(root: &Path) -> Result<Vec<String>> {
    let pattern = format!("{LOG_REF_PREFIX}*");
    let out = git_ok(root, &["for-each-ref", "--format=%(refname)", &pattern])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(LOG_REF_PREFIX))
        .map(str::to_string)
        .collect())
}

/// Fetch receipts from origin, merge them losslessly, push ours back.
pub fn sync(root: &Path) -> Result<SyncReport> {
    if rev_parse(root, "refs/remotes/origin/HEAD").is_none()
        && git_ok(root, &["remote"])?
            .lines()
            .all(|remote| remote.trim() != "origin")
    {
        return Err(Error::Git(git2::Error::from_str(
            "no \"origin\" remote configured — nothing to sync with",
        )));
    }

    // Log refs are content-addressed — the ref name *is* the blob sha — so a
    // wildcard fetch can never conflict. Nothing to merge, ever.
    let logs_spec = format!("{LOG_REF_PREFIX}*:{LOG_REF_PREFIX}*");
    let _ = git(root, &["fetch", "origin", &logs_spec])?;

    // The notes ref lands in a tracking ref and is merged deliberately. Never
    // fetched directly: that would overwrite local receipts with remote ones.
    let notes_spec = format!("+{NOTES_REF}:{NOTES_REMOTE_TRACKING_REF}");
    let fetch = git(root, &["fetch", "origin", &notes_spec])?;
    let stderr = String::from_utf8_lossy(&fetch.stderr).to_lowercase();
    let remote_has_notes = fetch.status.success();
    if !remote_has_notes && !stderr.contains("couldn't find remote ref") {
        return Err(Error::Git(git2::Error::from_str(&format!(
            "fetching {NOTES_REF} from origin failed: {}",
            String::from_utf8_lossy(&fetch.stderr).trim()
        ))));
    }

    let mut merged = false;
    if remote_has_notes {
        let local = rev_parse(root, NOTES_REF);
        let remote = rev_parse(root, NOTES_REMOTE_TRACKING_REF);
        match (&local, &remote) {
            (None, Some(remote)) => {
                // Nothing local to lose, so take theirs wholesale.
                git_ok(root, &["update-ref", NOTES_REF, remote])?;
                merged = true;
            }
            (Some(local), Some(remote)) if local != remote => {
                let notes_ref = format!("--ref={NOTES_REF}");
                let result = git(
                    root,
                    &[
                        "notes",
                        &notes_ref,
                        "merge",
                        "--strategy=cat_sort_uniq",
                        NOTES_REMOTE_TRACKING_REF,
                    ],
                )?;
                if !result.status.success() {
                    return Err(Error::Git(git2::Error::from_str(&format!(
                        "notes merge (cat_sort_uniq) failed:\n{}\nresolve with `git notes \
                         --ref={NOTES_REF} merge --abort` or inspect \
                         .git/NOTES_MERGE_WORKTREE",
                        String::from_utf8_lossy(&result.stderr).trim()
                    ))));
                }
                merged = true;
            }
            _ => {}
        }
    }

    let mut refspecs: Vec<String> = Vec::new();
    let local_notes = rev_parse(root, NOTES_REF);
    if local_notes.is_some() {
        refspecs.push(format!("{NOTES_REF}:{NOTES_REF}"));
    }
    if !log_refs(root)?.is_empty() {
        refspecs.push(logs_spec.clone());
    }

    let mut pushed = false;
    if !refspecs.is_empty() {
        let mut args: Vec<&str> = vec!["push", "origin"];
        args.extend(refspecs.iter().map(String::as_str));
        git_ok(root, &args)?;
        pushed = true;
        // The push succeeded, so origin's notes now equal ours. Advancing the
        // tracking ref keeps "unsynced receipts" honest immediately rather
        // than until the next fetch.
        if let Some(local) = &local_notes {
            git_ok(root, &["update-ref", NOTES_REMOTE_TRACKING_REF, local])?;
        }
    }

    Ok(SyncReport { merged, pushed })
}

/// Delete log refs whose receipts have aged out.
///
/// Receipt *lines* are permanent and outlive their logs: the proof that a
/// check passed on a tree is small and worth keeping forever, while the
/// captured output behind it is large and stops being interesting.
///
/// A log ref no receipt references is counted and left alone. It is more
/// likely evidence of a bug here than garbage, and deleting data we cannot
/// explain is the wrong instinct for a tool whose whole job is evidence.
pub fn gc(root: &Path, keep_success: u64, keep_failure: u64, now: i64) -> Result<GcReport> {
    use std::collections::HashMap;

    let repo = crate::notes::open(root)?;
    let receipts = crate::notes::read_all_receipts(&repo);

    // A log is kept while *any* receipt referencing it is still in date —
    // the same output can back several receipts, and the longest-lived one
    // decides.
    let mut expiry: HashMap<String, i64> = HashMap::new();
    for receipt in &receipts {
        let Some(sha) = receipt.log_blob() else {
            continue;
        };
        let Some(started) = parse_iso8601(&receipt.started) else {
            continue;
        };
        let window = if receipt.ok {
            keep_success
        } else {
            keep_failure
        } as i64;
        let expires = started + window;
        let entry = expiry.entry(sha.to_string()).or_insert(expires);
        *entry = (*entry).max(expires);
    }

    let mut report = GcReport::default();
    for refname in log_refs(root)? {
        let sha = refname.trim_start_matches(LOG_REF_PREFIX).to_string();
        match expiry.get(&sha) {
            None => report.orphans += 1,
            Some(expires) if *expires <= now => {
                git_ok(root, &["update-ref", "-d", &refname])?;
                report.deleted.push(sha);
            }
            Some(_) => report.kept += 1,
        }
    }
    Ok(report)
}

/// Seconds since the epoch for the `YYYY-MM-DDTHH:MM:SSZ` receipts use.
///
/// Hand-rolled to keep the dependency list honest: this is the only date
/// *parsing* in the crate, `checks.rs` already hand-rolls the formatting, and
/// receipts only ever carry this one shape.
fn parse_iso8601(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Days from civil, Howard Hinnant's algorithm — the same one `checks.rs`
    // uses in the other direction, so a receipt written by one is read by the
    // other without a rounding disagreement.
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_shift = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_shift + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned against the formatter rather than against constants computed by
    /// hand — the two directions have to agree, and a hand-computed epoch
    /// second proves only that I can do arithmetic. `gc` compares a parsed
    /// receipt timestamp against a retention window, so a one-day drift
    /// between writer and reader would silently delete logs early.
    #[test]
    fn parsing_inverts_the_formatting_receipts_are_written_with() {
        for secs in [
            0_i64,
            1_783_168_245, // 2026-07-04T12:30:45Z
            1_751_824_651, // a real receipt in this repo's notes
            951_782_400,   // 2000-02-29, a leap day in a leap century
            1_078_012_800, // 2004-02-29, an ordinary leap day
            4_102_444_800, // 2100-01-01, not a leap year
        ] {
            let (y, mo, d, h, mi, s) = crate::checks::civil_from_unix(secs);
            let text = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z");
            assert_eq!(
                parse_iso8601(&text),
                Some(secs),
                "{text} did not round trip"
            );
        }
    }

    #[test]
    fn anything_that_is_not_a_receipt_timestamp_is_none() {
        assert_eq!(parse_iso8601("not a date"), None);
        assert_eq!(parse_iso8601("2026-13-01T00:00:00Z"), None, "month 13");
        assert_eq!(parse_iso8601("2026-01-32T00:00:00Z"), None, "day 32");
        assert_eq!(parse_iso8601(""), None);
        assert_eq!(parse_iso8601("2026-07-04"), None, "date without a time");
    }

    fn project() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        for args in [
            &["git", "init", "-q", "-b", "main"][..],
            &["git", "config", "user.name", "t"],
            &["git", "config", "user.email", "t@t.local"],
            &["git", "config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .status()
                .unwrap()
                .success());
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        for args in [&["git", "add", "-A"][..], &["git", "commit", "-qm", "base"]] {
            assert!(Command::new(args[0])
                .args(&args[1..])
                .current_dir(&dir)
                .status()
                .unwrap()
                .success());
        }
        (temp, dir)
    }

    /// Mint a receipt with a stored log, the way `run` does.
    fn mint(root: &std::path::Path, check: &str, ok: bool, started: &str) -> String {
        let repo = crate::notes::open(root).unwrap();
        let tree = crate::treehash::compute(root).unwrap().tree;
        let log = crate::notes::store_log(&repo, format!("{check} output").as_bytes()).unwrap();
        let receipt = crate::receipt::Receipt {
            v: 1,
            check: check.to_string(),
            cmd: format!(".preceipts/checks/{check}"),
            tree: tree.clone(),
            ok,
            exit: if ok { 0 } else { 1 },
            started: started.to_string(),
            duration_ms: 10,
            runner: Default::default(),
            dirty: false,
            log: format!("blob:{log}"),
            check_blob: String::new(),
            fidelity: Default::default(),
        };
        crate::notes::append_receipt(&repo, &tree, &receipt.encode()).unwrap();
        log
    }

    /// Success and failure logs age out on different schedules, and the
    /// receipts themselves are never touched.
    #[test]
    fn gc_expires_success_logs_before_failure_logs_and_keeps_every_receipt() {
        let (_t, dir) = project();
        let day = 86_400_i64;
        // Both ran 45 days ago: past the 30-day success window, inside the
        // 90-day failure window.
        let long_ago = "2026-01-01T00:00:00Z";
        let now = parse_iso8601(long_ago).unwrap() + 45 * day;
        let passed = mint(&dir, "green", true, long_ago);
        let failed = mint(&dir, "red", false, long_ago);

        let report = gc(&dir, DEFAULT_KEEP_SUCCESS, DEFAULT_KEEP_FAILURE, now).unwrap();
        assert_eq!(report.deleted, vec![passed], "the passing log aged out");
        assert_eq!(report.kept, 1, "the failing log is still worth reading");
        assert_eq!(report.orphans, 0);

        let repo = crate::notes::open(&dir).unwrap();
        assert_eq!(
            crate::notes::read_all_receipts(&repo).len(),
            2,
            "receipt lines are permanent; only their logs age out"
        );
        assert!(
            repo.find_reference(&format!("{LOG_REF_PREFIX}{failed}"))
                .is_ok(),
            "the kept log ref is still there"
        );
    }

    #[test]
    fn gc_leaves_recent_logs_alone() {
        let (_t, dir) = project();
        let started = "2026-01-01T00:00:00Z";
        let now = parse_iso8601(started).unwrap() + 86_400;
        mint(&dir, "green", true, started);
        let report = gc(&dir, DEFAULT_KEEP_SUCCESS, DEFAULT_KEEP_FAILURE, now).unwrap();
        assert!(report.deleted.is_empty());
        assert_eq!(report.kept, 1);
    }

    /// A log nothing references is counted, never deleted. It is more likely
    /// a bug in us than garbage, and a tool whose job is evidence should not
    /// delete what it cannot explain.
    #[test]
    fn an_unreferenced_log_is_reported_rather_than_removed() {
        let (_t, dir) = project();
        let repo = crate::notes::open(&dir).unwrap();
        let orphan = crate::notes::store_log(&repo, b"nobody references this").unwrap();

        let report = gc(&dir, 0, 0, i64::MAX).unwrap();
        assert_eq!(report.orphans, 1);
        assert!(report.deleted.is_empty());
        assert!(
            repo.find_reference(&format!("{LOG_REF_PREFIX}{orphan}"))
                .is_ok(),
            "still there"
        );
    }

    #[test]
    fn a_repo_without_an_origin_is_told_so_rather_than_failing_obscurely() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        assert!(Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        let error = sync(dir).unwrap_err().to_string();
        assert!(error.contains("no \"origin\" remote"), "{error}");
    }
}
