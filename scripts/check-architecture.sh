#!/usr/bin/env bash
# Enforce the plugin-first crate boundary.
#
# `tact_kernel` is the runtime kernel: it may depend on the neutral protocol
# crate and a small set of infrastructure crates, never on an extensions, view,
# database, MCP or TUI crate. This guard runs from `scripts/check-rust.sh` and
# fails before a forbidden edge can be merged.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 - <<'PY'
from __future__ import annotations

import json
import subprocess
import sys

KERNEL = "tact_kernel"
ALLOWED = {
    "tact_protocol",
    "anyhow",
    "async-trait",
    "serde",
    "serde_json",
    "strum",
    "tokio",
}
FORBIDDEN = {
    "tact",
    "tui",
    "tact-ui",
    "tact_llm",
    "sqlx",
    "rmcp",
    "ratatui",
}

metadata = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)
packages = {package["name"]: package for package in metadata["packages"]}
kernel = packages.get(KERNEL)
if kernel is None:
    print(f"missing workspace package: {KERNEL}", file=sys.stderr)
    raise SystemExit(1)

failures: list[str] = []
for dependency in kernel["dependencies"]:
    name = dependency["name"]
    if name in FORBIDDEN:
        failures.append(f"forbidden dependency: {name}")
    elif name not in ALLOWED:
        failures.append(f"unexpected dependency: {name}")

if failures:
    for failure in failures:
        print(failure, file=sys.stderr)
    raise SystemExit(1)

print(f"architecture ok: {KERNEL} deps = {', '.join(sorted(ALLOWED))}")
PY
