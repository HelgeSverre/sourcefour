#!/bin/bash
# §12.4 visual-regression captures of the deterministic demo fixture.
#
#   scripts/visual-regression.sh            # compare against goldens
#   scripts/visual-regression.sh --record   # (re)record goldens
#
# The demo seeds its whole state before the first frame, so captures are
# stable even when the window never reaches the foreground.
set -euo pipefail
cd "$(dirname "$0")/.."

record=0
[ "${1:-}" = "--record" ] && record=1
sizes=("1280 800" "1000 700" "900 560")
golden_dir="fixtures/visual"
work_dir="$(mktemp -d)"
mkdir -p "$golden_dir"

cargo build --release --locked >/dev/null 2>&1

failures=0
for size in "${sizes[@]}"; do
  read -r width height <<<"$size"
  ./target/release/sourcefour --demo --width "$width" --height "$height" >/dev/null 2>&1 &
  pid=$!
  window_id=""
  for _ in $(seq 1 20); do
    sleep 0.5
    window_id="$(swift scripts/window-id.swift "$pid")" && [ -n "$window_id" ] && break
  done
  if [ -z "$window_id" ]; then
    echo "FAILED   demo ${width}x${height}: window never appeared"
    kill "$pid" 2>/dev/null || true
    failures=$((failures + 1))
    continue
  fi
  current="$work_dir/demo-${width}x${height}.png"
  # -o omits the window shadow, whose size depends on key-window state and
  # would make capture dimensions nondeterministic.
  screencapture -x -o -l"$window_id" "$current"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true

  golden="$golden_dir/demo-${width}x${height}.png"
  if [ "$record" = 1 ]; then
    cp "$current" "$golden"
    echo "recorded $golden"
    continue
  fi
  if [ ! -f "$golden" ]; then
    echo "MISSING golden $golden — run with --record first"
    failures=$((failures + 1))
    continue
  fi
  if python3 scripts/compare-captures.py "$golden" "$current"; then
    echo "ok       demo ${width}x${height}"
  else
    cp "$current" "$golden_dir/failed-${width}x${height}.png"
    echo "DIFFERS  demo ${width}x${height} (see $golden_dir/failed-${width}x${height}.png)"
    failures=$((failures + 1))
  fi
done

rm -rf "$work_dir"
if [ "$failures" -gt 0 ]; then
  echo "$failures capture(s) differ"
  exit 1
fi
echo "all captures match"
