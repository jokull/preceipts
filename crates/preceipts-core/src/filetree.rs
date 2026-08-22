//! The sidebar's file tree: changed-file paths folded into directory nodes
//! with aggregated ±stats and VS Code-style single-child chain compaction
//! ("a/b/c" reads as one row when a and b hold nothing else).
//!
//! Ported forward from `swift/Sources/PreceiptsKit/FileTree.swift` at fc3643e.

use crate::model::{FileDiff, FileStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTreeNode {
    /// Display name — compacted directory chains read "a/b".
    pub name: String,
    /// Repo-relative path of the file or directory.
    pub path: String,
    /// Index into `Changeset::files`; `None` for directories.
    pub file_index: Option<usize>,
    pub status: Option<FileStatus>,
    pub added: usize,
    pub removed: usize,
    pub children: Vec<FileTreeNode>,
}

impl FileTreeNode {
    pub fn is_directory(&self) -> bool {
        self.file_index.is_none()
    }
}

/// Root nodes: directories first, then files, each level sorted by name.
pub fn build(files: &[FileDiff]) -> Vec<FileTreeNode> {
    let mut root = MutableNode::new("", "");
    for (index, file) in files.iter().enumerate() {
        let components: Vec<&str> = file.path.split('/').filter(|c| !c.is_empty()).collect();
        let mut node = &mut root;
        for component in components.iter().take(components.len().saturating_sub(1)) {
            node = node.directory(component);
        }
        let mut leaf =
            MutableNode::new(components.last().copied().unwrap_or(&file.path), &file.path);
        leaf.file_index = Some(index);
        leaf.status = Some(file.status);
        leaf.added = file.added;
        leaf.removed = file.removed;
        node.children.push(leaf);
    }
    let mut roots: Vec<FileTreeNode> = root.children.iter().map(finalize).collect();
    sort(&mut roots);
    roots
}

/// Directories before files, then by name. Swift used
/// `localizedStandardCompare`; the practical part of that here is
/// case-insensitivity, so casing never splits a directory listing in two.
fn sort(nodes: &mut [FileTreeNode]) {
    nodes.sort_by(|a, b| {
        b.is_directory()
            .cmp(&a.is_directory())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

struct MutableNode {
    name: String,
    path: String,
    file_index: Option<usize>,
    status: Option<FileStatus>,
    added: usize,
    removed: usize,
    children: Vec<MutableNode>,
}

impl MutableNode {
    fn new(name: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            path: path.to_string(),
            file_index: None,
            status: None,
            added: 0,
            removed: 0,
            children: Vec::new(),
        }
    }

    fn directory(&mut self, component: &str) -> &mut MutableNode {
        if let Some(position) = self
            .children
            .iter()
            .position(|c| c.file_index.is_none() && c.name == component)
        {
            return &mut self.children[position];
        }
        let path = if self.path.is_empty() {
            component.to_string()
        } else {
            format!("{}/{}", self.path, component)
        };
        self.children.push(MutableNode::new(component, &path));
        self.children.last_mut().expect("just pushed")
    }
}

fn finalize(node: &MutableNode) -> FileTreeNode {
    if node.file_index.is_some() {
        return FileTreeNode {
            name: node.name.clone(),
            path: node.path.clone(),
            file_index: node.file_index,
            status: node.status,
            added: node.added,
            removed: node.removed,
            children: Vec::new(),
        };
    }

    // Compact single-child directory chains before recursing.
    let mut name = node.name.clone();
    let mut current = node;
    while current.children.len() == 1 && current.children[0].file_index.is_none() {
        current = &current.children[0];
        name.push('/');
        name.push_str(&current.name);
    }

    let mut children: Vec<FileTreeNode> = current.children.iter().map(finalize).collect();
    sort(&mut children);
    FileTreeNode {
        name,
        path: current.path.clone(),
        file_index: None,
        status: None,
        added: children.iter().map(|c| c.added).sum(),
        removed: children.iter().map(|c| c.removed).sum(),
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{file, file_with};

    fn names(nodes: &[FileTreeNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn directories_aggregate_and_sort_before_files() {
        let roots = build(&[
            file_with("readme.md", FileStatus::Modified, 5, 0),
            file_with("src/main.rs", FileStatus::Modified, 2, 1),
            file_with("src/util.rs", FileStatus::Modified, 3, 4),
        ]);
        assert_eq!(names(&roots), ["src", "readme.md"]);
        let src = &roots[0];
        assert!(src.is_directory());
        assert_eq!(src.added, 5);
        assert_eq!(src.removed, 5);
        assert_eq!(names(&src.children), ["main.rs", "util.rs"]);
    }

    #[test]
    fn single_child_directory_chains_compact() {
        let roots = build(&[
            file("apps/web/components/button.tsx"),
            file("apps/web/components/input.tsx"),
        ]);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].name, "apps/web/components");
        assert_eq!(roots[0].path, "apps/web/components");
        assert_eq!(roots[0].children.len(), 2);
        assert!(!roots[0].children[0].is_directory());
    }

    #[test]
    fn compaction_stops_at_branching_directories() {
        let roots = build(&[file("apps/web/a.ts"), file("apps/api/b.ts")]);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].name, "apps");
        assert_eq!(names(&roots[0].children), ["api", "web"]);
    }

    #[test]
    fn leaves_carry_file_index_and_status() {
        let roots = build(&[file_with("a.rs", FileStatus::Added, 1, 1)]);
        assert_eq!(roots[0].file_index, Some(0));
        assert_eq!(roots[0].status, Some(FileStatus::Added));
    }
}
