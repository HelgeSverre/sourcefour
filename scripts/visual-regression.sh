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

# Every capture: scene at a size. The overview repeats across sizes to catch
# responsive regressions; each overlay scene is captured once.
captures=()
for size in "${sizes[@]}"; do
  captures+=("overview $size")
done
captures+=("diff 1280 800" "split 1280 800" "image 1280 800" "video 1280 800" "settings 1280 800" "actions 1280 800" "commit 1280 800" "preview 1280 800")

failures=0
for capture in "${captures[@]}"; do
  read -r scene width height <<<"$capture"
  name="${scene}-${width}x${height}"
  current="$work_dir/$name.png"
  if ! scripts/capture.sh "$scene" "$width" "$height" "$current"; then
    echo "FAILED   $name: window never appeared"
    failures=$((failures + 1))
    continue
  fi

  golden="$golden_dir/$name.png"
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
    echo "ok       $name"
  else
    cp "$current" "$golden_dir/failed-$name.png"
    echo "DIFFERS  $name (see $golden_dir/failed-$name.png)"
    failures=$((failures + 1))
  fi
done

rm -rf "$work_dir"
if [ "$failures" -gt 0 ]; then
  echo "$failures capture(s) differ"
  exit 1
fi
echo "all captures match"
