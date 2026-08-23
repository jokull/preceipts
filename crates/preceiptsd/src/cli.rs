use usage::{Cli, Subcommands};

#[derive(Cli, Debug)]
#[usage(
    bin = "preceiptsd",
    version,
    about = "The workbench daemon. Driven by `preceipts`; not meant to be typed.",
    // Unknown flags parse as *values* by default here, which would turn a
    // typo into a positional argument and a silent wrong run. This binary is
    // invoked by launchd and by `preceipts up`, where nobody is reading the
    // output, so an error is the only useful answer.
    unknown_flags = "error"
)]
pub struct Cli {
    #[usage(subcommand)]
    pub cmd: Cmd,
    // No global options: both verbs below take what they need as arguments,
    // and advertising flags the binary ignores is worse than having none.
}

/// `preceiptsd` exposes only what launchd and `preceipts up` invoke.
///
/// Every verb a person or an agent uses lives on the `preceipts` CLI and
/// reaches this crate through `commands`. One implementation, two doors —
/// the daemon binary is not a second product with its own vocabulary.
#[derive(Subcommands, Debug)]
pub enum Cmd {
    /// Internal: the daemon itself (invoked by `preceipts up`).
    #[usage(hide = true)]
    Serve {
        tasks: Vec<String>,
        #[usage(long)]
        root: std::path::PathBuf,
        #[usage(long)]
        no_prebuild: bool,
    },
    /// Internal: root-owned loopback :443 → :8443 TCP forwarder.
    #[usage(hide = true)]
    Forward,
    /// Internal: the machine-wide SNI router (started on demand).
    #[usage(hide = true)]
    Route,
}

#[derive(Subcommands, Debug)]
pub enum TrustOp {
    /// Generate the local CA (if missing) and install it into the System keychain.
    /// Prompts for `sudo` (Touch ID works if pam_tid is enabled).
    Install {
        /// Also install the root-owned loopback :443 forwarder, so URLs need
        /// no port. Hostnames themselves need nothing installed.
        #[usage(long = "forwarder", aliases = ["pretty-urls"])]
        forwarder: bool,
    },
    /// Remove the local CA from the System keychain and delete its files.
    /// Also tears down the :443 forwarder if it was installed.
    Uninstall,
    /// Show CA status (installed? trusted? expires when?).
    Status,
}

#[derive(Subcommands, Debug)]
pub enum EnvOp {
    /// Set a secret value (prompts if --value not given).
    Set {
        key: String,
        /// Provide value inline (not recommended; leaks into shell history).
        #[usage(long)]
        value: Option<String>,
    },
    /// Print a secret value to stdout.
    Get { key: String },
    /// List secret keys stored for this repo. Values never shown.
    List {
        /// JSON output
        #[usage(long)]
        json: bool,
    },
    /// Remove a stored secret.
    Unset { key: String },
    /// Allocate a wormhole code and wait for a teammate to send secrets.
    Receive,
    /// Send selected secrets to a teammate who is waiting with a code.
    Send {
        /// The code printed by `preceipts secrets receive` (e.g. "12-circus-domino").
        code: String,
        /// Keys to send. Default: every stored key.
        keys: Vec<String>,
    },
}

#[derive(Subcommands, Debug)]
pub enum ProcOp {
    /// Tail the last N lines
    Tail {
        #[usage(short, long, default_value_t = 50, default = "50")]
        n: usize,
        #[usage(long)]
        json: bool,
    },
    /// Grep this process's buffer
    Grep {
        pattern: String,
        #[usage(short = 'A', long, default_value_t = 0, default = "0")]
        after: usize,
        #[usage(short = 'B', long, default_value_t = 0, default = "0")]
        before: usize,
        #[usage(long)]
        json: bool,
    },
    /// Lines since cursor (incremental polling)
    Since {
        cursor: u64,
        #[usage(long)]
        json: bool,
    },
    /// Send a Unix signal to this process group
    Signal {
        /// Signal name or number (HUP, SIGHUP, TERM, INT, KILL, USR1, ...)
        signal: String,
        /// Monitor status/logs for this long after sending. Examples: `5s`, `1m`.
        #[usage(long)]
        wait: Option<String>,
        /// While waiting, print new output lines from this process
        #[usage(long)]
        tail: bool,
        /// JSON output
        #[usage(long)]
        json: bool,
    },
}
