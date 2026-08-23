//! `preceiptsd` — the daemon binary.

use anyhow::Result;
use clap::Parser;

use preceiptsd::cli::{Cli, Cmd};
use preceiptsd::commands;
use preceiptsd::forwarder;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Cli::parse();
    match args.cmd {
        Cmd::Serve {
            tasks,
            root,
            no_prebuild,
        } => commands::daemon_inner(root, tasks, no_prebuild),
        Cmd::Forward => forwarder::run(),
        Cmd::Route => preceiptsd::router::run(),
    }
}
