#!/usr/bin/env bash
# Point this repo at version-controlled git hooks under .githooks/
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

git config core.hooksPath "${ROOT}/.githooks"
chmod +x "${ROOT}/.githooks/pre-push" "${ROOT}/scripts/check-rust.sh"

echo "Installed git hooks: core.hooksPath=${ROOT}/.githooks"
echo "Pre-push runs: ./scripts/check-rust.sh"
