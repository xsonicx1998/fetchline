#!/usr/bin/env python3
"""Regenerate Sonic Manager's icon files from the project's own artwork.

Source: ``app/src-tauri/icons/logo.jpg`` — the user's logo (neon
red/blue speed mark on dark navy). The file is binary, so it is not
stored in the repository the usual way: it was supplied by the user
and uploaded to the repo through the GitHub web UI. If it is missing,
this script fails loudly instead of silently falling back to the old
artwork.

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

The mark is a wide horizontal streak: scaled whole to 16 px it turns
to mud (verified), so the small extension icons (16/32) use a crop of
the mark's head — the red/blue curl on the right of the logo — while
48/128 and the 1024 app icon use the full logo.
"""

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print("Pillow is required: pip install pillow", file=sys.stderr)
    sys.exit(1)

ROOT = Path(__file__).resolve().parent.parent
LOGO = ROOT / "app" / "src-tauri" / "icons" / "logo.jpg"

# Square crop of the mark's head (the red/blue curl, right side of
# the 2048x2048 logo) — the part that stays legible at 16 px.
HEAD_CROP = (1150, 763, 1668, 1281)


def main() -> None:
    if not LOGO.is_file():
        print(
            f"logo source missing: {LOGO} — upload the user's logo.jpg "
            "to the repo (GitHub web UI) and re-run",
            file=sys.stderr,
        )
        sys.exit(1)

    tauri_dir = ROOT / "app" / "src-tauri" / "icons"
    ext_dir = ROOT / "app" / "extension" / "icons"
    tauri_dir.mkdir(parents=True, exist_ok=True)
    ext_dir.mkdir(parents=True, exist_ok=True)

    logo = Image.open(LOGO).convert("RGB")
    if logo.size != (2048, 2048):
        print(f"warning: logo is {logo.size}, expected 2048x2048", file=sys.stderr)

    source = logo.resize((1024, 1024), Image.LANCZOS)
    source.save(tauri_dir / "icon.png")
    print(f"wrote {tauri_dir / 'icon.png'}")

    head = logo.crop(HEAD_CROP)
    for px in (16, 32, 48, 128):
        out = ext_dir / f"icon{px}.png"
        base = head if px <= 32 else logo
        base.resize((px, px), Image.LANCZOS).save(out)
        print(f"wrote {out}")


if __name__ == "__main__":
    main()
