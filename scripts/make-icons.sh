#!/usr/bin/env bash
# Derives the packaging icons from crates/wandur-app/assets/icons/icon-1024.png into
# target/package/icons (nothing is committed), as the C# client's make-icons.sh does:
#   icon-macos-1024.png  full bleed on a dark tile (macOS masks it into its own rounded square)
#   icon-256.png         rounded corners, transparent outside (Linux menu icon)
#   Wandur.ico           16 to 256 pixels (Windows)
#   Wandur.icns          macOS bundle icon (macOS only: sips and iconutil)
# Needs python3 with Pillow for the crop and the .ico; without Pillow the master is used as it
# is (no crop, no .ico).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
master="$root/crates/wandur-app/assets/icons/icon-1024.png"
out="$root/target/package/icons"
[[ -f "$master" ]] || { echo "missing $master" >&2; exit 1; }
mkdir -p "$out"
if python3 -c 'import PIL' 2>/dev/null; then
  python3 - "$master" "$out" <<'PY'
import sys
from PIL import Image, ImageDraw
master, out = sys.argv[1], sys.argv[2]
art = Image.open(master).convert("RGBA")
box = art.getchannel("A").getbbox() or (0, 0, art.width, art.height)
side = max(box[2] - box[0], box[3] - box[1])
cx, cy = (box[0] + box[2]) // 2, (box[1] + box[3]) // 2
square = art.crop((cx - side // 2, cy - side // 2, cx - side // 2 + side, cy - side // 2 + side)).resize((1024, 1024), Image.LANCZOS)
full = Image.alpha_composite(Image.new("RGBA", (1024, 1024), (12, 11, 13, 255)), square)
full.save(f"{out}/icon-macos-1024.png", optimize=True)
mask = Image.new("L", (1024, 1024), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, 1023, 1023), radius=230, fill=255)
rounded = full.copy(); rounded.putalpha(mask)
rounded.resize((256, 256), Image.LANCZOS).save(f"{out}/icon-256.png", optimize=True)
sizes = [16, 24, 32, 48, 64, 128, 256]
frames = [rounded.resize((s, s), Image.LANCZOS) for s in sizes]
frames[-1].save(f"{out}/Wandur.ico", format="ICO", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
PY
else
  echo "Pillow not found: using the master icon as it is, and no .ico" >&2
  cp "$master" "$out/icon-macos-1024.png"
  if command -v sips >/dev/null; then
    sips -z 256 256 "$master" --out "$out/icon-256.png" >/dev/null
  else
    cp "$master" "$out/icon-256.png"
  fi
fi
if [[ "$(uname -s)" == "Darwin" ]]; then
  iconset="$out/Wandur.iconset"
  rm -rf "$iconset"; mkdir -p "$iconset"
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$out/icon-macos-1024.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$out/icon-macos-1024.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$out/Wandur.icns"
  rm -rf "$iconset"
fi
echo "Icons in $out"
