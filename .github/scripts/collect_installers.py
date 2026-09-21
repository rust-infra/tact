#!/usr/bin/env python3
"""Describe one platform's installers for the update manifest.

`cargo-packager` names its artifacts after the product, the version, and the
architecture, and the arch spelling differs per format (`amd64` for a .deb,
`x86_64` for an AppImage). Rather than re-deriving that mapping in the release
job, each build records what it produced: the updater only accepts one format
per platform, and this file says which one that is.

Usage:
  collect_installers.py --dir installers --target x86_64-unknown-linux-gnu \
      --version 1.1.31 --out installers/platform-x86_64-unknown-linux-gnu.json
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

# cargo-packager's updater accepts exactly these formats, and one per platform:
# an AppImage on Linux, an .app bundle on macOS, and an NSIS .exe or WiX .msi on
# Windows. The other artifacts (.deb, .dmg) are what a user installs by hand.
UPDATABLE = {
    "linux": {".AppImage": "appimage"},
    "darwin": {"app.tar.gz": "app"},
    "windows": {".exe": "nsis", ".msi": "wix"},
}

# `target triple` -> the `OS-ARCH` key the updater looks up in the manifest.
PLATFORMS = {
    "x86_64-unknown-linux-gnu": "linux-x86_64",
    "aarch64-unknown-linux-gnu": "linux-aarch64",
    "x86_64-apple-darwin": "darwin-x86_64",
    "aarch64-apple-darwin": "darwin-aarch64",
    "x86_64-pc-windows-msvc": "windows-x86_64",
}


def find(directory: pathlib.Path, suffix: str) -> pathlib.Path | None:
    matches = sorted(p for p in directory.iterdir() if p.name.endswith(suffix))
    return matches[0] if matches else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    platform = PLATFORMS.get(args.target)
    if platform is None:
        print(f"unknown target {args.target}; no manifest entry", file=sys.stderr)
        return 0

    directory = pathlib.Path(args.dir)
    os_name = platform.split("-")[0]
    updatable = UPDATABLE[os_name]

    entry = None
    for suffix, fmt in updatable.items():
        artifact = find(directory, suffix)
        if artifact is None:
            continue
        signature = artifact.with_name(artifact.name + ".sig")
        if not signature.exists():
            print(
                f"{artifact.name} has no signature; not offering it as an update",
                file=sys.stderr,
            )
            continue
        entry = {
            "platform": platform,
            "version": args.version,
            "format": fmt,
            "artifact": artifact.name,
            "signature": signature.read_text().strip(),
        }
        break

    if entry is None:
        print(
            f"no signed updatable artifact for {args.target}; "
            "the manual installers are still published",
            file=sys.stderr,
        )
        return 0

    pathlib.Path(args.out).write_text(json.dumps(entry, indent=2) + "\n")
    print(f"wrote {args.out}: {entry['format']} {entry['artifact']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
