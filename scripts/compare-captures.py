#!/usr/bin/env python3
"""Compares two captures with a small antialiasing tolerance (§12.4)."""

import sys

from PIL import Image, ImageChops

TOLERANCE_PER_CHANNEL = 12   # ignore subpixel/antialiasing wobble
MAX_DIFFERING_FRACTION = 0.002


def main() -> int:
    golden = Image.open(sys.argv[1]).convert("RGB")
    current = Image.open(sys.argv[2]).convert("RGB")
    if golden.size != current.size:
        print(f"size mismatch: {golden.size} vs {current.size}", file=sys.stderr)
        return 1
    diff = ImageChops.difference(golden, current).convert("L")
    histogram = diff.histogram()
    differing = sum(histogram[TOLERANCE_PER_CHANNEL + 1 :])
    fraction = differing / (golden.size[0] * golden.size[1])
    if fraction > MAX_DIFFERING_FRACTION:
        print(f"{fraction:.4%} of pixels differ", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
