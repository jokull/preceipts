#!/usr/bin/env bash
# Codesign the bundle package.sh assembled.
#
# Separate from packaging because it is the only step that needs something from
# Apple. Notarization is a further step still and is not here: it needs an App
# Store Connect key, and there is nothing to notarize until this passes.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PACKAGING="$ROOT/packaging"

APP_NAME=${APP_NAME:-Preceipts}
APP=${APP:-$ROOT/target/${APP_NAME}.app}

if [[ -z "${APP_IDENTITY:-}" ]]; then
  cat >&2 <<'MSG'
ERROR: APP_IDENTITY is not set.

Set it to the common name of a code-signing identity in your keychain:

  export APP_IDENTITY='Developer ID Application: Your Name (TEAMID)'

`security find-identity -v -p codesigning` lists what you have. For a local
identity that has no Apple account behind it:

  packaging/setup-dev-signing.sh

MSG
  exit 1
fi

if [[ ! -d "$APP" ]]; then
  echo "ERROR: no bundle at $APP — run packaging/package.sh first." >&2
  exit 1
fi

# ---------------------------------------------------------------------
# Team ID, for reporting only.
#
# It used to be substituted into $(TeamIdentifierPrefix) in the entitlements,
# back when they claimed a keychain access group. They no longer do: that key
# is a restricted entitlement, authorised by an embedded provisioning profile
# rather than by a certificate, and a binary that claims it without one is
# SIGKILLed at its first page fault. Sharing is an ACL on the keychain item
# now — see secrets.rs — which needs nothing from this script beyond a real
# signature. We still resolve the team so the summary at the end can print it,
# because "which team signed this" is the one fact that decides whether the
# team-scoped ACL is live.
TEAM_ID=${TEAM_ID:-}
if [[ -z "$TEAM_ID" && "$APP_IDENTITY" =~ \(([A-Z0-9]{10})\)$ ]]; then
  TEAM_ID="${BASH_REMATCH[1]}"
fi
if [[ -z "$TEAM_ID" ]]; then
  echo "! Identity '$APP_IDENTITY' names no Team ID." >&2
  echo "  The bundle will sign, but secrets fall back to the open ACL." >&2
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

APP_ENTS="$PACKAGING/entitlements/app.entitlements"
DAEMON_ENTS="$PACKAGING/entitlements/daemon.entitlements"

# --options runtime is the hardened runtime, which notarization requires.
# --timestamp asks Apple's timestamp server, so signatures stay valid after the
# certificate expires rather than everything shipped going stale with it.
SIGN=(codesign --force --options runtime --timestamp --sign "$APP_IDENTITY")

# ---------------------------------------------------------------------
# Inside out. A bundle's seal covers its nested code, so anything signed after
# the bundle invalidates the bundle's own signature.
shopt -s nullglob
for dylib in "$APP/Contents/Frameworks/"*.dylib; do
  echo "signing $(basename "$dylib")"
  "${SIGN[@]}" "$dylib"
done
shopt -u nullglob

for helper in preceipts preceiptsd; do
  echo "signing $helper"
  "${SIGN[@]}" --entitlements "$DAEMON_ENTS" "$APP/Contents/MacOS/$helper"
done

echo "signing $APP_NAME.app"
"${SIGN[@]}" --entitlements "$APP_ENTS" "$APP"

# ---------------------------------------------------------------------
# Verify. --deep --strict walks the nested code we just signed; without it a
# helper that failed to sign shows up only when Gatekeeper refuses to launch.
codesign --verify --deep --strict --verbose=2 "$APP"

echo
codesign -dv "$APP" 2>&1 | grep -E 'TeamIdentifier|Identifier=|Authority=' || true
echo
echo "Entitlements as embedded:"
codesign -d --entitlements :- --xml "$APP" 2>/dev/null \
  | plutil -convert xml1 -o - - 2>/dev/null || true
echo
echo "Signed $APP"
