#!/usr/bin/env bash
# Render the app icon (generate_icon.swift) and convert it to Icon.icns
# via sips + iconutil. Idempotent; output lands at swift/Icon.icns.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

swift "$ROOT/Scripts/generate_icon.swift" "$TMP/icon-1024.png"

ICONSET="$TMP/Icon.iconset"
mkdir -p "$ICONSET"
for entry in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" \
    "64 icon_32x32@2x" "128 icon_128x128" "256 icon_128x128@2x" \
    "256 icon_256x256" "512 icon_256x256@2x" "512 icon_512x512" \
    "1024 icon_512x512@2x"; do
  px=${entry%% *}
  name=${entry##* }
  sips -z "$px" "$px" "$TMP/icon-1024.png" --out "$ICONSET/$name.png" >/dev/null
done

iconutil --convert icns --output "$ROOT/Icon.icns" "$ICONSET"
echo "wrote $ROOT/Icon.icns"
