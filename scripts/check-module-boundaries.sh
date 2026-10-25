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
    # `skill` and `memory` now go through `ToolContext` helpers; `mcp` here is
    # the router reload path, which Phase 2's MCP plugin takes over.
    "crates/tact/src/agent/mod.rs": {"mcp"},
    # The MCP families still run through the router in this file until they are
    # registered as sources on the executor.
    "crates/tact/src/agent/tool_dispatch.rs": {"mcp"},
}

MODULES = ("skill", "memory", "mcp")
root = Path("crates/tact/src/agent")
if not root.is_dir():
    print(f"missing agent source directory: {root}", file=sys.stderr)
    raise SystemExit(1)

def production_text(text: str) -> str:
    """Drop everything from the first `#[cfg(test)]` item to the end of file.

    The edge being guarded is a production dependency: a test may name another
    module's constant without the runtime depending on it, and treating the two
    the same would force tests to route through a helper for no benefit.
    """
    idx = text.find("#[cfg(test)]")
    text = text if idx == -1 else text[:idx]
    # A mention inside a comment or doc comment is prose, not an edge.
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("//")
    )


failures = []
for path in sorted(root.rglob("*.rs")):
    text = production_text(path.read_text())
    allowed = PHASE0_ALLOWLIST.get(str(path), set())
    for module in MODULES:
        if f"crate::{module}" in text and module not in allowed:
            failures.append(f"{path}: forbidden crate::{module} import")

if failures:
    for failure in failures:
        print(failure, file=sys.stderr)
    raise SystemExit(1)

# Self-check: the guard must still reject a new edge, and must not be satisfiable
# by accident. A guard that silently passes is worse than no guard, so the two
# cases that matter are asserted here rather than trusted.
probe = "fn _probe() { let _ = crate::skill::X; }\n"
if production_text(probe).find("crate::skill") == -1:
    print("self-check failed: production_text dropped a live line", file=sys.stderr)
    raise SystemExit(1)
if production_text("// crate::skill::X\n") != "":
    print("self-check failed: production_text kept a comment", file=sys.stderr)
    raise SystemExit(1)
if production_text("fn a() {}\n#[cfg(test)]\nfn b() { crate::skill::X; }\n").find("crate::skill") != -1:
    print("self-check failed: production_text kept a test body", file=sys.stderr)
    raise SystemExit(1)

print(f"module boundaries ok: {len(PHASE0_ALLOWLIST)} Phase-0 allowlist entries")
PY
