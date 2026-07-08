// The PR drawer's data: title/body, commit log, and the check/deployment
// board. Deployments (Vercel, Cloudflare…) are NOT comment-scraped —
// they occupy a defined seat in GitHub's model: the head commit's
// statusCheckRollup contexts (CheckRun + StatusContext, each with a
// details/target URL). Pure parsing here; transports stay thin.

import Foundation

public struct PrCheck: Sendable, Equatable {
    public enum State: String, Sendable {
        case success, failure, pending, neutral
    }

    public let name: String
    public let state: State
    public let detailsUrl: String?
    public let description: String?

    public init(name: String, state: State, detailsUrl: String?, description: String?) {
        self.name = name
        self.state = state
        self.detailsUrl = detailsUrl
        self.description = description
    }
}

public struct PrCommit: Sendable, Equatable {
    public let sha: String
    public let headline: String
    public let author: String
    public let date: String

    public init(sha: String, headline: String, author: String, date: String) {
        self.sha = sha
        self.headline = headline
        self.author = author
        self.date = date
    }
}

public struct PrOverview: Sendable {
    public let number: Int
    public let title: String
    public let body: String
    public let url: String
    public let reviewDecision: String?
    public let commits: [PrCommit]
    public let checks: [PrCheck]

    public init(
        number: Int, title: String, body: String, url: String, reviewDecision: String?,
        commits: [PrCommit], checks: [PrCheck]
    ) {
        self.number = number
        self.title = title
        self.body = body
        self.url = url
        self.reviewDecision = reviewDecision
        self.commits = commits
        self.checks = checks
    }
}

/// GraphQL response → PrOverview (see `prOverviewQuery`). Nil when the
/// branch has no open PR.
public func parsePrOverview(_ data: Data) throws -> PrOverview? {
    guard
        let root = try JSONSerialization.jsonObject(with: data) as? [String: Any],
        let repository = ((root["data"] as? [String: Any])?["repository"]) as? [String: Any],
        let nodes = ((repository["pullRequests"] as? [String: Any])?["nodes"])
            as? [[String: Any]],
        let pr = nodes.first,
        let number = pr["number"] as? Int
    else {
        return nil
    }

    var commits: [PrCommit] = []
    if let history = ((pr["history"] as? [String: Any])?["nodes"]) as? [[String: Any]] {
        for node in history {
            guard let commit = node["commit"] as? [String: Any] else { continue }
            let author = commit["author"] as? [String: Any]
            let login = (author?["user"] as? [String: Any])?["login"] as? String
            commits.append(
                PrCommit(
                    sha: commit["abbreviatedOid"] as? String ?? "",
                    headline: commit["messageHeadline"] as? String ?? "",
                    author: login ?? author?["name"] as? String ?? "",
                    date: commit["committedDate"] as? String ?? ""))
        }
    }
    // Newest first — a log, not a story.
    commits.reverse()

    var checks: [PrCheck] = []
    if let head = ((pr["head"] as? [String: Any])?["nodes"]) as? [[String: Any]],
        let commit = head.first?["commit"] as? [String: Any],
        let rollup = commit["statusCheckRollup"] as? [String: Any],
        let contexts = (rollup["contexts"] as? [String: Any])?["nodes"] as? [[String: Any]]
    {
        for context in contexts {
            if let name = context["context"] as? String {
                // StatusContext (deployments live here: Vercel, CF Pages…).
                checks.append(
                    PrCheck(
                        name: name,
                        state: statusContextState(context["state"] as? String ?? ""),
                        detailsUrl: context["targetUrl"] as? String,
                        description: context["description"] as? String))
            } else if let name = context["name"] as? String {
                // CheckRun (Actions jobs, app-provided checks).
                checks.append(
                    PrCheck(
                        name: name,
                        state: checkRunState(
                            status: context["status"] as? String ?? "",
                            conclusion: context["conclusion"] as? String),
                        detailsUrl: context["detailsUrl"] as? String,
                        description: context["title"] as? String))
            }
        }
    }

    return PrOverview(
        number: number,
        title: pr["title"] as? String ?? "",
        body: pr["body"] as? String ?? "",
        url: pr["url"] as? String ?? "",
        reviewDecision: pr["reviewDecision"] as? String,
        commits: commits,
        checks: checks)
}

/// `gh pr view --json …` → PrOverview (signed-out fallback).
public func parsePrOverviewGh(_ data: Data) throws -> PrOverview? {
    guard let pr = try JSONSerialization.jsonObject(with: data) as? [String: Any],
        let number = pr["number"] as? Int
    else { return nil }

    var commits: [PrCommit] = []
    for item in pr["commits"] as? [[String: Any]] ?? [] {
        let authors = item["authors"] as? [[String: Any]]
        let author = authors?.first?["login"] as? String
            ?? authors?.first?["name"] as? String ?? ""
        commits.append(
            PrCommit(
                sha: String((item["oid"] as? String ?? "").prefix(7)),
                headline: item["messageHeadline"] as? String ?? "",
                author: author,
                date: item["committedDate"] as? String
                    ?? item["authoredDate"] as? String ?? ""))
    }
    commits.reverse()

    var checks: [PrCheck] = []
    for item in pr["statusCheckRollup"] as? [[String: Any]] ?? [] {
        if let name = item["context"] as? String {
            checks.append(
                PrCheck(
                    name: name,
                    state: statusContextState(item["state"] as? String ?? ""),
                    detailsUrl: item["targetUrl"] as? String,
                    description: item["description"] as? String))
        } else if let name = item["name"] as? String {
            checks.append(
                PrCheck(
                    name: name,
                    state: checkRunState(
                        status: item["status"] as? String ?? "",
                        conclusion: item["conclusion"] as? String),
                    detailsUrl: item["detailsUrl"] as? String,
                    description: item["description"] as? String))
        }
    }

    return PrOverview(
        number: number,
        title: pr["title"] as? String ?? "",
        body: pr["body"] as? String ?? "",
        url: pr["url"] as? String ?? "",
        reviewDecision: (pr["reviewDecision"] as? String).flatMap { $0.isEmpty ? nil : $0 },
        commits: commits,
        checks: checks)
}

private func statusContextState(_ state: String) -> PrCheck.State {
    switch state {
    case "SUCCESS": return .success
    case "FAILURE", "ERROR": return .failure
    case "PENDING", "EXPECTED": return .pending
    default: return .neutral
    }
}

private func checkRunState(status: String, conclusion: String?) -> PrCheck.State {
    guard status == "COMPLETED" else { return .pending }
    switch conclusion ?? "" {
    case "SUCCESS": return .success
    case "FAILURE", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED", "STARTUP_FAILURE":
        return .failure
    case "NEUTRAL", "SKIPPED", "STALE":
        return .neutral
    default:
        return .pending
    }
}

/// Shared by the signed-in client and the gh fallback.
public let prOverviewQuery = """
    query($owner: String!, $name: String!, $branch: String!) {
      repository(owner: $owner, name: $name) {
        pullRequests(headRefName: $branch, states: [OPEN], first: 1) {
          nodes {
            number title body url reviewDecision
            history: commits(last: 30) {
              nodes {
                commit {
                  abbreviatedOid messageHeadline committedDate
                  author { name user { login } }
                }
              }
            }
            head: commits(last: 1) {
              nodes {
                commit {
                  statusCheckRollup {
                    contexts(first: 100) {
                      nodes {
                        ... on CheckRun { name status conclusion detailsUrl title }
                        ... on StatusContext { context state targetUrl description }
                      }
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
    """
