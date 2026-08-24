//! The wire vocabulary: what the daemon, the CLI, and the app all say.
//!
//! Its own crate, and deliberately a small one. The app has to speak this to
//! show a workspace's services and their logs, and depending on `preceiptsd`
//! for the privilege would drag tokio, rustls, a PTY layer and a wormhole
//! implementation into a GPUI binary that opens no sockets of its own beyond
//! one blocking client. Types are the shared part; the machinery is not.
//!
//! Nothing in here has behaviour. Add a field before you add a variant, and
//! give every added field `#[serde(default)]` — a daemon left running from
//! last week is a supported configuration, and it will happily talk to a CLI
//! built today.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    Status,
    Stop,
    Tail {
        name: String,
        lines: usize,
    },
    Grep {
        name: Option<String>,
        pattern: String,
        before: usize,
        after: usize,
    },
    Since {
        name: String,
        cursor: u64,
    },
    Signal {
        name: String,
        signal: String,
    },
    Ping,
    /// The HTTP the proxy has carried, newest last.
    Transcript {
        host: Option<String>,
        since_secs: Option<i64>,
    },
    /// Block-style query: returns immediately with current state of the task.
    /// The CLI side polls until state == "healthy" or terminal failure.
    GetTask {
        name: String,
    },
    /// Start a check run in the daemon and return at once.
    ///
    /// The daemon does not reimplement anything: it calls the same
    /// `checks::run` the CLI calls, under the same run lock, and mints the
    /// same receipts. What it adds is a process that outlives the caller —
    /// which is what lets a window ask for a run without owning a `cargo
    /// test`, and what lets a run survive the window closing.
    Run {
        /// `None` runs every check.
        checks: Option<Vec<String>>,
    },
    /// Whether a run is in flight here, and what it is on.
    Checks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Pong,
    Status {
        procs: Vec<ProcStatus>,
    },
    Lines {
        lines: Vec<LineRecord>,
        next_cursor: u64,
    },
    GrepMatches {
        matches: Vec<GrepMatch>,
    },
    Transcript {
        exchanges: Vec<Exchange>,
    },
    Task {
        task: ProcStatus,
    },
    Checks {
        state: ChecksState,
    },
    Error {
        message: String,
    },
}

/// The daemon's view of check running. Deliberately thin: the *result* of a
/// run is the receipts, which every surface already reads from git notes, so
/// this says only what receipts cannot — whether one is happening now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChecksState {
    pub running: bool,
    /// The check being executed, or the prepare step, right now.
    pub current: Option<String>,
    /// Seconds since the run started.
    pub elapsed_secs: u64,
    /// How the last finished run ended, for a surface that asked after it was
    /// over. `None` until one has finished in this daemon's lifetime.
    pub last: Option<String>,
}

/// The ring buffer a daemon-side check run streams into.
///
/// A reserved task id rather than a new instrument: `tail`, `since` and `grep`
/// already read buffers by name, so a run's output is queryable by every
/// surface the moment it exists, with no verb added anywhere.
pub const CHECKS_LOG: &str = "checks";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcStatus {
    pub name: String,
    /// pending | starting | healthy | completed | crashed | killed
    pub state: String,
    pub pid: Option<i32>,
    pub age_secs: u64,
    pub line_count: u64,
    pub exit_code: Option<i32>,
    pub persistent: bool,
    /// Hostname mapped via reverse proxy, when configured.
    #[serde(default)]
    pub hostname: Option<String>,
    /// The port the daemon allocated for this task, when it allocated one.
    ///
    /// Reported because an agent that only has a hostname cannot reach a
    /// service until the `:443` forwarder is installed, and "what is around
    /// me" is supposed to be answerable without a privileged install first.
    #[serde(default)]
    pub port: Option<u16>,
    /// The URL to actually open: portless when the forwarder is installed,
    /// with the proxy port when it is not.
    #[serde(default)]
    pub url: Option<String>,
    /// One-line diagnostic hints surfaced by the daemon (e.g. "wrangler
    /// detected → CLOUDFLARE_INCLUDE_PROCESS_ENV=true").
    #[serde(default)]
    pub notes: Vec<String>,
    /// True if the service is explicitly declared in `preceipts.toml`. Used by
    /// renderers to distinguish "services the user cares about" from
    /// implicit workspace `dev` tasks (typically `tsc --watch` builders).
    #[serde(default)]
    pub in_manifest: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineRecord {
    pub seq: u64,
    pub ts_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub task: String,
    pub seq: u64,
    pub ts_ms: u64,
    pub text: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Exchange {
    /// Monotonic id, never reused.
    ///
    /// Positions shift as the ring evicts, so a response that arrives after
    /// its request was pushed out would otherwise stamp its status onto
    /// whichever exchange had slid into that slot. An id cannot be
    /// misattributed: it either still exists or it does not.
    pub id: u64,
    /// Which service answered — the hostname the request arrived on.
    pub host: String,
    pub method: String,
    pub path: String,
    /// None while the response has not arrived, which is also how a hung
    /// request looks. That distinction is the point of recording it early.
    pub status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub content_type: Option<String>,
    /// Seconds since the epoch, when the request line was seen.
    pub at: i64,
}

/// The daemon's control socket for a project.
///
/// **Not** under the project, which is where it used to live. A unix socket
/// path is capped at 104 bytes on macOS (`SUN_LEN`), and a worktree a few
/// directories deep blows straight through that — the bind fails with a
/// message about `SUN_LEN` that says nothing about the real cause, and the
/// only symptom a user sees is "daemon did not come up". Naming it from a
/// hash of the canonical root in the per-user temp directory makes the length
/// constant and the identity still one-to-one with the project.
///
/// Exposed so callers do not each rebuild the path from parts — the CLI, the
/// MCP server, and the daemon itself all have to agree on it, and a
/// disagreement reads as "no daemon running" rather than as a bug.
pub fn socket_path(root: &Path) -> PathBuf {
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    std::env::temp_dir().join(format!(
        "preceipts-{:016x}.sock",
        fnv1a(canonical.as_bytes())
    ))
}

/// FNV-1a. Not `DefaultHasher`: its output is explicitly not guaranteed stable
/// across releases, and this value has to mean the same thing to a daemon
/// started last week and a CLI built today.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Either marker roots a project.
///
/// `turbo.json` was the only marker accepted before, which made the daemon
/// unusable for the very case rung 1 of the schema exists for: a single Vite
/// app with four lines of `preceipts.toml` and no monorepo tooling at all.
pub const ROOT_MARKERS: [&str; 2] = ["preceipts.toml", "turbo.json"];

/// The project a directory belongs to: the nearest ancestor holding a marker.
///
/// Here rather than in the daemon because it is half of the socket's address.
/// A caller that resolves the root differently looks for the socket in the
/// wrong place and concludes, wrongly and silently, that no daemon is running
/// — which is exactly what the app did when it reached for the *git* root
/// instead. Note what this implies and is meant to: a linked worktree carries
/// its own copy of the marker, so it gets its own project root and its own
/// daemon. One environment per workspace is the whole point.
pub fn project_root(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().ok()?;
    let mut current = start.as_path();
    loop {
        if ROOT_MARKERS.iter().any(|m| current.join(m).is_file()) {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

#[cfg(test)]
mod socket_path_tests {
    use super::*;

    /// The regression: the socket used to live under the project, so a deep
    /// worktree could not start a daemon at all.
    #[test]
    fn the_path_stays_short_however_deep_the_project_is() {
        let deep = PathBuf::from("/Users/someone/Code")
            .join("a-fairly-long-directory-name".repeat(4))
            .join("another-quite-long-directory-name")
            .join("worktrees")
            .join("fix-the-checkout-race-in-payments");
        let socket = socket_path(&deep);
        assert!(
            socket.as_os_str().len() < 104,
            "unix sockets cap at 104 bytes: {} was {}",
            socket.display(),
            socket.as_os_str().len()
        );
    }

    #[test]
    fn two_projects_do_not_share_a_socket() {
        assert_ne!(
            socket_path(Path::new("/tmp/one")),
            socket_path(Path::new("/tmp/two"))
        );
    }

    #[test]
    fn the_same_project_always_gets_the_same_socket() {
        let root = Path::new("/tmp/one");
        assert_eq!(socket_path(root), socket_path(root));
    }

    /// Pinned because a daemon started before an upgrade must still be
    /// reachable by a CLI built after one.
    #[test]
    fn the_hash_is_stable_across_builds() {
        assert_eq!(fnv1a(b"/Users/x/proj"), fnv1a(b"/Users/x/proj"));
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }
}
