#!/bin/bash
# Builds the three social loops: frames via headless Chrome, audio + cut
# lists via generate.py, assembly via ffmpeg. Outputs land in out/.
set -euo pipefail
cd "$(dirname "$0")"

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
mkdir -p out/frames

python3 generate.py

render() { # aspect width height
  for state in 0 1 2 3 4 5; do
    "$CHROME" --headless --disable-gpu --hide-scrollbars \
      --screenshot="out/frames/$1-$state.png" --window-size="$2,$3" \
      "file://$PWD/animation.html?aspect=$1&state=$state" 2>/dev/null
  done
}
render square 1080 1080
render wide 1920 1080
render tall 1080 1920

assemble() { # aspect outname
  ffmpeg -y -loglevel error -f concat -safe 0 -i "out/list-$1.txt" \
    -i out/techno.wav -r 60 -c:v libx264 -preset slow -crf 18 \
    -pix_fmt yuv420p -c:a aac -b:a 192k -t 7.5 "out/$2"
  echo "out/$2"
}
assemble square sourcefour-square.mp4
assemble wide sourcefour-wide.mp4
assemble tall sourcefour-tiktok.mp4
