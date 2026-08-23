//! Receipts: one check run, keyed to a git **tree**.
//!
//! A receipt attached to a tree hash survives history rewrites. Rebase,
//! reword, squash — if the content is identical, the proof still stands. That
//! is the whole idea, and it is why the key is a tree and not a commit.
//!
//! Ported forward from `engine/src/lib/receipt.ts` at fc3643e. **The wire
//! format is a hard compatibility constraint, not an implementation detail**:
//! receipts minted by the TypeScript engine already exist in this repository's
//! notes, and a format change here would silently invalidate them — breaking
//! the survives-rewrites promise on our own history first. One JSON object per
//! line, field order stable, `v: 1`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runner {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub host: String,
    /// Set when a coding agent, not a person, drove the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub v: u8,
    pub check: String,
    #[serde(default)]
    pub cmd: String,
    /// The tree this proves something about.
    pub tree: String,
    pub ok: bool,
    #[serde(default = "minus_one")]
    pub exit: i32,
    pub started: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub runner: Runner,
    /// The worktree differed from HEAD when this ran.
    #[serde(default)]
    pub dirty: bool,
    /// `blob:<sha>` reference to the stored log.
    #[serde(default)]
    pub log: String,
    /// Blob sha of the check script that ran, so a changed check can be told
    /// from a stale result.
    #[serde(default)]
    pub check_blob: String,
}

fn minus_one() -> i32 {
    -1
}

impl Receipt {
    /// One JSON line, no trailing newline, stable field order.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("a receipt is always serializable")
    }

    /// The blob sha from `log` (`blob:<sha>`), if there is one.
    pub fn log_blob(&self) -> Option<&str> {
        self.log.strip_prefix("blob:")
    }
}

/// Parse one line. Blank lines and anything malformed yield `None` rather than
/// an error: a note is append-only and shared, so one bad line must never cost
/// you the rest of the receipts on that tree.
pub fn parse_line(line: &str) -> Option<Receipt> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let receipt: Receipt = serde_json::from_str(trimmed).ok()?;
    if receipt.v != 1 {
        return None;
    }
    Some(receipt)
}

/// Parse a whole note (JSONL, blank separator lines allowed).
pub fn parse_all(text: &str) -> Vec<Receipt> {
    text.lines().filter_map(parse_line).collect()
}

/// Latest receipt per check name, by `started`. ISO-8601 timestamps sort
/// lexically, which is the reason that format was chosen.
pub fn latest_by_check(receipts: &[Receipt]) -> HashMap<String, Receipt> {
    let mut latest: HashMap<String, Receipt> = HashMap::new();
    for receipt in receipts {
        match latest.get(&receipt.check) {
            Some(existing) if receipt.started < existing.started => {}
            _ => {
                latest.insert(receipt.check.clone(), receipt.clone());
            }
        }
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A receipt minted by the TypeScript engine on 2026-07-06, copied out of
    /// this repository's own notes. If this stops parsing, the rewrite has
    /// broken the promise that receipts outlive the tool that made them.
    const REAL: &str = r#"{"v":1,"check":"fmt","cmd":".preceipts/checks/fmt","tree":"2b8f46c30dfbbe65142b0f6aba24ebb95b04d432","ok":true,"exit":0,"started":"2026-07-06T17:26:02Z","duration_ms":284,"runner":{"name":"Jökull Sólberg","email":"jokull@solberg.is","host":"laptop.local"},"dirty":true,"log":"blob:e69de29bb2d1d6434b8b29ae775ad8c2e48c5391","check_blob":"2bfb997ced7bb08e379f49c7f1aa65157a149fb8"}"#;

    #[test]
    fn parses_a_receipt_the_typescript_engine_minted() {
        let receipt = parse_line(REAL).expect("the old format still parses");
        assert_eq!(receipt.check, "fmt");
        assert_eq!(receipt.tree, "2b8f46c30dfbbe65142b0f6aba24ebb95b04d432");
        assert!(receipt.ok);
        assert_eq!(receipt.exit, 0);
        assert_eq!(receipt.duration_ms, 284);
        assert!(receipt.dirty);
        assert_eq!(receipt.runner.name, "Jökull Sólberg");
        assert_eq!(receipt.runner.agent, None);
        assert_eq!(
            receipt.log_blob(),
            Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391")
        );
    }

    /// Byte-identical re-encoding is the compatibility contract: it proves
    /// field order and omission rules match, not merely that we can read.
    #[test]
    fn re_encoding_is_byte_identical() {
        let receipt = parse_line(REAL).unwrap();
        assert_eq!(receipt.encode(), REAL);
    }

    #[test]
    fn an_agent_runner_round_trips() {
        let mut receipt = parse_line(REAL).unwrap();
        receipt.runner.agent = Some("claude".to_string());
        let again = parse_line(&receipt.encode()).unwrap();
        assert_eq!(again.runner.agent.as_deref(), Some("claude"));
    }

    #[test]
    fn a_bad_line_costs_only_itself() {
        let text = format!("{REAL}\n\nnot json at all\n{{\"v\":2}}\n{REAL}");
        assert_eq!(parse_all(&text).len(), 2, "both good lines survive");
    }

    #[test]
    fn blank_notes_yield_nothing() {
        assert!(parse_all("").is_empty());
        assert!(parse_all("\n\n  \n").is_empty());
    }

    #[test]
    fn latest_wins_by_start_time() {
        let mut early = parse_line(REAL).unwrap();
        early.started = "2026-07-06T17:00:00Z".to_string();
        early.ok = false;
        let mut late = parse_line(REAL).unwrap();
        late.started = "2026-07-06T18:00:00Z".to_string();
        late.ok = true;

        // Order in the note must not matter; the timestamp decides.
        let latest = latest_by_check(&[late.clone(), early.clone()]);
        assert!(latest["fmt"].ok);
        let latest = latest_by_check(&[early, late]);
        assert!(latest["fmt"].ok);
    }
}
