#!/usr/bin/env python3
"""Render the Tact app icon into every platform asset.

`crates/desktop/assets/app-icon.svg` is the only editable source. This script
turns it into the three platform formats, so the bundle icon, the executable
resource, and the Linux theme icon can never drift apart:

    macOS     assets/app-icon.icns                  Contents/Resources/Tact.icns
    Windows   assets/app-icon.ico                   embedded by build.rs (RC icon)
    Linux     assets/linux/hicolor/<n>x<n>/apps/…   hicolor theme + desktop entry

    python3 scripts/app-icon.py           regenerate every asset
    python3 scripts/app-icon.py --check   fail if a committed asset is stale

Rendering needs `rsvg-convert` (librsvg 2.5x).
"""

from __future__ import annotations

import argparse
import os
import shutil
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "crates" / "desktop" / "assets"
SVG = ASSETS / "app-icon.svg"
LINUX = ASSETS / "linux"
DESKTOP_ENTRY = LINUX / "tact-desktop.desktop"

# Windows reads 16/32/48/256 most often; the rest cover high-DPI explorer views.
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)

# Redistributable Linux icon sizes. `scalable/` ships the SVG itself.
LINUX_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)

# ICNS element types -> raster size. Apple names each slot by its meaning
# (`icp4` is 16pt, `ic11` is 16pt@2x = 32px), so several types share a size.
ICNS_SLOTS = (
    ("icp4", 16),
    ("icp5", 32),
    ("icp6", 64),
    ("ic07", 128),
    ("ic08", 256),
    ("ic09", 512),
    ("ic10", 1024),
    ("ic11", 32),
    ("ic12", 64),
    ("ic13", 256),
    ("ic14", 512),
)

DESKTOP_ENTRY_TEXT = """\
[Desktop Entry]
Type=Application
Name=Tact Desktop
GenericName=AI Coding Agent
Comment=Native desktop shell for the Tact agent
Exec=tact-desktop
Icon=tact-desktop
Terminal=false
Categories=Development;IDE;
Keywords=tact;agent;ai;coding;
StartupNotify=true
# X11 and Wayland match the running window to this entry through the class /
# app_id the shell sets in its window options.
StartupWMClass=tact-desktop
"""


def rsvg_binary() -> str:
    binary = os.environ.get("RSVG_CONVERT") or shutil.which("rsvg-convert")
    if not binary:
        sys.exit(
            "app-icon.py needs rsvg-convert (librsvg); set RSVG_CONVERT to its path"
        )
    return binary


def render(binary: str, size: int) -> bytes:
    """Rasterise the icon at `size` square pixels."""
    result = subprocess.run(
        [binary, "-w", str(size), "-h", str(size), str(SVG)],
        check=True,
        capture_output=True,
    )
    return result.stdout


def png_size(data: bytes) -> tuple[int, int]:
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit("rsvg-convert did not return a PNG")
    width, height = struct.unpack(">II", data[16:24])
    if width != height:
        raise SystemExit(f"icon raster is not square: {width}x{height}")
    return width, height


def encode_ico(rasters: dict[int, bytes]) -> bytes:
    """Encode a PNG-compressed ICO (every size modern Windows reads)."""
    sizes = sorted(rasters)
    header = struct.pack("<HHH", 0, 1, len(sizes))
    entries = bytearray()
    body = bytearray()
    offset = len(header) + 16 * len(sizes)
    for size in sizes:
        data = rasters[size]
        entries += struct.pack(
            "<BBBBHHII",
            size if size < 256 else 0,
            size if size < 256 else 0,
            0,  # palette entries
            0,  # reserved
            1,  # colour planes
            32,  # bits per pixel
            len(data),
            offset,
        )
        body += data
        offset += len(data)
    return header + bytes(entries) + bytes(body)


def encode_icns(rasters: dict[int, bytes]) -> bytes:
    """Encode an ICNS container from PNG rasters."""
    body = bytearray()
    for kind, size in ICNS_SLOTS:
        data = rasters[size]
        body += kind.encode("ascii") + struct.pack(">I", len(data) + 8) + data
    return b"icns" + struct.pack(">I", len(body) + 8) + bytes(body)


def generated_files() -> dict[Path, bytes]:
    """Every asset this script owns, as path -> exact bytes."""
    binary = rsvg_binary()
    files: dict[Path, bytes] = {}

    master = render(binary, 1024)
    png_size(master)
    files[ASSETS / "app-icon.png"] = master

    cache: dict[int, bytes] = {}

    def raster(size: int) -> bytes:
        if size not in cache:
            cache[size] = master if size == 1024 else render(binary, size)
            png_size(cache[size])
        return cache[size]

    files[ASSETS / "app-icon.ico"] = encode_ico(
        {size: raster(size) for size in ICO_SIZES}
    )
    files[ASSETS / "app-icon.icns"] = encode_icns(
        {size: raster(size) for _, size in ICNS_SLOTS}
    )

    for size in LINUX_SIZES:
        files[LINUX / "hicolor" / f"{size}x{size}" / "apps" / "tact-desktop.png"] = raster(
            size
        )
    files[LINUX / "hicolor" / "scalable" / "apps" / "tact-desktop.svg"] = (
        SVG.read_bytes()
    )
    files[DESKTOP_ENTRY] = DESKTOP_ENTRY_TEXT.encode("utf-8")
    return files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the committed assets match the SVG instead of writing them",
    )
    args = parser.parse_args()

    if not SVG.is_file():
        sys.exit(f"missing icon source: {SVG}")
    files = generated_files()

    if args.check:
        stale = [
            path
            for path, data in files.items()
            if not path.is_file() or path.read_bytes() != data
        ]
        for path in stale:
            print(f"stale: {path.relative_to(ROOT)}", file=sys.stderr)
        if stale:
            print(
                "run `python3 scripts/app-icon.py` and commit the result",
                file=sys.stderr,
            )
            return 1
        print(f"app icon assets are up to date ({len(files)} files)")
        return 0

    for path, data in files.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        print(path.relative_to(ROOT))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
