//! PR feedback: review comments, review bodies, and conversation comments
//! from GitHub — across human users and GitHub Apps (bots).
//!
//! Interim fetch path shells out to the `gh` CLI (already authenticated on
//! dev machines); the OAuth-device-flow + Keychain path replaces the
//! transport later without changing this model. Every comment renders to
//! the clipboard through the same `CommentContext` as local drafts.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::comments::{format_digest, CommentContext};
use crate::error::CoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackKind {
    /// Line-anchored review comment on the diff.
    ReviewComment,
    /// A review's top-level body (APPROVED / CHANGES_REQUESTED / COMMENTED).
    Review,
    /// PR conversation (issue) comment.
    Conversation,
}

#[derive(Clone, Debug)]
pub struct FeedbackComment {
    pub kind: FeedbackKind,
    pub author: String,
    pub is_bot: bool,
    pub body: String,
    /// Anchor, when the comment is line-anchored and still current.
    pub path: Option<String>,
    pub line: Option<u32>,
    /// The anchored diff line's text (tail of GitHub's diff_hunk).
    pub line_text: String,
    /// Line-anchored but the diff moved on — still shown, flagged.
    pub outdated: bool,
    pub created_at: String,
    pub url: String,
    /// Review state for `Review` kind ("APPROVED", "CHANGES_REQUESTED", …).
    pub state: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PrFeedback {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub comments: Vec<FeedbackComment>,
}

impl FeedbackComment {
    pub fn context(&self) -> CommentContext<'_> {
        CommentContext {
            path: self.path.as_deref().unwrap_or("(conversation)"),
            line: self.line.unwrap_or(0),
            line_text: &self.line_text,
            body: &self.body,
            author: Some(&self.author),
        }
    }
}

impl PrFeedback {
    /// "Copy all" for whatever subset the UI filtered down to.
    pub fn digest<'a>(comments: impl IntoIterator<Item = &'a FeedbackComment>) -> String {
        format_digest(comments.into_iter().map(FeedbackComment::context))
    }
}

/// Fetch feedback for the PR associated with the repo's current branch,
/// via the `gh` CLI. Expensive and network-bound: call off the UI thread.
pub fn fetch_pr_feedback(workdir: &Path) -> Result<PrFeedback, CoreError> {
    let pr = gh_json(workdir, &["pr", "view", "--json", "number,title,url"])?;
    let number = pr["number"]
        .as_u64()
        .ok_or_else(|| feedback_err("no PR for this branch"))?;
    let title = pr["title"].as_str().unwrap_or_default().to_string();
    let url = pr["url"].as_str().unwrap_or_default().to_string();

    let endpoint = |suffix: &str| format!("repos/{{owner}}/{{repo}}/{suffix}");
    let review_comments = gh_json(
        workdir,
        &[
            "api",
            "--paginate",
            &endpoint(&format!("pulls/{number}/comments")),
        ],
    )?;
    let reviews = gh_json(
        workdir,
        &[
            "api",
            "--paginate",
            &endpoint(&format!("pulls/{number}/reviews")),
        ],
    )?;
    let conversation = gh_json(
        workdir,
        &[
            "api",
            "--paginate",
            &endpoint(&format!("issues/{number}/comments")),
        ],
    )?;

    let mut comments = parse_feedback(&review_comments, &reviews, &conversation);
    comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(PrFeedback {
        number,
        title,
        url,
        comments,
    })
}

/// Pure mapping from the three GitHub payloads to the model (unit-tested;
/// the network edge above stays thin).
pub fn parse_feedback(review_comments: &Value, reviews: &Value, conversation: &Value) -> Vec<FeedbackComment> {
    let mut out = Vec::new();

    for item in as_array(review_comments) {
        let line = item["line"].as_u64().map(|n| n as u32);
        let original_line = item["original_line"].as_u64().map(|n| n as u32);
        out.push(FeedbackComment {
            kind: FeedbackKind::ReviewComment,
            author: author_login(item),
            is_bot: is_bot(item),
            body: str_field(item, "body"),
            path: item["path"].as_str().map(str::to_string),
            line: line.or(original_line),
            line_text: last_hunk_line(item["diff_hunk"].as_str().unwrap_or_default()),
            // GitHub nulls `line`/`position` when the diff has moved on.
            outdated: line.is_none(),
            created_at: str_field(item, "created_at"),
            url: str_field(item, "html_url"),
            state: None,
        });
    }

    for item in as_array(reviews) {
        let body = str_field(item, "body");
        let state = item["state"].as_str().unwrap_or_default().to_string();
        // Empty COMMENTED review bodies are shells around line comments.
        if body.trim().is_empty() && state == "COMMENTED" {
            continue;
        }
        out.push(FeedbackComment {
            kind: FeedbackKind::Review,
            author: author_login(item),
            is_bot: is_bot(item),
            body,
            path: None,
            line: None,
            line_text: String::new(),
            outdated: false,
            created_at: str_field(item, "submitted_at"),
            url: str_field(item, "html_url"),
            state: Some(state),
        });
    }

    for item in as_array(conversation) {
        out.push(FeedbackComment {
            kind: FeedbackKind::Conversation,
            author: author_login(item),
            is_bot: is_bot(item),
            body: str_field(item, "body"),
            path: None,
            line: None,
            line_text: String::new(),
            outdated: false,
            created_at: str_field(item, "created_at"),
            url: str_field(item, "html_url"),
            state: None,
        });
    }

    out
}

fn gh_json(workdir: &Path, args: &[&str]) -> Result<Value, CoreError> {
    let output = Command::new("gh")
        .args(args)
        .current_dir(workdir)
        .output()
        .map_err(|_| feedback_err("gh CLI not found — install GitHub CLI or sign in later"))?;
    if !output.status.success() {
        return Err(feedback_err(
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    // --paginate concatenates JSON arrays; normalize "][" seams.
    let text = String::from_utf8_lossy(&output.stdout).replace("][", ",");
    Ok(serde_json::from_str(&text)?)
}

fn feedback_err(message: impl AsRef<str>) -> CoreError {
    CoreError::Feedback(message.as_ref().to_string())
}

fn as_array(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn author_login(item: &Value) -> String {
    item["user"]["login"]
        .as_str()
        .unwrap_or("(unknown)")
        .to_string()
}

fn is_bot(item: &Value) -> bool {
    item["user"]["type"].as_str() == Some("Bot")
        || item["user"]["login"]
            .as_str()
            .is_some_and(|login| login.ends_with("[bot]"))
}

fn str_field(item: &Value, field: &str) -> String {
    item[field].as_str().unwrap_or_default().to_string()
}

/// The last line of a diff_hunk is the line the comment anchors to.
fn last_hunk_line(hunk: &str) -> String {
    hunk.lines()
        .last()
        .map(|line| {
            line.strip_prefix(['+', '-', ' '])
                .unwrap_or(line)
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_review_comments_with_bot_and_outdated_detection() {
        let review_comments = json!([
            {
                "user": {"login": "jokull", "type": "User"},
                "body": "rename this",
                "path": "src/a.rs",
                "line": 12,
                "original_line": 12,
                "diff_hunk": "@@ -1,3 +1,3 @@\n context\n+let x = 1;",
                "created_at": "2026-07-07T00:00:00Z",
                "html_url": "https://github.com/o/r/pull/1#discussion_r1"
            },
            {
                "user": {"login": "coderabbitai[bot]", "type": "Bot"},
                "body": "possible null deref",
                "path": "src/b.rs",
                "line": null,
                "original_line": 30,
                "diff_hunk": "@@ -1 +1 @@\n-old()",
                "created_at": "2026-07-07T01:00:00Z",
                "html_url": "https://github.com/o/r/pull/1#discussion_r2"
            }
        ]);
        let comments = parse_feedback(&review_comments, &json!([]), &json!([]));
        assert_eq!(comments.len(), 2);

        assert_eq!(comments[0].author, "jokull");
        assert!(!comments[0].is_bot);
        assert!(!comments[0].outdated);
        assert_eq!(comments[0].line, Some(12));
        assert_eq!(comments[0].line_text, "let x = 1;");

        assert!(comments[1].is_bot);
        assert!(comments[1].outdated);
        assert_eq!(comments[1].line, Some(30)); // falls back to original_line
        assert_eq!(comments[1].line_text, "old()");
    }

    #[test]
    fn skips_empty_commented_review_shells_keeps_verdicts() {
        let reviews = json!([
            {"user": {"login": "a"}, "body": "", "state": "COMMENTED",
             "submitted_at": "2026-07-07T00:00:00Z", "html_url": "u1"},
            {"user": {"login": "b"}, "body": "LGTM", "state": "APPROVED",
             "submitted_at": "2026-07-07T01:00:00Z", "html_url": "u2"}
        ]);
        let comments = parse_feedback(&json!([]), &reviews, &json!([]));
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].state.as_deref(), Some("APPROVED"));
        assert_eq!(comments[0].kind, FeedbackKind::Review);
    }

    #[test]
    fn digest_includes_authors_and_anchors() {
        let review_comments = json!([{
            "user": {"login": "codex[bot]"},
            "body": "this loop is O(n^2)",
            "path": "src/hot.rs",
            "line": 88,
            "diff_hunk": "@@ @@\n+for x in xs { for y in ys {} }",
            "created_at": "2026-07-07T00:00:00Z",
            "html_url": "u"
        }]);
        let comments = parse_feedback(&review_comments, &json!([]), &json!([]));
        let digest = PrFeedback::digest(&comments);
        assert!(digest.contains("## src/hot.rs"));
        assert!(digest.contains("src/hot.rs:88"));
        assert!(digest.contains("> for x in xs { for y in ys {} }"));
        assert!(digest.contains("— codex[bot]: this loop is O(n^2)"));
    }
}
