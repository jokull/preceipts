//! Reading `turbo.json`, for one question: which env vars are in the hash.
//!
//! Turborepo already formalised the distinction the manifest needs. `env` and
//! `globalEnv` are hashed into a task's cache key, so changing one misses
//! cache everywhere. `passThroughEnv` and `globalPassThroughEnv` reach the
//! process and never touch the hash. `envMode = "strict"` filters everything
//! undeclared out of the task's environment entirely.
//!
//! That gives two failure modes worth catching, and they fail in opposite
//! directions:
//!
//! - A **secret in the hash**. Every developer's value differs, so every
//!   developer misses cache forever — and the value becomes part of a key
//!   travelling to a shared remote cache.
//! - **Behaviour-changing config outside the hash**. This is the worse one,
//!   because it produces a wrong cache *hit*: green, fast, and built from the
//!   wrong inputs.
//!
//! We read `turbo.json` rather than owning it. The manifest says what a value
//! *is*; turbo says how it is *treated*; `doctor` reports where they disagree.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Task {
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default, rename = "passThroughEnv")]
    pub pass_through_env: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Turbo {
    #[serde(default, rename = "globalEnv")]
    pub global_env: Vec<String>,
    #[serde(default, rename = "globalPassThroughEnv")]
    pub global_pass_through_env: Vec<String>,
    #[serde(default, rename = "envMode")]
    pub env_mode: Option<String>,
    /// `tasks` in turbo 2.x; `pipeline` was its name in 1.x.
    #[serde(default, alias = "pipeline")]
    pub tasks: std::collections::BTreeMap<String, Task>,
}

impl Turbo {
    /// Every pattern whose value is hashed into some cache key.
    pub fn hashed_patterns(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self.global_env.iter().cloned().collect();
        for task in self.tasks.values() {
            out.extend(task.env.iter().cloned());
        }
        out
    }

    /// Every pattern that reaches a process without being hashed.
    pub fn passthrough_patterns(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self.global_pass_through_env.iter().cloned().collect();
        for task in self.tasks.values() {
            out.extend(task.pass_through_env.iter().cloned());
        }
        out
    }
}

/// Load `turbo.json` from a project root, if there is one.
pub fn load(root: &Path) -> Result<Option<Turbo>> {
    let path = root.join("turbo.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    parse(&text).map(Some).map_err(|e| {
        Error::Git(git2::Error::from_str(&format!(
            "cannot read {}: {e}",
            path.display()
        )))
    })
}

/// Parse a `turbo.json`, tolerating comments.
///
/// `turbo.json` is JSONC in practice — the schema allows comments and editors
/// encourage them. A strict parser would fail on exactly the well-documented
/// repositories `doctor` is most useful for, and failing to read a config is a
/// worse outcome than reading one with comments in it.
pub fn parse(text: &str) -> std::result::Result<Turbo, serde_json::Error> {
    serde_json::from_str(&strip_comments(text))
}

/// Remove `//` and `/* */` comments, leaving string literals alone.
///
/// String-aware because `"$schema": "https://…"` is in every turbo.json ever
/// written, and a naive strip would cut the URL in half and take the rest of
/// the file with it. Comments are replaced by spaces rather than deleted so
/// that byte offsets in any parse error still point at the right place.
fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            out.push(byte as char);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push('"');
            i += 1;
            continue;
        }
        if byte == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(' ');
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                while i < bytes.len() {
                    let at_end = i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/';
                    // Newlines are kept so line numbers survive.
                    out.push(if bytes[i] == b'\n' { '\n' } else { ' ' });
                    i += 1;
                    if at_end {
                        out.push(' ');
                        i += 1;
                        break;
                    }
                }
                continue;
            }
        }
        // Non-ASCII bytes pass through as bytes, not chars, so multi-byte
        // sequences outside strings (rare, but legal in whitespace) survive.
        out.push_str(&text[i..i + 1]);
        i += 1;
    }
    out
}

/// Does `pattern` — a literal, or a turbo wildcard like `STRIPE_*` — cover
/// `key`? A leading `!` is turbo's negation and never covers anything.
pub fn matches(pattern: &str, key: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix('!') {
        let _ = rest;
        return false;
    }
    match pattern.split_once('*') {
        None => pattern == key,
        Some((prefix, suffix)) => {
            key.len() >= prefix.len() + suffix.len()
                && key.starts_with(prefix)
                && key.ends_with(suffix)
        }
    }
}

/// Is `key` excluded by an explicit `!KEY` negation in `patterns`?
pub fn negated(patterns: &BTreeSet<String>, key: &str) -> bool {
    patterns
        .iter()
        .filter_map(|p| p.strip_prefix('!'))
        .any(|p| matches(p, key))
}

/// Does any pattern in `patterns` cover `key`, negations respected?
pub fn covers(patterns: &BTreeSet<String>, key: &str) -> bool {
    !negated(patterns, key) && patterns.iter().any(|p| matches(p, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schema_url_is_not_a_comment() {
        let text = r#"{"$schema": "https://turborepo.dev/schema.json", "globalEnv": ["A"]}"#;
        let turbo = parse(text).expect("a url is not a comment");
        assert_eq!(turbo.global_env, vec!["A".to_string()]);
    }

    /// Turbo names its root tasks `//#format`, which looks exactly like a
    /// comment to anything that is not string-aware.
    #[test]
    fn a_root_task_name_is_not_a_comment() {
        let text = r#"{"tasks": {"//#format": {"env": ["FMT"]}}}"#;
        let turbo = parse(text).unwrap();
        assert!(turbo.tasks.contains_key("//#format"));
        assert!(turbo.hashed_patterns().contains("FMT"));
    }

    #[test]
    fn real_comments_are_stripped() {
        let text = r#"{
            // the whole point of this file
            "globalEnv": ["A"], /* and a block
                                   comment too */
            "globalPassThroughEnv": ["B"]
        }"#;
        let turbo = parse(text).expect("JSONC parses");
        assert_eq!(turbo.global_env, vec!["A".to_string()]);
        assert_eq!(turbo.global_pass_through_env, vec!["B".to_string()]);
    }

    #[test]
    fn turbo_1_pipelines_still_read() {
        let text = r#"{"pipeline": {"build": {"env": ["OLD"]}}}"#;
        assert!(parse(text).unwrap().hashed_patterns().contains("OLD"));
    }

    #[test]
    fn wildcards_match_the_way_turbo_documents_them() {
        assert!(matches("STRIPE_*", "STRIPE_SECRET_KEY"));
        assert!(matches("STRIPE_*", "STRIPE_"));
        assert!(!matches("STRIPE_*", "STRIP"));
        assert!(matches("EXACT", "EXACT"));
        assert!(!matches("EXACT", "EXACTLY"));
        assert!(
            !matches("!STRIPE_*", "STRIPE_SECRET_KEY"),
            "a negation covers nothing"
        );
    }

    #[test]
    fn a_negation_removes_a_key_a_wildcard_would_have_caught() {
        let patterns: BTreeSet<String> = ["MY_API_*".to_string(), "!MY_API_URL".to_string()]
            .into_iter()
            .collect();
        assert!(covers(&patterns, "MY_API_KEY"));
        assert!(!covers(&patterns, "MY_API_URL"));
    }

    #[test]
    fn task_and_global_patterns_are_both_collected() {
        let text = r#"{
            "globalEnv": ["G"],
            "globalPassThroughEnv": ["GP"],
            "tasks": {"build": {"env": ["T"], "passThroughEnv": ["TP"]}}
        }"#;
        let turbo = parse(text).unwrap();
        assert!(turbo.hashed_patterns().contains("G"));
        assert!(turbo.hashed_patterns().contains("T"));
        assert!(turbo.passthrough_patterns().contains("GP"));
        assert!(turbo.passthrough_patterns().contains("TP"));
    }

    #[test]
    fn a_project_without_turbo_is_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load(temp.path()).unwrap().is_none());
    }
}
