// GitHub identity of a repo: owner/name parsed from the origin remote,
// plus the PR-status model the titlebar chip renders. Transport-agnostic —
// filled by gh today, by the OAuth URLSession client when signed in.

import Clibgit2
import Foundation

public struct GitHubRepo: Sendable, Equatable {
    public let owner: String
    public let name: String

    public init(owner: String, name: String) {
        self.owner = owner
        self.name = name
    }

    /// Parse "owner/name" out of a git remote URL. Handles the wild forms:
    /// https://github.com/o/r(.git), git@github.com:o/r(.git),
    /// ssh://git@github.com/o/r. Nil for non-GitHub remotes.
    public static func parse(remoteURL: String) -> GitHubRepo? {
        var rest: Substring
        if let range = remoteURL.range(of: "github.com") {
            rest = remoteURL[range.upperBound...]
        } else {
            return nil
        }
        guard let first = rest.first, first == "/" || first == ":" else { return nil }
        rest = rest.dropFirst()
        var parts = rest.split(separator: "/")
        guard parts.count >= 2 else { return nil }
        let owner = String(parts.removeFirst())
        var name = String(parts.removeFirst())
        if name.hasSuffix(".git") {
            name = String(name.dropLast(4))
        }
        guard !owner.isEmpty, !name.isEmpty else { return nil }
        return GitHubRepo(owner: owner, name: name)
    }
}

extension GitReader {
    /// URL of the "origin" remote, or nil when there isn't one.
    public func originRemoteURL() -> String? {
        var remote: OpaquePointer?
        guard git_remote_lookup(&remote, repo, "origin") == 0, let remote else { return nil }
        defer { git_remote_free(remote) }
        guard let url = git_remote_url(remote) else { return nil }
        return String(cString: url)
    }
}

/// The titlebar's PR association: number, title, review + CI rollup.
public struct PrStatus: Sendable, Equatable {
    public enum ReviewDecision: String, Sendable {
        case approved = "APPROVED"
        case changesRequested = "CHANGES_REQUESTED"
        case reviewRequired = "REVIEW_REQUIRED"
    }

    public enum CiState: String, Sendable {
        case success = "SUCCESS"
        case failure = "FAILURE"
        case pending = "PENDING"
        case error = "ERROR"
        case expected = "EXPECTED"
    }

    public let number: Int
    public let title: String
    public let url: String
    public let reviewDecision: ReviewDecision?
    public let ciState: CiState?

    public init(
        number: Int, title: String, url: String,
        reviewDecision: ReviewDecision?, ciState: CiState?
    ) {
        self.number = number
        self.title = title
        self.url = url
        self.reviewDecision = reviewDecision
        self.ciState = ciState
    }
}
