//! preceipts HUD state for the cockpit: a snapshot of `preceipts-engine hud
//! --json`, refreshed on a background thread and read by the footer each
//! frame (same global-access pattern as `theme::get`). Absence of a snapshot
//! means "no engine / no .preceipts here" and renders as nothing — the
//! cockpit degrades to plain lumen.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HudSnapshot {
    pub branch: Option<String>,
    pub base: String,
    pub dirty: bool,
    pub ahead: u64,
    pub behind: u64,
    pub merge_clean: Option<bool>,
    #[serde(default)]
    pub conflict_files: Vec<String>,
    pub land_fresh: bool,
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
