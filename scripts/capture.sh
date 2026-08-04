#!/bin/bash
# Captures one `--demo` scene of the release build to a PNG.
#
#   scripts/capture.sh SCENE WIDTH HEIGHT OUTPUT.png
#
# The demo seeds its whole state — including the scene's overlay — before the
# first frame, so a capture is stable even when the window never reaches the
# foreground (§12.4). Caller is responsible for having built the release binary.
set -euo pipefail
cd "$(dirname "$0")/.."

scene="$1"
width="$2"
height="$3"
output="$4"

./target/release/sourcefour --demo --scene "$scene" --width "$width" --height "$height" \
  >/dev/null 2>&1 &
pid=$!

window_id=""
for _ in $(seq 1 20); do
  sleep 0.5
  window_id="$(swift scripts/window-id.swift "$pid")" && [ -n "$window_id" ] && break
done
if [ -z "$window_id" ]; then
  kill "$pid" 2>/dev/null || true
  echo "capture failed: the $scene window never appeared" >&2
  exit 1
fi

# -o omits the window shadow, whose size depends on key-window state and would
# make capture dimensions nondeterministic.
screencapture -x -o -l"$window_id" "$output"
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
