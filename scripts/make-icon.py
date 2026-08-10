#!/usr/bin/env python3
"""Generates every installed and web icon from the Sourcefour Hinge mark.

The same geometry becomes .icns (macOS), .ico (Windows and favicon), and PNG
(AppImage, previews, and Apple touch icon), so no platform carries an old mark.

Requires Pillow and the macOS `iconutil`. Rerun after design changes; the
results are committed so packaging needs no Python.
"""

import pathlib
import subprocess
import tempfile

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "apps" / "sourcefour" / "assets" / "icon"
WIX_ICON = ROOT / "apps" / "sourcefour" / "wix" / "Sourcefour.ico"
WEBSITE_DIR = ROOT / "website"

BG = (18, 21, 27, 255)        # near the app's chrome background
BLUE = (91, 157, 255, 255)
GREEN = (126, 201, 111, 255)
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
    """Two equal lozenges meeting at one changing edge: the Hinge mark."""
    draw.polygon(((216, 318), (458, 178), (599, 420), (357, 560)), fill=GREEN)
    draw.polygon(((426, 582), (668, 442), (808, 684), (566, 824)), fill=BLUE)


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
    image.save(WIX_ICON, sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)])
    image.save(WEBSITE_DIR / "favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)])
    image.resize((180, 180), Image.LANCZOS).save(WEBSITE_DIR / "apple-touch-icon.png")
    for path in (
        OUT_DIR / "Sourcefour.icns",
        OUT_DIR / "Sourcefour.ico",
        OUT_DIR / "Sourcefour.png",
        OUT_DIR / "preview.png",
        WIX_ICON,
        WEBSITE_DIR / "favicon.ico",
        WEBSITE_DIR / "apple-touch-icon.png",
    ):
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
