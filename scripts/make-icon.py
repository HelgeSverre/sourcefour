#!/usr/bin/env python3
"""Generates the app icon in every form packaging needs: a flat dark tile with
the graph glyph, written as .icns (macOS), .ico (Windows installer), and .png
(AppImage, and the website preview).

Requires Pillow and the macOS `iconutil`. Rerun after design changes; the
results are committed so packaging needs no Python.
"""

import pathlib
import subprocess
import tempfile

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "apps" / "sourcefour" / "assets" / "icon"

BG = (18, 21, 27, 255)        # near the app's chrome background
BLUE = (91, 157, 255, 255)    # accent lane
GREEN = (126, 201, 111, 255)  # second lane
SIZE = 1024
MARGIN = 100                  # macOS icon grid margin
RADIUS = 232                  # macOS squircle-ish corner radius


def rounded_tile(draw: ImageDraw.ImageDraw) -> None:
    draw.rounded_rectangle(
        (MARGIN, MARGIN, SIZE - MARGIN, SIZE - MARGIN),
        radius=RADIUS,
        fill=BG,
    )


def glyph(draw: ImageDraw.ImageDraw) -> None:
    """The commit graph: a main lane, a branch splitting off and merging back."""
    stroke = 40
    node = 58
    main_x = 420
    branch_x = 646
    top = 292
    bottom = 732
    out_y = 386
    in_y = 638
    elbow_up = 462
    elbow_down = 562

    def capsule(a, b):
        draw.line((a[0], a[1], b[0], b[1]), fill=GREEN, width=stroke)
        for x, y in (a, b):
            r = stroke // 2
            draw.ellipse((x - r, y - r, x + r, y + r), fill=GREEN)

    # main lane
    draw.line((main_x, top, main_x, bottom), fill=BLUE, width=stroke)
    # branch: out, along, and back in
    capsule((main_x, out_y), (branch_x, elbow_up))
    capsule((branch_x, elbow_up), (branch_x, elbow_down))
    capsule((branch_x, elbow_down), (main_x, in_y))

    for x, y, color in (
        (main_x, top, BLUE),
        (main_x, bottom, BLUE),
        (branch_x, (elbow_up + elbow_down) // 2, GREEN),
    ):
        draw.ellipse((x - node, y - node, x + node, y + node), fill=BG)
        draw.ellipse(
            (x - node + 16, y - node + 16, x + node - 16, y + node - 16),
            fill=color,
        )


def main() -> None:
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    rounded_tile(draw)
    glyph(draw)

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as scratch:
        iconset = pathlib.Path(scratch) / "Sourcefour.iconset"
        iconset.mkdir()
        for points in (16, 32, 64, 128, 256, 512):
            for scale in (1, 2):
                pixels = points * scale
                name = f"icon_{points}x{points}" + ("@2x" if scale == 2 else "") + ".png"
                image.resize((pixels, pixels), Image.LANCZOS).save(iconset / name)
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(OUT_DIR / "Sourcefour.icns")],
            check=True,
        )
    image.save(
        OUT_DIR / "Sourcefour.ico",
        sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)],
    )
    image.resize((512, 512), Image.LANCZOS).save(OUT_DIR / "Sourcefour.png")
    image.resize((256, 256), Image.LANCZOS).save(OUT_DIR / "preview.png")
    for name in ("Sourcefour.icns", "Sourcefour.ico", "Sourcefour.png", "preview.png"):
        print(f"wrote {OUT_DIR / name}")


if __name__ == "__main__":
    main()
