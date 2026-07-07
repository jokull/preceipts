// Remote-URL → owner/name parsing for the GitHub integration.

import XCTest

@testable import PreceiptsKit

final class GitHubRepoTests: XCTestCase {
    func testParsesCommonRemoteForms() {
        for url in [
            "https://github.com/jokull/preceipts.git",
            "https://github.com/jokull/preceipts",
            "git@github.com:jokull/preceipts.git",
            "git@github.com:jokull/preceipts",
            "ssh://git@github.com/jokull/preceipts.git",
        ] {
            XCTAssertEqual(
                GitHubRepo.parse(remoteURL: url),
                GitHubRepo(owner: "jokull", name: "preceipts"),
                url)
        }
    }

    func testRejectsNonGitHubAndMalformed() {
        XCTAssertNil(GitHubRepo.parse(remoteURL: "https://gitlab.com/o/r.git"))
        XCTAssertNil(GitHubRepo.parse(remoteURL: "git@github.com:justowner"))
        XCTAssertNil(GitHubRepo.parse(remoteURL: ""))
    }
}
