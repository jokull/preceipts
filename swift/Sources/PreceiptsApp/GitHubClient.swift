// Signed-in GitHub transport: URLSession with the Keychain token.
// GraphQL for PR association (reviewDecision + statusCheckRollup are
// GraphQL-only), REST for the three feedback endpoints — same payloads
// the Kit's parseFeedback consumes from the gh path. Deliberately no
// client library: see the "No wrapper deps" note in the design doc.

import Foundation
import PreceiptsKit

final class GitHubClient {
    private let token: String
    private let session: URLSession

    init(token: String) {
        self.token = token
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpAdditionalHeaders = [
            "Authorization": "Bearer \(token)",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        ]
        self.session = URLSession(configuration: configuration)
    }

    deinit {
        // URLSessions outlive their owner until invalidated — clients are
        // recreated on every auth change, so finish and release each one.
        session.finishTasksAndInvalidate()
    }

    /// Signed-in login, for the Settings pane.
    func viewer() async throws -> String {
        let (data, _) = try await get(URL(string: "https://api.github.com/user")!)
        guard let user = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let login = user["login"] as? String
        else {
            throw GitHubError.unexpectedPayload
        }
        return login
    }

    /// The open PR for a branch, with review + CI rollup. Nil when none.
    func prStatus(repo: GitHubRepo, branch: String) async throws -> PrStatus? {
        let query = """
            query($owner: String!, $name: String!, $branch: String!) {
              repository(owner: $owner, name: $name) {
                pullRequests(headRefName: $branch, states: [OPEN], first: 1) {
                  nodes {
                    number title url reviewDecision
                    commits(last: 1) {
                      nodes { commit { statusCheckRollup { state } } }
                    }
                  }
                }
              }
            }
            """
        var request = URLRequest(url: URL(string: "https://api.github.com/graphql")!)
        request.httpMethod = "POST"
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "query": query,
            "variables": ["owner": repo.owner, "name": repo.name, "branch": branch],
        ])
        let (data, response) = try await session.data(for: request)
        try Self.checkStatus(response, data: data)

        guard
            let root = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let repository = ((root["data"] as? [String: Any])?["repository"])
                as? [String: Any],
            let nodes = ((repository["pullRequests"] as? [String: Any])?["nodes"])
                as? [[String: Any]]
        else {
            throw GitHubError.unexpectedPayload
        }
        guard let pr = nodes.first, let number = pr["number"] as? Int else {
            return nil
        }
        let rollup =
            (((pr["commits"] as? [String: Any])?["nodes"] as? [[String: Any]])?
                .first?["commit"] as? [String: Any])?["statusCheckRollup"] as? [String: Any]
        return PrStatus(
            number: number,
            title: pr["title"] as? String ?? "",
            url: pr["url"] as? String ?? "",
            reviewDecision: (pr["reviewDecision"] as? String)
                .flatMap(PrStatus.ReviewDecision.init(rawValue:)),
            ciState: (rollup?["state"] as? String).flatMap(PrStatus.CiState.init(rawValue:)))
    }

    /// Feedback for the branch's open PR — the same three payloads the gh
    /// path fetches, parsed by the Kit. Nil when the branch has no PR.
    func fetchFeedback(repo: GitHubRepo, branch: String) async throws -> PrFeedback? {
        guard let status = try await prStatus(repo: repo, branch: branch) else {
            return nil
        }
        let base = "https://api.github.com/repos/\(repo.owner)/\(repo.name)"
        async let reviewComments = paginated("\(base)/pulls/\(status.number)/comments")
        async let reviews = paginated("\(base)/pulls/\(status.number)/reviews")
        async let conversation = paginated("\(base)/issues/\(status.number)/comments")
        async let meta = reviewThreadMeta(repo: repo, number: status.number)
        let comments = try await parseFeedback(
            reviewComments: reviewComments,
            reviews: reviews,
            conversation: conversation)
        return PrFeedback(
            number: status.number, title: status.title, url: status.url, comments: comments,
            threadMeta: (try? await meta) ?? [:])
    }

    /// The PR drawer's payload: title/body, commit log, checks board
    /// (deployments included — statusCheckRollup contexts).
    func fetchPrOverview(repo: GitHubRepo, branch: String) async throws -> PrOverview? {
        let data = try await graphql(
            query: prOverviewQuery,
            variables: ["owner": repo.owner, "name": repo.name, "branch": branch])
        return try parsePrOverview(data)
    }

    private func reviewThreadMeta(
        repo: GitHubRepo, number: Int
    ) async throws -> [Int: ReviewThreadMeta] {
        let data = try await graphql(
            query: reviewThreadMetaQuery,
            variables: ["owner": repo.owner, "name": repo.name, "number": number])
        return try parseReviewThreadMeta(data)
    }

    /// Toggle a review thread's resolution — the one thing REST can't do.
    func setThreadResolved(nodeId: String, resolved: Bool) async throws {
        let mutation = resolved ? "resolveReviewThread" : "unresolveReviewThread"
        let query = """
            mutation($threadId: ID!) {
              \(mutation)(input: {threadId: $threadId}) { thread { isResolved } }
            }
            """
        let data = try await graphql(query: query, variables: ["threadId": nodeId])
        if let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let errors = root["errors"] as? [[String: Any]], !errors.isEmpty
        {
            let message = errors.first?["message"] as? String ?? "mutation failed"
            throw GitHubError.http(status: 200, body: message)
        }
    }

    /// Edit the viewer's own comment. Review comments and conversation
    /// comments live under different REST namespaces.
    func editComment(repo: GitHubRepo, kind: FeedbackKind, id: Int, body: String) async throws {
        let namespace = kind == .conversation ? "issues" : "pulls"
        let url = URL(
            string:
                "https://api.github.com/repos/\(repo.owner)/\(repo.name)/\(namespace)/comments/\(id)"
        )!
        var request = URLRequest(url: url)
        request.httpMethod = "PATCH"
        request.httpBody = try JSONSerialization.data(withJSONObject: ["body": body])
        let (data, response) = try await session.data(for: request)
        try Self.checkStatus(response, data: data)
    }

    private func graphql(query: String, variables: [String: Any]) async throws -> Data {
        var request = URLRequest(url: URL(string: "https://api.github.com/graphql")!)
        request.httpMethod = "POST"
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "query": query, "variables": variables,
        ])
        let (data, response) = try await session.data(for: request)
        try Self.checkStatus(response, data: data)
        return data
    }

    /// Follow Link rel="next" and splice the page arrays — the same
    /// "][" → "," seam-join gh --paginate produces.
    private func paginated(_ urlString: String) async throws -> Data {
        var url = URL(string: urlString + "?per_page=100")
        var pages: [String] = []
        while let current = url {
            let (data, response) = try await get(current)
            pages.append(String(decoding: data, as: UTF8.self))
            url = Self.nextLink(response)
        }
        return Data(pages.joined().replacingOccurrences(of: "][", with: ",").utf8)
    }

    private func get(_ url: URL) async throws -> (Data, URLResponse) {
        let (data, response) = try await session.data(from: url)
        try Self.checkStatus(response, data: data)
        return (data, response)
    }

    private static func checkStatus(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else { return }
        guard (200..<300).contains(http.statusCode) else {
            if http.statusCode == 401 {
                throw GitHubError.unauthorized
            }
            let body = String(decoding: data.prefix(200), as: UTF8.self)
            throw GitHubError.http(status: http.statusCode, body: body)
        }
    }

    private static func nextLink(_ response: URLResponse) -> URL? {
        guard let http = response as? HTTPURLResponse,
            let header = http.value(forHTTPHeaderField: "Link")
        else { return nil }
        for part in header.split(separator: ",") {
            guard part.contains("rel=\"next\""),
                let start = part.firstIndex(of: "<"),
                let end = part.firstIndex(of: ">")
            else { continue }
            return URL(string: String(part[part.index(after: start)..<end]))
        }
        return nil
    }
}

/// Shared by the signed-in client and the gh fallback (`gh api graphql`).
let reviewThreadMetaQuery = """
    query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) {
        pullRequest(number: $number) {
          reviewThreads(first: 100) {
            nodes { id isResolved comments(first: 1) { nodes { databaseId } } }
          }
        }
      }
    }
    """

enum GitHubError: LocalizedError {
    case unauthorized
    case unexpectedPayload
    case http(status: Int, body: String)

    var errorDescription: String? {
        switch self {
        case .unauthorized:
            return "GitHub sign-in expired \u{2014} sign in again in Settings"
        case .unexpectedPayload:
            return "GitHub returned an unexpected response"
        case .http(let status, let body):
            return "GitHub error \(status): \(body)"
        }
    }
}
