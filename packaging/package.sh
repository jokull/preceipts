#!/usr/bin/env bash
# Assemble Preceipts.app from the cargo release build.
#
# Structure follows the Swift-era Scripts/package_app.sh; what is different is
# what goes inside. There are three binaries rather than one, and they must
# ship together — see the MacOS layout below — and there is a LaunchDaemon
# plist for SMAppService to register.
#
# This script does not sign. Signing is packaging/sign.sh, because assembling a
# bundle needs nothing from Apple and signing needs an identity, and folding the
# two together means you cannot exercise the first without the second.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PACKAGING="$ROOT/packaging"
cd "$ROOT"

APP_NAME=${APP_NAME:-Preceipts}
BUNDLE_ID=${BUNDLE_ID:-is.solberg.preceipts}
# The main executable is NOT called "Preceipts". On a default (case-insensitive)
# APFS volume, Contents/MacOS/Preceipts and Contents/MacOS/preceipts are one
# file, so the CLI lands on top of the app and the bundle silently ships one
# binary twice. The Swift-era script hit the same wall — hence its EXEC_NAME
# note. CFBundleName is still Preceipts; only the file on disk differs.
EXEC_NAME=${EXEC_NAME:-preceipts-app}
MACOS_MIN_VERSION=${MACOS_MIN_VERSION:-14.0}
PROFILE=${PROFILE:-release}

# shellcheck source=version.env
source "$PACKAGING/version.env"

# The bundle lands in target/ rather than the repo root: it is a build product,
# and target/ is already ignored, so packaging leaves nothing to clean up.
APP="$ROOT/target/${APP_NAME}.app"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --profile "$PROFILE" 2>&1 | tail -5
fi

BUILD_DIR="$ROOT/target/$PROFILE"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" \
         "$APP/Contents/Resources" \
         "$APP/Contents/Frameworks" \
         "$APP/Contents/Library/LaunchDaemons"

# ---------------------------------------------------------------------
# Binaries.
#
# All three sit in Contents/MacOS side by side, and that adjacency is load
# bearing rather than tidy: preceiptsd::daemon_exe() finds the daemon by looking
# for a sibling of the running executable before it falls back to PATH. A CLI
# copied out on its own cannot find its daemon, and one that fell back to PATH
# would drive whatever daemon happened to be installed instead of the one it
# shipped with.
install_binary() {
  local name="$1" dest="$2"
  local src="$BUILD_DIR/$name"
  if [[ ! -f "$src" ]]; then
    echo "ERROR: no ${name} at ${src}" >&2
    echo "       Build it first:  cargo build --profile ${PROFILE}" >&2
    echo "       (packaging refuses to assemble a bundle that is missing a binary —" >&2
    echo "        a Preceipts.app without ${name} looks installed and is not.)" >&2
    exit 1
  fi
  cp "$src" "$dest"
  chmod +x "$dest"
}

MAIN_BINARY="$APP/Contents/MacOS/$EXEC_NAME"
install_binary preceipts-app "$MAIN_BINARY"
install_binary preceipts "$APP/Contents/MacOS/preceipts"
install_binary preceiptsd "$APP/Contents/MacOS/preceiptsd"

# Three names must be three files. If a rename ever reintroduces a case-only
# collision, the last copy wins and the bundle looks fine until it is launched.
if [[ $(ls "$APP/Contents/MacOS" | wc -l) -ne 3 ]]; then
  echo "ERROR: Contents/MacOS does not hold three binaries — names that differ" >&2
  echo "       only by case collapse into one file on case-insensitive volumes." >&2
  ls -l "$APP/Contents/MacOS" >&2
  exit 1
fi

# ---------------------------------------------------------------------
# Vendor any Homebrew dylib the binaries link against.
#
# The release build currently picks up /opt/homebrew openssl through the git
# stack. That path does not exist on a machine without brew, and a bundle that
# only runs on the machine that built it is the broken-bundle case this script
# is supposed to refuse. So: copy the transitive chain into Contents/Frameworks
# and rewrite every install name to @rpath. Adapted from the Swift-era script,
# generalised to seed from all three binaries instead of one known dylib.
BREW_PREFIX_PATTERN=${BREW_PREFIX_PATTERN:-/opt/homebrew/}

brew_deps() {
  otool -L "$1" | tail -n +2 | awk -v pat="$BREW_PREFIX_PATTERN" 'index($1, pat) == 1 { print $1 }'
}

vendor_dylibs() {
  local binaries=("$@")
  local queue=() seen="" lib base dep file

  for file in "${binaries[@]}"; do
    while IFS= read -r dep; do queue+=("$dep"); done < <(brew_deps "$file")
  done

  while ((${#queue[@]})); do
    lib="${queue[0]}"
    queue=("${queue[@]:1}")
    base=$(basename "$lib")
    [[ "$seen" == *"|$base|"* ]] && continue
    seen="${seen}|$base|"
    if [[ ! -f "$lib" ]]; then
      echo "ERROR: ${binaries[0]} needs $lib, which is not on this machine" >&2
      exit 1
    fi
    cp "$lib" "$APP/Contents/Frameworks/$base"
    chmod u+w "$APP/Contents/Frameworks/$base"
    install_name_tool -id "@rpath/$base" "$APP/Contents/Frameworks/$base"
    while IFS= read -r dep; do
      [[ "$(basename "$dep")" == "$base" ]] && continue
      queue+=("$dep")
    done < <(brew_deps "$lib")
  done

  shopt -s nullglob
  local vendored=("$APP/Contents/Frameworks/"*.dylib)
  shopt -u nullglob
  ((${#vendored[@]})) || return 0

  for file in "${vendored[@]}" "${binaries[@]}"; do
    while IFS= read -r dep; do
      install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$file"
    done < <(brew_deps "$file")
  done
  # Only add the rpath if it is not already there — cargo may have set it, and
  # install_name_tool treats a duplicate LC_RPATH as a fatal error.
  for file in "${binaries[@]}"; do
    if ! otool -l "$file" | grep -q "@executable_path/../Frameworks"; then
      install_name_tool -add_rpath "@executable_path/../Frameworks" "$file"
    fi
  done

  # Nothing may still point at brew. A missed reference is invisible here and
  # fatal on someone else's machine, so it is checked rather than assumed.
  for file in "${vendored[@]}" "${binaries[@]}"; do
    if [[ -n "$(brew_deps "$file")" ]]; then
      echo "ERROR: $file still references $BREW_PREFIX_PATTERN" >&2
      brew_deps "$file" >&2
      exit 1
    fi
  done
}

vendor_dylibs "$MAIN_BINARY" \
              "$APP/Contents/MacOS/preceipts" \
              "$APP/Contents/MacOS/preceiptsd"

# ---------------------------------------------------------------------
# The :443 forwarder's LaunchDaemon plist.
#
# SMAppService.daemon(plistName:) reads plists out of Contents/Library/
# LaunchDaemons, and it requires the filename to equal the job's Label. Hence
# is.solberg.preceipts.forwarder.plist carrying Label
# is.solberg.preceipts.forwarder.
#
# NOTE: crates/preceiptsd/src/forwarder.rs still labels the job
# com.preceipts.forwarder and sudo-writes it to /Library/LaunchDaemons. The two
# have to agree before SMAppService registration can work; the Rust side is what
# moves, when direction-2026-08 step 22 lands. Until then this plist is the
# bundled shape and the sudo path is the one in use.
#
# BundleProgram is bundle-relative on purpose: an absolute path would bake in
# where the .app happened to sit when it was packaged, and the whole point of
# shipping the helper inside the bundle is that moving or deleting the app moves
# or deletes the helper with it.
cat > "$APP/Contents/Library/LaunchDaemons/is.solberg.preceipts.forwarder.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>is.solberg.preceipts.forwarder</string>
  <key>BundleProgram</key><string>Contents/MacOS/preceiptsd</string>
  <key>ProgramArguments</key>
  <array>
    <string>Contents/MacOS/preceiptsd</string>
    <string>forward</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/var/log/preceipts-forwarder.log</string>
  <key>StandardErrorPath</key><string>/var/log/preceipts-forwarder.log</string>
  <key>AssociatedBundleIdentifiers</key>
  <array>
    <!-- So System Settings → Login Items names the app rather than a bare label. -->
    <string>is.solberg.preceipts</string>
  </array>
</dict>
</plist>
PLIST

# ---------------------------------------------------------------------
# Info.plist.
BUILD_TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
GIT_COMMIT=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)

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
    <!-- Stamped so a bundle someone hands you can be traced back to a tree. -->
    <key>BuildTimestamp</key><string>${BUILD_TIMESTAMP}</string>
    <key>GitCommit</key><string>${GIT_COMMIT}</string>
</dict>
</plist>
PLIST

if [[ -f "$PACKAGING/Icon.icns" ]]; then
  cp "$PACKAGING/Icon.icns" "$APP/Contents/Resources/Icon.icns"
fi

# Extended attributes turn into AppleDouble files that break code sealing, so
# they go before anything signs this.
chmod -R u+w "$APP"
xattr -cr "$APP"
find "$APP" -name '._*' -delete

# Ad-hoc signature, not a substitute for sign.sh.
#
# arm64 refuses to execute an unsigned or tampered binary — SIGKILL, no message —
# and rewriting install names above invalidated the signature the linker left.
# Without this the bundle assembles and then every binary in it dies on launch,
# which looks like a crash rather than a signing problem. sign.sh --force
# replaces these with the real identity.
shopt -s nullglob
for dylib in "$APP/Contents/Frameworks/"*.dylib; do
  codesign --force --sign - "$dylib" 2>/dev/null
done
shopt -u nullglob
for helper in preceipts preceiptsd; do
  codesign --force --sign - "$APP/Contents/MacOS/$helper" 2>/dev/null
done
codesign --force --sign - "$APP" 2>/dev/null

echo "Packaged $APP (${MARKETING_VERSION} build ${BUILD_NUMBER})"
echo "Unsigned. Next:  APP_IDENTITY='…' $PACKAGING/sign.sh"
