//! Check runs, supervised by the daemon.
//!
//! The daemon does not reimplement `preceipts run`. It calls
//! `preceipts_core::checks::run_with` — the same function, the same run lock,
//! the same receipts — and adds the one thing a library call cannot have: a
//! lifetime independent of whoever asked. That is what lets the app offer a
//! Run button without a GPUI window owning a `cargo test`, and what lets a run
//! survive the window closing.
//!
//! It also means there is nothing here to drift. The CLI still links core
//! directly, because a terminal has its own lifetime and `preceipts run` must
//! work at rung 0 with no daemon at all. Two doors, one implementation.

use crate::buffer::SharedBuffer;
use parking_lot::Mutex;
use preceipts_core::checks::{self, Progress};
use preceipts_proto::ChecksState;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Clears `running` however the run ends — including a panic.
///
/// `checks::run_with` executes whatever scripts a project put in
/// `.preceipts/checks/` and touches git on the way, so it can panic as well as
/// error. Without this, one panic leaves `running` true for the daemon's whole
/// life and every later run — button, `--detach`, quiet watcher — is refused
/// with "already in progress". The same stuck-state bug as the `spawn_blocking`
/// one, a layer down, and a guard is the answer to the whole class rather than
/// to the paths someone thought of.
struct RunGuard {
    state: SharedChecks,
    summary: Option<String>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut guard = self.state.lock();
        guard.running = false;
        guard.current = None;
        guard.last = Some(
            self.summary
                .take()
                .unwrap_or_else(|| "run ended unexpectedly".to_string()),
        );
    }
}

#[derive(Default)]
pub struct ChecksRuntime {
    running: bool,
    current: Option<String>,
    started: Option<Instant>,
    last: Option<String>,
}

pub type SharedChecks = Arc<Mutex<ChecksRuntime>>;

/// A second attempt after a transient failure that was not the lock.
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// How long to wait for someone else's run before giving up on this one.
///
/// Generous, because the thing being waited for is a full check suite in a
/// terminal and `cargo test` is measured in minutes. Waiting is strictly
/// better than the alternative: a run refused by the lock used to be recorded
/// as a failed run, and since nothing tries again until the next edit, a tree
/// could sit unchecked with the badge grey for good.
const WAIT_FOR_LOCK: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const LOCK_POLL: std::time::Duration = std::time::Duration::from_secs(2);

pub fn new_shared() -> SharedChecks {
    Arc::new(Mutex::new(ChecksRuntime::default()))
}

impl ChecksRuntime {
    pub fn snapshot(&self) -> ChecksState {
        ChecksState {
            running: self.running,
            current: self.current.clone(),
            // Only while it is running. Left as-is it kept counting after the
            // run ended, so an idle daemon reported a duration for a run that
            // was already over — a number that reads as progress.
            elapsed_secs: match (self.running, self.started) {
                (true, Some(at)) => at.elapsed().as_secs(),
                _ => 0,
            },
            last: self.last.clone(),
        }
    }
}

/// Kick off a run and return immediately.
///
/// Refuses a second concurrent run here rather than letting the flock refuse
/// it deeper down: the lock's error is written for a person reading a
/// terminal, and a surface that asked politely deserves the polite answer.
/// The lock still guards the case this cannot see — a `preceipts run` in a
/// terminal, on the same worktree, from a process the daemon knows nothing
/// about.
/// Whether every check already has a receipt for the current tree, pass or
/// fail — so there is nothing left to learn about it.
///
/// One copy, asked from two places: the quiet watcher asks before starting a
/// run, and a run that waited on the lock asks again, because the tree may
/// have been answered while it waited. Two copies of this rule would be the
/// exact drift the rest of this change is about.
pub fn settled(root: &std::path::Path) -> bool {
    use preceipts_core::checks::CheckState;
    preceipts_core::checks::status(root, None)
        .map(|status| {
            status
                .rows
                .iter()
                .all(|row| !matches!(row.state, CheckState::Missing | CheckState::StaleDefinition))
        })
        .unwrap_or(false)
}

pub fn start(
    root: PathBuf,
    only: Option<Vec<String>>,
    state: SharedChecks,
    log: SharedBuffer,
) -> Result<(), String> {
    {
        let mut guard = state.lock();
        if guard.running {
            return Err("a check run is already in progress here".to_string());
        }
        guard.running = true;
        guard.current = None;
        guard.started = Some(Instant::now());
    }

    // A plain thread, not `spawn_blocking`. A run is minutes of synchronous
    // child processes, so it must not sit on the runtime that also carries the
    // proxy, the healthcheck loop and every socket client — and the caller is
    // not always *on* that runtime: the quiet watcher is an ordinary thread,
    // where `spawn_blocking` panics rather than working. Measured the hard
    // way, as a run stuck at "running" with an empty log.
    std::thread::spawn(move || {
        let say = |line: String| {
            let mut text = line;
            text.push('\n');
            log.lock().ingest(text.as_bytes());
        };
        say(format!("[preceipts] run started in {}", root.display()));

        let mut on_progress = |progress: Progress<'_>| match progress {
            Progress::Prepare { step } => {
                state.lock().current = Some(format!("prepare: {step}"));
                say(format!("[prepare] {step}"));
            }
            Progress::CheckStarted { name } => {
                state.lock().current = Some(name.to_string());
                say(format!("[check] {name}"));
            }
            Progress::CheckFinished { outcome } => {
                // The check's own output, verbatim and attributed. A ring
                // buffer of interleaved unlabelled lines is not evidence.
                for line in outcome.output.lines() {
                    say(format!("{}| {line}", outcome.name));
                }
                say(format!(
                    "[{}] {} in {}ms",
                    if outcome.ok { "ok" } else { "fail" },
                    outcome.name,
                    outcome.duration.as_millis()
                ));
            }
            Progress::Minted { tree } => say(format!("[receipts] minted for tree {tree}")),
        };

        let mut guard = RunGuard {
            state: state.clone(),
            summary: None,
        };

        // Wait for a `preceipts run` in a terminal rather than treating its
        // lock as a verdict about the tree. Measured: with a six-second check
        // held in a terminal, a fixed one-shot retry lost the race twice and
        // left the tree permanently unchecked — the failure the wait exists
        // to remove, found by racing them on purpose rather than reasoning
        // about it.
        let deadline = Instant::now() + WAIT_FOR_LOCK;
        let mut announced = false;
        while !preceipts_core::runlock::is_free(&root) {
            if Instant::now() >= deadline {
                break;
            }
            if !announced {
                say("[preceipts] another run holds the lock — waiting".to_string());
                state.lock().current = Some("waiting for the run lock".to_string());
                announced = true;
            }
            std::thread::sleep(LOCK_POLL);
        }
        if announced {
            state.lock().current = None;
            // The run we waited for may have been about this very tree. Asking
            // costs one status read; not asking costs a whole suite.
            if settled(&root) {
                say("[preceipts] the other run already covered this tree".to_string());
                guard.summary = Some("covered by another run".to_string());
                return;
            }
        }

        // One further retry, for a failure that was not the lock.
        let mut summary = None;
        for attempt in 0..2 {
            if attempt > 0 {
                say("[preceipts] retrying".to_string());
                std::thread::sleep(RETRY_AFTER);
            }
            match checks::run_with(&root, only.as_deref(), &mut on_progress) {
                Ok(report) => {
                    let failed: Vec<&str> = report
                        .outcomes
                        .iter()
                        .filter(|o| !o.ok)
                        .map(|o| o.name.as_str())
                        .collect();
                    // A check that failed is an answer, not a failure to
                    // answer, so this is where retrying stops.
                    let text = if failed.is_empty() {
                        format!("green — {} checks", report.outcomes.len())
                    } else {
                        format!("failed: {}", failed.join(", "))
                    };
                    say(format!("[preceipts] {text}"));
                    summary = Some(text);
                    break;
                }
                Err(error) => {
                    // A run that could not happen is reported, not swallowed:
                    // the surfaces poll this and would otherwise show a run
                    // that simply stopped existing.
                    say(format!("[preceipts] run failed: {error}"));
                    summary = Some(format!("run failed: {error}"));
                }
            }
        }
        guard.summary = summary;
    });

    Ok(())
}
