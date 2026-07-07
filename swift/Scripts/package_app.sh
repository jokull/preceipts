#!/usr/bin/env bash
# Package Preceipts.app from the SwiftPM build — adapted from the vendored
# macos-spm-app-packaging skill template. Additions over the template:
# EXEC_NAME (binary name differs from app name) and the libgit2 dylib
# chain vendored from Homebrew into Contents/Frameworks with @rpath
# rewrites, so the app runs without brew installed.
set -euo pipefail

CONF=${1:-release}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

APP_NAME=${APP_NAME:-Preceipts}
EXEC_NAME=${EXEC_NAME:-PreceiptsApp}
BUNDLE_ID=${BUNDLE_ID:-is.solberg.preceipts}
MACOS_MIN_VERSION=${MACOS_MIN_VERSION:-14.0}
SIGNING_MODE=${SIGNING_MODE:-}
APP_IDENTITY=${APP_IDENTITY:-}

if [[ -f "$ROOT/version.env" ]]; then
  source "$ROOT/version.env"
else
  MARKETING_VERSION=${MARKETING_VERSION:-0.1.0}
  BUILD_NUMBER=${BUILD_NUMBER:-1}
fi

ARCH_LIST=( ${ARCHES:-} )
if [[ ${#ARCH_LIST[@]} -eq 0 ]]; then
  HOST_ARCH=$(uname -m)
  ARCH_LIST=("$HOST_ARCH")
fi

for ARCH in "${ARCH_LIST[@]}"; do
  swift build -c "$CONF" --arch "$ARCH"
done

APP="$ROOT/${APP_NAME}.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"

# Icon: generate if missing (Scripts/build_icon.sh renders + converts).
ICON_TARGET="$ROOT/Icon.icns"
if [[ ! -f "$ICON_TARGET" && -x "$ROOT/Scripts/build_icon.sh" ]]; then
  "$ROOT/Scripts/build_icon.sh"
fi

BUILD_TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
GIT_COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key><string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
    <key>CFBundleExecutable</key><string>${EXEC_NAME}</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${MARKETING_VERSION}</string>
    <key>CFBundleVersion</key><string>${BUILD_NUMBER}</string>
    <key>LSMinimumSystemVersion</key><string>${MACOS_MIN_VERSION}</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHumanReadableCopyright</key><string>© Jökull Sólberg</string>
    <key>CFBundleIconFile</key><string>Icon</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>BuildTimestamp</key><string>${BUILD_TIMESTAMP}</string>
    <key>GitCommit</key><string>${GIT_COMMIT}</string>
</dict>
</plist>
PLIST

build_product_path() {
  local name="$1"
  local arch="$2"
  case "$arch" in
    arm64|x86_64) echo ".build/${arch}-apple-macosx/$CONF/$name" ;;
    *) echo ".build/$CONF/$name" ;;
  esac
}

verify_binary_arches() {
  local binary="$1"; shift
  local expected=("$@")
  local actual
  actual=$(lipo -archs "$binary")
  for arch in "${expected[@]}"; do
    if [[ "$actual" != *"$arch"* ]]; then
      echo "ERROR: $binary missing arch $arch (have: ${actual})" >&2
      exit 1
    fi
  done
}

install_binary() {
  local name="$1"
  local dest="$2"
  local binaries=()
  for arch in "${ARCH_LIST[@]}"; do
    local src
    src=$(build_product_path "$name" "$arch")
    if [[ ! -f "$src" ]]; then
      echo "ERROR: Missing ${name} build for ${arch} at ${src}" >&2
      exit 1
    fi
    binaries+=("$src")
  done
  if [[ ${#ARCH_LIST[@]} -gt 1 ]]; then
    lipo -create "${binaries[@]}" -output "$dest"
  else
    cp "${binaries[0]}" "$dest"
  fi
  chmod +x "$dest"
  verify_binary_arches "$dest" "${ARCH_LIST[@]}"
}

MAIN_BINARY="$APP/Contents/MacOS/$EXEC_NAME"
install_binary "$EXEC_NAME" "$MAIN_BINARY"

# ---------------------------------------------------------------------
# Vendor the libgit2 dylib chain from Homebrew. Walks transitive
# /opt/homebrew dependencies (libgit2 → llhttp, libssh2 → openssl),
# copies them into Contents/Frameworks, and rewrites every install name
# to @rpath so nothing references brew paths at runtime.

LIBGIT2_ROOT=$(brew --prefix libgit2 2>/dev/null || echo /opt/homebrew/opt/libgit2)
LIBGIT2_DYLIB=$(ls "$LIBGIT2_ROOT"/lib/libgit2.*.dylib | grep -E 'libgit2\.[0-9]+\.[0-9]+\.dylib$' | head -1)
if [[ -z "$LIBGIT2_DYLIB" ]]; then
  LIBGIT2_DYLIB=$(readlink -f "$LIBGIT2_ROOT/lib/libgit2.dylib")
fi

vendor_dylibs() {
  local queue=("$LIBGIT2_DYLIB")
  local seen=""
  while ((${#queue[@]})); do
    local lib="${queue[0]}"
    queue=("${queue[@]:1}")
    local base
    base=$(basename "$lib")
    if [[ "$seen" == *"|$base|"* ]]; then
      continue
    fi
    seen="${seen}|$base|"
    cp "$lib" "$APP/Contents/Frameworks/$base"
    chmod u+w "$APP/Contents/Frameworks/$base"
    install_name_tool -id "@rpath/$base" "$APP/Contents/Frameworks/$base"
    while IFS= read -r dep; do
      queue+=("$dep")
    done < <(otool -L "$lib" | tail -n +2 \
      | awk '/\/opt\/homebrew\//{print $1}' | grep -v "/$base\$" || true)
  done

  # Retarget brew references in the vendored dylibs and the app binary.
  local file
  for file in "$APP/Contents/Frameworks/"*.dylib "$MAIN_BINARY"; do
    while IFS= read -r dep; do
      install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$file"
    done < <(otool -L "$file" | tail -n +2 | awk '/\/opt\/homebrew\//{print $1}')
  done
  install_name_tool -add_rpath "@executable_path/../Frameworks" "$MAIN_BINARY"

  # Nothing may still point at brew.
  for file in "$APP/Contents/Frameworks/"*.dylib "$MAIN_BINARY"; do
    if otool -L "$file" | grep -q "/opt/homebrew/"; then
      echo "ERROR: $file still references /opt/homebrew" >&2
      otool -L "$file" | grep "/opt/homebrew/" >&2
      exit 1
    fi
  done
}
vendor_dylibs

# SwiftPM resource bundles are emitted next to the built binary.
PREFERRED_BUILD_DIR="$(dirname "$(build_product_path "$EXEC_NAME" "${ARCH_LIST[0]}")")"
shopt -s nullglob
SWIFTPM_BUNDLES=("${PREFERRED_BUILD_DIR}/"*.bundle)
shopt -u nullglob
if [[ ${#SWIFTPM_BUNDLES[@]} -gt 0 ]]; then
  for bundle in "${SWIFTPM_BUNDLES[@]}"; do
    cp -R "$bundle" "$APP/Contents/Resources/"
  done
fi

if [[ -f "$ICON_TARGET" ]]; then
  cp "$ICON_TARGET" "$APP/Contents/Resources/Icon.icns"
fi

# Ensure contents are writable before stripping attributes and signing.
chmod -R u+w "$APP"

# Strip extended attributes to prevent AppleDouble files that break code sealing.
xattr -cr "$APP"
find "$APP" -name '._*' -delete

ENTITLEMENTS_DIR="$ROOT/.build/entitlements"
APP_ENTITLEMENTS=${APP_ENTITLEMENTS:-$ENTITLEMENTS_DIR/${APP_NAME}.entitlements}
mkdir -p "$ENTITLEMENTS_DIR"
if [[ ! -f "$APP_ENTITLEMENTS" ]]; then
  cat > "$APP_ENTITLEMENTS" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <!-- Add entitlements here if needed. -->
</dict>
</plist>
PLIST
fi

if [[ "$SIGNING_MODE" == "adhoc" || -z "$APP_IDENTITY" ]]; then
  CODESIGN_ARGS=(--force --sign "-")
else
  CODESIGN_ARGS=(--force --timestamp --options runtime --sign "$APP_IDENTITY")
fi

# Sign vendored dylibs before the app bundle.
for dylib in "$APP/Contents/Frameworks/"*.dylib; do
  codesign "${CODESIGN_ARGS[@]}" "$dylib"
done
codesign "${CODESIGN_ARGS[@]}" --entitlements "$APP_ENTITLEMENTS" "$APP"

echo "Packaged $APP (${MARKETING_VERSION} build ${BUILD_NUMBER}, ${ARCH_LIST[*]})"
