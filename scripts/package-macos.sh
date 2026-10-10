#!/usr/bin/env bash
# Builds the macOS app bundle and disk image, unsigned (no Developer ID, no notarization):
#
#   scripts/package-macos.sh [--version X] [--check-updates-as X] [--output DIR] [--no-dmg]
#
#   DIR/Wandur Rust.app       the bundle (executable Contents/MacOS/wandur, the icon, Info.plist)
#   DIR/Wandur-Rust-X-macos-<arch>.dmg   a compressed image with the app and an Applications link
#
# DIR defaults to target/package/macos. --version (default: the workspace version) goes into
# Info.plist (CFBundleShortVersionString takes its numeric part). The bundle is named and
# identified apart from the C# client ("Wandur Rust", net.wandur.client.rust), so both can be
# installed side by side; its data stays in Application Support/Wandur-Rust.
#
# Update checks: a build from source never asks wandur.net for a newer release. Only
# --check-updates-as X builds the binary as release X (WANDUR_RELEASE_VERSION), which then
# asks the configured directory's /client/latest once a day. Leave it off until wandur.net
# publishes releases of this client; otherwise it would offer the C# client's releases.
#
# Unsigned: Gatekeeper warns on a downloaded copy (right-click > Open, or
# xattr -dr com.apple.quarantine). The arm64 executable keeps the ad-hoc signature the linker
# gives it, which Apple Silicon needs to run it at all. The image is made with hdiutil only
# (no window background; the C# client's dmgbuild layout needs a Python package download).
set -euo pipefail
export COPYFILE_DISABLE=1

version=""; updates_as=""; output=""; dmg=1
usage() { sed -n '2,8p' "$0" >&2; exit 2; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:-}"; shift ;;
    --check-updates-as) updates_as="${2:-}"; shift ;;
    --output) output="${2:-}"; shift ;;
    --no-dmg) dmg=0 ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
  shift
done
[[ "$(uname -s)" == "Darwin" ]] || { echo "Build the macOS app on macOS." >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd -P)"
if [[ -z "$version" ]]; then
  version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
fi
semver='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
[[ "$version" =~ $semver ]] || { echo "--version must look like 1.2.3 or 1.2.3-beta.1, not $version" >&2; exit 2; }
if [[ -n "$updates_as" && ! "$updates_as" =~ $semver ]]; then
  echo "--check-updates-as must look like 1.2.3, not $updates_as" >&2; exit 2
fi
output="${output:-$root/target/package/macos}"
mkdir -p "$output"
output="$(cd "$output" && pwd -P)"
arch="$(uname -m)"
app="$output/Wandur Rust.app"

if [[ -n "$updates_as" ]]; then
  (cd "$root" && WANDUR_RELEASE_VERSION="$updates_as" cargo build --release -p wandur-app)
else
  (cd "$root" && cargo build --release -p wandur-app)
fi
binary="$root/target/release/wandur"
[[ -x "$binary" ]] || { echo "the build produced no $binary" >&2; exit 1; }
bash "$root/scripts/make-icons.sh" >/dev/null

# Stage in the system temp directory: on exFAT every file gains an AppleDouble ._ companion,
# which would end up sealed inside the bundle and the image.
work="$(cd "$(mktemp -d "${TMPDIR:-/tmp}/wandur-rust-macos.XXXXXX")" && pwd -P)"
trap 'rm -rf "$work"' EXIT
stage="$work/Wandur Rust.app"
mkdir -p "$stage/Contents/MacOS" "$stage/Contents/Resources"
cp "$binary" "$stage/Contents/MacOS/wandur"
cp "$root/target/package/icons/Wandur.icns" "$stage/Contents/Resources/Wandur.icns"
cp "$root/LICENSE" "$stage/Contents/Resources/LICENSE"
cat > "$stage/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Wandur</string>
  <key>CFBundleDisplayName</key><string>Wandur Rust</string>
  <key>CFBundleIdentifier</key><string>net.wandur.client.rust</string>
  <key>CFBundleExecutable</key><string>wandur</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>Wandur</string>
  <key>CFBundleShortVersionString</key><string>${version%%-*}</string>
  <key>CFBundleVersion</key><string>${version%%-*}.$(date -u +%Y%m%d%H%M)</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
xattr -cr "$stage" 2>/dev/null || true
rm -rf "$app"
# ditto keeps the bundle as it is (no ._ files written by rsync on exFAT).
ditto --norsrc --noextattr "$stage" "$app"
echo "Built: $app"

if [[ $dmg -eq 1 ]]; then
  image="$output/Wandur-Rust-$version-macos-$arch.dmg"
  content="$work/dmg"
  mkdir -p "$content"
  ditto --norsrc --noextattr "$stage" "$content/Wandur Rust.app"
  ln -s /Applications "$content/Applications"
  rm -f "$image"
  hdiutil create -quiet -volname "Wandur Rust $version" -srcfolder "$content" -ov -format UDZO "$image"
  echo "Built: $image"
fi
