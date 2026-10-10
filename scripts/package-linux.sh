#!/usr/bin/env bash
# The Linux download: a tarball with the wandur executable, the menu entry template, its
# installer and the icon.
#
#   scripts/package-linux.sh [--version X] [--target TRIPLE] [--output DIR] [--check]
#
#   DIR/Wandur-Rust-X-linux-<arch>.tar.gz   (folder Wandur-Rust-X-linux-<arch> with wandur,
#   install-desktop-entry.sh, net.wandur.client.rust.desktop.in, net.wandur.client.rust.png, LICENSE)
#
# TRIPLE defaults to x86_64-unknown-linux-gnu; DIR to target/package/linux. Build it on Linux
# (or with a cross linker for the target). The target must already be installed for the pinned
# toolchain; this script never installs one. --check only runs `cargo check --target TRIPLE`.
# Status: untested on Linux (written on macOS, where no Linux target is installed). The app
# needs the usual X11 or Wayland libraries and, for the vault, secret-tool.
set -euo pipefail
export COPYFILE_DISABLE=1
version=""; target="x86_64-unknown-linux-gnu"; output=""; check=0
usage() { sed -n '2,10p' "$0" >&2; exit 2; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:-}"; shift ;;
    --target) target="${2:-}"; shift ;;
    --output) output="${2:-}"; shift ;;
    --check) check=1 ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
  shift
done
root="$(cd "$(dirname "$0")/.." && pwd -P)"
if ! (cd "$root" && rustup target list --installed 2>/dev/null) | grep -qx "$target"; then
  echo "The $target target is not installed for the pinned toolchain; this script does not install it." >&2
  echo "On a Linux machine: rustup target add $target (from the project folder), then run this again." >&2
  exit 3
fi
if [[ $check -eq 1 ]]; then
  (cd "$root" && cargo check --release --target "$target" -p wandur-app)
  exit 0
fi
version="${version:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "bad --version $version" >&2; exit 2; }
output="${output:-$root/target/package/linux}"
mkdir -p "$output"; output="$(cd "$output" && pwd -P)"
(cd "$root" && cargo build --release --target "$target" -p wandur-app)
binary="$root/target/$target/release/wandur"
[[ -x "$binary" ]] || { echo "the build produced no $binary" >&2; exit 1; }
bash "$root/scripts/make-icons.sh" >/dev/null
name="Wandur-Rust-$version-linux-${target%%-*}"
work="$(cd "$(mktemp -d "${TMPDIR:-/tmp}/wandur-rust-linux.XXXXXX")" && pwd -P)"
trap 'rm -rf "$work"' EXIT
stage="$work/$name"
mkdir -p "$stage"
cp "$binary" "$stage/wandur"
cp "$root/scripts/linux/install-desktop-entry.sh" "$root/scripts/linux/net.wandur.client.rust.desktop.in" "$stage/"
cp "$root/target/package/icons/icon-256.png" "$stage/net.wandur.client.rust.png"
cp "$root/LICENSE" "$stage/LICENSE"
find "$stage" -type d -exec chmod 755 {} +
find "$stage" -type f -exec chmod 644 {} +
chmod 755 "$stage/wandur" "$stage/install-desktop-entry.sh"
archive="$output/$name.tar.gz"
if tar --version 2>/dev/null | grep -q GNU; then
  tar -C "$work" --owner=0 --group=0 --numeric-owner -czf "$archive" "$name"
else
  tar -C "$work" --uid 0 --gid 0 -czf "$archive" "$name"
fi
echo "Built: $archive"
