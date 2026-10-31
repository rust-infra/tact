#!/usr/bin/env python3
"""Compose prototype-vs-implementation pairs for Tact Desktop review.

The desktop shell has two visual sources:

* ``docs/design/tact-desktop/previews`` — the approved HTML prototype, and
* ``target/ui-capture`` — the real GPUI shell rendered off-screen.

A capture that is never compared with the prototype is not verification.
This script puts both sides next to each other so the difference is visible
in one image. It only needs Pillow.

Usage:
    python3 scripts/ui-parity.py [capture-dir] [out-dir]

The default capture directory is ``target/ui-capture`` and the default output
directory is ``target/ui-parity``.
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
PROTOTYPE_DIR = ROOT / "docs/design/tact-desktop/previews"

# (name, prototype file, implementation file)
PAIRS = [
    ("shell-light", "light.png", "01-shell-light.png"),
    ("shell-dark", "dark.png", "02-shell-dark.png"),
    ("command-center", "command-palette.png", "03-command-center.png"),
    ("session-manager", "session-manager.png", "06-session-manager.png"),
    ("extensions", "extensions-popover.png", "07-extensions-mcp.png"),
    ("permission-gate", "permission-gate.png", "33-gate.png"),
    ("sticky-tasks", "sticky-tasks.png", "26b-transcript-bottom.png"),
    ("composer", "composer-attachments-queue.png", "26b-transcript-bottom.png"),
]

TARGET_HEIGHT = 900
PAD = 24
HEADER = 44
CAPTION = 24


def fit(image: Image.Image, height: int) -> Image.Image:
    width = round(image.width * height / image.height)
    return image.resize((width, height), Image.LANCZOS)


def compose(prototype: Image.Image, capture: Image.Image, name: str, out: Path) -> Path:
    left = fit(prototype, TARGET_HEIGHT)
    right = fit(capture, TARGET_HEIGHT)
    width = left.width + right.width + PAD * 3
    height = TARGET_HEIGHT + HEADER + CAPTION + PAD * 2
    canvas = Image.new("RGB", (width, height), "#f4f4f5")
    draw = ImageDraw.Draw(canvas)
    draw.text((PAD, 12), f"Tact Desktop · {name} · prototype (left) vs implementation (right)", fill="#18181b")
    y = HEADER
    canvas.paste(left, (PAD, y))
    canvas.paste(right, (PAD * 2 + left.width, y))
    draw.text((PAD, y + TARGET_HEIGHT + 4), "prototype", fill="#71717a")
    draw.text((PAD * 2 + left.width, y + TARGET_HEIGHT + 4), "implementation", fill="#71717a")
    path = out / f"{name}.png"
    canvas.save(path)
    return path


def contact_sheet(images: list[tuple[str, Path]], out: Path) -> Path:
    thumbs = []
    for name, path in images:
        image = Image.open(path)
        thumbs.append((name, fit(image, 300)))
    width = max(thumb.width for _, thumb in thumbs) + PAD * 2
    height = sum(thumb.height + CAPTION + PAD for _, thumb in thumbs) + PAD
    sheet = Image.new("RGB", (width, height), "#f4f4f5")
    draw = ImageDraw.Draw(sheet)
    y = PAD
    for name, thumb in thumbs:
        sheet.paste(thumb, (PAD, y))
        draw.text((PAD, y + thumb.height + 2), name, fill="#18181b")
        y += thumb.height + CAPTION + PAD
    path = out / "contact-sheet.png"
    sheet.save(path)
    return path


def main(argv: list[str]) -> int:
    capture_dir = Path(argv[1]) if len(argv) > 1 else ROOT / "target/ui-capture"
    out_dir = Path(argv[2]) if len(argv) > 2 else ROOT / "target/ui-parity"
    if not capture_dir.is_dir():
        print(f"capture directory does not exist: {capture_dir}", file=sys.stderr)
        print("run: cargo run -p tact-desktop --example ui_capture --features ui-capture", file=sys.stderr)
        return 1
    out_dir.mkdir(parents=True, exist_ok=True)

    made: list[tuple[str, Path]] = []
    for name, prototype_name, capture_name in PAIRS:
        prototype = PROTOTYPE_DIR / prototype_name
        capture = capture_dir / capture_name
        if not prototype.is_file():
            print(f"skip {name}: missing prototype {prototype}", file=sys.stderr)
            continue
        if not capture.is_file():
            print(f"skip {name}: missing capture {capture}", file=sys.stderr)
            continue
        made.append((name, compose(Image.open(prototype), Image.open(capture), name, out_dir)))
        print(made[-1][1])

    if made:
        print(contact_sheet(made, out_dir))
    return 0 if made else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
