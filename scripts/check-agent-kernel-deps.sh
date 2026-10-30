#!/usr/bin/env bash
# Enforce direct and transitive dependency direction for the agent kernel.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found on PATH" >&2
  exit 1
fi

packages=("$@")
if [ "${#packages[@]}" -eq 0 ]; then
  packages=(tact_agent_api tact_agent_core)
fi

cargo metadata --format-version 1 --all-features \
  | python3 "$ROOT/scripts/check-agent-kernel-deps.py" "${packages[@]}"
