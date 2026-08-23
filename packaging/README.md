# packaging

Assembling and signing `Preceipts.app`. Three binaries, one bundle.

```sh
packaging/package.sh                                  # → target/Preceipts.app
APP_IDENTITY='Developer ID Application: … (TEAMID)' \
  packaging/sign.sh
```

## Scripts

| | |
|---|---|
| `package.sh` | Builds (`cargo build --release`) and assembles the bundle. Does not sign beyond ad hoc. |
| `sign.sh` | Codesigns the assembled bundle inside-out, then verifies. |
| `setup-dev-signing.sh` | Makes a self-signed certificate so `sign.sh` runs without an Apple account. |
| `version.env` | `MARKETING_VERSION` / `BUILD_NUMBER`, read by `package.sh`. |
| `entitlements/` | `app.entitlements` and `daemon.entitlements`. Each explains itself. |

## What is in the bundle, and why

```
Contents/MacOS/preceipts-app      the GPUI app (CFBundleExecutable)
Contents/MacOS/preceipts          the CLI
Contents/MacOS/preceiptsd         the daemon
Contents/Frameworks/libssl.3, libcrypto.3
Contents/Library/LaunchDaemons/is.solberg.preceipts.forwarder.plist
```

All three binaries are siblings because `preceiptsd::daemon_exe()` locates the
daemon next to the running executable before falling back to `PATH` — so the app
and the CLI drive the daemon they shipped with, not one that happens to be
installed.

The main executable is `preceipts-app`, not `Preceipts`: `Preceipts` and
`preceipts` are the same filename on a case-insensitive volume, which is every
default macOS install. `CFBundleName` is still `Preceipts`.

The openssl dylibs are vendored out of Homebrew and rewritten to `@rpath`,
because the release build links them and a bundle that needs brew installed is a
bundle that only runs here.

The LaunchDaemon plist is the `:443` forwarder, for registration through
`SMAppService.daemon` — which requires the plist's filename to equal its `Label`.
`crates/preceiptsd/src/forwarder.rs` currently uses the label
`com.preceipts.forwarder` and `sudo`-writes into `/Library/LaunchDaemons`; the
two have to converge before SMAppService registration works, and the Rust side is
what moves (direction-2026-08, step 22).

## Environment

| | |
|---|---|
| `APP_IDENTITY` | Required by `sign.sh`. The common name of a code-signing identity. |
| `TEAM_ID` | Only needed if the identity's name does not end in `(TEAMID)`. |
| `SKIP_BUILD=1` | Package what is already in `target/release`. |
| `APP_NAME`, `BUNDLE_ID`, `EXEC_NAME`, `PROFILE`, `MACOS_MIN_VERSION` | Overrides; the defaults are what ships. |

## What Apple has to give you first

1. A paid Apple Developer membership, which is where a **Team ID** comes from.
2. A **Developer ID Application** certificate in the login keychain. Not "Apple
   Development" — that one signs for your machines only and cannot be notarized.
3. An **App Store Connect API key** (key `.p8`, key id, issuer id) for
   `notarytool`. There is no notarize script here yet; it is step 22's other
   half.
4. An App ID / keychain access group registered for
   `<TEAM_ID>.is.solberg.preceipts.shared`.

The App Store is not a path: the daemon supervises arbitrary user processes and
binds ports, so the app cannot be sandboxed. Developer ID plus notarization is
the distribution route.

## What cannot be tested without a real identity

`setup-dev-signing.sh` gets you a stable code identity and nothing else. With a
self-signed certificate you can confirm the bundle assembles, signs, and passes
`codesign --verify --deep --strict`. You cannot confirm:

- **The shared keychain group.** Groups are namespaced by Team ID, and a
  self-signed cert has none. `sign.sh` drops the entitlement rather than
  embedding a group that would never match, so app/daemon secret sharing — and
  with it the retirement of the open-ACL workaround in `secrets.rs` — is
  untested until there is a team.
- **SMAppService registration.** Registering the bundled forwarder requires a
  signed, Gatekeeper-accepted bundle; an unnotarized one is refused.
- **Notarization and Gatekeeper.** `spctl -a -t exec` will reject anything signed
  this way on any machine.
