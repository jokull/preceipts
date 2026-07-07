// Interim PR-feedback transport: shell out to the gh CLI (already
// authenticated on dev machines). The OAuth-device-flow + Keychain path
// replaces this without touching the Kit model. Expensive and
// network-bound — everything runs off the main thread, single-flight.

import Foundation
import PreceiptsKit

final class GhClient {
    private let workdir: URL
    private let binary: URL?
    private var fetchInFlight = false

    init(workdir: URL) {
        self.workdir = workdir
        self.binary = Self.locateBinary()
    }

    var available: Bool { binary != nil }

    private static func locateBinary() -> URL? {
        let fm = FileManager.default
        let candidates =
            ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
            + (ProcessInfo.processInfo.environment["PATH"] ?? "")
                .split(separator: ":")
                .map { "\($0)/gh" }
        for candidate in candidates where fm.isExecutableFile(atPath: candidate) {
            return URL(fileURLWithPath: candidate)
        }
        return nil
    }

    /// Fetch feedback for the PR associated with the current branch.
    /// Completion on the main thread. No-ops when a fetch is in flight.
    /// `repo` enables the review-thread resolution lookup (GraphQL).
    func fetchFeedback(
        repo: GitHubRepo?, completion: @escaping (Result<PrFeedback, Error>) -> Void
    ) {
        guard let binary else {
            completion(.failure(GhError.notInstalled))
            return
        }
        guard !fetchInFlight else { return }
        fetchInFlight = true
        let workdir = workdir
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let result = Result { try Self.fetch(binary: binary, workdir: workdir, repo: repo) }
            DispatchQueue.main.async {
                self?.fetchInFlight = false
                completion(result)
            }
        }
    }

    private static func fetch(binary: URL, workdir: URL, repo: GitHubRepo?) throws -> PrFeedback {
        let view = try run(
            binary: binary, workdir: workdir,
            arguments: ["pr", "view", "--json", "number,title,url"])
        guard
            let pr = try JSONSerialization.jsonObject(with: view) as? [String: Any],
            let number = pr["number"] as? Int
        else {
            throw GhError.noPr
        }
        let title = pr["title"] as? String ?? ""
        let url = pr["url"] as? String ?? ""

        func api(_ suffix: String) throws -> Data {
            try run(
                binary: binary, workdir: workdir,
                arguments: [
                    "api", "--paginate", "repos/{owner}/{repo}/\(suffix)",
                ])
        }
        let comments = try parseFeedback(
            reviewComments: normalizeSeams(try api("pulls/\(number)/comments")),
            reviews: normalizeSeams(try api("pulls/\(number)/reviews")),
            conversation: normalizeSeams(try api("issues/\(number)/comments")))

        // Resolution is GraphQL-only; best-effort, empty on any failure.
        var resolved: Set<Int> = []
        if let repo,
            let data = try? run(
                binary: binary, workdir: workdir,
                arguments: [
                    "api", "graphql",
                    "-f", "query=\(reviewThreadResolutionQuery)",
                    "-F", "owner=\(repo.owner)",
                    "-F", "name=\(repo.name)",
                    "-F", "number=\(number)",
                ])
        {
            resolved = (try? parseReviewThreadResolution(data)) ?? []
        }
        return PrFeedback(
            number: number, title: title, url: url, comments: comments,
            resolvedRootIds: resolved)
    }

    /// PR association for the current branch — the signed-out titlebar
    /// chip. Completion on the main thread; nil when there's no PR.
    func fetchPrStatus(completion: @escaping (PrStatus?) -> Void) {
        guard let binary else {
            completion(nil)
            return
        }
        let workdir = workdir
        DispatchQueue.global(qos: .utility).async {
            let status = try? Self.prStatus(binary: binary, workdir: workdir)
            DispatchQueue.main.async { completion(status) }
        }
    }

    private static func prStatus(binary: URL, workdir: URL) throws -> PrStatus? {
        let data = try run(
            binary: binary, workdir: workdir,
            arguments: [
                "pr", "view", "--json", "number,title,url,reviewDecision,statusCheckRollup",
            ])
        guard let pr = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let number = pr["number"] as? Int
        else { return nil }
        return PrStatus(
            number: number,
            title: pr["title"] as? String ?? "",
            url: pr["url"] as? String ?? "",
            reviewDecision: (pr["reviewDecision"] as? String)
                .flatMap(PrStatus.ReviewDecision.init(rawValue:)),
            ciState: ciState(pr["statusCheckRollup"] as? [[String: Any]] ?? []))
    }

    /// Fold gh's rollup array (CheckRun status/conclusion + StatusContext
    /// state) into one verdict.
    private static func ciState(_ rollup: [[String: Any]]) -> PrStatus.CiState? {
        guard !rollup.isEmpty else { return nil }
        var pending = false
        for item in rollup {
            if let status = item["status"] as? String, status != "COMPLETED" {
                pending = true
                continue
            }
            let verdict = item["conclusion"] as? String ?? item["state"] as? String ?? ""
            switch verdict {
            case "FAILURE", "ERROR", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED":
                return .failure
            case "PENDING", "EXPECTED":
                pending = true
            default:
                break
            }
        }
        return pending ? .pending : .success
    }

    /// `--paginate` concatenates JSON arrays; join the "][" seams.
    private static func normalizeSeams(_ data: Data) -> Data {
        guard let text = String(data: data, encoding: .utf8), text.contains("][") else {
            return data
        }
        return Data(text.replacingOccurrences(of: "][", with: ",").utf8)
    }

    private static func run(binary: URL, workdir: URL, arguments: [String]) throws -> Data {
        let process = Process()
        process.executableURL = binary
        process.arguments = arguments
        process.currentDirectoryURL = workdir
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        try process.run()
        var errorOutput = Data()
        let drained = DispatchSemaphore(value: 0)
        DispatchQueue.global(qos: .utility).async {
            errorOutput = stderr.fileHandleForReading.readDataToEndOfFile()
            drained.signal()
        }
        let output = stdout.fileHandleForReading.readDataToEndOfFile()
        drained.wait()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let message = String(decoding: errorOutput, as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if message.contains("no pull requests found") {
                throw GhError.noPr
            }
            throw GhError.commandFailed(message.isEmpty ? "gh exited nonzero" : message)
        }
        return output
    }
}

enum GhError: LocalizedError {
    case notInstalled
    case noPr
    case commandFailed(String)

    var errorDescription: String? {
        switch self {
        case .notInstalled:
            return "GitHub CLI not found \u{2014} brew install gh (OAuth sign-in comes later)"
        case .noPr:
            return "No pull request for this branch"
        case .commandFailed(let message):
            return message
        }
    }
}
