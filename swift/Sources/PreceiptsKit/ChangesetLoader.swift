// Assemble the full changeset for a repo path and scope. Expensive —
// call off the main thread and cache the result.

import Foundation

public enum ChangesetLoader {
    public static func load(repoPath: URL, scope: DiffScope) throws -> Changeset {
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

        var files: [FileDiff] = []
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
            files.append(
                FileDiff(
                    path: entry.path,
                    oldPath: entry.oldPath,
                    status: entry.status,
                    isBinary: isBinary,
                    added: added,
                    removed: removed,
                    hunks: hunks
                )
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
