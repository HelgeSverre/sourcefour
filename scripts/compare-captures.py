#!/usr/bin/env python3
"""Compares two captures with a small antialiasing tolerance (§12.4)."""

import sys

from PIL import Image, ImageChops

TOLERANCE_PER_CHANNEL = 12   # ignore subpixel/antialiasing wobble
MAX_DIFFERING_FRACTION = 0.002


def main() -> int:
    golden = Image.open(sys.argv[1]).convert("RGB")
    current = Image.open(sys.argv[2]).convert("RGB")
    # Captures are device pixels, so the same window records at 1x or 2x
    # depending on which display it lands on; compare at the smaller scale.
    if golden.size != current.size:
        ratio_w = current.size[0] / golden.size[0]
        ratio_h = current.size[1] / golden.size[1]
        if abs(ratio_w - ratio_h) > 0.01:
            print(f"size mismatch: {golden.size} vs {current.size}", file=sys.stderr)
            return 1
        target = min(golden.size, current.size)
        golden = golden.resize(target, Image.LANCZOS)
        current = current.resize(target, Image.LANCZOS)
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
