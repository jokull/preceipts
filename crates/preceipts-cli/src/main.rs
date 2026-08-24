//! `preceipts` — the lab's scriptable face.
//!
//! This is the CLI door from docs/direction-2026-08.md: the default path for
//! creating a workspace, and the one with no handoff problem at all, because
//! you were already in a terminal. It is also the beginning of the agent-facing
//! surface — every verb here is meant to be as usable by a script as by a
//! person, which is why `--json` is on the read commands from the start rather
//! than bolted on later.

mod branch;
mod mcp;

use anyhow::{bail, Context, Result};
use preceipts_core::checks::{self, CheckState};
use preceipts_core::workspace::{self, Workspace};
use preceipts_core::LandOptions;
use preceipts_core::{load, DiffScope};
use serde_json::json;
use std::path::{Path, PathBuf};
use usage::{Args, Cli, Subcommands};

#[derive(Cli)]
#[usage(
    bin = "preceipts",
    about = "The workbench around AI coding.",
    version,
    // Left alone, a flag this CLI does not know becomes a positional *value*:
    // `preceipts new --brnach x` would quietly file the typo as part of the
    // prompt. Every verb here is meant to be scripted, and a script cannot
    // notice that. Say so instead.
    unknown_flags = "error"
)]
struct Cli {
    /// Repository or worktree to act on. Defaults to the working directory,
    /// which is what makes every command work from inside a workspace.
    #[usage(long, short = 'C', global = true)]
    path: Option<PathBuf>,

    /// Keychain database to store/read project secrets in. Defaults to the
    /// default keychain; also settable with PRECEIPTS_KEYCHAIN.
    #[usage(long, short = 'k', global = true)]
    keychain: Option<String>,

    #[usage(subcommand)]
    command: Command,
}

#[derive(Subcommands)]
enum Command {
    /// Create a workspace: a worktree, a branch named from your intent, and
    /// that intent recorded.
    New {
        /// What this workspace is for. Becomes the branch name and is kept.
        prompt: Vec<String>,
        /// Override the derived branch name.
        #[usage(long)]
        branch: Option<String>,
        /// Where to put the worktree. Defaults to a sibling of the project.
        #[usage(long)]
        parent: Option<PathBuf>,
        #[usage(long)]
        json: bool,
    },
    /// List every workspace of this project.
    #[usage(aliases = ["ls"])]
    List {
        #[usage(long)]
        json: bool,
    },
    /// Which workspace does this directory stand in?
    Where {
        #[usage(long)]
        json: bool,
    },
    /// Summarize the diff: files, lines, base.
    Diff {
        /// Compare against the working tree instead of the merge base.
        #[usage(long)]
        uncommitted: bool,
        #[usage(long)]
        json: bool,
    },
    /// Run the checks and mint receipts for the working tree.
    Run {
        /// Only these checks. Defaults to all of them.
        checks: Vec<String>,
        /// Hand the run to the daemon and return at once. Same checks, same
        /// lock, same receipts — it just does not hold this terminal.
        #[usage(long)]
        detach: bool,
        #[usage(long)]
        json: bool,
    },
    /// Receipt table for a tree.
    Status {
        /// Ref to inspect. Defaults to the working tree — the same tree `run`
        /// mints against.
        #[usage(long)]
        r#ref: Option<String>,
        #[usage(long)]
        json: bool,
    },
    /// Squash-land a branch onto its base, receipts willing.
    Land {
        /// Branch to land. Defaults to the current one.
        branch: Option<String>,
        /// Base branch. Defaults to main.
        #[usage(long)]
        onto: Option<String>,
        /// Subject line. Defaults to "<branch> (squash)".
        #[usage(long, short = 'm')]
        message: Option<String>,
        /// Land even though the base moved, recording a Receipts-Stale trailer.
        #[usage(long)]
        allow_stale: bool,
        /// Make a moved base a hard failure rather than a prompt.
        #[usage(long)]
        require_fresh: bool,
        #[usage(long)]
        no_push: bool,
        #[usage(long)]
        json: bool,
    },
    /// Every receipt recorded in this repository.
    Log {
        #[usage(long)]
        json: bool,
    },
    /// Validate preceipts.toml and explain what is wrong with it.
    Doctor {
        #[usage(long)]
        json: bool,
    },
    /// Run the checks whenever the worktree goes quiet.
    Watch {
        /// Seconds the tree must hold still before checks fire.
        #[usage(long, default = "2")]
        quiet: u64,
        /// Only these checks.
        checks: Vec<String>,
    },
    /// Whether the daemon is running checks here, and on what.
    Checks {
        #[usage(long)]
        json: bool,
    },
    /// Serve the lab's instruments over MCP on stdio.
    Mcp,
    /// Scaffold .preceipts/ in a repository that has none.
    Init,
    /// Retire procpane: convert its manifest, move its secrets, clean up.
    Migrate {
        /// Show what would change without changing anything.
        #[usage(long)]
        dry_run: bool,
    },
    /// Share receipts with origin: fetch, merge losslessly, push.
    Sync {
        #[usage(long)]
        json: bool,
    },
    /// Prune stored logs whose receipts have aged out. Receipts are permanent.
    Gc {
        /// How long a passing check's log is kept.
        #[usage(long, default = "30d")]
        keep_success: String,
        /// How long a failing check's log is kept — longer, because those are
        /// the ones someone comes back to.
        #[usage(long, default = "90d")]
        keep_failure: String,
        #[usage(long)]
        json: bool,
    },
    /// Remove a workspace's worktree and its registration.
    Remove {
        /// Workspace id. Defaults to the one you are standing in.
        id: Option<String>,
    },

    // ---- the environment -------------------------------------------------
    // These came from the absorbed daemon. They are here rather than on a
    // second binary
    // because the lab has one door: anything the app can do, `preceipts` can
    // do, and an agent should not have to learn which tool owns which verb.
    /// Bring the project's services up, healthcheck-gated, in the background.
    #[usage(aliases = ["start"])]
    Up {
        /// Task names (`dev`) or qualified ids (`web#dev`). Omit for every
        /// task the project declares.
        tasks: Vec<String>,
        /// Run in the foreground rather than detaching.
        #[usage(long)]
        foreground: bool,
        /// Skip the prebuild step for non-persistent dependencies.
        #[usage(long)]
        no_prebuild: bool,
    },
    /// Stop the daemon for this project.
    #[usage(aliases = ["stop"])]
    Down,
    /// What is running, and is it healthy.
    Services {
        #[usage(long)]
        json: bool,
        /// Include background builders, not just declared services.
        #[usage(long)]
        all: bool,
    },
    /// Block until a service is healthy. 0 = healthy, 1 = failed, 2 = timeout.
    WaitFor {
        /// Task id, e.g. `api#dev`.
        name: String,
        #[usage(long, default = "5m")]
        timeout: String,
    },
    /// Per-service operations: logs, restart, stop.
    Proc(ProcArgs),
    /// Every HTTP request the proxy carried, and what answered it.
    Requests {
        /// Only this hostname.
        #[usage(long)]
        host: Option<String>,
        /// Only the last `2m`, `30s`, `1h`.
        #[usage(long)]
        since: Option<String>,
        #[usage(long)]
        json: bool,
    },
    /// Search every service's output at once.
    Grep {
        pattern: String,
        #[usage(short = 'A', long, default_value_t = 0, default = "0")]
        after: usize,
        #[usage(short = 'B', long, default_value_t = 0, default = "0")]
        before: usize,
        #[usage(long)]
        json: bool,
    },
    /// Project secrets in the macOS Keychain.
    #[usage(aliases = ["env"])]
    Secrets {
        #[usage(subcommand)]
        op: preceiptsd::cli::EnvOp,
    },
    /// The local Certificate Authority, and the :443 forwarder.
    Trust {
        #[usage(subcommand)]
        op: preceiptsd::cli::TrustOp,
    },
}

// `preceipts proc web#dev tail`: the service name, then the operation. Its own
// struct rather than fields on the variant, because that shape needs
// `subcommand_precedence_over_arg`, and only a command declaration can say it —
// a word selects a subcommand only while this command's positionals are still
// empty, so `name` would take the slot and `tail` would have nowhere to go.
// A doc comment here would replace the variant's help text, hence `//`.
//
// The cost: a service actually named `tail`, `grep`, `since` or `signal`
// cannot be addressed this way — the first word routes as the operation and
// the invocation fails with "unexpected argument". It fails loudly rather
// than acting on the wrong service, which is the trade worth taking.
#[derive(Args)]
#[usage(subcommand_precedence_over_arg = true)]
struct ProcArgs {
    name: String,
    #[usage(subcommand)]
    op: preceiptsd::cli::ProcOp,
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

    let keychain = cli
        .keychain
        .or_else(|| std::env::var("PRECEIPTS_KEYCHAIN").ok());

    match cli.command {
        Command::Up {
            tasks,
            foreground,
            no_prebuild,
        } => preceiptsd::commands::run_cmd(path, tasks, foreground, no_prebuild),
        Command::Down => preceiptsd::commands::stop_cmd(path),
        Command::Services { json, all } => preceiptsd::commands::status_cmd(path, json, all),
        Command::WaitFor { name, timeout } => {
            preceiptsd::commands::wait_for_cmd(path, name, timeout)
        }
        Command::Proc(ProcArgs { name, op }) => preceiptsd::commands::proc_cmd(path, name, op),
        Command::Requests { host, since, json } => {
            preceiptsd::commands::requests_cmd(path, host, since, json)
        }
        Command::Grep {
            pattern,
            after,
            before,
            json,
        } => preceiptsd::commands::grep_cmd(path, pattern, before, after, json),
        Command::Secrets { op } => preceiptsd::commands::env_cmd(path, op, keychain.as_deref()),
        Command::Trust { op } => preceiptsd::commands::trust_cmd(op),

        Command::New {
            prompt,
            branch,
            parent,
            json,
        } => new(&path, &prompt.join(" "), branch, parent.as_deref(), json),
        Command::List { json } => list(&path, json),
        Command::Where { json } => locate(&path, json),
        Command::Diff { uncommitted, json } => diff(&path, uncommitted, json),
        Command::Run {
            checks,
            detach,
            json,
        } => {
            if detach {
                preceiptsd::commands::run_detached_cmd(path, checks)
            } else {
                run_checks(&path, &checks, json)
            }
        }
        Command::Checks { json } => preceiptsd::commands::checks_cmd(path, json),
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
        Command::Doctor { json } => doctor(&path, json),
        Command::Watch { quiet, checks } => watch(&path, quiet, checks),
        Command::Mcp => mcp::serve(&path),
        Command::Init => init(&path),
        Command::Migrate { dry_run } => migrate(&path, dry_run, keychain.as_deref()),
        Command::Sync { json } => sync(&path, json),
        Command::Gc {
            keep_success,
            keep_failure,
            json,
        } => gc(&path, &keep_success, &keep_failure, json),
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
                    "fidelity": row.receipt.as_ref().map(|r| r.fidelity.clone()),
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

    // Green is a weaker word when half the environment was a stand-in, so the
    // qualification sits next to the verdict rather than a flag away from it.
    // Only the services that are not fully real are named: listing every
    // local-real service would bury the one line that matters.
    let qualified: std::collections::BTreeSet<String> = status
        .rows
        .iter()
        .filter_map(|row| row.receipt.as_ref())
        .flat_map(|receipt| receipt.fidelity.iter())
        .filter(|(_, fidelity)| fidelity.as_str() != "local-real")
        .map(|(service, fidelity)| format!("{service} {fidelity}"))
        .collect();
    if !qualified.is_empty() {
        println!(
            "  with {}",
            qualified.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
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

/// The north-star loop: the agent stops typing, checks fire in the already-warm
/// workspace, the answer arrives without a push or a queue.
fn watch(path: &Path, quiet: u64, only: Vec<String>) -> Result<()> {
    use preceipts_core::watch::{self, Event};
    use std::time::Duration;

    let only = (!only.is_empty()).then_some(only);
    let watched = watch::watched_paths(path).len();
    eprintln!(
        "watching {watched} files in {} — checks fire after {quiet}s of quiet",
        path.display()
    );

    watch::watch(path, Duration::from_secs(quiet), |event| {
        match event {
            Event::Busy => eprintln!("…"),
            Event::Quiet => {
                match checks::run(path, only.as_deref()) {
                    Ok(report) => {
                        let failed: Vec<&str> = report
                            .outcomes
                            .iter()
                            .filter(|o| !o.ok)
                            .map(|o| o.name.as_str())
                            .collect();
                        if failed.is_empty() {
                            println!("✓ green — tree {}", &report.tree[..12]);
                        } else {
                            println!("✗ {} — tree {}", failed.join(", "), &report.tree[..12]);
                        }
                    }
                    // A failed run must not end the watch: the usual cause is
                    // a half-saved file, and the next quiet period fixes it.
                    Err(error) => eprintln!("run failed: {error}"),
                }
            }
        }
        true
    })
    .context("watching")?;
    Ok(())
}

fn doctor(path: &Path, json: bool) -> Result<()> {
    use preceipts_core::manifest;

    let (manifest, source) = match manifest::load(path).context("loading preceipts.toml")? {
        Some(manifest) => (Some(manifest), "preceipts.toml"),
        // No file is a valid state — rung 0 of the schema. Say what would be
        // assumed rather than complaining that a file is missing.
        None => (manifest::detect(path), "detected"),
    };

    let Some(manifest) = manifest else {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "source": "none",
                    "problems": ["nothing to run: no preceipts.toml, and no dev script or Cargo.toml to infer one from"],
                }))?
            );
        } else {
            println!(
                "no preceipts.toml, and nothing obvious to infer — write one, or add a dev script"
            );
        }
        std::process::exit(1);
    };

    let problems = manifest.problems();
    // The cache half. turbo.json is a tracked file, so the copy that matters
    // is this worktree's — a branch may well be the one changing it, and
    // reading the primary worktree's copy would report on the wrong tree.
    let project_root = workspace::locate(path)
        .map(|ws| ws.path.clone())
        .unwrap_or_else(|_| path.to_path_buf());
    let cache = match preceipts_core::turbo::load(&project_root)? {
        Some(turbo) => manifest.cache_findings(&turbo),
        None => Vec::new(),
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "source": source,
                "services": manifest.services.iter().map(|s| json!({
                    "name": s.name,
                    "runtime": s.runtime.as_str(),
                    "fidelity": s.fidelity.as_str(),
                    "host": s.host,
                    "needs": s.needs,
                })).collect::<Vec<_>>(),
                "actions": manifest.actions.iter().map(|a| &a.name).collect::<Vec<_>>(),
                "drains": manifest.drains.iter().map(|d| &d.name).collect::<Vec<_>>(),
                "problems": problems,
                "unsupported": manifest.unsupported(),
                "cache": cache.iter().map(|f| json!({
                    "key": f.key,
                    "message": f.message,
                    "silent": f.silent,
                })).collect::<Vec<_>>(),
            }))?
        );
        if !problems.is_empty() {
            std::process::exit(1);
        }
        return Ok(());
    }

    println!("{source}");
    if let Ok(order) = manifest.boot_order() {
        for service in order {
            let host = service
                .host
                .as_deref()
                .map(|h| format!("  {h}.<workspace>"))
                .unwrap_or_default();
            println!(
                "  {:<18} {:<10} {:<16}{host}",
                service.name,
                service.runtime.as_str(),
                service.fidelity.as_str()
            );
        }
    }
    if !manifest.actions.is_empty() {
        let names: Vec<&str> = manifest.actions.iter().map(|a| a.name.as_str()).collect();
        println!("  actions: {}", names.join(", "));
    }
    if !manifest.drains.is_empty() {
        let names: Vec<&str> = manifest.drains.iter().map(|d| d.name.as_str()).collect();
        println!("  drains:  {}", names.join(", "));
    }

    let unsupported = manifest.unsupported();
    if problems.is_empty() && cache.is_empty() && unsupported.is_empty() {
        println!("no problems");
        return Ok(());
    }
    println!();
    for problem in &problems {
        println!("✗ {problem}");
    }
    // Capability gaps read differently from mistakes: nothing is wrong with
    // the file, we simply cannot honour it yet. Exiting non-zero over one
    // would punish an author for describing their stack accurately.
    for gap in manifest.unsupported() {
        println!("· {gap}");
    }
    // Cache findings are warnings, not errors: they are about turbo.json,
    // which this tool does not own. Saying so and exiting 0 is the difference
    // between a useful neighbour and one that fails your build over its
    // opinion. The silent ones lead, because a wrong cache hit costs more
    // than a miss.
    let mut cache = cache;
    cache.sort_by_key(|f| !f.silent);
    for finding in &cache {
        println!("! {}", finding.message);
    }
    if problems.is_empty() {
        return Ok(());
    }
    std::process::exit(1);
}

/// One-time migration off procpane. **Scaffolding — delete this verb once the
/// machines that need it have run it.**
///
/// This project has no users to keep compatible, so nothing here is a
/// compatibility layer: it is a one-shot tool for the handful of repositories
/// and keychains that predate the rename. Everything it does is something a
/// person would otherwise do by hand and get subtly wrong — the manifest is a
/// different *shape*, not a different spelling, and the secrets are real
/// values that exist nowhere else. It earns its place by being the reason the
/// old name could be removed everywhere else, rather than read forever.
fn migrate(path: &Path, dry_run: bool, keychain: Option<&str>) -> Result<()> {
    use preceipts_core::migrate as convert;

    // The directory you are standing in wins when it has the old file.
    //
    // Resolving to the project root first is right for secrets, which belong
    // to the repository — but it means a project nested inside another repo,
    // or a plain directory that is not a repo at all, reports "no
    // procpane.toml here" while staring straight at one. Convert what is in
    // front of you; fall back to the project root when there is nothing.
    let project_root = workspace::locate(path)
        .map(|ws| ws.project_root)
        .unwrap_or_else(|_| path.to_path_buf());
    let root = if path.join("procpane.toml").is_file() {
        path.to_path_buf()
    } else {
        project_root
    };
    let mut did_something = false;

    match convert::from_procpane(&root)? {
        None => println!("no procpane.toml here"),
        Some(conversion) => {
            let target = root.join("preceipts.toml");
            if target.exists() {
                println!("preceipts.toml already exists — leaving both files alone");
                println!("  delete procpane.toml when you are satisfied the new one is right");
            } else if dry_run {
                println!("would write {}:\n", target.display());
                println!("{}", conversion.manifest);
            } else {
                std::fs::write(&target, &conversion.manifest)
                    .with_context(|| format!("writing {}", target.display()))?;
                // The old file is left on disk rather than deleted. It is
                // still in git, it costs nothing, and a conversion a person
                // has not read yet is not one they have accepted.
                println!("wrote {}", target.display());
                println!("  procpane.toml is untouched; delete it once you have read the new one");
                did_something = true;
            }
            for note in &conversion.notes {
                println!("  ! {note}");
            }
        }
    }

    if dry_run {
        let legacy = preceiptsd::secrets::legacy_service_name(&root);
        match preceiptsd::secrets::list_accounts(&legacy, keychain) {
            Ok(accounts) if !accounts.is_empty() => {
                println!(
                    "would move {} secret(s) to the new namespace:",
                    accounts.len()
                );
                for account in accounts {
                    println!("  {account}");
                }
            }
            _ => println!("no secrets under the old namespace"),
        }
        return Ok(());
    }

    match preceiptsd::secrets::migrate_namespace(&root, keychain) {
        Ok(moved) if moved.is_empty() => {}
        Ok(moved) => {
            println!(
                "moved {} secret(s) into the preceipts namespace",
                moved.len()
            );
            did_something = true;
        }
        Err(error) => eprintln!("could not move secrets: {error:#}"),
    }

    if !did_something {
        println!("nothing left to migrate");
    }
    Ok(())
}

fn sync(path: &Path, json: bool) -> Result<()> {
    let report = preceipts_core::sync::sync(path).context("syncing receipts")?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "merged": report.merged,
                "pushed": report.pushed,
            }))?
        );
        return Ok(());
    }
    // Say what changed, not that a command ran. "nothing to do" is a real and
    // common answer, and deserves to be said plainly.
    match (report.merged, report.pushed) {
        (false, false) => println!("already in sync"),
        (true, false) => println!("merged receipts from origin"),
        (false, true) => println!("pushed receipts to origin"),
        (true, true) => println!("merged receipts from origin, and pushed ours back"),
    }
    Ok(())
}

fn gc(path: &Path, keep_success: &str, keep_failure: &str, json: bool) -> Result<()> {
    let parse = |text: &str, flag: &str| -> Result<u64> {
        checks::parse_duration(text)
            .map(|d| d.as_secs())
            .map_err(|e| anyhow::anyhow!("--{flag}: {e}"))
    };
    let keep_success = parse(keep_success, "keep-success")?;
    let keep_failure = parse(keep_failure, "keep-failure")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let report =
        preceipts_core::sync::gc(path, keep_success, keep_failure, now).context("pruning logs")?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "deleted": report.deleted,
                "kept": report.kept,
                "orphans": report.orphans,
            }))?
        );
        return Ok(());
    }
    println!(
        "{} log{} pruned, {} kept",
        report.deleted.len(),
        if report.deleted.len() == 1 { "" } else { "s" },
        report.kept
    );
    if report.orphans > 0 {
        // Reported rather than swept: a log no receipt points at is more
        // likely a bug here than garbage.
        println!(
            "  {} log{} referenced by no receipt, left alone",
            report.orphans,
            if report.orphans == 1 { "" } else { "s" }
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

/// The faces must not drift.
///
/// Nothing here checks that two implementations agree, because there are no
/// two implementations: every read is `preceipts-core`, every environment fact
/// is one socket, and a check run is `checks::run_with` wherever it is called
/// from. What *can* drift is reach — a verb that exists on the wire and on one
/// face only, which is exactly how the app ended up unable to run checks while
/// both agent-facing surfaces could.
///
/// So this asserts reach, not behaviour. The `match` is the load-bearing part:
/// it is exhaustive, so a new wire verb cannot be added without someone
/// writing down which faces are supposed to offer it.
#[cfg(test)]
mod parity {
    use preceiptsd::proto::Request;

    enum Face {
        /// Not something anyone asks for by name — plumbing between a client
        /// and the daemon, with a verb of its own nowhere. The reason is the
        /// point of the variant: it is read by whoever adds the next verb and
        /// wonders why this one got a pass.
        Internal(#[allow(dead_code)] &'static str),
        Public {
            cli: &'static str,
            /// `None` records a deliberate absence: the CLI is the agent's
            /// door for this one. It is not an oversight, and writing it down
            /// is the difference between the two.
            mcp: Option<&'static str>,
        },
    }

    fn face(request: &Request) -> Face {
        match request {
            Request::Ping => Face::Internal("liveness"),
            Request::GetTask { .. } => Face::Internal("the polling half of wait-for"),
            Request::Status => Face::Public {
                cli: "services",
                mcp: Some("environment"),
            },
            Request::Stop => Face::Public {
                cli: "down",
                mcp: None,
            },
            Request::Tail { .. } | Request::Since { .. } => Face::Public {
                cli: "proc",
                mcp: None,
            },
            Request::Grep { .. } => Face::Public {
                cli: "grep",
                mcp: None,
            },
            Request::Signal { .. } => Face::Public {
                cli: "proc",
                mcp: None,
            },
            Request::Transcript { .. } => Face::Public {
                cli: "requests",
                mcp: Some("requests"),
            },
            Request::Run { .. } | Request::Checks => Face::Public {
                cli: "checks",
                mcp: Some("checks"),
            },
        }
    }

    /// A distinct index per variant, so the list below can be checked rather
    /// than trusted.
    ///
    /// Without this the exhaustive `match` in `face` forces a *decision* about
    /// a new verb but not a *test* of it: the arm could exist while the list
    /// missed the variant, and the assertions would quietly cover one verb
    /// fewer. This match is exhaustive too, and the test asserts every index
    /// appears exactly once — so a new variant fails to compile here, and a
    /// variant left out of the list fails the test there.
    /// How many variants `index` hands out. Fixed rather than derived from
    /// the list, which is the whole point: a list missing a variant is exactly
    /// what this is here to catch, so it cannot be allowed to shorten the
    /// thing it is compared against.
    const VARIANTS: usize = 11;

    fn index(request: &Request) -> usize {
        match request {
            Request::Ping => 0,
            Request::Status => 1,
            Request::Stop => 2,
            Request::GetTask { .. } => 3,
            Request::Tail { .. } => 4,
            Request::Since { .. } => 5,
            Request::Grep { .. } => 6,
            Request::Signal { .. } => 7,
            Request::Transcript { .. } => 8,
            Request::Run { .. } => 9,
            Request::Checks => 10,
        }
    }

    /// One value per variant, guarded by [`index`].
    fn every_request() -> Vec<Request> {
        vec![
            Request::Ping,
            Request::Status,
            Request::Stop,
            Request::GetTask {
                name: String::new(),
            },
            Request::Tail {
                name: String::new(),
                lines: 0,
            },
            Request::Since {
                name: String::new(),
                cursor: 0,
            },
            Request::Grep {
                name: None,
                pattern: String::new(),
                before: 0,
                after: 0,
            },
            Request::Signal {
                name: String::new(),
                signal: String::new(),
            },
            Request::Transcript {
                host: None,
                since_secs: None,
            },
            Request::Run { checks: None },
            Request::Checks,
        ]
    }

    #[test]
    fn the_list_covers_every_wire_verb() {
        let mut seen: Vec<usize> = every_request().iter().map(index).collect();
        seen.sort_unstable();
        let expected: Vec<usize> = (0..VARIANTS).collect();
        assert_eq!(
            seen, expected,
            "every_request() must hold each variant exactly once — a verb \
             missing from it is a verb the parity assertions never see"
        );
    }

    #[test]
    fn every_public_wire_verb_has_a_cli_door() {
        let spec = super::Cli::spec();
        let verbs: Vec<&str> = spec
            .root
            .cmd
            .subcommands
            .iter()
            .map(|command| command.name)
            .collect();
        for request in every_request() {
            if let Face::Public { cli, .. } = face(&request) {
                assert!(
                    verbs.contains(&cli),
                    "the daemon answers {request:?} but `preceipts {cli}` does not exist"
                );
            }
        }
    }

    #[test]
    fn every_declared_mcp_tool_exists() {
        let tools = crate::mcp::tools(std::path::Path::new("."));
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for request in every_request() {
            if let Face::Public { mcp: Some(mcp), .. } = face(&request) {
                assert!(
                    names.contains(&mcp),
                    "{request:?} is declared to have the MCP tool `{mcp}`, which is not served"
                );
            }
        }
    }
}
