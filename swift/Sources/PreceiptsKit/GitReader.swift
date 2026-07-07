// libgit2 reads, in-process — the GitUp/Sublime-Merge lesson from
// docs/desktop-foundations.md: never block on a `git` subprocess.

import Clibgit2
import Foundation

private let initOnce: Void = {
    git_libgit2_init()
    return ()
}()

private func check(_ code: Int32, _ what: String) throws {
    guard code >= 0 else {
        let message = git_error_last().map { String(cString: $0.pointee.message) }
        throw PreceiptsError.git("\(what): \(message ?? "error \(code)")")
    }
}

public struct ChangedFile {
    public let path: String
    public let oldPath: String?
    public let status: FileStatus
}

/// One opened repository. Not thread-safe; use one per load.
public final class GitReader {
    let repo: OpaquePointer

    public init(path: URL) throws {
        _ = initOnce
        var repo: OpaquePointer?
        let code = git_repository_open_ext(&repo, path.path, 0, nil)
        guard code == 0, let repo else {
            throw PreceiptsError.notARepo(path.path)
        }
        self.repo = repo
    }

    deinit {
        git_repository_free(repo)
    }

    public var workdir: URL {
        get throws {
            guard let dir = git_repository_workdir(repo) else {
                throw PreceiptsError.notARepo("bare repository")
            }
            return URL(fileURLWithPath: String(cString: dir))
        }
    }

    public var gitDir: URL {
        URL(fileURLWithPath: String(cString: git_repository_path(repo)))
    }

    public func headBranch() throws -> (name: String?, oid: git_oid) {
        var head: OpaquePointer?
        try check(git_repository_head(&head, repo), "HEAD")
        defer { git_reference_free(head) }
        guard let target = git_reference_target(head) else {
            throw PreceiptsError.unbornHead
        }
        let name = git_reference_shorthand(head).map { String(cString: $0) }
        return (name, target.pointee)
    }

    /// First base candidate that resolves, as (short name, commit id).
    public func resolveBase() throws -> (name: String, oid: git_oid) {
        let candidates = [
            ("origin/main", "refs/remotes/origin/main"),
            ("main", "refs/heads/main"),
            ("origin/master", "refs/remotes/origin/master"),
            ("master", "refs/heads/master"),
        ]
        for (short, full) in candidates {
            var reference: OpaquePointer?
            if git_reference_lookup(&reference, repo, full) == 0, let reference {
                defer { git_reference_free(reference) }
                var resolved: OpaquePointer?
                if git_reference_resolve(&resolved, reference) == 0, let resolved {
                    defer { git_reference_free(resolved) }
                    if let target = git_reference_target(resolved) {
                        return (short, target.pointee)
                    }
                }
            }
        }
        throw PreceiptsError.noBase
    }

    public func mergeBase(_ a: git_oid, _ b: git_oid) throws -> git_oid {
        var out = git_oid()
        var a = a
        var b = b
        try check(git_merge_base(&out, repo, &a, &b), "merge-base")
        return out
    }

    func commitTree(_ oid: git_oid) throws -> OpaquePointer {
        var oid = oid
        var commit: OpaquePointer?
        try check(git_commit_lookup(&commit, repo, &oid), "commit lookup")
        defer { git_commit_free(commit) }
        var tree: OpaquePointer?
        try check(git_commit_tree(&tree, commit), "commit tree")
        return tree!
    }

    /// Changed files between a base commit's tree and the working tree
    /// (untracked included — they are part of what a run would mint).
    public func changedFiles(baseCommit: git_oid) throws -> [ChangedFile] {
        let tree = try commitTree(baseCommit)
        defer { git_tree_free(tree) }

        var options = git_diff_options()
        git_diff_options_init(&options, UInt32(GIT_DIFF_OPTIONS_VERSION))
        options.flags |=
            GIT_DIFF_INCLUDE_UNTRACKED.rawValue
            | GIT_DIFF_RECURSE_UNTRACKED_DIRS.rawValue
            | GIT_DIFF_INCLUDE_TYPECHANGE.rawValue

        var diff: OpaquePointer?
        try check(
            git_diff_tree_to_workdir_with_index(&diff, repo, tree, &options),
            "diff tree to workdir"
        )
        defer { git_diff_free(diff) }

        var findOptions = git_diff_find_options()
        git_diff_find_options_init(&findOptions, UInt32(GIT_DIFF_FIND_OPTIONS_VERSION))
        findOptions.flags = GIT_DIFF_FIND_RENAMES.rawValue
        try check(git_diff_find_similar(diff, &findOptions), "find renames")

        var files: [ChangedFile] = []
        for index in 0..<git_diff_num_deltas(diff) {
            guard let delta = git_diff_get_delta(diff, index)?.pointee else { continue }
            let newPath = delta.new_file.path.map { String(cString: $0) }
            let oldPath = delta.old_file.path.map { String(cString: $0) }
            switch delta.status {
            case GIT_DELTA_ADDED, GIT_DELTA_UNTRACKED:
                if let path = newPath {
                    files.append(ChangedFile(path: path, oldPath: nil, status: .added))
                }
            case GIT_DELTA_DELETED:
                if let path = oldPath {
                    files.append(ChangedFile(path: path, oldPath: nil, status: .deleted))
                }
            case GIT_DELTA_RENAMED:
                if let path = newPath {
                    files.append(ChangedFile(path: path, oldPath: oldPath, status: .renamed))
                }
            case GIT_DELTA_MODIFIED, GIT_DELTA_TYPECHANGE:
                if let path = newPath {
                    files.append(ChangedFile(path: path, oldPath: nil, status: .modified))
                }
            default:
                continue
            }
        }
        files.sort { $0.path < $1.path }
        var seen = Set<String>()
        files = files.filter { seen.insert($0.path).inserted }
        return files
    }

    /// (content, isBinary) of a blob at `path` in the base commit's tree.
    public func blobContent(baseCommit: git_oid, path: String) throws -> (String, Bool) {
        let tree = try commitTree(baseCommit)
        defer { git_tree_free(tree) }
        var entry: OpaquePointer?
        guard git_tree_entry_bypath(&entry, tree, path) == 0, let entry else {
            return ("", false)
        }
        defer { git_tree_entry_free(entry) }
        var blob: OpaquePointer?
        guard git_blob_lookup(&blob, repo, git_tree_entry_id(entry)) == 0, let blob else {
            return ("", false)
        }
        defer { git_blob_free(blob) }
        if git_blob_is_binary(blob) != 0 {
            return ("", true)
        }
        let size = Int(git_blob_rawsize(blob))
        guard size > 0, let raw = git_blob_rawcontent(blob) else {
            return ("", false)
        }
        let data = Data(bytes: raw, count: size)
        return (String(decoding: data, as: UTF8.self), false)
    }
}

/// (content, isBinary) of a worktree file.
public func worktreeContent(workdir: URL, path: String) -> (String, Bool) {
    guard let data = try? Data(contentsOf: workdir.appendingPathComponent(path)) else {
        return ("", false)
    }
    if data.prefix(8000).contains(0) {
        return ("", true)
    }
    return (String(decoding: data, as: UTF8.self), false)
}
