// Settings (⌘,): GitHub sign-in via the OAuth device flow. The client ID
// is a public identifier (user-supplied OAuth app with device flow
// enabled); the token lands in the Keychain. Sign-in is optional — the
// app never gates local features on it.

import AppKit
import SwiftUI

final class SettingsWindowController: NSWindowController {
    private let model = SettingsModel()

    convenience init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 480, height: 320),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false)
        window.title = "Settings"
        window.center()
        self.init(window: window)
        window.contentViewController = NSHostingController(
            rootView: SettingsView(model: model))
        model.loadInitialState()
    }

    func show() {
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}

@MainActor
final class SettingsModel: ObservableObject {
    private static let clientIDKey = "GitHubOAuthClientID"

    @Published var clientID: String = UserDefaults.standard.string(forKey: clientIDKey) ?? "" {
        didSet { UserDefaults.standard.set(clientID, forKey: Self.clientIDKey) }
    }
    @Published var signedInAs: String?
    @Published var deviceCode: GitHubAuth.DeviceCode?
    @Published var busy = false
    @Published var errorText: String?

    private var signInTask: Task<Void, Never>?

    func loadInitialState() {
        guard let token = Keychain.loadToken() else { return }
        busy = true
        Task {
            defer { busy = false }
            signedInAs = (try? await GitHubClient(token: token).viewer()) ?? "(signed in)"
        }
    }

    func signIn() {
        errorText = nil
        busy = true
        let clientID = clientID.trimmingCharacters(in: .whitespaces)
        signInTask = Task {
            defer {
                busy = false
                deviceCode = nil
            }
            do {
                let code = try await GitHubAuth.requestDeviceCode(clientID: clientID)
                deviceCode = code
                NSWorkspace.shared.open(code.verificationURL)
                let token = try await GitHubAuth.pollForToken(clientID: clientID, code: code)
                Keychain.saveToken(token)
                signedInAs = (try? await GitHubClient(token: token).viewer()) ?? "(signed in)"
                NotificationCenter.default.post(name: .gitHubAuthChanged, object: nil)
            } catch is CancellationError {
                // user cancelled — no error surface
            } catch {
                errorText = error.localizedDescription
            }
        }
    }

    func cancelSignIn() {
        signInTask?.cancel()
        signInTask = nil
        deviceCode = nil
        busy = false
    }

    func signOut() {
        Keychain.deleteToken()
        signedInAs = nil
        NotificationCenter.default.post(name: .gitHubAuthChanged, object: nil)
    }
}

struct SettingsView: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("GitHub") {
                if let login = model.signedInAs {
                    LabeledContent("Account") {
                        HStack {
                            Image(systemName: "checkmark.circle.fill")
                                .foregroundStyle(.green)
                            Text(login)
                        }
                    }
                    Button("Sign Out", action: model.signOut)
                } else if let code = model.deviceCode {
                    LabeledContent("Your code") {
                        Text(code.userCode)
                            .font(.title2.monospaced().bold())
                            .textSelection(.enabled)
                    }
                    Text("Enter it on GitHub to finish signing in.")
                        .foregroundStyle(.secondary)
                    HStack {
                        Button("Open GitHub") {
                            NSWorkspace.shared.open(code.verificationURL)
                        }
                        Button("Cancel", action: model.cancelSignIn)
                        ProgressView()
                            .controlSize(.small)
                    }
                } else {
                    TextField("Client ID", text: $model.clientID)
                        .textFieldStyle(.roundedBorder)
                    Text(
                        """
                        A public OAuth app client ID with device flow enabled \
                        (GitHub → Settings → Developer settings → OAuth Apps). \
                        No secret is stored; the token lives in your Keychain.
                        """
                    )
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    Button("Sign In\u{2026}", action: model.signIn)
                        .disabled(
                            model.busy
                                || model.clientID.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                if let error = model.errorText {
                    Text(error)
                        .font(.callout)
                        .foregroundStyle(.red)
                }
            }
            Section {
                Text(
                    "Signed out, PR feedback falls back to the gh CLI when it's installed."
                )
                .font(.callout)
                .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .frame(width: 480)
        .frame(minHeight: 280)
    }
}
