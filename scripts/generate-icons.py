#!/usr/bin/env python3
"""Regenerate Fetchline's icon files from the project's own artwork.

The icon is drawn fresh for this project (no third-party artwork): a violet
rounded square (design token light accent #6d28d9) with a white download
arrow above a tray line — the same design as app/extension/icons/icon.svg.

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
"""

import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw
except ImportError:
    print("Pillow is required: pip install pillow", file=sys.stderr)
    sys.exit(1)

ROOT = Path(__file__).resolve().parent.parent
VIOLET = (109, 40, 217, 255)  # #6d28d9
WHITE = (255, 255, 255, 255)


def draw_icon(size: int) -> Image.Image:
    """Draw the Fetchline icon at the given square size."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    s = size / 128.0  # the SVG design is on a 128x128 grid
    # Violet rounded square.
    d.rounded_rectangle(
        [2 * s, 2 * s, 126 * s, 126 * s], radius=24 * s, fill=VIOLET
    )
    # Download arrow: stem + head.
    d.rectangle([58 * s, 24 * s, 70 * s, 72 * s], fill=WHITE)
    d.polygon([(36 * s, 62 * s), (92 * s, 62 * s), (64 * s, 92 * s)], fill=WHITE)
    # Tray line.
    d.rectangle([26 * s, 100 * s, 102 * s, 109 * s], fill=WHITE)
    return img


def main() -> None:
    tauri_dir = ROOT / "app" / "src-tauri" / "icons"
    ext_dir = ROOT / "app" / "extension" / "icons"
    tauri_dir.mkdir(parents=True, exist_ok=True)
    ext_dir.mkdir(parents=True, exist_ok=True)

    source = draw_icon(1024)
    source.save(tauri_dir / "icon.png")
    print(f"wrote {tauri_dir / 'icon.png'}")

    for px in (16, 32, 48, 128):
        out = ext_dir / f"icon{px}.png"
        draw_icon(px).save(out)
        print(f"wrote {out}")


if __name__ == "__main__":
    main()
