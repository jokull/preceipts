use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "procpane",
    version,
    about = "Turborepo sidecar with batteries-included dev DX: healthcheck-gated services, local HTTPS, Keychain secrets."
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,

    /// Override repo root (defaults to nearest turbo.json ancestor)
    #[arg(long, global = true)]
    pub cwd: Option<std::path::PathBuf>,

    /// Keychain database to store/read repo secrets in. Defaults to the
    /// default keychain; can also be set via PROCPANE_KEYCHAIN.
    #[arg(long, short = 'k', global = true)]
    pub keychain: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Launch tasks in the background and return immediately.
    ///
    /// With no tasks given, brings up every task declared in
    /// `procpane.toml` — the "service manifest" mode. This is the
    /// recommended way to start your dev stack:
    ///
    ///     procpane up
    ///
    /// You can also pass explicit task names (`dev`) or qualified ids
    /// (`web#dev`, `@trip/api#dev`); this bypasses the sidecar and runs
    /// just what you asked for.
    ///
    /// Alias: `run` (kept for backward compatibility).
    #[command(alias = "run")]
    Up {
        /// Task names (`dev`) or qualified ids (`web#dev`). Omit to use
        /// every task declared in `procpane.toml`.
        tasks: Vec<String>,
        /// Run in the foreground (do not detach)
        #[arg(long)]
        foreground: bool,
        /// Skip the `turbo run` prebuild step for non-persistent deps
        #[arg(long)]
        no_prebuild: bool,
    },
    /// Block until a task becomes healthy (or fails). Exit 0 = healthy, 1 = failed, 2 = timeout.
    WaitFor {
        /// Task id (e.g. `api#dev` or `@demo/api#dev`).
        name: String,
        /// Max time to wait. Examples: `30s`, `2m`.
        #[arg(long, default_value = "5m")]
        timeout: String,
    },
    /// List running processes
    Status {
        /// JSON output
        #[arg(long)]
        json: bool,
        /// Include non-persistent dep procs (workspace `^build` etc.). By
        /// default only persistent tasks and the prebuild proc are shown.
        /// `--json` always includes everything regardless of this flag.
        #[arg(long)]
        all: bool,
    },
    /// Stop the running daemon
    Stop,
    /// Per-process operations
    Proc {
        name: String,
        #[command(subcommand)]
        op: ProcOp,
    },
    /// Manage repo-scoped secrets stored in the macOS Keychain.
    Env {
        #[command(subcommand)]
        op: EnvOp,
    },
    /// Manage the local Certificate Authority for local HTTPS URLs.
    Trust {
        #[command(subcommand)]
        op: TrustOp,
    },
    /// Cross-task grep
    Grep {
        pattern: String,
        #[arg(short = 'A', long, default_value_t = 0)]
        after: usize,
        #[arg(short = 'B', long, default_value_t = 0)]
        before: usize,
        #[arg(long)]
        json: bool,
    },
    /// Internal: daemon entry point (invoked by `up`).
    #[command(hide = true)]
    DaemonInner {
        tasks: Vec<String>,
        #[arg(long)]
        root: std::path::PathBuf,
        #[arg(long)]
        no_prebuild: bool,
    },
    /// Internal: root-owned loopback :443 → :8443 TCP forwarder.
    #[command(hide = true)]
    PrettyUrlProxy,
}

#[derive(Subcommand, Debug)]
pub enum TrustOp {
    /// Generate the local CA (if missing) and install it into the System keychain.
    /// Prompts for `sudo` (Touch ID works if pam_tid is enabled).
    Install {
        /// Also configure hostname entries and a root-owned loopback :443 proxy. Optional.
        #[arg(long)]
        pretty_urls: bool,
    },
    /// Remove the local CA from the System keychain and delete its files.
    /// Also tears down `--pretty-urls` if it was installed.
    Uninstall,
    /// Show CA status (installed? trusted? expires when?).
    Status,
}

#[derive(Subcommand, Debug)]
pub enum EnvOp {
    /// Set a secret value (prompts if --value not given).
    Set {
        key: String,
        /// Provide value inline (not recommended; leaks into shell history).
        #[arg(long)]
        value: Option<String>,
    },
    /// Print a secret value to stdout.
    Get { key: String },
    /// List secret keys stored for this repo. Values never shown.
    List {
        /// JSON output
        #[arg(long)]
        json: bool,
    },
    /// Remove a stored secret.
    Unset { key: String },
    /// Allocate a wormhole code and wait for a teammate to send secrets.
    Receive,
    /// Send selected secrets to a teammate who is waiting with a code.
    Send {
        /// The code printed by `procpane env receive` (e.g. "12-circus-domino").
        code: String,
        /// Keys to send. Default: every stored key.
        keys: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProcOp {
    /// Tail the last N lines
    Tail {
        #[arg(short, long, default_value_t = 50)]
        n: usize,
        #[arg(long)]
        json: bool,
    },
    /// Grep this process's buffer
    Grep {
        pattern: String,
        #[arg(short = 'A', long, default_value_t = 0)]
        after: usize,
        #[arg(short = 'B', long, default_value_t = 0)]
        before: usize,
        #[arg(long)]
        json: bool,
    },
    /// Lines since cursor (incremental polling)
    Since {
        cursor: u64,
        #[arg(long)]
        json: bool,
    },
    /// Send a Unix signal to this process group
    Signal {
        /// Signal name or number (HUP, SIGHUP, TERM, INT, KILL, USR1, ...)
        signal: String,
        /// Monitor status/logs for this long after sending. Examples: `5s`, `1m`.
        #[arg(long)]
        wait: Option<String>,
        /// While waiting, print new output lines from this process
        #[arg(long)]
        tail: bool,
        /// JSON output
        #[arg(long)]
        json: bool,
    },
}
