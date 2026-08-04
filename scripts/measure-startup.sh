#!/bin/bash
# §12.5 startup measurement: process start (main) to the end of the first
# frame, sampled over N runs of the release build.
#
#   scripts/measure-startup.sh [iterations] [repo-path]
#
# With no repo path the deterministic --demo fixture runs, which measures the
# window shell without repository I/O. Results append to
# fixtures/stress/<date>-startup.txt; p50/p95 are nearest-rank.
set -euo pipefail
cd "$(dirname "$0")/.."

iterations="${1:-10}"
repo="${2:-}"

cargo build --release --locked >/dev/null 2>&1

samples=()
for _ in $(seq 1 "$iterations"); do
  if [ -n "$repo" ]; then
    line="$(SOURCEFOUR_STARTUP_LOG=1 ./target/release/sourcefour "$repo" \
      --width 1280 --height 800 2>/dev/null | grep first-frame-ms)"
  else
    line="$(SOURCEFOUR_STARTUP_LOG=1 ./target/release/sourcefour --demo \
      --width 1280 --height 800 2>/dev/null | grep first-frame-ms)"
  fi
  samples+=("${line#first-frame-ms }")
done

out="fixtures/stress/$(date +%Y-%m-%d)-startup.txt"
{
  echo "startup first-frame samples (ms), $iterations runs, target: ${repo:---demo}"
  printf '%s\n' "${samples[@]}"
} >> "$out"

python3 - "${samples[@]}" <<'EOF'
import sys
samples = sorted(int(value) for value in sys.argv[1:])
def rank(percentile):
    index = max(0, -(-percentile * len(samples) // 100) - 1)
    return samples[index]
print(f"n={len(samples)} min={samples[0]} p50={rank(50)} p95={rank(95)} max={samples[-1]} (ms)")
EOF
echo "raw samples appended to $out"
