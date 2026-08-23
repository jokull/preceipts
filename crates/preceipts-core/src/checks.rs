//! `.preceipts/` — the check configuration, and running it.
//!
//! Ported forward from `engine/src/lib/{config,runner,status}.ts` at fc3643e.
//!
//! Decision 8 is the shape of this module and it is worth restating, because
//! the ordering is the whole guarantee:
//!
//! 1. `[prepare]` runs first, serially. Formatters and codegen belong here and
//!    are *expected* to mutate the worktree.
//! 2. The receipt tree is computed **after** prepare settles.
//! 3. Checks run. A check that mutates the worktree invalidates the run — no
//!    receipts are minted, and the error points at `[prepare]`.
//!
//! Minting is deferred until the tree is verified stable across the whole run,
//! because a receipt filed under a tree that no longer describes what was
//! tested is worse than no receipt at all.

use crate::error::{Error, Result};
use crate::notes;
use crate::receipt::{latest_by_check, Receipt};
use crate::treehash;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const PRECEIPTS_DIR: &str = ".preceipts";
pub const CHECKS_DIR: &str = ".preceipts/checks";
pub const CONFIG_FILE: &str = ".preceipts/config.toml";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareStep {
    pub name: String,
    /// Shell command, run from the repository root.
    pub cmd: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Check names that must be green for `status` and `land`.
    pub required: Vec<String>,
    /// Normalization commands run before the receipt tree is computed.
    pub prepare: Vec<PrepareStep>,
}

/// Load `.preceipts/config.toml`. A missing `.preceipts/` is an honest error —
/// it means this repository has never been set up — while a missing config
/// file inside it simply means no required checks.
pub fn load_config(root: &Path) -> Result<Config> {
    let dir = root.join(PRECEIPTS_DIR);
    if !dir.is_dir() {
        return Err(Error::Git(git2::Error::from_str(&format!(
            "no {PRECEIPTS_DIR}/ directory in {} — run `preceipts init` first",
            root.display()
        ))));
    }
    let path = root.join(CONFIG_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Config::default());
    };
    // `str::parse::<toml::Value>` reads a bare TOML *value*, not a document —
    // it stops after `[required]` and calls the rest unexpected. `from_str`
    // into a Table is the document parser.
    let parsed: toml::Table = toml::from_str(&text).map_err(|e| {
        Error::Git(git2::Error::from_str(&format!(
            "cannot parse {CONFIG_FILE}: {e}"
        )))
    })?;

    let required = parsed
        .get("required")
        .and_then(|r| r.get("checks"))
        .and_then(|c| c.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let prepare = parse_prepare(parsed.get("prepare"))?;
    Ok(Config { required, prepare })
}

/// `[prepare] commands = [...]` with a `[prepare.<name>]` table each.
///
/// The list is both the execution order and the source of truth. A name
/// without a table, or a table without a listing, is an error — a
/// half-configured normalization step that silently does nothing is exactly
/// the kind of thing that makes a green run untrustworthy.
fn parse_prepare(section: Option<&toml::Value>) -> Result<Vec<PrepareStep>> {
    let Some(section) = section else {
        return Ok(Vec::new());
    };
    let table = section.as_table().ok_or_else(|| {
        Error::Git(git2::Error::from_str(&format!(
            "[prepare] in {CONFIG_FILE} must be a table"
        )))
    })?;

    let names: Vec<String> = match table.get("commands") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| {
                Error::Git(git2::Error::from_str(&format!(
                    "[prepare].commands in {CONFIG_FILE} must be an array of step names"
                )))
            })?
            .iter()
            .map(|v| {
                v.as_str().map(str::to_string).ok_or_else(|| {
                    Error::Git(git2::Error::from_str(&format!(
                        "[prepare].commands entries in {CONFIG_FILE} must be strings"
                    )))
                })
            })
            .collect::<Result<Vec<String>>>()?,
    };

    let mut steps = Vec::new();
    for name in &names {
        let cmd = table
            .get(name)
            .and_then(|t| t.get("cmd"))
            .and_then(|c| c.as_str())
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| {
                Error::Git(git2::Error::from_str(&format!(
                    "prepare step \"{name}\" is listed in [prepare].commands but has no \
                     [prepare.{name}] table with a cmd string"
                )))
            })?;
        steps.push(PrepareStep {
            name: name.clone(),
            cmd: cmd.to_string(),
        });
    }

    for key in table.keys() {
        if key == "commands" || names.contains(key) {
            continue;
        }
        return Err(Error::Git(git2::Error::from_str(&format!(
            "[prepare.{key}] is defined in {CONFIG_FILE} but \"{key}\" is not listed in \
             [prepare].commands — add it there (order matters) or remove the table"
        ))));
    }

    Ok(steps)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFile {
    pub name: String,
    pub path: PathBuf,
    pub executable: bool,
}

/// Check scripts in `.preceipts/checks/`, sorted by name.
pub fn list_checks(root: &Path) -> Result<Vec<CheckFile>> {
    let dir = root.join(CHECKS_DIR);
    let entries = std::fs::read_dir(&dir).map_err(|_| {
        Error::Git(git2::Error::from_str(&format!(
            "no {CHECKS_DIR}/ directory in {} — run `preceipts init` first",
            root.display()
        )))
    })?;

    let mut checks = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        checks.push(CheckFile {
            name,
            path,
            executable: is_executable(&meta),
        });
    }
    checks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(checks)
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Ok,
    Fail,
    Missing,
    /// A receipt exists, but the check script has changed since — so the
    /// receipt speaks about a different check than the one defined now.
    StaleDefinition,
}

impl CheckState {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckState::Ok => "ok",
            CheckState::Fail => "fail",
            CheckState::Missing => "missing",
            CheckState::StaleDefinition => "stale-definition",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StatusRow {
    pub check: String,
    pub required: bool,
    pub state: CheckState,
    pub receipt: Option<Receipt>,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub reference: String,
    pub tree: String,
    pub rows: Vec<StatusRow>,
    /// Every required check has an ok receipt whose definition still matches.
    pub green: bool,
}

/// Where "the check as currently defined" is read from, for telling a stale
/// receipt from a current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionSource {
    /// The script on disk right now. `status` answers "do these receipts speak
    /// to my checks as I have them?"
    Worktree,
    /// The script inside the inspected tree itself. `land` uses this, because a
    /// branch that legitimately changes a check is still self-consistent
    /// evidence for itself.
    Tree,
}

/// Receipt table for a tree.
///
/// `reference` of `None` means "the working tree as it stands" — the same tree
/// `run` mints against. That is the default on purpose: after a run in a dirty
/// worktree, status must agree with what was just minted, and HEAD would say
/// "missing".
pub fn status(root: &Path, reference: Option<&str>) -> Result<Status> {
    status_with(root, reference, DefinitionSource::Worktree)
}

pub fn status_with(
    root: &Path,
    reference: Option<&str>,
    source: DefinitionSource,
) -> Result<Status> {
    let config = load_config(root)?;
    let tree = match reference {
        None => treehash::compute(root)?.tree,
        Some(reference) => treehash::resolve_tree(root, reference)?,
    };

    let repo = notes::open(root)?;
    let receipts = latest_by_check(&notes::read_receipts(&repo, &tree));

    let mut names = config.required.clone();
    let mut extras: Vec<String> = receipts
        .keys()
        .filter(|name| !config.required.contains(name))
        .cloned()
        .collect();
    extras.sort();
    names.extend(extras);

    let mut rows = Vec::with_capacity(names.len());
    for name in names {
        let current = match source {
            DefinitionSource::Worktree => worktree_check_blob(root, &name),
            DefinitionSource::Tree => tree_check_blob(root, &tree, &name),
        };
        let receipt = receipts.get(&name).cloned();
        let state = match (&receipt, current) {
            (None, _) => CheckState::Missing,
            (Some(_), None) => CheckState::StaleDefinition,
            (Some(receipt), Some(blob)) if receipt.check_blob != blob => {
                CheckState::StaleDefinition
            }
            (Some(receipt), _) => {
                if receipt.ok {
                    CheckState::Ok
                } else {
                    CheckState::Fail
                }
            }
        };
        rows.push(StatusRow {
            required: config.required.contains(&name),
            check: name,
            state,
            receipt,
        });
    }

    let green = rows
        .iter()
        .filter(|row| row.required)
        .all(|row| row.state == CheckState::Ok);

    Ok(Status {
        reference: reference.unwrap_or("worktree").to_string(),
        tree,
        rows,
        green,
    })
}

/// Blob sha of a check script inside a tree, or `None` when the tree lacks it.
fn tree_check_blob(root: &Path, tree: &str, check: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["ls-tree", tree, "--", &format!("{CHECKS_DIR}/{check}")])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // "<mode> blob <sha>\t<path>"
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().nth(2).map(str::to_string)
}

/// Blob sha of a check script as it exists on disk, matching `git hash-object`.
fn worktree_check_blob(root: &Path, check: &str) -> Option<String> {
    let path = root.join(CHECKS_DIR).join(check);
    let bytes = std::fs::read(path).ok()?;
    Some(
        git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)
            .ok()?
            .to_string(),
    )
}

#[derive(Debug, Clone)]
pub struct CheckOutcome {
    pub name: String,
    pub ok: bool,
    pub exit: i32,
    pub duration: Duration,
    pub output: String,
}

#[derive(Debug, Clone)]
pub struct RunReport {
    /// The tree receipts were filed under.
    pub tree: String,
    pub dirty: bool,
    pub outcomes: Vec<CheckOutcome>,
    /// Paths that `[prepare]` changed, if any.
    pub prepared: Vec<String>,
}

/// Run prepare, then every check, then mint receipts.
///
/// The tree is computed after prepare and verified unchanged after the checks.
/// If a check moved the worktree, nothing is minted and the error says so.
pub fn run(root: &Path, only: Option<&[String]>) -> Result<RunReport> {
    let config = load_config(root)?;

    let before_prepare = treehash::compute(root)?.tree;
    for step in &config.prepare {
        let output = shell(root, &step.cmd)?;
        if !output.status.success() {
            return Err(Error::Git(git2::Error::from_str(&format!(
                "prepare step \"{}\" failed: {}",
                step.name,
                String::from_utf8_lossy(&output.stderr).trim()
            ))));
        }
    }
    let settled = treehash::compute(root)?;
    let prepared = if settled.tree != before_prepare {
        vec![format!(
            "prepare changed the tree: {before_prepare} → {}",
            settled.tree
        )]
    } else {
        Vec::new()
    };

    let checks: Vec<CheckFile> = list_checks(root)?
        .into_iter()
        .filter(|check| match only {
            None => true,
            Some(names) => names.contains(&check.name),
        })
        .collect();

    let mut outcomes = Vec::with_capacity(checks.len());
    for check in &checks {
        if !check.executable {
            return Err(Error::Git(git2::Error::from_str(&format!(
                "check \"{}\" is not executable — chmod +x {}",
                check.name,
                check.path.display()
            ))));
        }
        let started = Instant::now();
        let output = Command::new(&check.path)
            .current_dir(root)
            .output()
            .map_err(|e| {
                Error::Git(git2::Error::from_str(&format!(
                    "running check \"{}\": {e}",
                    check.name
                )))
            })?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        outcomes.push(CheckOutcome {
            name: check.name.clone(),
            ok: output.status.success(),
            exit: output.status.code().unwrap_or(-1),
            duration: started.elapsed(),
            output: text,
        });
    }

    // Decision 8: a check that mutates the worktree invalidates the run.
    let after = treehash::compute(root)?;
    if after.tree != settled.tree {
        return Err(Error::Git(git2::Error::from_str(
            "a check modified the working tree, so no receipts were minted — \
             move whatever normalizes the tree into [prepare], which runs before \
             the receipt tree is computed",
        )));
    }

    let repo = notes::open(root)?;
    let runner = notes::runner_identity(&repo);
    let started = timestamp();

    for (check, outcome) in checks.iter().zip(&outcomes) {
        let log = notes::store_log(&repo, outcome.output.as_bytes())?;
        let check_blob = worktree_check_blob(root, &check.name).unwrap_or_default();
        let receipt = Receipt {
            v: 1,
            check: outcome.name.clone(),
            cmd: format!("{CHECKS_DIR}/{}", outcome.name),
            tree: settled.tree.clone(),
            ok: outcome.ok,
            exit: outcome.exit,
            started: started.clone(),
            duration_ms: outcome.duration.as_millis() as u64,
            runner: runner.clone(),
            dirty: settled.dirty,
            log: format!("blob:{log}"),
            check_blob,
        };
        notes::append_receipt(&repo, &settled.tree, &receipt.encode())?;
    }

    Ok(RunReport {
        tree: settled.tree,
        dirty: settled.dirty,
        outcomes,
        prepared,
    })
}

fn shell(root: &Path, cmd: &str) -> Result<std::process::Output> {
    Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(root)
        .output()
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("running {cmd:?}: {e}"))))
}

/// ISO-8601 to the second, UTC — the format receipts already use, and one that
/// sorts lexically so `latest_by_check` can compare strings.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days-from-civil, inverted — Howard Hinnant's algorithm. Avoids a date
/// dependency for the one thing core needs a calendar for.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// Scaffold `.preceipts/` in a repository that has none.
pub fn init(root: &Path) -> Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    let checks = root.join(CHECKS_DIR);
    if !checks.is_dir() {
        std::fs::create_dir_all(&checks).map_err(|e| {
            Error::Git(git2::Error::from_str(&format!(
                "creating {CHECKS_DIR}: {e}"
            )))
        })?;
        created.push(checks);
    }
    let config = root.join(CONFIG_FILE);
    if !config.exists() {
        std::fs::write(
            &config,
            "# Checks that must be green to land.\n[required]\nchecks = []\n\n\
             # Normalization run before the receipt tree is computed.\n\
             # [prepare]\n# commands = [\"format\"]\n# [prepare.format]\n# cmd = \"cargo fmt\"\n",
        )
        .map_err(|e| {
            Error::Git(git2::Error::from_str(&format!(
                "writing {CONFIG_FILE}: {e}"
            )))
        })?;
        created.push(config);
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes;

    fn sh(dir: &Path, args: &[&str]) {
        assert!(Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    }

    fn write_check(dir: &Path, name: &str, body: &str) {
        let path = dir.join(CHECKS_DIR).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn project(config: &str) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        sh(&dir, &["git", "init", "-q", "-b", "main"]);
        sh(&dir, &["git", "config", "user.name", "t"]);
        sh(&dir, &["git", "config", "user.email", "t@t.local"]);
        sh(&dir, &["git", "config", "commit.gpgsign", "false"]);
        std::fs::create_dir_all(dir.join(CHECKS_DIR)).unwrap();
        std::fs::write(dir.join(CONFIG_FILE), config).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "base"]);
        (temp, dir)
    }

    #[test]
    fn the_full_loop_mints_receipts_that_status_reads_back() {
        let (_t, dir) = project("[required]\nchecks = [\"green\"]\n");
        write_check(&dir, "green", "#!/bin/bash\necho fine\n");

        let report = run(&dir, None).unwrap();
        assert_eq!(report.outcomes.len(), 1);
        assert!(report.outcomes[0].ok);

        let status = status(&dir, None).unwrap();
        assert!(
            status.green,
            "a passing required check makes the tree green"
        );
        assert_eq!(
            status.tree, report.tree,
            "status agrees with what run minted"
        );
        assert_eq!(status.rows[0].state, CheckState::Ok);
        assert!(status.rows[0].required);

        // The log was stored and is readable back through the receipt.
        let repo = notes::open(&dir).unwrap();
        let receipt = status.rows[0].receipt.as_ref().unwrap();
        let log = notes::read_log(&repo, receipt.log_blob().unwrap()).unwrap();
        assert!(log.contains("fine"), "log: {log}");
    }

    #[test]
    fn a_failing_check_is_recorded_and_the_tree_is_not_green() {
        let (_t, dir) = project("[required]\nchecks = [\"red\"]\n");
        write_check(&dir, "red", "#!/bin/bash\necho broke >&2\nexit 3\n");

        let report = run(&dir, None).unwrap();
        assert!(!report.outcomes[0].ok);
        assert_eq!(report.outcomes[0].exit, 3);

        let status = status(&dir, None).unwrap();
        assert!(!status.green);
        assert_eq!(status.rows[0].state, CheckState::Fail);
        assert_eq!(status.rows[0].receipt.as_ref().unwrap().exit, 3);
    }

    /// Decision 8: normalization belongs in [prepare], which runs before the
    /// receipt tree is computed, so its edits are part of what gets proven.
    #[test]
    fn prepare_runs_before_the_tree_is_computed() {
        let (_t, dir) = project(
            "[required]\nchecks = []\n\n[prepare]\ncommands = [\"norm\"]\n\n\
             [prepare.norm]\ncmd = \"echo normalized > generated.txt\"\n",
        );
        write_check(&dir, "green", "#!/bin/bash\nexit 0\n");

        let report = run(&dir, None).unwrap();
        assert!(dir.join("generated.txt").exists(), "prepare ran");
        assert!(!report.prepared.is_empty(), "the tree change was reported");

        // The receipt is filed under the tree that includes prepare's output.
        let after = crate::treehash::compute(&dir).unwrap();
        assert_eq!(report.tree, after.tree);
    }

    /// The other half of decision 8: a *check* that mutates the worktree
    /// invalidates the run, because the tree it was filed under no longer
    /// describes what was tested.
    #[test]
    fn a_check_that_mutates_the_worktree_invalidates_the_run() {
        let (_t, dir) = project("[required]\nchecks = []\n");
        write_check(&dir, "dirty", "#!/bin/bash\necho oops > side-effect.txt\n");

        let error = run(&dir, None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("modified the working tree"), "{message}");
        assert!(
            message.contains("[prepare]"),
            "the error points at the fix: {message}"
        );

        // Nothing was minted.
        let repo = notes::open(&dir).unwrap();
        assert!(notes::read_all_receipts(&repo).is_empty());
    }

    /// A receipt that no longer speaks about the check as it is defined now is
    /// stale, not wrong — and stale is not green.
    ///
    /// This is asked against a *ref*, not the working tree: editing a check
    /// changes the worktree's tree hash, so against the worktree the honest
    /// answer is `Missing` (no receipts exist for that new tree at all). The
    /// stale case is precisely "this tree has receipts, but the check has
    /// moved on since".
    #[test]
    fn editing_a_check_makes_its_receipt_stale_rather_than_wrong() {
        let (_t, dir) = project("[required]\nchecks = [\"c\"]\n");
        write_check(&dir, "c", "#!/bin/bash\nexit 0\n");
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "add check"]);

        run(&dir, None).unwrap();
        assert_eq!(
            status(&dir, Some("HEAD")).unwrap().rows[0].state,
            CheckState::Ok
        );

        write_check(&dir, "c", "#!/bin/bash\n# different now\nexit 0\n");
        let status = status(&dir, Some("HEAD")).unwrap();
        assert_eq!(status.rows[0].state, CheckState::StaleDefinition);
        assert!(!status.green, "a stale definition cannot be green");
    }

    /// The other reading of the same edit: against the working tree, a changed
    /// check means a changed tree, so there are simply no receipts yet.
    #[test]
    fn editing_a_check_leaves_the_working_tree_unproven() {
        let (_t, dir) = project("[required]\nchecks = [\"c\"]\n");
        write_check(&dir, "c", "#!/bin/bash\nexit 0\n");
        run(&dir, None).unwrap();
        assert_eq!(status(&dir, None).unwrap().rows[0].state, CheckState::Ok);

        write_check(&dir, "c", "#!/bin/bash\n# different now\nexit 0\n");
        let status = status(&dir, None).unwrap();
        assert_eq!(status.rows[0].state, CheckState::Missing);
        assert!(!status.green);
    }

    #[test]
    fn a_required_check_that_never_ran_is_missing() {
        let (_t, dir) = project("[required]\nchecks = [\"never\"]\n");
        let status = status(&dir, None).unwrap();
        assert_eq!(status.rows[0].state, CheckState::Missing);
        assert!(!status.green);
    }

    /// Receipts key on content, not history. Rewriting the commit that
    /// produced a tree must not cost the proof — that is the whole idea.
    #[test]
    fn receipts_survive_a_history_rewrite() {
        let (_t, dir) = project("[required]\nchecks = [\"c\"]\n");
        write_check(&dir, "c", "#!/bin/bash\nexit 0\n");
        sh(&dir, &["git", "add", "-A"]);
        sh(&dir, &["git", "commit", "-qm", "add check"]);

        run(&dir, None).unwrap();
        let before = status(&dir, None).unwrap();
        assert!(before.green);

        // Reword the commit: new sha, identical tree.
        sh(&dir, &["git", "commit", "-q", "--amend", "-m", "reworded"]);

        let after = status(&dir, None).unwrap();
        assert_eq!(after.tree, before.tree, "the tree is unchanged by a reword");
        assert!(after.green, "the receipt still stands after the rewrite");
    }

    #[test]
    fn running_a_subset_leaves_the_others_missing() {
        let (_t, dir) = project("[required]\nchecks = [\"a\", \"b\"]\n");
        write_check(&dir, "a", "#!/bin/bash\nexit 0\n");
        write_check(&dir, "b", "#!/bin/bash\nexit 0\n");

        run(&dir, Some(&["a".to_string()])).unwrap();
        let status = status(&dir, None).unwrap();
        let by_name: std::collections::HashMap<&str, CheckState> = status
            .rows
            .iter()
            .map(|r| (r.check.as_str(), r.state))
            .collect();
        assert_eq!(by_name["a"], CheckState::Ok);
        assert_eq!(by_name["b"], CheckState::Missing);
        assert!(!status.green);
    }

    #[test]
    fn a_non_executable_check_is_an_honest_error() {
        let (_t, dir) = project("[required]\nchecks = []\n");
        let path = dir.join(CHECKS_DIR).join("plain");
        std::fs::write(&path, "#!/bin/bash\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let message = run(&dir, None).unwrap_err().to_string();
        assert!(message.contains("not executable"), "{message}");
        assert!(message.contains("chmod +x"), "the error says how to fix it");
    }

    #[test]
    fn a_repo_without_preceipts_says_so() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        sh(&dir, &["git", "init", "-q"]);
        let message = load_config(&dir).unwrap_err().to_string();
        assert!(message.contains("preceipts init"), "{message}");
    }

    #[test]
    fn a_half_configured_prepare_step_is_refused() {
        let (_t, dir) = project("[prepare]\ncommands = [\"missing\"]\n");
        let message = load_config(&dir).unwrap_err().to_string();
        assert!(message.contains("missing"), "{message}");

        let (_t2, dir2) = project("[prepare]\n[prepare.orphan]\ncmd = \"true\"\n");
        let message = load_config(&dir2).unwrap_err().to_string();
        assert!(message.contains("not listed"), "{message}");
    }

    #[test]
    fn a_failing_prepare_step_stops_the_run() {
        let (_t, dir) =
            project("[prepare]\ncommands = [\"bad\"]\n\n[prepare.bad]\ncmd = \"exit 1\"\n");
        let message = run(&dir, None).unwrap_err().to_string();
        assert!(message.contains("prepare step \"bad\" failed"), "{message}");
    }

    #[test]
    fn init_scaffolds_only_what_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        sh(&dir, &["git", "init", "-q"]);

        let created = init(&dir).unwrap();
        assert_eq!(created.len(), 2, "checks dir and config file");
        assert!(load_config(&dir).is_ok());

        let again = init(&dir).unwrap();
        assert!(again.is_empty(), "init is idempotent");
    }

    #[test]
    fn timestamps_are_sortable_iso_utc() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), 20, "YYYY-MM-DDTHH:MM:SSZ — got {stamp}");
        assert!(stamp.ends_with('Z'));
        // Sanity-check the calendar maths against a known instant.
        let (y, m, d, h, min, s) = civil_from_unix(1_700_000_000);
        assert_eq!((y, m, d, h, min, s), (2023, 11, 14, 22, 13, 20));
    }
}
