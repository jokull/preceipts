// Integration: ChangesetLoader against a scratch repo, both scopes —
// port of the Rust core's integration suite (core/tests/changeset.rs).

import XCTest

@testable import PreceiptsKit

final class ChangesetTests: XCTestCase {
    private func sh(_ dir: URL, _ args: [String]) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = args
        process.currentDirectoryURL = dir
        let stderr = Pipe()
        process.standardError = stderr
        process.standardOutput = Pipe()
        try process.run()
        process.waitUntilExit()
        XCTAssertEqual(process.terminationStatus, 0, "\(args) failed")
    }

    private func write(_ dir: URL, _ path: String, _ content: String) throws {
        let url = dir.appendingPathComponent(path)
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try content.write(to: url, atomically: true, encoding: .utf8)
    }

    /// main has a.txt + src/keep.rs; feature modifies a.txt, adds new.rs,
    /// deletes src/keep.rs (committed), plus uncommitted edits on top.
    private func scratchRepo() throws -> URL {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("preceipts-swift-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try sh(dir, ["git", "init", "-q", "-b", "main"])
        try sh(dir, ["git", "config", "user.name", "t"])
        try sh(dir, ["git", "config", "user.email", "t@t.local"])
        try sh(dir, ["git", "config", "commit.gpgsign", "false"])
        try write(dir, "a.txt", "one\ntwo\nthree\n")
        try write(dir, "src/keep.rs", "fn keep() {}\n")
        try sh(dir, ["git", "add", "-A"])
        try sh(dir, ["git", "commit", "-qm", "base"])
        try sh(dir, ["git", "checkout", "-qb", "feature"])
        try write(dir, "a.txt", "one\nTWO\nthree\n")
        try write(dir, "new.rs", "fn new_thing() {}\n")
        try sh(dir, ["git", "rm", "-q", "src/keep.rs"])
        try sh(dir, ["git", "add", "-A"])
        try sh(dir, ["git", "commit", "-qm", "feature work"])
        try write(dir, "a.txt", "one\nTWO\nthree\nfour\n")
        try write(dir, "untracked.md", "# notes\n")
        return dir
    }

    func testBranchScopeSpansCommitsAndWorkingTree() throws {
        let dir = try scratchRepo()
        defer { try? FileManager.default.removeItem(at: dir) }
        let changeset = try ChangesetLoader.load(repoPath: dir, scope: .branch)

        XCTAssertEqual(changeset.branch, "feature")
        XCTAssertTrue(changeset.baseName.hasPrefix("main"))

        let statuses = Dictionary(
            uniqueKeysWithValues: changeset.files.map { ($0.path, $0.status) })
        XCTAssertEqual(statuses["a.txt"], .modified)
        XCTAssertEqual(statuses["new.rs"], .added)
        XCTAssertEqual(statuses["src/keep.rs"], .deleted)
        XCTAssertEqual(statuses["untracked.md"], .added)

        let a = changeset.files.first { $0.path == "a.txt" }!
        XCTAssertEqual(a.added, 2)
        XCTAssertEqual(a.removed, 1)

        let keep = changeset.files.first { $0.path == "src/keep.rs" }!
        XCTAssertEqual(keep.removed, 1)
        XCTAssertEqual(keep.added, 0)
    }

    func testUncommittedScopeSeesOnlyWorkingTreeChanges() throws {
        let dir = try scratchRepo()
        defer { try? FileManager.default.removeItem(at: dir) }
        let changeset = try ChangesetLoader.load(repoPath: dir, scope: .uncommitted)

        let paths = Set(changeset.files.map(\.path))
        XCTAssertTrue(paths.contains("a.txt"))
        XCTAssertTrue(paths.contains("untracked.md"))
        XCTAssertFalse(paths.contains("new.rs"))
        XCTAssertFalse(paths.contains("src/keep.rs"))

        let a = changeset.files.first { $0.path == "a.txt" }!
        XCTAssertEqual(a.added, 1)
        XCTAssertEqual(a.removed, 0)
    }

    func testBinaryFilesAreFlaggedNotDiffed() throws {
        let dir = try scratchRepo()
        defer { try? FileManager.default.removeItem(at: dir) }
        let bytes: [UInt8] = [0, 1, 2, 3, 0, 255]
        try Data(bytes).write(to: dir.appendingPathComponent("blob.bin"))
        let changeset = try ChangesetLoader.load(repoPath: dir, scope: .uncommitted)
        let bin = changeset.files.first { $0.path == "blob.bin" }!
        XCTAssertTrue(bin.isBinary)
        XCTAssertTrue(bin.hunks.isEmpty)
    }
}
