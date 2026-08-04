#!/bin/bash
# Regenerates every screenshot the website ships (§12.4).
#
#   scripts/screenshots.sh
#
# Captures are deterministic: the same fixture, the same window sizes, the same
# scenes. The two crops are cut from the overview capture using the layout
# constants in apps/sourcefour/src/theme.rs, so nothing here hardcodes pixels
# that only hold for one build.
#
# Captured at the display's backing scale, so a retina Mac produces assets at
# twice these dimensions. Regenerate on the best display available.
set -euo pipefail
cd "$(dirname "$0")/.."

shots="website/screenshots"
width=1600
height=1000
mkdir -p "$shots"

cargo build --release --locked >/dev/null 2>&1

for scene in overview diff split image actions; do
  scripts/capture.sh "$scene" "$width" "$height" "$shots/$scene.png"
  echo "captured $shots/$scene.png"
done

python3 - "$shots/overview.png" "$width" "$shots" <<'PY'
"""Cuts the graph and sidebar close-ups out of the overview capture."""
import sys
from PIL import Image

source, logical_width, shots = sys.argv[1], int(sys.argv[2]), sys.argv[3]
image = Image.open(source)
scale = image.width / logical_width

# apps/sourcefour/src/theme.rs
TITLEBAR, TOOLBAR, SIDEBAR, GRAPH, HEADER, ROW = 38, 46, 236, 76, 26, 30

def cut(name, left, top, right, bottom):
    box = tuple(round(value * scale) for value in (left, top, right, bottom))
    image.crop(box).save(f"{shots}/{name}.png")
    print(f"cropped {shots}/{name}.png")

content_top = TITLEBAR + TOOLBAR
# The graph beside the summaries it belongs to: lanes, curves, and ref chips.
cut("graph", SIDEBAR, content_top, SIDEBAR + GRAPH + 580, content_top + HEADER + ROW * 15)
# The whole sidebar: worktrees, branches, remotes.
cut("sidebar", 0, content_top, SIDEBAR, content_top + HEADER + ROW * 17)
PY
