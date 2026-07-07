// The sidebar's file tree: changed-file paths folded into directory nodes
// with aggregated ±stats and VS Code-style single-child chain compaction
// ("a/b/c" as one row when a and b hold nothing else).

import Foundation

public final class FileTreeNode {
    /// Display name — compacted directory chains read "a/b".
    public let name: String
    /// Repo-relative path of the file or directory.
    public let path: String
    /// Index into `Changeset.files`; nil for directories.
    public let fileIndex: Int?
    public let status: FileStatus?
    public let added: Int
    public let removed: Int
    public let children: [FileTreeNode]

    public var isDirectory: Bool { fileIndex == nil }

    public init(
        name: String,
        path: String,
        fileIndex: Int?,
        status: FileStatus?,
        added: Int,
        removed: Int,
        children: [FileTreeNode]
    ) {
        self.name = name
        self.path = path
        self.fileIndex = fileIndex
        self.status = status
        self.added = added
        self.removed = removed
        self.children = children
    }
}

public enum FileTree {
    /// Root nodes, directories first then files, each level sorted by name.
    public static func build(_ files: [FileDiff]) -> [FileTreeNode] {
        let root = MutableNode(name: "", path: "")
        for (index, file) in files.enumerated() {
            var node = root
            let components = file.path.split(separator: "/").map(String.init)
            for component in components.dropLast() {
                node = node.directory(component)
            }
            let leaf = MutableNode(name: components.last ?? file.path, path: file.path)
            leaf.fileIndex = index
            leaf.status = file.status
            leaf.added = file.added
            leaf.removed = file.removed
            node.children.append(leaf)
        }
        return sorted(root.children.map { finalize($0) })
    }

    private static func sorted(_ nodes: [FileTreeNode]) -> [FileTreeNode] {
        nodes.sorted { a, b in
            if a.isDirectory != b.isDirectory { return a.isDirectory }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
    }

    private final class MutableNode {
        let name: String
        let path: String
        var fileIndex: Int?
        var status: FileStatus?
        var added = 0
        var removed = 0
        var children: [MutableNode] = []

        init(name: String, path: String) {
            self.name = name
            self.path = path
        }

        func directory(_ component: String) -> MutableNode {
            if let existing = children.first(where: {
                $0.fileIndex == nil && $0.name == component
            }) {
                return existing
            }
            let child = MutableNode(
                name: component,
                path: path.isEmpty ? component : "\(path)/\(component)")
            children.append(child)
            return child
        }
    }

    private static func finalize(_ node: MutableNode) -> FileTreeNode {
        if node.fileIndex != nil {
            return FileTreeNode(
                name: node.name,
                path: node.path,
                fileIndex: node.fileIndex,
                status: node.status,
                added: node.added,
                removed: node.removed,
                children: []
            )
        }
        // Compact single-child directory chains before recursing.
        var name = node.name
        var current = node
        while current.children.count == 1, let only = current.children.first,
            only.fileIndex == nil
        {
            name += "/\(only.name)"
            current = only
        }
        let children = sorted(current.children.map { finalize($0) })
        return FileTreeNode(
            name: name,
            path: current.path,
            fileIndex: nil,
            status: nil,
            added: children.reduce(0) { $0 + $1.added },
            removed: children.reduce(0) { $0 + $1.removed },
            children: children
        )
    }
}
