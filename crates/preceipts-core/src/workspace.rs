//! The workspace: a git worktree of a project, plus the branch, intent,
//! environment, diff, and receipts that hang off it.
//!
//! This is the central object of decision 12, and the one rule that shapes
//! this module is **adoption, not creation, is the primitive**. A workspace
//! exists because a worktree exists — whether the CLI made it, the app made
//! it, or an agent ran `git worktree add` on its own. Nothing here requires
//! having been the thing that created it.
//!
//! Which is also why discovery reads git rather than a database of our own: a
//! registry we wrote could disagree with reality, and reality would be right.

use crate::error::{Error, Result};
use git2::{Repository, WorktreeAddOptions};
use std::path::{Path, PathBuf};

/// Environment variable naming the workspace a process is standing in.
///
/// Set in every worktree's environment so that an agent started there locates
/// its lab without being configured — the compatibility between "never own the
/// harness" and "the agent is aware of its workspace".
pub const WORKSPACE_ENV: &str = "PRECEIPTS_WORKSPACE";

/// Marker file at a worktree root. Belt and braces with the env var: the env
/// var travels with a process, the marker travels with the directory, and an
/// agent may arrive by either route.
pub const MARKER: &str = ".preceipts/workspace.toml";

/// Where a workspace is in its life. The daemon drives this; the UI observes
/// it. `Draft` exists so intent can be recorded before a worktree does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Draft,
    Creating,
    Booting,
    Live,
    Landed,
    Archived,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Draft => "draft",
            State::Creating => "creating",
            State::Booting => "booting",
            State::Live => "live",
            State::Landed => "landed",
            State::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// Stable, filesystem-safe identity. Every keyed resource in the daemon —
    /// socket path, port block, URL registry, secret scope, certificate — is
    /// namespaced by this.
    pub id: String,
    /// The worktree's own directory.
    pub path: PathBuf,
    /// The project this belongs to: the main worktree's root.
    pub project_root: PathBuf,
    pub branch: Option<String>,
    /// What this workspace was created to do, if anyone said. Shown on the
    /// card and carried into the land trailer — it is what makes a list of
    /// eight worktrees legible a week later.
    pub genesis: Option<String>,
    /// True for the repository's own working directory, which is a workspace
    /// like any other — you can read a diff and run checks in it — but must
    /// never be removed as one.
    pub is_primary: bool,
}

impl Workspace {
    /// Hostname label for this workspace's services: `<service>.<id>.<project>`
    /// resolves through the daemon's DNS responder.
    pub fn host_label(&self, service: &str) -> String {
        format!("{}.{}.{}", service, self.id, self.project_name())
    }

    pub fn project_name(&self) -> String {
        self.project_root
            .file_name()
            .map(|n| slug(&n.to_string_lossy()))
            .unwrap_or_else(|| "project".to_string())
    }
}

/// Every workspace of a project: the primary working directory plus each
/// linked worktree.
///
/// `path` may be any worktree of the project — discovery normalizes to the
/// main repository first, so an agent calling this from inside its own
/// worktree sees all its siblings.
pub fn discover(path: &Path) -> Result<Vec<Workspace>> {
    let repo = Repository::discover(path).map_err(|_| Error::NotARepo(path.to_path_buf()))?;
    let main = main_repository(&repo)?;
    let project_root = main
        .workdir()
        .ok_or_else(|| Error::NotARepo(main.path().to_path_buf()))?
        .to_path_buf();

    let mut out = vec![Workspace {
        id: "main".to_string(),
        path: project_root.clone(),
        project_root: project_root.clone(),
        branch: branch_of(&main),
        genesis: read_genesis(&project_root),
        is_primary: true,
    }];

    for name in main.worktrees()?.iter().flatten() {
        let Ok(worktree) = main.find_worktree(name) else {
            continue;
        };
        // A worktree whose directory is gone is registered but not real. Git
        // calls this prunable; we simply do not adopt it.
        if worktree.validate().is_err() {
            continue;
        }
        let path = worktree.path().to_path_buf();
        let branch = Repository::open(&path).ok().and_then(|r| branch_of(&r));
        out.push(Workspace {
            id: slug(name),
            path: path.clone(),
            project_root: project_root.clone(),
            branch,
            genesis: read_genesis(&path),
            is_primary: false,
        });
    }

    Ok(out)
}

/// The workspace containing `path`, if any. This is self-location: an agent
/// that knows only its working directory can find out which lab it is in.
pub fn locate(path: &Path) -> Result<Workspace> {
    let repo = Repository::discover(path).map_err(|_| Error::NotARepo(path.to_path_buf()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| Error::NotARepo(repo.path().to_path_buf()))?
        .to_path_buf();
    let workdir = workdir.canonicalize().unwrap_or(workdir);

    discover(path)?
        .into_iter()
        .find(|w| w.path.canonicalize().unwrap_or_else(|_| w.path.clone()) == workdir)
        .ok_or(Error::NotARepo(workdir))
}

/// Create a worktree on a new branch, and adopt it.
///
/// `genesis` is the recorded intent. It is optional because two of the three
/// doors — an agent running `git worktree add`, a person doing the same — do
/// not go through here at all, and a workspace without stated intent is still
/// a workspace.
pub fn create(
    project_path: &Path,
    branch: &str,
    genesis: Option<&str>,
    parent: Option<&Path>,
) -> Result<Workspace> {
    let repo = Repository::discover(project_path)
        .map_err(|_| Error::NotARepo(project_path.to_path_buf()))?;
    let main = main_repository(&repo)?;
    let project_root = main
        .workdir()
        .ok_or_else(|| Error::NotARepo(main.path().to_path_buf()))?
        .to_path_buf();

    let id = slug(branch);
    // Siblings of the project by default: keeping worktrees outside the
    // project root means no tool has to learn to ignore them.
    let parent = parent
        .map(Path::to_path_buf)
        .or_else(|| project_root.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| project_root.clone());
    let path = parent.join(format!("{}-{}", project_root_name(&project_root), id));

    let head = main.head()?.peel_to_commit()?;
    let reference = main.branch(branch, &head, false)?;
    let mut options = WorktreeAddOptions::new();
    let reference = reference.into_reference();
    options.reference(Some(&reference));

    main.worktree(&id, &path, Some(&options))?;

    // Our bookkeeping must never show up as the user's change. `info/exclude`
    // lives in the common dir and is shared by every worktree, which is
    // exactly right — every workspace has a marker — and it keeps us out of
    // the user's own .gitignore.
    exclude_marker(&main);

    if let Some(genesis) = genesis {
        write_genesis(&path, genesis)?;
    }

    Ok(Workspace {
        id,
        path,
        project_root,
        branch: Some(branch.to_string()),
        genesis: genesis.map(str::to_string),
        is_primary: false,
    })
}

/// Remove a workspace's worktree and prune its registration.
///
/// Refuses the primary working directory: it is a workspace for reading and
/// checking, never one to delete.
pub fn remove(workspace: &Workspace) -> Result<()> {
    if workspace.is_primary {
        return Err(Error::Git(git2::Error::from_str(
            "refusing to remove the project's primary working directory",
        )));
    }
    let repo = Repository::discover(&workspace.project_root)
        .map_err(|_| Error::NotARepo(workspace.project_root.clone()))?;
    let main = main_repository(&repo)?;

    if workspace.path.exists() {
        std::fs::remove_dir_all(&workspace.path).map_err(|e| {
            Error::Git(git2::Error::from_str(&format!(
                "removing {}: {e}",
                workspace.path.display()
            )))
        })?;
    }
    if let Ok(worktree) = main.find_worktree(&workspace.id) {
        let mut options = git2::WorktreePruneOptions::new();
        options.valid(true).working_tree(true);
        worktree.prune(Some(&mut options))?;
    }
    Ok(())
}

/// The main repository behind any worktree of it.
fn main_repository(repo: &Repository) -> Result<Repository> {
    if !repo.is_worktree() {
        return Ok(Repository::open(repo.path())?);
    }
    // A linked worktree's git dir is `<main>/.git/worktrees/<name>`, and holds
    // a `commondir` file pointing back at the main git dir — usually relative.
    // git2 0.19 does not expose `git_repository_commondir`, so read it.
    let git_dir = repo.path();
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .map_err(|e| Error::Git(git2::Error::from_str(&format!("reading commondir: {e}"))))?;
    let common = Path::new(common.trim());
    let common = if common.is_absolute() {
        common.to_path_buf()
    } else {
        git_dir.join(common)
    };
    Ok(Repository::open(common)?)
}

fn branch_of(repo: &Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_string))
}

fn project_root_name(root: &Path) -> String {
    root.file_name()
        .map(|n| slug(&n.to_string_lossy()))
        .unwrap_or_else(|| "project".to_string())
}

/// Add the marker to the repository's `info/exclude`, once.
///
/// Best-effort: a repo we cannot write to still works, the marker just shows
/// up as an untracked file. Not worth failing a workspace over.
fn exclude_marker(main: &Repository) -> Option<()> {
    let exclude = main.path().join("info").join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| line.trim() == MARKER) {
        return Some(());
    }
    std::fs::create_dir_all(exclude.parent()?).ok()?;
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&format!("{MARKER}\n"));
    std::fs::write(&exclude, next).ok()
}

fn read_genesis(worktree: &Path) -> Option<String> {
    let text = std::fs::read_to_string(worktree.join(MARKER)).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("genesis = ") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn write_genesis(worktree: &Path, genesis: &str) -> Result<()> {
    let marker = worktree.join(MARKER);
    let write = || -> std::io::Result<()> {
        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&marker, format!("genesis = {}\n", toml_string(genesis)))
    };
    write().map_err(|e| Error::Git(git2::Error::from_str(&format!("writing marker: {e}"))))
}

/// Minimal TOML basic-string quoting — enough for one recorded sentence.
fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Lowercase, hyphen-separated, filesystem- and hostname-safe.
///
/// This runs on branch names, which routinely carry `/` and `_`, and the
/// result becomes a DNS label — so it has to be conservative rather than
/// merely tidy.
pub fn slug(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_dash = true; // leading dashes are not allowed in a label
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        // DNS labels cap at 63 characters.
        out.chars().take(63).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn sh(dir: &Path, args: &[&str]) {
        let status = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .status()
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        assert!(status.success(), "{args:?} failed");
    }

    fn project() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        // The project lives one level down so worktrees get a sibling
        // directory inside the temp dir rather than beside it.
        let dir = temp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
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
    fn a_fresh_project_has_one_primary_workspace() {
        let (_temp, dir) = project();
        let all = discover(&dir).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].is_primary);
        assert_eq!(all[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn create_makes_a_branch_a_worktree_and_records_intent() {
        let (_temp, dir) = project();
        let ws = create(&dir, "fix-checkout", Some("fix the checkout race"), None).unwrap();

        assert_eq!(ws.id, "fix-checkout");
        assert!(ws.path.is_dir(), "worktree directory exists");
        assert!(!ws.is_primary);
        assert_eq!(ws.branch.as_deref(), Some("fix-checkout"));
        assert_eq!(ws.genesis.as_deref(), Some("fix the checkout race"));

        let all = discover(&dir).unwrap();
        assert_eq!(all.len(), 2);
        let found = all.iter().find(|w| w.id == "fix-checkout").unwrap();
        assert_eq!(found.genesis.as_deref(), Some("fix the checkout race"));
        assert_eq!(found.branch.as_deref(), Some("fix-checkout"));
    }

    /// The ambient door: a worktree nobody told us about is a workspace.
    #[test]
    fn worktrees_created_outside_are_adopted() {
        let (temp, dir) = project();
        let path = temp.path().join("stray");
        sh(
            &dir,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "-b",
                "stray",
                path.to_str().unwrap(),
            ],
        );

        let all = discover(&dir).unwrap();
        assert_eq!(all.len(), 2);
        let stray = all.iter().find(|w| w.id == "stray").unwrap();
        assert_eq!(stray.branch.as_deref(), Some("stray"));
        assert_eq!(
            stray.genesis, None,
            "no intent was stated, and that is fine"
        );
    }

    #[test]
    fn locate_finds_the_workspace_a_path_stands_in() {
        let (_temp, dir) = project();
        let ws = create(&dir, "feature", None, None).unwrap();

        let from_worktree = locate(&ws.path).unwrap();
        assert_eq!(from_worktree.id, "feature");
        assert!(!from_worktree.is_primary);

        let from_project = locate(&dir).unwrap();
        assert!(from_project.is_primary);
    }

    #[test]
    fn discovery_from_inside_a_worktree_sees_its_siblings() {
        let (_temp, dir) = project();
        let a = create(&dir, "alpha", None, None).unwrap();
        create(&dir, "beta", None, None).unwrap();

        let mut ids: Vec<String> = discover(&a.path)
            .unwrap()
            .into_iter()
            .map(|w| w.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["alpha", "beta", "main"]);
    }

    #[test]
    fn remove_deletes_the_worktree_and_its_registration() {
        let (_temp, dir) = project();
        let ws = create(&dir, "temporary", None, None).unwrap();
        assert_eq!(discover(&dir).unwrap().len(), 2);

        remove(&ws).unwrap();
        assert!(!ws.path.exists());
        let all = discover(&dir).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].is_primary);
    }

    #[test]
    fn remove_refuses_the_primary_working_directory() {
        let (_temp, dir) = project();
        let primary = discover(&dir).unwrap().remove(0);
        assert!(remove(&primary).is_err());
        assert!(dir.is_dir(), "the project is still there");
    }

    #[test]
    fn host_labels_are_dns_safe() {
        let (_temp, dir) = project();
        let ws = create(&dir, "feat/Add_Thing", None, None).unwrap();
        assert_eq!(ws.id, "feat-add-thing");
        assert_eq!(ws.host_label("api"), "api.feat-add-thing.proj");
    }

    #[test]
    fn slugs_are_conservative() {
        assert_eq!(slug("feature/JIRA-123_fix"), "feature-jira-123-fix");
        assert_eq!(slug("---"), "workspace");
        assert_eq!(slug(""), "workspace");
        assert_eq!(slug("trailing///"), "trailing");
        assert!(!slug("/leading").starts_with('-'));
        assert_eq!(slug(&"x".repeat(100)).len(), 63);
    }

    #[test]
    fn genesis_survives_quotes_and_newlines() {
        let (_temp, dir) = project();
        let intent = "fix \"checkout\"\nand the race";
        create(&dir, "quoted", Some(intent), None).unwrap();
        let found = discover(&dir)
            .unwrap()
            .into_iter()
            .find(|w| w.id == "quoted")
            .unwrap();
        // Newlines are escaped into the marker; the first line round-trips.
        assert!(found.genesis.unwrap().contains("fix \\\"checkout\\\""));
    }

    /// The marker is our bookkeeping. If it shows up as the user's change,
    /// every workspace starts life with a spurious added file in its diff.
    #[test]
    fn the_marker_never_appears_as_a_change() {
        let (_temp, dir) = project();
        let ws = create(&dir, "quiet", Some("some intent"), None).unwrap();

        let changeset =
            crate::loader::load(&ws.path, crate::model::DiffScope::Branch, false).unwrap();
        assert!(
            !changeset
                .files
                .iter()
                .any(|f| f.path.contains(".preceipts")),
            "marker leaked into the diff: {:?}",
            changeset.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }
}
