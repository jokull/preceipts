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
use preceipts_core::workspace::{self, Workspace};
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
