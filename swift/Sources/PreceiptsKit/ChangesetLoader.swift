// Assemble the full changeset for a repo path and scope. Expensive —
// call off the main thread and cache the result.

import Foundation

public enum ChangesetLoader {
    public static func load(
        repoPath: URL, scope: DiffScope, highlighting: Bool = true
    ) throws -> Changeset {
        let reader = try GitReader(path: repoPath)
        let workdir = try reader.workdir
        let head = try reader.headBranch()

        let baseName: String
        let baseCommit: git_oid_boxed
        switch scope {
        case .uncommitted:
            baseName = "HEAD"
            baseCommit = git_oid_boxed(head.oid)
        case .branch:
            let base = try reader.resolveBase()
            baseName = "\(base.name)\u{2026}"
            baseCommit = git_oid_boxed(try reader.mergeBase(base.oid, head.oid))
        }

        // Sequential git pass (libgit2 objects are not thread-safe), then a
        // parallel highlight pass — parsing every changed file dominates
        // load time on large changesets when done serially.
        struct Pending {
            let file: FileDiff
            let oldSource: String
            let oldContent: String
            let newContent: String
            var oldHighlight: FileHighlight?
            var newHighlight: FileHighlight?
        }
        var pending: [Pending] = []
        for entry in try reader.changedFiles(baseCommit: baseCommit.oid) {
            let oldSource = entry.oldPath ?? entry.path
            let (oldContent, oldBinary): (String, Bool)
            switch entry.status {
            case .added: (oldContent, oldBinary) = ("", false)
            default:
                (oldContent, oldBinary) = try reader.blobContent(
                    baseCommit: baseCommit.oid, path: oldSource)
            }
            let (newContent, newBinary): (String, Bool)
            switch entry.status {
            case .deleted: (newContent, newBinary) = ("", false)
            default: (newContent, newBinary) = worktreeContent(workdir: workdir, path: entry.path)
            }

            let isBinary = oldBinary || newBinary
            let (hunks, added, removed): ([DiffHunk], Int, Int)
            if isBinary {
                (hunks, added, removed) = ([], 0, 0)
            } else {
                let result = diffRows(old: oldContent, new: newContent)
                (hunks, added, removed) = (result.hunks, result.added, result.removed)
            }
            // A file can appear changed by stat but diff clean (e.g. touch).
            if hunks.isEmpty && entry.status == .modified {
                continue
            }
            pending.append(
                Pending(
                    file: FileDiff(
                        path: entry.path,
                        oldPath: entry.oldPath,
                        status: entry.status,
                        isBinary: isBinary,
                        added: added,
                        removed: removed,
                        hunks: hunks
                    ),
                    oldSource: oldSource,
                    oldContent: oldContent,
                    newContent: newContent
                )
            )
        }

        if highlighting {
            pending.withUnsafeMutableBufferPointer { buffer in
                DispatchQueue.concurrentPerform(iterations: buffer.count) { index in
                    let item = buffer[index]
                    guard !item.file.hunks.isEmpty else { return }
                    buffer[index].oldHighlight = item.oldContent.isEmpty
                        ? nil : Highlighter.highlight(item.oldContent, path: item.oldSource)
                    buffer[index].newHighlight = item.newContent.isEmpty
                        ? nil : Highlighter.highlight(item.newContent, path: item.file.path)
                }
            }
        }

        let files = pending.map { item in
            FileDiff(
                path: item.file.path,
                oldPath: item.file.oldPath,
                status: item.file.status,
                isBinary: item.file.isBinary,
                added: item.file.added,
                removed: item.file.removed,
                hunks: item.file.hunks,
                oldHighlight: item.oldHighlight,
                newHighlight: item.newHighlight
            )
        }

        return Changeset(
            scope: scope,
            baseName: baseName,
            branch: head.name,
            workdir: workdir,
            gitDir: reader.gitDir,
            files: files
        )
    }
}

/// git_oid is a C struct; box it so switch arms above can assign once.
struct git_oid_boxed {
    let oid: Clibgit2.git_oid
    init(_ oid: Clibgit2.git_oid) { self.oid = oid }
}

import Clibgit2
