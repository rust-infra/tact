#!/usr/bin/env bash
# Check the desktop shell on the real engine, headless.
#
# The shell is TSX on Native Shell; `nsh test` runs its Rust crate's tests
# (surface rendering and the catalog parity checks) and every TSX test, each
# opening the app on a headless engine and driving it as a user would.
set -euo pipefail

cd "$(dirname "$0")/../crates/desktop"

nsh test
