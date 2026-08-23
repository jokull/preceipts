//! `preceipts` — the lab's scriptable face.
//!
//! This is the CLI door from docs/direction-2026-08.md: the default path for
//! creating a workspace, and the one with no handoff problem at all, because
//! you were already in a terminal. It is also the beginning of the agent-facing
//! surface — every verb here is meant to be as usable by a script as by a
//! person, which is why `--json` is on the read commands from the start rather
//! than bolted on later.

mod branch;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use preceipts_core::checks::{self, CheckState};
use preceipts_core::workspace::{self, Workspace};
use preceipts_core::LandOptions;
use preceipts_core::{load, DiffScope};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "preceipts", about = "The workbench around AI coding.", version)]
struct Cli {
    /// Repository or worktree to act on. Defaults to the working directory,
    /// which is what makes every command work from inside a workspace.
    #[arg(long, short = 'C', global = true)]
    path: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a workspace: a worktree, a branch named from your intent, and
    /// that intent recorded.
    New {
        /// What this workspace is for. Becomes the branch name and is kept.
        prompt: Vec<String>,
        /// Override the derived branch name.
        #[arg(long)]
        branch: Option<String>,
        /// Where to put the worktree. Defaults to a sibling of the project.
        #[arg(long)]
        parent: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List every workspace of this project.
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Which workspace does this directory stand in?
    Where {
        #[arg(long)]
        json: bool,
    },
    /// Summarize the diff: files, lines, base.
    Diff {
        /// Compare against the working tree instead of the merge base.
        #[arg(long)]
        uncommitted: bool,
        #[arg(long)]
        json: bool,
    },
    /// Run the checks and mint receipts for the working tree.
    Run {
        /// Only these checks. Defaults to all of them.
        checks: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Receipt table for a tree.
    Status {
        /// Ref to inspect. Defaults to the working tree — the same tree `run`
        /// mints against.
        #[arg(long)]
        r#ref: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Squash-land a branch onto its base, receipts willing.
    Land {
        /// Branch to land. Defaults to the current one.
        branch: Option<String>,
        /// Base branch. Defaults to main.
        #[arg(long)]
        onto: Option<String>,
        /// Subject line. Defaults to "<branch> (squash)".
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// Land even though the base moved, recording a Receipts-Stale trailer.
        #[arg(long)]
        allow_stale: bool,
        /// Make a moved base a hard failure rather than a prompt.
        #[arg(long)]
        require_fresh: bool,
        #[arg(long)]
        no_push: bool,
        #[arg(long)]
        json: bool,
    },
    /// Every receipt recorded in this repository.
    Log {
        #[arg(long)]
        json: bool,
    },
    /// Scaffold .preceipts/ in a repository that has none.
    Init,
    /// Remove a workspace's worktree and its registration.
    Remove {
        /// Workspace id. Defaults to the one you are standing in.
        id: Option<String>,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("preceipts: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = match cli.path {
        Some(path) => path,
        None => std::env::current_dir().context("reading the working directory")?,
    };

    match cli.command {
        Command::New {
            prompt,
            branch,
            parent,
            json,
        } => new(&path, &prompt.join(" "), branch, parent.as_deref(), json),
        Command::List { json } => list(&path, json),
        Command::Where { json } => locate(&path, json),
        Command::Diff { uncommitted, json } => diff(&path, uncommitted, json),
        Command::Run { checks, json } => run_checks(&path, &checks, json),
        Command::Status { r#ref, json } => show_status(&path, r#ref.as_deref(), json),
        Command::Land {
            branch,
            onto,
            message,
            allow_stale,
            require_fresh,
            no_push,
            json,
        } => land(
            &path,
            branch,
            LandOptions {
                onto,
                message,
                allow_stale,
                require_fresh,
                no_push,
            },
            json,
        ),
        Command::Log { json } => log(&path, json),
        Command::Init => init(&path),
        Command::Remove { id } => remove(&path, id),
    }
}

fn new(
    path: &Path,
    prompt: &str,
    branch: Option<String>,
    parent: Option<&Path>,
    json: bool,
) -> Result<()> {
    let name = match branch {
        Some(branch) => branch,
        None if prompt.trim().is_empty() => {
            bail!("say what the workspace is for, or pass --branch")
        }
        None => branch::from_prompt(prompt),
    };
    let genesis = (!prompt.trim().is_empty()).then_some(prompt);

    let workspace = workspace::create(path, &name, genesis, parent)
        .with_context(|| format!("creating workspace {name}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&as_json(&workspace))?);
    } else {
        println!("{}", workspace.path.display());
        eprintln!(
            "workspace {} on branch {}",
            workspace.id,
            workspace.branch.as_deref().unwrap_or("?")
        );
        // The path goes to stdout alone so `cd "$(preceipts new ...)"` works.
        // Everything else is commentary and belongs on stderr.
    }
    Ok(())
}

fn list(path: &Path, json: bool) -> Result<()> {
    let all = workspace::discover(path).context("discovering workspaces")?;
    if json {
        let rows: Vec<_> = all.iter().map(as_json).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for workspace in &all {
        let marker = if workspace.is_primary { "*" } else { " " };
        let branch = workspace.branch.as_deref().unwrap_or("(detached)");
        println!("{marker} {:<24} {branch}", workspace.id);
        if let Some(genesis) = &workspace.genesis {
            println!("    {genesis}");
        }
    }
    Ok(())
}

fn locate(path: &Path, json: bool) -> Result<()> {
    let workspace = workspace::locate(path).context("locating a workspace")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&as_json(&workspace))?);
    } else {
        println!("{}", workspace.id);
    }
    Ok(())
}

fn diff(path: &Path, uncommitted: bool, json: bool) -> Result<()> {
    let scope = if uncommitted {
        DiffScope::Uncommitted
    } else {
        DiffScope::Branch
    };
    // No highlighting: nothing here renders, and the parse is the expensive
    // part of a load.
    let changeset = load(path, scope, false).context("loading the changeset")?;

    if json {
        let files: Vec<_> = changeset
            .files
            .iter()
            .map(|f| {
                json!({
                    "path": f.path,
                    "status": f.status.code().to_string(),
                    "added": f.added,
                    "removed": f.removed,
                    "binary": f.is_binary,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "base": changeset.base_name,
                "branch": changeset.branch,
                "added": changeset.total_added(),
                "removed": changeset.total_removed(),
                "files": files,
            }))?
        );
        return Ok(());
    }

    for file in &changeset.files {
        println!(
            "{} {:<48} +{} −{}",
            file.status.code(),
            file.path,
            file.added,
            file.removed
        );
    }
    println!(
        "{} files, +{} −{} vs {}",
        changeset.files.len(),
        changeset.total_added(),
        changeset.total_removed(),
        changeset.base_name
    );
    Ok(())
}

fn remove(path: &Path, id: Option<String>) -> Result<()> {
    let target = match id {
        Some(id) => workspace::discover(path)?
            .into_iter()
            .find(|w| w.id == id)
            .with_context(|| format!("no workspace {id}"))?,
        None => workspace::locate(path).context("locating a workspace")?,
    };
    workspace::remove(&target).with_context(|| format!("removing workspace {}", target.id))?;
    eprintln!("removed {}", target.id);
    Ok(())
}

fn run_checks(path: &Path, only: &[String], json: bool) -> Result<()> {
    let only = (!only.is_empty()).then_some(only);
    let report = checks::run(path, only).context("running checks")?;

    if json {
        let rows: Vec<_> = report
            .outcomes
            .iter()
            .map(|o| {
                json!({
                    "check": o.name,
                    "ok": o.ok,
                    "exit": o.exit,
                    "duration_ms": o.duration.as_millis() as u64,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "tree": report.tree,
                "dirty": report.dirty,
                "checks": rows,
            }))?
        );
        return Ok(());
    }

    for note in &report.prepared {
        eprintln!("{note}");
    }
    for outcome in &report.outcomes {
        println!(
            "{} {:<20} {:>6}ms",
            if outcome.ok { "✓" } else { "✗" },
            outcome.name,
            outcome.duration.as_millis()
        );
        if !outcome.ok {
            for line in outcome.output.lines().take(20) {
                println!("    {line}");
            }
        }
    }
    println!(
        "tree {}{}",
        &report.tree[..12],
        if report.dirty { " (dirty)" } else { "" }
    );
    // A failing check is a failing command: scripts and agents should be able
    // to gate on the exit code without parsing anything.
    if report.outcomes.iter().any(|o| !o.ok) {
        std::process::exit(1);
    }
    Ok(())
}

fn show_status(path: &Path, reference: Option<&str>, json: bool) -> Result<()> {
    let status = checks::status(path, reference).context("computing status")?;

    if json {
        let rows: Vec<_> = status
            .rows
            .iter()
            .map(|row| {
                json!({
                    "check": row.check,
                    "required": row.required,
                    "state": row.state.as_str(),
                    "ok": row.receipt.as_ref().map(|r| r.ok),
                    "started": row.receipt.as_ref().map(|r| r.started.clone()),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ref": status.reference,
                "tree": status.tree,
                "green": status.green,
                "checks": rows,
            }))?
        );
        return Ok(());
    }

    for row in &status.rows {
        let mark = match row.state {
            CheckState::Ok => "✓",
            CheckState::Fail => "✗",
            CheckState::Missing => "·",
            CheckState::StaleDefinition => "~",
        };
        let required = if row.required { "required" } else { "" };
        println!(
            "{mark} {:<20} {:<18} {required}",
            row.check,
            row.state.as_str()
        );
    }
    println!(
        "{} — tree {} — {}",
        status.reference,
        &status.tree[..12],
        if status.green { "green" } else { "not green" }
    );
    if !status.green {
        std::process::exit(1);
    }
    Ok(())
}

fn land(path: &Path, branch: Option<String>, options: LandOptions, json: bool) -> Result<()> {
    let branch = match branch {
        Some(branch) => branch,
        None => {
            let repo = preceipts_core::GitReader::open(path).context("opening the repository")?;
            repo.head_branch()
                .context("resolving HEAD")?
                .0
                .context("HEAD is detached — name the branch to land")?
        }
    };
    let result = preceipts_core::land(path, &branch, &options).context("landing")?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "branch": result.branch,
                "base": result.base,
                "tree": result.tree,
                "commit": result.commit,
                "pushed": result.pushed,
                "stale": result.stale.as_ref().map(|s| json!({
                    "merge_base": s.merge_base,
                    "base_head": s.base_head,
                })),
            }))?
        );
        return Ok(());
    }

    println!("{}", result.commit);
    eprintln!(
        "landed {} onto {} — tree {}{}",
        result.branch,
        result.base,
        &result.tree[..12],
        if result.pushed { ", pushed" } else { "" }
    );
    if result.stale.is_some() {
        eprintln!("note: the base had moved; a Receipts-Stale trailer records it");
    }
    Ok(())
}

fn log(path: &Path, json: bool) -> Result<()> {
    let repo = preceipts_core::notes::open(path).context("opening the repository")?;
    let mut receipts = preceipts_core::notes::read_all_receipts(&repo);
    receipts.sort_by(|a, b| b.started.cmp(&a.started));

    if json {
        println!("{}", serde_json::to_string_pretty(&receipts)?);
        return Ok(());
    }
    for receipt in &receipts {
        println!(
            "{} {} {:<16} {:<12} {}",
            if receipt.ok { "✓" } else { "✗" },
            receipt.started,
            receipt.check,
            preceipts_core::land::format_duration(receipt.duration_ms),
            &receipt.tree[..12]
        );
    }
    Ok(())
}

fn init(path: &Path) -> Result<()> {
    let created = checks::init(path).context("scaffolding .preceipts/")?;
    if created.is_empty() {
        eprintln!("already set up");
    }
    for path in created {
        println!("{}", path.display());
    }
    Ok(())
}

fn as_json(workspace: &Workspace) -> serde_json::Value {
    json!({
        "id": workspace.id,
        "path": workspace.path,
        "project": workspace.project_root,
        "branch": workspace.branch,
        "genesis": workspace.genesis,
        "primary": workspace.is_primary,
    })
}
