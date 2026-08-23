use serde::{Deserialize, Serialize};

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
    /// Block-style query: returns immediately with current state of the task.
    /// The CLI side polls until state == "healthy" or terminal failure.
    GetTask {
        name: String,
    },
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
    Task {
        task: ProcStatus,
    },
    Error {
        message: String,
    },
}

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
    /// True if the task is explicitly declared in `procpane.toml`. Used by
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
