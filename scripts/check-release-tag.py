#!/usr/bin/env python3
"""Reject a release tag that does not match the workspace version."""

from __future__ import annotations

import argparse
import pathlib
import tomllib


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("tag")
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parent.parent
    with (root / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["workspace"]["package"]["version"]
    expected = f"v{version}"
    if args.tag != expected:
        raise SystemExit(
            f"release tag {args.tag!r} does not match workspace version {expected!r}"
        )
    print(f"Release tag {args.tag} matches workspace version.")


if __name__ == "__main__":
    main()
