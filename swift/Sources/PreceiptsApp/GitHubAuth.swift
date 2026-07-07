// GitHub sign-in: OAuth device flow with a public client ID (native app,
// no secret), token in the macOS Keychain — never on disk. Sign-in lives
// in Settings and is never required; signed out, the app is exactly the
// local tool plus the gh-CLI fallback.

import Foundation
import Security

extension Notification.Name {
    /// Posted after sign-in/sign-out; consumers re-read the Keychain.
    static let gitHubAuthChanged = Notification.Name("PreceiptsGitHubAuthChanged")
}

enum Keychain {
    private static let service = "is.solberg.preceipts"
    private static let account = "github-oauth-token"

    static func saveToken(_ token: String) {
        deleteToken()
        let attributes: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: Data(token.utf8),
        ]
        SecItemAdd(attributes as CFDictionary, nil)
    }

    static func loadToken() -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
            let data = result as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func deleteToken() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)
    }
}

enum GitHubAuth {
    struct DeviceCode {
        let deviceCode: String
        /// Shown to the user; entered at the verification URL.
        let userCode: String
        let verificationURL: URL
        /// Seconds between polls (GitHub's floor).
        let interval: TimeInterval
    }

    enum AuthError: LocalizedError {
        case http(String)
        case denied
        case expired

        var errorDescription: String? {
            switch self {
            case .http(let message): return message
            case .denied: return "Sign-in was denied on GitHub"
            case .expired: return "The device code expired \u{2014} try signing in again"
            }
        }
    }

    static func requestDeviceCode(clientID: String) async throws -> DeviceCode {
        let fields = try await post(
            url: "https://github.com/login/device/code",
            body: ["client_id": clientID, "scope": "repo"])
        guard
            let device = fields["device_code"] as? String,
            let user = fields["user_code"] as? String,
            let verification = (fields["verification_uri"] as? String).flatMap(URL.init)
        else {
            throw AuthError.http(errorMessage(fields))
        }
        let interval = fields["interval"] as? Double ?? 5
        return DeviceCode(
            deviceCode: device, userCode: user, verificationURL: verification,
            interval: interval)
    }

    /// Poll until the user authorizes in the browser. Honors GitHub's
    /// pacing (authorization_pending / slow_down).
    static func pollForToken(clientID: String, code: DeviceCode) async throws -> String {
        var interval = code.interval
        while true {
            try await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            let fields = try await post(
                url: "https://github.com/login/oauth/access_token",
                body: [
                    "client_id": clientID,
                    "device_code": code.deviceCode,
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                ])
            if let token = fields["access_token"] as? String {
                return token
            }
            switch fields["error"] as? String {
            case "authorization_pending":
                continue
            case "slow_down":
                interval += 5
            case "access_denied":
                throw AuthError.denied
            case "expired_token":
                throw AuthError.expired
            default:
                throw AuthError.http(errorMessage(fields))
            }
        }
    }

    private static func post(
        url: String, body: [String: String]
    ) async throws -> [String: Any] {
        var request = URLRequest(url: URL(string: url)!)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, _) = try await URLSession.shared.data(for: request)
        return try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
    }

    private static func errorMessage(_ fields: [String: Any]) -> String {
        (fields["error_description"] as? String)
            ?? (fields["error"] as? String)
            ?? "GitHub returned an unexpected response"
    }
}
