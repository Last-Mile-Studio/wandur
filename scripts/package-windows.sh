#!/usr/bin/env bash
# The Windows download: a zip with wandur.exe and the licence.
#
#   scripts/package-windows.sh [--version X] [--target TRIPLE] [--output DIR] [--check]
#
#   DIR/Wandur-Rust-X-windows-<arch>.zip   (folder Wandur-Rust-X-windows-<arch> with wandur.exe, LICENSE)
#
# TRIPLE defaults to x86_64-pc-windows-msvc; DIR to target/package/windows. Build it on Windows
# (Git Bash or WSL with the MSVC toolchain). The target must already be installed for the pinned
# toolchain; this script never installs one. --check only runs `cargo check --target TRIPLE`.
# Unsigned: SmartScreen warns on a downloaded copy. The executable has no icon resource yet
# (target/package/icons/Wandur.ico is made, but no build step embeds it).
# Status: untested on Windows (written on macOS, where no Windows target is installed).
set -euo pipefail
version=""; target="x86_64-pc-windows-msvc"; output=""; check=0
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
  echo "On a Windows machine: rustup target add $target (from the project folder), then run this again." >&2
  exit 3
fi
if [[ $check -eq 1 ]]; then
  (cd "$root" && cargo check --release --target "$target" -p wandur-app)
  exit 0
fi
version="${version:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "bad --version $version" >&2; exit 2; }
output="${output:-$root/target/package/windows}"
mkdir -p "$output"; output="$(cd "$output" && pwd -P)"
(cd "$root" && cargo build --release --target "$target" -p wandur-app)
binary="$root/target/$target/release/wandur.exe"
[[ -f "$binary" ]] || { echo "the build produced no $binary" >&2; exit 1; }
name="Wandur-Rust-$version-windows-${target%%-*}"
archive="$output/$name.zip"
python=python3
"$python" -c '' 2>/dev/null || python=python
"$python" - "$binary" "$root/LICENSE" "$name" "$archive" <<'PY'
import sys, zipfile
binary, licence, name, archive = sys.argv[1:]
with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
    zf.write(binary, f"{name}/wandur.exe")
    zf.write(licence, f"{name}/LICENSE")
PY
echo "Built: $archive"
