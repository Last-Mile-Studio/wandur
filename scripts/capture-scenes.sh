#!/bin/bash
# Usage: scripts/capture-scenes.sh OUT_DIR [SCENE...]
#
# Captures every scene (or the ones named) as OUT_DIR/<scene>.png, headless, against loopback
# bench servers this script starts and stops: The Lantern Road (mud-server --page lantern),
# Starfall Reach's login screen (mud-server --page login), Legends of the Jedi's one room
# (mud-server --page jedi, for world-theme-session) and the fixture directory
# (directory-server --fixture). Nothing reaches the network. Each run uses a
# fresh throwaway data directory under .superpowers/. Builds the release binaries first unless
# WANDUR_CAPTURE_PROFILE=debug. OUT_DIR/http-requests.log lists every address the app asked
# (the run fails if one is not on loopback); OUT_DIR/directory-requests.log has the request
# heads the bench directory received.
#
# Environment passed through: WANDUR_SCREENSHOT_SCALE (pixels per point, default 1),
# WANDUR_SCENE_TIMEOUT (seconds, default 15). WANDUR_CAPTURE_WINDOW=WxH renders at that window
# size instead of the default 1300x820.
set -u
if [ $# -lt 1 ]; then
    echo "usage: $0 OUT_DIR [SCENE...]" >&2
    exit 2
fi
R="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$1"
OUT="$(cd "$1" && pwd)"
shift
profile="${WANDUR_CAPTURE_PROFILE:-release}"
if [ "$profile" = release ]; then
    (cd "$R" && cargo build --release -q -p wandur-app -p wandur-bench) || exit 1
else
    (cd "$R" && cargo build -q -p wandur-app -p wandur-bench) || exit 1
fi
APP="$R/target/$profile/wandur"
BENCH="$R/target/$profile/wandur-bench"

# A free loopback port (ask the OS through a short-lived listener).
free_port() {
    python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}
mud_port=$(free_port)
login_port=$(free_port)
jedi_port=$(free_port)
dir_port=$(free_port)
"$BENCH" mud-server --port "$mud_port" --page lantern >/dev/null 2>&1 &
mud_pid=$!
"$BENCH" mud-server --port "$login_port" --page login >/dev/null 2>&1 &
login_pid=$!
"$BENCH" mud-server --port "$jedi_port" --page jedi >/dev/null 2>&1 &
jedi_pid=$!
# The directory also answers the update check (0.1.6, for update-notice) and logs every
# request it gets; WANDUR_HTTP_LOG has the app log every address it asks.
"$BENCH" directory-server --fixture --port "$dir_port" --latest 0.1.6 --log "$OUT/directory-requests.log" >/dev/null 2>&1 &
dir_pid=$!
cleanup() {
    kill "$mud_pid" "$login_pid" "$jedi_pid" "$dir_pid" 2>/dev/null
    wait "$mud_pid" "$login_pid" "$jedi_pid" "$dir_pid" 2>/dev/null
}
trap cleanup EXIT INT TERM

# Wait until both servers listen.
for _ in $(seq 50); do
    if nc -z 127.0.0.1 "$mud_port" 2>/dev/null && nc -z 127.0.0.1 "$login_port" 2>/dev/null \
        && nc -z 127.0.0.1 "$jedi_port" 2>/dev/null \
        && nc -z 127.0.0.1 "$dir_port" 2>/dev/null; then break; fi
    sleep 0.1
done

if [ $# -gt 0 ]; then
    scenes=("$@")
else
    scenes=($("$APP" --scene list | awk '{print $1}'))
fi
failed=0
size=()
if [ -n "${WANDUR_CAPTURE_WINDOW:-}" ]; then
    size=(--window-size "$WANDUR_CAPTURE_WINDOW")
fi
for scene in "${scenes[@]}"; do
    data="$R/.superpowers/scene-data/$scene"
    rm -rf "$data"
    mkdir -p "$data"
    if WANDUR_SCENE_MUD="127.0.0.1:$mud_port" WANDUR_SCENE_LOGIN_MUD="127.0.0.1:$login_port" \
        WANDUR_SCENE_JEDI_MUD="127.0.0.1:$jedi_port" WANDUR_DIRECTORY_URL="http://127.0.0.1:$dir_port" \
        WANDUR_HTTP_LOG="$OUT/http-requests.log" \
        WANDUR_SCREENSHOT="$OUT/$scene.png" "$APP" --data-dir "$data" ${size[@]+"${size[@]}"} --scene "$scene"; then
        echo "ok    $scene -> $OUT/$scene.png"
    else
        echo "FAIL  $scene" >&2
        failed=1
    fi
done
# Every address the app asked must be on loopback (see OUT_DIR/http-requests.log).
if [ -f "$OUT/http-requests.log" ] && grep -v -E '^GET http://127\.0\.0\.1:' "$OUT/http-requests.log" >/dev/null; then
    echo "FAIL  requests left loopback:" >&2
    grep -v -E '^GET http://127\.0\.0\.1:' "$OUT/http-requests.log" >&2
    failed=1
fi
exit $failed
