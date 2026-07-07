#!/usr/bin/env bash
# Dev loop: kill the running app, package, relaunch. From the vendored
# macos-spm-app-packaging skill template.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP_NAME=${APP_NAME:-Preceipts}
CONF=${1:-release}

pkill -x "PreceiptsApp" 2>/dev/null || true

"$ROOT/Scripts/package_app.sh" "$CONF"

open "$ROOT/${APP_NAME}.app" --args "${2:-$PWD}"
