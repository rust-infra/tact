#!/usr/bin/env bash
# CI-style Rust checks: formatting + clippy with warnings denied (+ integration tests).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found on PATH" >&2
  exit 1
fi

echo "==> cargo fmt"
cargo fmt 
