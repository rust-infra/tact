#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
exec "${ROOT}/scripts/fmt-rust.sh"
