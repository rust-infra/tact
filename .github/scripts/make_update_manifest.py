#!/usr/bin/env python3
"""Assemble the `latest.json` the in-app updater reads.

Each build job writes one `platform-*.json` describing the single updatable
artifact it produced (see `collect_installers.py`). This script folds those into
the manifest format `cargo-packager-updater` expects:

    {"version": "...", "notes": "...", "pub_date": "...",
     "platforms": {"linux-x86_64": {"signature": ..., "url": ..., "format": ...}}}

If no build described a signed artifact, the script exits non-zero so the
workflow can publish the release without a manifest rather than publishing one
that announces an update the app would then refuse to install.

Usage:
  make_update_manifest.py --dist dist --version 1.1.31 \
      --out dist/latest.json --repo rust-infra/tact
"""

from __future__ import annotations

import argparse
import datetime
import json
import pathlib
import sys


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dist", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--notes", default="")
    args = parser.parse_args()

    dist = pathlib.Path(args.dist)
    platforms: dict[str, dict[str, str]] = {}

    for path in sorted(dist.glob("platform-*.json")):
        entry = json.loads(path.read_text())
        platform = entry["platform"]
        asset = pathlib.Path(entry["artifact"]).name
        platforms[platform] = {
            "signature": entry["signature"],
            "url": (
                f"https://github.com/{args.repo}/releases/download/"
                f"v{args.version}/{asset}"
            ),
            "format": entry["format"],
        }

    if not platforms:
        print("no signed platform artifacts; refusing to write a manifest", file=sys.stderr)
        return 1

    manifest = {
        "version": args.version,
        "notes": args.notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "platforms": platforms,
    }
    pathlib.Path(args.out).write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {args.out} with {len(platforms)} platform(s): {', '.join(platforms)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
