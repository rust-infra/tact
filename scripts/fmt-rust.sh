#!/usr/bin/env bash
# Apply rustfmt using the workspace rustfmt.toml (stable-compatible).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found on PATH" >&2
  exit 1
fi

echo "==> cargo fmt"
cargo fmt
