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

#[derive(Default)]
pub struct ChecksRuntime {
    running: bool,
    current: Option<String>,
    started: Option<Instant>,
    last: Option<String>,
}

pub type SharedChecks = Arc<Mutex<ChecksRuntime>>;

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

        let summary = match checks::run_with(&root, only.as_deref(), &mut on_progress) {
            Ok(report) => {
                let failed: Vec<&str> = report
                    .outcomes
                    .iter()
                    .filter(|o| !o.ok)
                    .map(|o| o.name.as_str())
                    .collect();
                let summary = if failed.is_empty() {
                    format!("green — {} checks", report.outcomes.len())
                } else {
                    format!("failed: {}", failed.join(", "))
                };
                say(format!("[preceipts] {summary}"));
                summary
            }
            Err(error) => {
                // A run that could not happen is reported, not swallowed: the
                // surfaces poll this and would otherwise show a run that
                // simply stopped existing.
                say(format!("[preceipts] run failed: {error}"));
                format!("run failed: {error}")
            }
        };

        let mut guard = state.lock();
        guard.running = false;
        guard.current = None;
        guard.last = Some(summary);
    });

    Ok(())
}
