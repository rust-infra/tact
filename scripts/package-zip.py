#!/usr/bin/env python3
"""Create a deterministic ZIP from files or directory trees."""

from __future__ import annotations

import argparse
import os
import zipfile
from pathlib import Path


def archive_name(base: Path, path: Path) -> str:
    return path.relative_to(base).as_posix()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--base", default=".")
    parser.add_argument("paths", nargs="+")
    args = parser.parse_args()

    epoch = max(int(os.environ.get("SOURCE_DATE_EPOCH", "315532800")), 315532800)
    date_time = (
        1980,
        1,
        1,
        0,
        0,
        0,
    )
    base = Path(args.base).resolve()
    entries: list[tuple[str, Path]] = []
    for raw in args.paths:
        path = Path(raw).resolve()
        if path.is_dir():
            entries.extend(
                (archive_name(base, candidate), candidate)
                for candidate in sorted(path.rglob("*"))
            )
            entries.append((archive_name(base, path), path))
        else:
            entries.append((archive_name(base, path), path))
    entries.sort(key=lambda item: item[0])

    with zipfile.ZipFile(
        args.output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        written: set[str] = set()
        for name, path in entries:
            if name in written:
                continue
            written.add(name)
            mode = path.lstat().st_mode
            info = zipfile.ZipInfo(name.rstrip("/") + ("/" if path.is_dir() else ""))
            info.date_time = date_time
            info.compress_type = (
                zipfile.ZIP_STORED if path.is_dir() else zipfile.ZIP_DEFLATED
            )
            info.external_attr = (mode & 0xFFFF) << 16
            if path.is_dir():
                archive.writestr(info, b"")
            else:
                archive.writestr(info, path.read_bytes())
    os.utime(args.output, (epoch, epoch))


if __name__ == "__main__":
    main()
