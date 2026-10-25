#!/usr/bin/env bash
# Guard the intra-`tact` module edges that the crate graph cannot see.
#
# Phase 0 still has Agent code calling the Skills/Memory/MCP implementations
# directly. That debt is listed explicitly below; any *new* file or module
# edge fails immediately, and the allowlist shrinks as Phase 2 moves the calls
# behind plugin services.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 - <<'PY'
from pathlib import Path
import sys

# Phase-0 debt: repo-relative file path -> forbidden modules imported there.
PHASE0_ALLOWLIST = {
    "crates/tact/src/agent/mod.rs": {"skill", "memory", "mcp"},
    "crates/tact/src/agent/tool_dispatch.rs": {"mcp"},
}

MODULES = ("skill", "memory", "mcp")
root = Path("crates/tact/src/agent")
if not root.is_dir():
    print(f"missing agent source directory: {root}", file=sys.stderr)
    raise SystemExit(1)

failures = []
for path in sorted(root.rglob("*.rs")):
    text = path.read_text()
    allowed = PHASE0_ALLOWLIST.get(str(path), set())
    for module in MODULES:
        if f"crate::{module}" in text and module not in allowed:
            failures.append(f"{path}: forbidden crate::{module} import")

if failures:
    for failure in failures:
        print(failure, file=sys.stderr)
    raise SystemExit(1)

print(f"module boundaries ok: {len(PHASE0_ALLOWLIST)} Phase-0 allowlist entries")
PY
