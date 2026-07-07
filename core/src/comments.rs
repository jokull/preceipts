//! Review comments: local drafts (notes-to-agent, never posted) and the
//! clipboard formats that feed them to a coding agent.
//!
//! Local comments live under `.git/preceipts/comments.json`, keyed by
//! branch — they are review state, not repo content, and they survive the
//! app closing but not intentional cleanup. GitHub feedback (fetched
//! separately) renders through the same `CommentContext` so every comment
//! in the app has the same "Copy to Clipboard" affordance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalComment {
    pub id: u64,
    pub path: String,
    /// 1-based line number on `side` of the diff.
    pub line: u32,
    pub side: Side,
    /// The diff line's text at the time the comment was written — quoted in
    /// the clipboard format and used to flag the comment as outdated later.
    pub line_text: String,
    pub body: String,
    /// Unix seconds; display formatting is the UI's job.
    pub created_at: u64,
}

/// Everything needed to render one comment to the clipboard, regardless of
/// origin (local draft or GitHub).
pub struct CommentContext<'a> {
    pub path: &'a str,
    pub line: u32,
    pub line_text: &'a str,
    pub body: &'a str,
    /// None for local drafts; "octocat" / "coderabbit[bot]" for GitHub.
    pub author: Option<&'a str>,
}

/// One comment as a markdown block with file:line context:
///
/// ```text
/// apps/next/components/foo.tsx:123
/// > const x = useMemo(...)
/// This memo is unnecessary — props are primitives.
/// ```
pub fn format_comment(comment: &CommentContext) -> String {
    let mut out = format!("{}:{}\n", comment.path, comment.line);
    if !comment.line_text.trim().is_empty() {
        out.push_str(&format!("> {}\n", comment.line_text.trim_end()));
    }
    if let Some(author) = comment.author {
        out.push_str(&format!("— {author}: "));
    }
    out.push_str(comment.body.trim());
    out.push('\n');
    out
}

/// Many comments as one digest, grouped by file — a paste-ready worklist
/// for a coding agent.
pub fn format_digest<'a>(comments: impl IntoIterator<Item = CommentContext<'a>>) -> String {
    let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for comment in comments {
        by_file
            .entry(comment.path.to_string())
            .or_default()
            .push(format_comment(&comment));
    }
    let mut out = String::new();
    for (path, blocks) in by_file {
        out.push_str(&format!("## {path}\n\n"));
        for block in blocks {
            out.push_str(&block);
            out.push('\n');
        }
    }
    out.trim_end().to_string() + "\n"
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    /// branch name → comments. Drafts are review state for a branch's PR.
    branches: BTreeMap<String, Vec<LocalComment>>,
    next_id: u64,
}

pub struct CommentStore {
    file: PathBuf,
    branch: String,
    data: StoreFile,
}

impl CommentStore {
    /// Open (or create) the store for a repo's git dir and current branch.
    pub fn open(git_dir: &Path, branch: &str) -> Result<CommentStore, CoreError> {
        let dir = git_dir.join("preceipts");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("comments.json");
        let data = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => StoreFile::default(),
        };
        Ok(CommentStore {
            file,
            branch: branch.to_string(),
            data,
        })
    }

    pub fn comments(&self) -> &[LocalComment] {
        self.data
            .branches
            .get(&self.branch)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn add(
        &mut self,
        path: &str,
        line: u32,
        side: Side,
        line_text: &str,
        body: &str,
    ) -> Result<&LocalComment, CoreError> {
        self.data.next_id += 1;
        let comment = LocalComment {
            id: self.data.next_id,
            path: path.to_string(),
            line,
            side,
            line_text: line_text.to_string(),
            body: body.to_string(),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let list = self.data.branches.entry(self.branch.clone()).or_default();
        list.push(comment);
        self.save()?;
        Ok(self.comments().last().expect("just pushed"))
    }

    pub fn remove(&mut self, id: u64) -> Result<(), CoreError> {
        if let Some(list) = self.data.branches.get_mut(&self.branch) {
            list.retain(|c| c.id != id);
        }
        self.save()
    }

    pub fn update_body(&mut self, id: u64, body: &str) -> Result<(), CoreError> {
        if let Some(list) = self.data.branches.get_mut(&self.branch) {
            if let Some(comment) = list.iter_mut().find(|c| c.id == id) {
                comment.body = body.to_string();
            }
        }
        self.save()
    }

    /// Digest of every draft on this branch — the "Copy all" affordance.
    pub fn digest(&self) -> String {
        format_digest(self.comments().iter().map(|c| CommentContext {
            path: &c.path,
            line: c.line,
            line_text: &c.line_text,
            body: &c.body,
            author: None,
        }))
    }

    fn save(&self) -> Result<(), CoreError> {
        let bytes = serde_json::to_vec_pretty(&self.data)?;
        std::fs::write(&self.file, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_comment_includes_context() {
        let text = format_comment(&CommentContext {
            path: "src/foo.rs",
            line: 42,
            line_text: "let x = 1;  ",
            body: "  rename to count  ",
            author: None,
        });
        assert_eq!(text, "src/foo.rs:42\n> let x = 1;\nrename to count\n");
    }

    #[test]
    fn format_comment_attributes_github_authors() {
        let text = format_comment(&CommentContext {
            path: "a.ts",
            line: 7,
            line_text: "",
            body: "nit: prefer const",
            author: Some("coderabbit[bot]"),
        });
        assert_eq!(text, "a.ts:7\n— coderabbit[bot]: nit: prefer const\n");
    }

    #[test]
    fn digest_groups_by_file() {
        let digest = format_digest(vec![
            CommentContext {
                path: "b.rs",
                line: 2,
                line_text: "bar()",
                body: "extract helper",
                author: None,
            },
            CommentContext {
                path: "a.rs",
                line: 1,
                line_text: "foo()",
                body: "typo",
                author: None,
            },
            CommentContext {
                path: "a.rs",
                line: 9,
                line_text: "baz()",
                body: "dead code",
                author: None,
            },
        ]);
        assert!(digest.starts_with("## a.rs\n"));
        assert!(digest.contains("## b.rs\n"));
        let a_pos = digest.find("a.rs:1").unwrap();
        let a9_pos = digest.find("a.rs:9").unwrap();
        let b_pos = digest.find("b.rs:2").unwrap();
        assert!(a_pos < a9_pos && a9_pos < b_pos);
    }

    #[test]
    fn store_roundtrips_per_branch() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = CommentStore::open(dir.path(), "feature-x").unwrap();
        store
            .add("src/a.rs", 10, Side::New, "let y = 2;", "why 2?")
            .unwrap();
        store
            .add("src/b.rs", 5, Side::Old, "old()", "this was removed — ok?")
            .unwrap();
        assert_eq!(store.comments().len(), 2);

        // Reopen: same branch sees both, another branch sees none.
        let store2 = CommentStore::open(dir.path(), "feature-x").unwrap();
        assert_eq!(store2.comments().len(), 2);
        let other = CommentStore::open(dir.path(), "main").unwrap();
        assert_eq!(other.comments().len(), 0);

        let digest = store2.digest();
        assert!(digest.contains("src/a.rs:10"));
        assert!(digest.contains("> let y = 2;"));
    }

    #[test]
    fn remove_and_update() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = CommentStore::open(dir.path(), "b").unwrap();
        let id = store
            .add("f.rs", 1, Side::New, "x", "first")
            .unwrap()
            .id;
        store.update_body(id, "revised").unwrap();
        assert_eq!(store.comments()[0].body, "revised");
        store.remove(id).unwrap();
        assert!(store.comments().is_empty());
    }
}
