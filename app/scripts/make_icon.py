#!/usr/bin/env python3
"""Render app/Resources/AppIcon.icns: a gradient "r" on a black rounded rect.

Local and free: Pillow draws it, `iconutil` packs the .icns. Re-run only when
the design changes, and commit the .icns.

    python3 -m venv app/.venv && app/.venv/bin/pip install pillow
    app/.venv/bin/python app/scripts/make_icon.py

The gradient uses the ASCII logo's stops (Theme.logoStopHex): violet → cyan →
green on a 45° diagonal, with a soft phosphor glow behind the glyph.
"""

import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

APP_DIR = Path(__file__).resolve().parent.parent
OUT = APP_DIR / "Resources" / "AppIcon.icns"

CANVAS = 1024
# macOS Big Sur+ icon grid: an 824 pt tile centred on the 1024 canvas.
TILE = 824
RADIUS = 185
STOPS = [(0x7C, 0x3A, 0xED), (0x22, 0xD3, 0xEE), (0x39, 0xFF, 0x14)]
BORDER = (0x3E, 0x6B, 0x4A)  # Theme.dim
FONTS = [
    ("/System/Library/Fonts/Menlo.ttc", 1),  # Menlo Bold
    ("/System/Library/Fonts/SFNSMono.ttf", 0),
]


def blend(t: float) -> tuple[int, int, int]:
    t = min(max(t, 0.0), 1.0) * (len(STOPS) - 1)
    i = min(int(t), len(STOPS) - 2)
    f = t - i
    a, b = STOPS[i], STOPS[i + 1]
    return tuple(round(a[k] * (1 - f) + b[k] * f) for k in range(3))


def gradient(size: int, bbox: tuple[int, int, int, int]) -> Image.Image:
    """A 45° gradient spanning `bbox`: violet at its top-left corner, green at
    its bottom-right, clamped outside. Spanning the glyph, not the canvas, puts
    all three stops on the letter."""
    left, top, right, bottom = bbox
    span = max(1, (right - left) + (bottom - top))
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            px[x, y] = blend(((x - left) + (y - top)) / span)
    return img


def font(size: int) -> ImageFont.FreeTypeFont:
    for path, index in FONTS:
        if Path(path).exists():
            return ImageFont.truetype(path, size, index=index)
    sys.exit("no monospaced system font found")


def glyph_mask() -> Image.Image:
    """The "r", centred optically on the tile."""
    mask = Image.new("L", (CANVAS, CANVAS), 0)
    draw = ImageDraw.Draw(mask)
    f = font(720)
    left, top, right, bottom = draw.textbbox((0, 0), "r", font=f)
    w, h = right - left, bottom - top
    x = (CANVAS - w) / 2 - left
    y = (CANVAS - h) / 2 - top + 10
    draw.text((x, y), "r", font=f, fill=255)
    return mask


def render() -> Image.Image:
    icon = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    off = (CANVAS - TILE) // 2
    box = (off, off, off + TILE - 1, off + TILE - 1)

    tile = Image.new("L", (CANVAS, CANVAS), 0)
    ImageDraw.Draw(tile).rounded_rectangle(box, RADIUS, fill=255)
    icon.paste((0, 0, 0, 255), (0, 0), tile)

    mask = glyph_mask()
    colours = gradient(CANVAS, mask.getbbox())
    # Phosphor glow: the blurred glyph at low alpha under the sharp one.
    glow = mask.filter(ImageFilter.GaussianBlur(28)).point(lambda v: int(v * 0.55))
    glow = ImageChops.multiply(glow, tile)
    icon.paste(colours, (0, 0), glow)
    icon.paste(colours, (0, 0), mask)

    ImageDraw.Draw(icon).rounded_rectangle(box, RADIUS, outline=BORDER + (255,), width=6)
    return icon


def main() -> None:
    master = render()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "AppIcon.iconset"
        iconset.mkdir()
        for pt in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                px = pt * scale
                name = f"icon_{pt}x{pt}{'@2x' if scale == 2 else ''}.png"
                master.resize((px, px), Image.LANCZOS).save(iconset / name)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(OUT)], check=True)
    print(f"{OUT} ({OUT.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
