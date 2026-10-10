#!/bin/sh
# Adds Wandur Rust to your desktop's application menu, for this user only. Run it from the
# folder you unpacked, after moving that folder where you want to keep it:
#   ./install-desktop-entry.sh
# Undo with: rm ~/.local/share/applications/net.wandur.client.rust.desktop \
#   ~/.local/share/icons/hicolor/256x256/apps/net.wandur.client.rust.png
# (The C# client's script of the same name, adapted.)
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
chmod +x "$here/wandur"
mkdir -p "$data/applications" "$data/icons/hicolor/256x256/apps"
cp "$here/net.wandur.client.rust.png" "$data/icons/hicolor/256x256/apps/net.wandur.client.rust.png"

# Desktop Entry Exec quoting: the path goes in double quotes, where " ` $ and \ take a
# backslash; the string-value escape rule then doubles every backslash, and a literal % is
# written %%.
exec_path="$(printf '%s' "$here/wandur" | sed -e 's/\\/\\\\\\\\/g' -e 's/["`$]/\\\\&/g' -e 's/%/%%/g')"
EXEC_LINE="Exec=\"$exec_path\"" awk '/^Exec=/ { print ENVIRON["EXEC_LINE"]; next } /^# / { next } { print }' \
  "$here/net.wandur.client.rust.desktop.in" > "$data/applications/net.wandur.client.rust.desktop"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$data/applications" || true
echo "Added Wandur Rust to the application menu ($data/applications/net.wandur.client.rust.desktop)."
