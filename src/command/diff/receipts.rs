//! preceipts state for the cockpit: HUD and status snapshots of
//! `preceipts-engine … --json`, refreshed on background threads and read by
//! the footer / receipts rail each frame (same global-access pattern as
//! `theme::get`), plus the live check-run session (`run --events` NDJSON)
//! and the land action. Absence of a snapshot means "no engine / no
//! .preceipts here" and renders as nothing — the cockpit degrades to plain
//! lumen.

use std::collections::HashMap;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, RwLock};
use std::time::Instant;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HudSnapshot {
    /// Wire contract field (the cockpit no longer lands; agents do).
    #[allow(dead_code)]
    pub branch: Option<String>,
    pub base: String,
    pub dirty: bool,
    pub ahead: u64,
    pub behind: u64,
    pub merge_clean: Option<bool>,
    #[serde(default)]
    pub conflict_files: Vec<String>,
    pub land_fresh: bool,
    /// Wire contract field; footer use planned (staleness-of-fetch warning).
    #[allow(dead_code)]
    pub fetch_age_ms: Option<f64>,
    pub unsynced_receipts: Option<i64>,
    pub green: bool,
}

static HUD: RwLock<Option<HudSnapshot>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn get() -> Option<HudSnapshot> {
    HUD.read().ok().and_then(|guard| guard.clone())
}

fn load(base: Option<&str>) -> Option<HudSnapshot> {
    let engine = crate::preceipts::engine_binary();
    let mut command = Command::new(engine);
    command.args(["hud", "--json"]);
    if let Some(base) = base {
        command.args(["--base", base]);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Refresh the snapshot off-thread. Stale results are dropped: only the most
/// recently requested refresh may publish (watch mode can fire these fast).
pub fn refresh(base: Option<String>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let snapshot = load(base.as_deref());
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Ok(mut guard) = HUD.write() {
            // Keep the last good snapshot on transient failure; clear only if
            // we never had one (so a repo without preceipts shows nothing).
            if snapshot.is_some() || guard.is_none() {
                *guard = snapshot;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Status snapshot (`preceipts-engine status --json`): the receipt table for
// the working tree, shown in the receipts rail.

#[derive(Clone, Debug, Deserialize)]
pub struct RunnerInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub host: String,
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReceiptInfo {
    #[allow(dead_code)]
    pub ok: bool,
    pub started: String,
    pub duration_ms: u64,
    pub runner: RunnerInfo,
    #[serde(default)]
    #[allow(dead_code)]
    pub dirty: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StatusRow {
    pub check: String,
    pub required: bool,
    /// "ok" | "fail" | "missing" | "stale-definition"
    pub state: String,
    pub receipt: Option<ReceiptInfo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StatusSnapshot {
    pub tree: String,
    pub rows: Vec<StatusRow>,
    pub green: bool,
}

static STATUS: RwLock<Option<StatusSnapshot>> = RwLock::new(None);
static STATUS_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn status_get() -> Option<StatusSnapshot> {
    STATUS.read().ok().and_then(|guard| guard.clone())
}

pub fn refresh_status() {
    let generation = STATUS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let engine = crate::preceipts::engine_binary();
        // `status` exits 1 when not green by design — parse stdout regardless
        // of exit code; parse failure (no .preceipts, no engine) means None.
        let snapshot = Command::new(engine)
            .args(["status", "--json"])
            .stdin(Stdio::null())
            .output()
            .ok()
            .and_then(|output| serde_json::from_slice::<StatusSnapshot>(&output.stdout).ok());
        if STATUS_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Ok(mut guard) = STATUS.write() {
            if snapshot.is_some() || guard.is_none() {
                *guard = snapshot;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Live check runs: `preceipts-engine run --events` NDJSON streamed into the
// rail. One session at a time, owned by the app loop.

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum RunEvent {
    PrepareStarted {
        step: String,
    },
    PrepareOutput {
        step: String,
        chunk: String,
    },
    PrepareFinished {
        step: String,
        ok: bool,
        duration_ms: u64,
    },
    TreeNormalized {
        changed: Vec<String>,
    },
    RunStarted,
    WorktreeChanged {
        changed: Vec<String>,
    },
    CheckStarted {
        check: String,
    },
    Output {
        check: String,
        chunk: String,
    },
    CheckFinished {
        check: String,
        ok: bool,
        duration_ms: u64,
    },
    ReceiptMinted {
        check: String,
    },
}

enum SessionMsg {
    Event(RunEvent),
    Exited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    Running,
    Passed,
    Failed,
}

pub struct LiveCheck {
    pub state: LiveState,
    pub started_at: Instant,
    pub duration_ms: Option<u64>,
    /// Last complete output line (for the inline progress readout).
    pub last_line: String,
    partial: String,
}

pub struct RunSession {
    child: Child,
    rx: mpsc::Receiver<SessionMsg>,
    /// Rows in the order they started; prepare steps are keyed "prepare:<name>".
    pub order: Vec<String>,
    pub live: HashMap<String, LiveCheck>,
    pub finished: bool,
    /// Run-level message (prepare normalized files / run invalidated).
    pub note: Option<String>,
}

impl RunSession {
    pub fn start() -> std::io::Result<RunSession> {
        let engine = crate::preceipts::engine_binary();
        let mut child = Command::new(engine)
            .args(["run", "--events"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<RunEvent>(&line) {
                    if tx.send(SessionMsg::Event(event)).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(SessionMsg::Exited);
        });
        Ok(RunSession {
            child,
            rx,
            order: Vec::new(),
            live: HashMap::new(),
            finished: false,
            note: None,
        })
    }

    fn begin(&mut self, key: String) {
        self.order.push(key.clone());
        self.live.insert(
            key,
            LiveCheck {
                state: LiveState::Running,
                started_at: Instant::now(),
                duration_ms: None,
                last_line: String::new(),
                partial: String::new(),
            },
        );
    }

    fn append_output(&mut self, key: &str, chunk: &str) {
        if let Some(entry) = self.live.get_mut(key) {
            entry.partial.push_str(chunk);
            if let Some(pos) = entry.partial.rfind('\n') {
                let complete = &entry.partial[..pos];
                if let Some(line) = complete.lines().rev().find(|l| !l.trim().is_empty()) {
                    entry.last_line = line.trim_end().to_string();
                }
                entry.partial = entry.partial[pos + 1..].to_string();
            }
        }
    }

    fn finish(&mut self, key: &str, ok: bool, duration_ms: u64) {
        if let Some(entry) = self.live.get_mut(key) {
            entry.state = if ok {
                LiveState::Passed
            } else {
                LiveState::Failed
            };
            entry.duration_ms = Some(duration_ms);
        }
    }

    pub fn is_running(&self) -> bool {
        !self.finished
    }

    /// Drain pending events into the live table. Returns true when the run
    /// completed on this pump (caller refreshes status + HUD then).
    pub fn pump(&mut self) -> bool {
        let mut just_finished = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                SessionMsg::Event(RunEvent::PrepareStarted { step }) => {
                    self.begin(format!("prepare:{step}"));
                }
                SessionMsg::Event(RunEvent::PrepareOutput { step, chunk }) => {
                    self.append_output(&format!("prepare:{step}"), &chunk);
                }
                SessionMsg::Event(RunEvent::PrepareFinished {
                    step,
                    ok,
                    duration_ms,
                }) => {
                    self.finish(&format!("prepare:{step}"), ok, duration_ms);
                }
                SessionMsg::Event(RunEvent::TreeNormalized { changed }) => {
                    self.note = Some(format!(
                        "prepare normalized {} file(s) — receipts key to the normalized tree",
                        changed.len()
                    ));
                }
                SessionMsg::Event(RunEvent::RunStarted) => {}
                SessionMsg::Event(RunEvent::WorktreeChanged { changed }) => {
                    self.note = Some(format!(
                        "✗ worktree changed during the run ({} file(s)) — no receipts minted; move mutating commands to [prepare]",
                        changed.len()
                    ));
                }
                SessionMsg::Event(RunEvent::CheckStarted { check }) => {
                    self.begin(check);
                }
                SessionMsg::Event(RunEvent::Output { check, chunk }) => {
                    self.append_output(&check, &chunk);
                }
                SessionMsg::Event(RunEvent::CheckFinished {
                    check,
                    ok,
                    duration_ms,
                }) => {
                    self.finish(&check, ok, duration_ms);
                }
                SessionMsg::Event(RunEvent::ReceiptMinted { check }) => {
                    // Minting is deferred until the engine verifies the tree
                    // held still — only then is "receipt minted" true.
                    if let Some(entry) = self.live.get_mut(&check) {
                        entry.last_line = "receipt minted".to_string();
                    }
                }
                SessionMsg::Exited => {
                    let _ = self.child.wait();
                    self.finished = true;
                    just_finished = true;
                }
            }
        }
        just_finished
    }

    /// Kill the engine child if the user quits mid-run. Checks that already
    /// finished have their receipts minted; the in-flight one simply has none.
    pub fn abort(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// "48s", "3m12s", "1h03m" — mirrors the engine's duration formatting.
pub fn format_ms(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let (hours, minutes, seconds) = (
        total_seconds / 3600,
        (total_seconds % 3600) / 60,
        total_seconds % 60,
    );
    if hours > 0 {
        format!("{}h{:02}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m{:02}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}
