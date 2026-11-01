#!/usr/bin/env python3
"""Set every path's mtime to SOURCE_DATE_EPOCH for deterministic archives."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def set_mtime(path: Path, epoch: int) -> None:
    try:
        os.utime(path, (epoch, epoch), follow_symlinks=False)
    except NotImplementedError:
        # Windows has no separate symlink timestamps.
        os.utime(path, (epoch, epoch))


def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit("usage: normalize-mtime.py PATH...")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
    for raw in sys.argv[1:]:
        root = Path(raw)
        paths = sorted(root.rglob("*"), reverse=True) if root.is_dir() else [root]
        for path in paths:
            set_mtime(path, epoch)
        set_mtime(root, epoch)


if __name__ == "__main__":
    main()
