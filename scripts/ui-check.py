#!/usr/bin/env python3
"""Assert that the rendered shell's structural anchors match the prototype.

The comparison is anchor-based rather than per-pixel text comparison: text
rendering differs between Chrome and GPUI, but the rail edge, titlebar height,
and composer box are layout decisions and must match the design.

Usage:
    python3 scripts/ui-check.py <capture-dir> [prototype.png]

Exit code 0 means every anchor is within tolerance.
"""

from __future__ import annotations

import sys
from collections import Counter
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
TOLERANCE = 3  # device pixels (2x)


def vertical_edge(image: Image.Image, y_from: int, y_to: int, x_to: int) -> int:
    counts: Counter[int] = Counter()
    for y in range(y_from, y_to, 8):
        previous = image.getpixel((0, y))
        for x in range(1, x_to):
            current = image.getpixel((x, y))
            if abs(previous[0] - current[0]) + abs(previous[1] - current[1]) + abs(
                previous[2] - current[2]
            ) > 10:
                counts[x] += 1
            previous = current
    return counts.most_common(1)[0][0]


def horizontal_edge(image: Image.Image, x: int, y_from: int, y_to: int) -> int:
    previous = image.getpixel((x, y_from))
    for y in range(y_from + 1, y_to):
        current = image.getpixel((x, y))
        if abs(previous[0] - current[0]) + abs(previous[1] - current[1]) + abs(
            previous[2] - current[2]
        ) > 10:
            return y
        previous = current
    return -1


def composer_edges(image: Image.Image, rail_edge: int) -> tuple[int, int]:
    # The composer's top border is the longest crisp horizontal run in the
    # band. Shadows are diffuse and never produce a 900px line.
    background = image.getpixel((rail_edge + 8, 1420))

    def differs(pixel) -> bool:
        return abs(pixel[0] - background[0]) + abs(pixel[1] - background[1]) + abs(
            pixel[2] - background[2]
        ) > 12

    best = (-1, -1, -1)
    for y in range(1420, 1540):
        hits = [
            x
            for x in range(rail_edge + 20, image.width - 20)
            if differs(image.getpixel((x, y)))
        ]
        if hits and len(hits) > best[0]:
            best = (len(hits), hits[0], hits[-1])
    return best[1], best[2]


def main(argv: list[str]) -> int:
    capture_dir = Path(argv[1]) if len(argv) > 1 else ROOT / "target/ui-capture"
    prototype = (
        Path(argv[2]) if len(argv) > 2 else ROOT / "target/ui-proto/prototype-light.png"
    )
    shot_path = capture_dir / "01-shell-light.png"
    if not shot_path.is_file() or not prototype.is_file():
        print(f"missing capture or prototype: {shot_path} / {prototype}", file=sys.stderr)
        return 1

    shot = Image.open(shot_path).convert("RGB")
    proto = Image.open(prototype).convert("RGB")
    if shot.size != proto.size:
        print(f"size mismatch: capture {shot.size}, prototype {proto.size}", file=sys.stderr)
        return 1

    checks: list[tuple[str, int, int]] = []
    checks.append(
        (
            "rail edge",
            vertical_edge(shot, 300, shot.height - 100, 900),
            vertical_edge(proto, 300, proto.height - 100, 900),
        )
    )
    checks.append(
        (
            "titlebar edge",
            horizontal_edge(shot, 10, 40, 400),
            horizontal_edge(proto, 10, 40, 400),
        )
    )

    rail = checks[0][1]
    # The design centers a 900px composer (1800 device px) in the content
    # column; compare the detected frame against that construction.
    expected_left = rail + (shot.width - rail - 1800) // 2
    expected_right = expected_left + 1799
    shot_left, shot_right = composer_edges(shot, rail)
    checks.append(("composer left", shot_left, expected_left))
    checks.append(("composer right", shot_right, expected_right))

    failures = 0
    for name, actual, expected in checks:
        delta = abs(actual - expected)
        status = "ok" if delta <= TOLERANCE else "FAIL"
        if status == "FAIL":
            failures += 1
        print(f"{status:4} {name}: capture={actual} prototype={expected} delta={delta}")

    if failures:
        print(f"{failures} anchor(s) outside ±{TOLERANCE}px", file=sys.stderr)
        return 1
    print("ui anchors match the prototype")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
