#!/usr/bin/env python3
"""Compare the GPUI desktop capture with the HTML prototype at the same size.

Both inputs are expected to be 1280x840 @2x (2560x1680). The prototype shot is
produced by ``node scripts/proto-shot.mjs``.

Outputs, under ``target/ui-compare``:
  * ``<name>-overlay.png``  prototype | implementation | amplified diff
  * ``<name>-metrics.json`` layout anchors and per-region mean difference

The layout anchors are measured from the implementation so a drift in the rail
width, titlebar height, or content width fails the automated check.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
WIDTH, HEIGHT = 2560, 1680


def load(path: Path) -> Image.Image:
    image = Image.open(path).convert("RGB")
    if image.size != (WIDTH, HEIGHT):
        image = image.resize((WIDTH, HEIGHT), Image.LANCZOS)
    return image


def region_diff(a: Image.Image, b: Image.Image, box: tuple[int, int, int, int]) -> float:
    diff = ImageChops.difference(a.crop(box), b.crop(box))
    pixels = list(diff.getdata())
    return sum(sum(pixel) / 3 for pixel in pixels) / max(1, len(pixels))


def detect_vertical_border(image: Image.Image, y: int) -> int | None:
    """First x where the column differs from the rail background by a border."""
    row = [image.getpixel((x, y)) for x in range(0, image.width)]
    for x in range(1, len(row)):
        left, right = row[x - 1], row[x]
        if abs(left[0] - right[0]) + abs(left[1] - right[1]) + abs(left[2] - right[2]) > 18:
            return x
    return None


def main(argv: list[str]) -> int:
    capture_dir = Path(argv[1]) if len(argv) > 1 else ROOT / "target/ui-capture"
    prototype = Path(argv[2]) if len(argv) > 2 else ROOT / "target/ui-proto/prototype-light.png"
    name = Path(argv[3]).name if len(argv) > 3 else "shell-light"
    out_dir = ROOT / "target/ui-compare"
    out_dir.mkdir(parents=True, exist_ok=True)

    capture_path = capture_dir / f"{name}.png"
    if not capture_path.is_file() or not prototype.is_file():
        print(f"missing input: {capture_path} / {prototype}", file=sys.stderr)
        return 1

    proto = load(prototype)
    shot = load(capture_path)
    diff = ImageChops.difference(proto, shot)
    amplified = diff.point(lambda value: min(255, value * 4))

    gap = 24
    canvas = Image.new("RGB", (WIDTH * 3 + gap * 4, HEIGHT + 60), "#f4f4f5")
    draw = ImageDraw.Draw(canvas)
    draw.text((gap, 12), f"{name}: prototype | implementation | diff", fill="#18181b")
    canvas.paste(proto, (gap, 40))
    canvas.paste(shot, (gap * 2 + WIDTH, 40))
    canvas.paste(amplified, (gap * 3 + WIDTH * 2, 40))
    overlay_path = out_dir / f"{name}-overlay.png"
    canvas.resize((canvas.width // 2, canvas.height // 2), Image.LANCZOS).save(overlay_path)

    # Layout anchors measured from the implementation.
    rail_edge = detect_vertical_border(shot, 400)
    titlebar_edge = None
    column = [shot.getpixel((10, y)) for y in range(0, 400)]
    for y in range(60, len(column)):
        above, below = column[y - 1], column[y]
        if abs(above[0] - below[0]) + abs(above[1] - below[1]) + abs(above[2] - below[2]) > 18:
            titlebar_edge = y
            break

    regions = {
        "titlebar": (0, 0, WIDTH, 116),
        "rail": (0, 116, 500, HEIGHT),
        "transcript": (500, 116, 1880, HEIGHT - 300),
        "composer": (500, HEIGHT - 300, 1880, HEIGHT),
        "inspector": (1880, 116, WIDTH, HEIGHT),
    }
    metrics = {
        "rail_edge_px": rail_edge,
        "rail_edge_logical": None if rail_edge is None else rail_edge / 2,
        "titlebar_edge_px": titlebar_edge,
        "titlebar_edge_logical": None if titlebar_edge is None else titlebar_edge / 2,
        "region_mean_diff": {
            region: round(region_diff(proto, shot, box), 2)
            for region, box in regions.items()
        },
    }
    metrics_path = out_dir / f"{name}-metrics.json"
    metrics_path.write_text(json.dumps(metrics, indent=2))
    print(json.dumps(metrics, indent=2))
    print(overlay_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
