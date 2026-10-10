#!/bin/bash
# Usage: scripts/footprint-sample.sh LABEL RATE|none APP [ARGS...]
# Samples the macOS physical footprint 40 times over 10 s after 5 s of settling, with the app's
# perf probe on (no ticker) and, unless RATE is none, one session to a loopback MUD at RATE bytes/s.
label=$1; rate=$2; shift 2
R="$(cd "$(dirname "$0")/.." && pwd)"
port=$((4480 + RANDOM % 100))
if [ "$rate" != none ]; then "$R/target/release/wandur-bench" mud-server --port $port --rate $rate >/dev/null 2>&1 & srv=$!; sleep 0.3; fi
export WANDUR_PERF_PROBE="${TMPDIR:-/tmp}/wandur-fp-$label.jsonl" WANDUR_PERF_SCENARIO=$([ "$rate" = none ] && echo idle || echo sessions) WANDUR_PERF_HOST=127.0.0.1:$port WANDUR_PERF_SESSIONS=1 WANDUR_PERF_SECONDS=40 WANDUR_PERF_NO_TICK=1 WANDUR_DIRECTORY_URL=http://127.0.0.1:9/
rm -f "$WANDUR_PERF_PROBE"
"$@" >/dev/null 2>&1 & app=$!
sleep 5
vals=()
for i in $(seq 40); do v=$(/usr/bin/footprint -f bytes --noCategories $app 2>/dev/null | sed -nE 's/.*Footprint: ([0-9]+) B.*/\1/p'); vals+=($((v/1048576))); sleep 0.25; done
peak=$(/usr/bin/footprint -f bytes --noCategories $app 2>/dev/null | sed -nE 's/.*phys_footprint_peak: ([0-9]+) B.*/\1/p')
kill $app; [ -n "$srv" ] && kill $srv
sorted=($(printf "%s\n" "${vals[@]}" | sort -n))
echo "$label min=${sorted[0]} median=${sorted[20]} max=${sorted[39]} peak=$((peak/1048576)) MB"
