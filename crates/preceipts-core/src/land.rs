//! Squash-landing a branch, receipts willing.
//!
//! Ported forward from `engine/src/lib/land.ts` at fc3643e.
//!
//! The move that makes the whole idea work: the landed commit is created with
//! `commit-tree <branch-tree> -p <base-head>`, so the commit's tree **is** the
//! proven tree. "Are we landing what we tested?" becomes a hash comparison
//! rather than a policy — which is only possible because receipts are keyed to
//! trees in the first place.

use crate::checks::{status_with, CheckState, DefinitionSource, Status};
use crate::error::{Error, Result};
use crate::notes::{LOG_REF_PREFIX, NOTES_REF};
use crate::receipt::Receipt;
use std::path::Path;
use std::process::Command;

/// The base moved past the merge-base, so the squashed tree would combine
/// proven content with unproven content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleInfo {
    /// Where the branch's receipts were minted from.
    pub merge_base: String,
    /// Where the base points now.
    pub base_head: String,
}

#[derive(Debug, Clone, Default)]
pub struct LandOptions {
    pub onto: Option<String>,
    pub no_push: bool,
    /// Land anyway when the base has moved, recording a `Receipts-Stale`
    /// trailer.
    pub allow_stale: bool,
    /// Make a moved base a hard failure — for scripts and agents that must not
    /// guess.
    pub require_fresh: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LandResult {
    pub branch: String,
    pub base: String,
    pub tree: String,
    pub commit: String,
    pub stale: Option<StaleInfo>,
    pub pushed: bool,
    pub message: String,
    pub status: Status,
}

/// Millisecond durations the way the trailers read them: 9s, 48s, 3m12s, 1h4m.
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let total_seconds = (ms as f64 / 1000.0).round() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        return if minutes > 0 {
            format!("{hours}h{minutes}m")
        } else {
            format!("{hours}h")
        };
    }
    if minutes > 0 {
        return if seconds > 0 {
            format!("{minutes}m{seconds}s")
        } else {
            format!("{minutes}m")
        };
    }
    format!("{seconds}s")
}

/// The commit trailers that carry the evidence into history.
///
/// These are the durable record: long after the notes ref is gone from a
/// clone, the commit still says what was proven, by whom, and on which tree.
pub fn build_trailers(receipts: &[Receipt], tree: &str, stale: Option<&StaleInfo>) -> Vec<String> {
    let mut trailers = Vec::new();

    let summary = receipts
        .iter()
        .map(|r| {
            format!(
                "{} {} {}",
                r.check,
                if r.ok { "✓" } else { "✗" },
                format_duration(r.duration_ms)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if !summary.is_empty() {
        trailers.push(format!("Receipts: {summary}"));
    }

    trailers.push(format!("Receipts-Tree: {tree}"));

    if let Some(latest) = receipts.iter().max_by(|a, b| a.started.cmp(&b.started)) {
        let local_part = latest
            .runner
            .email
            .split('@')
            .next()
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if latest.runner.name.is_empty() {
                    "unknown".to_string()
                } else {
                    latest.runner.name.clone()
                }
            });
        let agent = latest
            .runner
            .agent
            .as_deref()
            .map(|a| format!(" ({})", a.split('/').next().unwrap_or(a)))
            .unwrap_or_default();
        trailers.push(format!(
            "Receipts-Runner: {local_part}@{}{agent}",
            latest.runner.host
        ));
    }

    if let Some(stale) = stale {
        trailers.push(format!(
            "Receipts-Stale: base moved {}→{}",
            &stale.merge_base[..12.min(stale.merge_base.len())],
            &stale.base_head[..12.min(stale.base_head.len())]
        ));
    }

    trailers
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("git {args:?}: {e}"))))?;
    if !output.status.success() {
        return Err(Error::Git(git2::Error::from_str(&format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn rev_parse(root: &Path, reference: &str) -> Option<String> {
    git(root, &["rev-parse", "--verify", "--quiet", reference]).ok()
}

pub fn land(root: &Path, branch: &str, options: &LandOptions) -> Result<LandResult> {
    let base = options.onto.clone().unwrap_or_else(|| "main".to_string());

    if rev_parse(root, branch).is_none() {
        return Err(Error::Git(git2::Error::from_str(&format!(
            "cannot resolve branch \"{branch}\""
        ))));
    }
    let base_head = rev_parse(root, &format!("refs/heads/{base}")).ok_or_else(|| {
        Error::Git(git2::Error::from_str(&format!(
            "base branch \"{base}\" does not exist locally"
        )))
    })?;

    // Judge receipts against the branch tree's *own* check definitions: a
    // branch that legitimately changes a check script is still self-consistent
    // evidence for itself.
    let status = status_with(root, Some(branch), DefinitionSource::Tree)?;
    let required: Vec<_> = status.rows.iter().filter(|row| row.required).collect();
    if required.is_empty() {
        return Err(Error::Git(git2::Error::from_str(
            "no required checks configured in .preceipts/config.toml — \
             nothing to verify, refusing to land",
        )));
    }
    if !status.green {
        let problems = required
            .iter()
            .filter(|row| row.state != CheckState::Ok)
            .map(|row| format!("  {}: {}", row.check, row.state.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(Error::Git(git2::Error::from_str(&format!(
            "required receipts are not green for {branch} (tree {}):\n{problems}\n\
             run `preceipts run` on that tree, or check `preceipts status --ref {branch}`",
            &status.tree[..12.min(status.tree.len())]
        ))));
    }

    let merge_base = git(root, &["merge-base", &base, branch]).map_err(|e| {
        Error::Git(git2::Error::from_str(&format!(
            "cannot find a merge-base between \"{base}\" and \"{branch}\": {e}"
        )))
    })?;

    let mut stale = None;
    if base_head != merge_base {
        let info = StaleInfo {
            merge_base: merge_base.clone(),
            base_head: base_head.clone(),
        };
        if options.require_fresh {
            return Err(Error::Git(git2::Error::from_str(&format!(
                "base \"{base}\" moved since receipts were minted ({}→{}) — \
                 rebase & re-run (--require-fresh set)",
                &info.merge_base[..12],
                &info.base_head[..12]
            ))));
        }
        if !options.allow_stale {
            return Err(Error::Git(git2::Error::from_str(&format!(
                "base \"{base}\" moved since receipts were minted ({}→{}) — \
                 the squashed tree would be unproven.\n\
                 Rebase & re-run, or pass --allow-stale to land anyway (records a \
                 Receipts-Stale trailer). --require-fresh makes this a hard failure \
                 for scripts.",
                &info.merge_base[..12],
                &info.base_head[..12]
            ))));
        }
        stale = Some(info);
    }

    let receipts: Vec<Receipt> = status
        .rows
        .iter()
        .filter_map(|r| r.receipt.clone())
        .collect();
    let trailers = build_trailers(&receipts, &status.tree, stale.as_ref());
    let subject = options
        .message
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{branch} (squash)"));
    let message = format!("{subject}\n\n{}\n", trailers.join("\n"));

    // The commit's tree IS the proven tree. That is the whole point.
    let commit = commit_tree(root, &status.tree, &base_head, &message)?;

    // Fast-forward the base. When base is checked out here, go through merge
    // so the worktree and index follow; otherwise move the ref directly, with
    // the old value as a guard against concurrent movement.
    let head_ref = git(root, &["symbolic-ref", "--quiet", "HEAD"]).unwrap_or_default();
    if head_ref == format!("refs/heads/{base}") {
        git(root, &["merge", "--ff-only", &commit])?;
    } else {
        git(
            root,
            &[
                "update-ref",
                &format!("refs/heads/{base}"),
                &commit,
                &base_head,
            ],
        )?;
    }

    let mut pushed = false;
    if !options.no_push && has_remote(root, "origin") {
        let mut refspecs = vec![format!("refs/heads/{base}:refs/heads/{base}")];
        if rev_parse(root, NOTES_REF).is_some() {
            refspecs.push(format!("{NOTES_REF}:{NOTES_REF}"));
        }
        let log_refs = git(
            root,
            &[
                "for-each-ref",
                "--format=%(refname)",
                &format!("{LOG_REF_PREFIX}*"),
            ],
        )
        .unwrap_or_default();
        if !log_refs.trim().is_empty() {
            refspecs.push(format!("{LOG_REF_PREFIX}*:{LOG_REF_PREFIX}*"));
        }
        let mut args = vec!["push", "origin"];
        args.extend(refspecs.iter().map(String::as_str));
        git(root, &args)?;
        pushed = true;
    }

    Ok(LandResult {
        branch: branch.to_string(),
        base,
        tree: status.tree.clone(),
        commit,
        stale,
        pushed,
        message,
        status,
    })
}

/// `git commit-tree`, which takes its message on stdin.
fn commit_tree(root: &Path, tree: &str, parent: &str, message: &str) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("git")
        .args(["commit-tree", tree, "-p", parent])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("git commit-tree: {e}"))))?;
    child
        .stdin
        .as_mut()
        .expect("stdin was piped")
        .write_all(message.as_bytes())
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("writing message: {e}"))))?;
    let output = child
        .wait_with_output()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("git commit-tree: {e}"))))?;
    if !output.status.success() {
        return Err(Error::Git(git2::Error::from_str(&format!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn has_remote(root: &Path, name: &str) -> bool {
    git(root, &["remote"])
        .map(|out| out.lines().any(|line| line.trim() == name))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::{parse_line, Runner};

    const TREE: &str = "8f3a1b2c3d4e5f60718293a4b5c6d7e8f9012345";

    fn receipt(check: &str, duration_ms: u64, started: &str) -> Receipt {
        let mut r = parse_line(
            r#"{"v":1,"check":"typecheck","cmd":"c","tree":"t","ok":true,"exit":0,"started":"2026-07-06T12:40:11Z","duration_ms":48211,"runner":{"name":"Jökull Sólberg","email":"jokull@triptojapan.com","host":"mbp.local"},"dirty":false,"log":"blob:1c9e","check_blob":"9a1f"}"#,
        )
        .unwrap();
        r.check = check.to_string();
        r.duration_ms = duration_ms;
        r.started = started.to_string();
        r.tree = TREE.to_string();
        r
    }

    /// The exact expectations from the engine's own land tests. Trailers end up
    /// in history, so their shape is a contract with every commit already
    /// landed by the TypeScript engine.
    #[test]
    fn trailers_match_the_prd_shape() {
        let receipts = [
            receipt("typecheck", 48_000, "2026-07-06T12:40:11Z"),
            receipt("test", 192_000, "2026-07-06T12:45:00Z"),
            receipt("lint", 9_000, "2026-07-06T12:40:11Z"),
        ];
        assert_eq!(
            build_trailers(&receipts, TREE, None),
            [
                "Receipts: typecheck ✓ 48s · test ✓ 3m12s · lint ✓ 9s".to_string(),
                format!("Receipts-Tree: {TREE}"),
                "Receipts-Runner: jokull@mbp.local".to_string(),
            ]
        );
    }

    #[test]
    fn agent_runs_are_labeled_and_failures_marked() {
        let mut r = receipt("typecheck", 48_000, "2026-07-06T12:40:11Z");
        r.ok = false;
        r.exit = 1;
        r.runner = Runner {
            name: "J".into(),
            email: "jokull@x.com".into(),
            host: "mbp.local".into(),
            agent: Some("claude-code/2.x".into()),
        };
        let trailers = build_trailers(&[r], TREE, None);
        assert_eq!(trailers[0], "Receipts: typecheck ✗ 48s");
        assert_eq!(
            trailers[2],
            "Receipts-Runner: jokull@mbp.local (claude-code)"
        );
    }

    #[test]
    fn a_stale_base_is_recorded_in_a_trailer() {
        let stale = StaleInfo {
            merge_base: "aaaaaaaaaaaabbbbbbbb".into(),
            base_head: "ccccccccccccdddddddd".into(),
        };
        let trailers = build_trailers(
            &[receipt("c", 1000, "2026-07-06T12:40:11Z")],
            TREE,
            Some(&stale),
        );
        assert_eq!(
            trailers.last().unwrap(),
            "Receipts-Stale: base moved aaaaaaaaaaaa→cccccccccccc"
        );
    }

    #[test]
    fn durations_read_the_way_the_prd_writes_them() {
        assert_eq!(format_duration(0), "0ms");
        assert_eq!(format_duration(284), "284ms");
        assert_eq!(format_duration(9_000), "9s");
        assert_eq!(format_duration(48_000), "48s");
        assert_eq!(format_duration(192_000), "3m12s");
        assert_eq!(format_duration(120_000), "2m");
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(3_840_000), "1h4m");
    }

    #[test]
    fn an_unknown_runner_still_produces_a_trailer() {
        let mut r = receipt("c", 1000, "2026-07-06T12:40:11Z");
        r.runner = Runner::default();
        let trailers = build_trailers(&[r], TREE, None);
        assert_eq!(trailers[2], "Receipts-Runner: unknown@");
    }

    // ---- end to end ----

    fn sh(dir: &Path, args: &[&str]) {
        let status = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "{args:?} failed");
    }

    fn project() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        sh(&dir, &["git", "init", "-q", "-b", "main"]);
        sh(&dir, &["git", "config", "user.name", "t"]);
        sh(&dir, &["git", "config", "user.email", "t@t.local"]);
        sh(&dir, &["git", "config", "commit.gpgsign", "false"]);
        std::fs::create_dir_all(dir.join(crate::checks::CHECKS_DIR)).unwrap();
        std::fs::write(
            dir.join(crate::checks::CONFIG_FILE),
            "[required]\nchecks = [\"c\"]\n",
        )
        .unwrap();
        let check = dir.join(crate::checks::CHECKS_DIR).join("c");
        std::fs::write(&check, "#!/bin/bash\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&check, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "base"]);
        (temp, dir)
    }

    fn tree_of(dir: &Path, reference: &str) -> String {
        git(dir, &["rev-parse", &format!("{reference}^{{tree}}")]).unwrap()
    }

    /// The property the whole design rests on: the landed commit's tree *is*
    /// the tree the receipts were minted against.
    #[test]
    fn the_landed_commit_carries_the_proven_tree() {
        let (_t, dir) = project();
        sh(&dir, &["git", "checkout", "-qb", "feature"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        sh(&dir, &["git", "commit", "-qam", "work"]);
        crate::checks::run(&dir, None).unwrap();

        sh(&dir, &["git", "checkout", "-q", "main"]);
        let result = land(
            &dir,
            "feature",
            &LandOptions {
                no_push: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.tree, tree_of(&dir, "feature"), "the proven tree");
        assert_eq!(
            tree_of(&dir, &result.commit),
            result.tree,
            "the landed commit's tree IS the proven tree"
        );
        assert_eq!(tree_of(&dir, "main"), result.tree, "main fast-forwarded");
        assert!(result.message.contains("Receipts-Tree:"));
        assert!(!result.pushed, "no remote, nothing pushed");
    }

    #[test]
    fn landing_without_green_receipts_is_refused() {
        let (_t, dir) = project();
        sh(&dir, &["git", "checkout", "-qb", "feature"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        sh(&dir, &["git", "commit", "-qam", "work"]);
        // No run: the tree has no receipts at all.
        sh(&dir, &["git", "checkout", "-q", "main"]);

        let message = land(
            &dir,
            "feature",
            &LandOptions {
                no_push: true,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("not green"), "{message}");
        assert!(message.contains("preceipts run"), "the error says the fix");
    }

    #[test]
    fn a_moved_base_blocks_the_land_until_told_otherwise() {
        let (_t, dir) = project();
        sh(&dir, &["git", "checkout", "-qb", "feature"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        sh(&dir, &["git", "commit", "-qam", "work"]);
        crate::checks::run(&dir, None).unwrap();

        // main moves on independently.
        sh(&dir, &["git", "checkout", "-q", "main"]);
        std::fs::write(dir.join("b.txt"), "elsewhere\n").unwrap();
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "meanwhile"]);

        let blocked = land(
            &dir,
            "feature",
            &LandOptions {
                no_push: true,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            blocked.contains("moved since receipts were minted"),
            "{blocked}"
        );
        assert!(
            blocked.contains("--allow-stale"),
            "the error offers the escape"
        );

        let hard = land(
            &dir,
            "feature",
            &LandOptions {
                no_push: true,
                require_fresh: true,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(hard.contains("--require-fresh"), "{hard}");

        let landed = land(
            &dir,
            "feature",
            &LandOptions {
                no_push: true,
                allow_stale: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(landed.stale.is_some());
        assert!(
            landed.message.contains("Receipts-Stale:"),
            "landing anyway records why: {}",
            landed.message
        );
    }

    #[test]
    fn a_project_with_no_required_checks_refuses_to_land() {
        let (_t, dir) = project();
        std::fs::write(
            dir.join(crate::checks::CONFIG_FILE),
            "[required]\nchecks = []\n",
        )
        .unwrap();
        sh(&dir, &["git", "commit", "-qam", "drop required"]);
        sh(&dir, &["git", "checkout", "-qb", "feature"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        sh(&dir, &["git", "commit", "-qam", "work"]);
        sh(&dir, &["git", "checkout", "-q", "main"]);

        let message = land(
            &dir,
            "feature",
            &LandOptions {
                no_push: true,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("no required checks"), "{message}");
    }

    /// A branch may change a check script and still be self-consistent: its
    /// receipts are judged against its own tree's definitions.
    #[test]
    fn a_branch_that_changes_a_check_can_still_land() {
        let (_t, dir) = project();
        sh(&dir, &["git", "checkout", "-qb", "feature"]);
        let check = dir.join(crate::checks::CHECKS_DIR).join("c");
        std::fs::write(&check, "#!/bin/bash\n# improved\nexit 0\n").unwrap();
        sh(&dir, &["git", "commit", "-qam", "improve the check"]);
        crate::checks::run(&dir, None).unwrap();
        sh(&dir, &["git", "checkout", "-q", "main"]);

        let result = land(
            &dir,
            "feature",
            &LandOptions {
                no_push: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(tree_of(&dir, &result.commit), result.tree);
    }
}
