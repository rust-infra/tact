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
    # Empty: MCP *dispatch* is a catalog source (McpToolSource / McpControlSource),
    # name classification is neutral (`tool::name_class`), and the router's
    # lifecycle moved to `mcp::lifecycle`. The guard keeps the mechanism so a new
    # edge has to be declared rather than merely appear.
}

# Edges that are not debt and are not expected to go away, kept in their own map
# so "how much Phase-0 debt is left" stays answerable by reading one of them.
DECLARED_EDGES = {
    # `Agent::new` takes an `MCPToolRouter`, so the port that assembles a child
    # cannot avoid naming the type. The *policy* stays with the caller: the spec
    # carries the router, and `spawn_subagent` decides that a child gets an empty
    # one. Declared rather than laundered through a re-export, because the edge is
    # real and a renamed path would only hide it.
    "crates/tact/src/agent/factory.rs": {"mcp"},
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


# Facades a module may expose: `mcp::lifecycle` is the one operation the agent
# loop is allowed to ask for (refresh the external tool set), and naming it is
# not the edge this guard is about. Anything else under `mcp` still is.
FACADES = {
    "mcp": ("crate::mcp::lifecycle",),
}

failures = []
for path in sorted(root.rglob("*.rs")):
    text = production_text(path.read_text())
    allowed = PHASE0_ALLOWLIST.get(str(path), set()) | DECLARED_EDGES.get(str(path), set())
    for module in MODULES:
        if f"crate::{module}" not in text or module in allowed:
            continue
        # Strip the sanctioned facade and re-check: a file that only mentions
        # `crate::mcp::lifecycle` has no edge to `mcp` at large.
        remaining = text
        for facade in FACADES.get(module, ()):
            remaining = remaining.replace(facade, "")
        if f"crate::{module}" in remaining:
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

# The facade rule must be narrow: the sanctioned path is stripped, anything else
# under that module is still an edge. Without this, emptying the allowlist could
# have been achieved by a rule that strips too much.
def edge_after_facades(module: str, text: str) -> bool:
    for facade in FACADES.get(module, ()):
        text = text.replace(facade, "")
    return f"crate::{module}" in text

if edge_after_facades("mcp", "let _ = crate::mcp::lifecycle::reload();\n"):
    print("self-check failed: the facade was not stripped", file=sys.stderr)
    raise SystemExit(1)
if not edge_after_facades("mcp", "let _ = crate::mcp::MCPToolRouter::new();\n"):
    print("self-check failed: a direct mcp edge was stripped", file=sys.stderr)
    raise SystemExit(1)
if not edge_after_facades(
    "mcp", "let _ = crate::mcp::lifecycle::reload(); let _ = crate::mcp::McpClient;\n"
):
    print("self-check failed: a facade hid a second edge", file=sys.stderr)
    raise SystemExit(1)

print(f"module boundaries ok: {len(PHASE0_ALLOWLIST)} Phase-0 allowlist entries")
PY
