#!/usr/bin/env python3
"""Regenerate Sonic Manager's icon files from the project's own artwork.

Source (Round 45): ``app/src-tauri/icons/logo.svg`` — the user's
asset-pack logo: a purple-gradient rounded square with a black
lightning bolt (cyan outline) plus "Sonic Manager" text. The SVG is
text, so it lives in the repository like any other file. The icon
uses only the square mark (the left 96x96 of the 420x96 viewBox);
the text is left out — it would be unreadable at icon sizes.

The mark is rendered with Pillow only (no SVG rasterizer, no numpy):
the diagonal gradient is drawn as anti-diagonal lines of constant
x+y, the rounded square is a mask, and the bolt is the SVG path's
polygon (filled dark, cyan outline drawn as thick joined lines).
THIS mark only — if the logo SVG ever changes shape, this function
must change with it. Verified by eye against the pack's reference
image when it was written.

Fallback: if ``logo.svg`` is missing, the old ``logo.jpg`` (the
2026-11-15 neon red/blue speed mark) is used as before. If neither
exists, this script fails loudly instead of silently producing a
wrong icon.

Why this script exists: the PNG/ICO icon files are binary, so they are not
stored in the repository. Run this once after cloning (or let CI run it)
to generate them before building:

    pip install pillow
    python3 scripts/generate-icons.py

Outputs:
  - app/src-tauri/icons/icon.png            (1024x1024 source for `tauri icon`)
  - app/extension/icons/icon{16,32,48,128}.png
Run `npx @tauri-apps/cli icon app/src-tauri/icons/icon.png` afterwards
(or let the CI workflow do it) to produce the Tauri set
(32x32.png, 128x128.png, 128x128@2x.png, icon.ico) in app/src-tauri/icons.

The mark is a bold square, so it stays legible at 16 px — all four
extension icons use the whole mark (the old logo was a wide
horizontal streak and needed a head crop; that crop is gone).
"""

import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw
except ImportError:
    print("Pillow is required: pip install pillow", file=sys.stderr)
    sys.exit(1)

ROOT = Path(__file__).resolve().parent.parent
LOGO_SVG = ROOT / "app" / "src-tauri" / "icons" / "logo.svg"
LOGO_JPG = ROOT / "app" / "src-tauri" / "icons" / "logo.jpg"

# The mark, in the logo SVG's viewBox coordinates (viewBox 420x96;
# the mark is the 96x96 square at the left).
MARK_VIEWBOX = 96.0
MARK_RECT = (4.0, 4.0, 88.0, 88.0)  # x, y, w, h
MARK_RADIUS = 20.0
GRADIENT_FROM = (0x8B, 0x20, 0xFF)  # #8B20FF, top-left
GRADIENT_TO = (0x34, 0x24, 0xC8)  # #3424C8, bottom-right
BOLT_POINTS = [(54, 13), (27, 53), (46, 53), (39, 83), (69, 40), (49, 40)]
BOLT_FILL = (0x07, 0x10, 0x27)  # #071027
BOLT_STROKE = (0x00, 0xC8, 0xFF)  # #00C8FF
BOLT_STROKE_WIDTH = 2.0  # in viewBox units

SIZE = 1024


def _lerp(a: int, b: int, t: float) -> int:
    return round(a + (b - a) * t)


def render_mark(size: int = SIZE) -> Image.Image:
    """Render the pack's lightning mark at `size`x`size` (RGBA)."""
    # Diagonal gradient: the SVG's x1=0,y1=0 -> x2=1,y2=1 linear
    # gradient runs top-left (#8B20FF) to bottom-right (#3424C8).
    # Draw it as anti-diagonal lines of constant x+y: the colour
    # at a point is the lerp by (x+y) / (2*size). (A rotate of a
    # horizontal gradient does NOT work here — the rotated image's
    # corners fill black and show through as dark triangles.)
    grad = Image.new("RGB", (size, size))
    draw = ImageDraw.Draw(grad)
    for c in range(2 * size):
        t = c / (2 * size - 1)
        col = tuple(_lerp(GRADIENT_FROM[i], GRADIENT_TO[i], t) for i in range(3))
        x0 = max(0, c - (size - 1))
        y0 = c - x0
        x1 = min(size - 1, c)
        y1 = c - x1
        draw.line([(x0, y0), (x1, y1)], fill=col)

    # Rounded-square mask for the mark's rect.
    scale = size / MARK_VIEWBOX
    x0, y0, rw, rh = MARK_RECT
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [x0 * scale, y0 * scale, (x0 + rw) * scale, (y0 + rh) * scale],
        radius=MARK_RADIUS * scale,
        fill=255,
    )
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(grad.convert("RGBA"), (0, 0), mask)

    # Lightning bolt: thick cyan outline first, dark fill on top —
    # the fill covers the inner half of the stroke, leaving the
    # ~1-unit outline the SVG draws.
    bolt = [ (x * scale, y * scale) for x, y in BOLT_POINTS ]
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    closed = bolt + [bolt[0]]
    draw.line(closed, fill=BOLT_STROKE + (255,), width=max(1, round(BOLT_STROKE_WIDTH * scale)), joint="curve")
    draw.polygon(bolt, fill=BOLT_FILL + (255,))
    out.alpha_composite(layer)
    return out


def main() -> None:
    tauri_dir = ROOT / "app" / "src-tauri" / "icons"
    ext_dir = ROOT / "app" / "extension" / "icons"
    tauri_dir.mkdir(parents=True, exist_ok=True)
    ext_dir.mkdir(parents=True, exist_ok=True)

    if LOGO_SVG.is_file():
        source = render_mark(SIZE)
        print(f"rendered mark from {LOGO_SVG}")
    elif LOGO_JPG.is_file():
        print(f"warning: {LOGO_SVG} missing, falling back to the old {LOGO_JPG}", file=sys.stderr)
        logo = Image.open(LOGO_JPG).convert("RGB")
        if logo.size != (2048, 2048):
            print(f"warning: logo is {logo.size}, expected 2048x2048", file=sys.stderr)
        source = logo.resize((SIZE, SIZE), Image.LANCZOS).convert("RGBA")
    else:
        print(
            f"logo source missing: neither {LOGO_SVG} nor {LOGO_JPG} exists",
            file=sys.stderr,
        )
        sys.exit(1)

    source.save(tauri_dir / "icon.png")
    print(f"wrote {tauri_dir / 'icon.png'}")

    for px in (16, 32, 48, 128):
        out = ext_dir / f"icon{px}.png"
        source.resize((px, px), Image.LANCZOS).save(out)
        print(f"wrote {out}")


if __name__ == "__main__":
    main()
