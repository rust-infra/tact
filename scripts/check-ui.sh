#!/usr/bin/env bash
# Render the desktop shell and the HTML prototype at the same size, then check
# the structural anchors (rail, titlebar, composer) against the design.
set -euo pipefail

cd "$(dirname "$0")/.."
PYTHON="${PYTHON:-python3}"

if ! "$PYTHON" -c 'import PIL' >/dev/null 2>&1; then
  echo "check-ui.sh needs Pillow: $PYTHON -m pip install pillow" >&2
  exit 2
fi

if ! command -v node >/dev/null 2>&1; then
  echo "check-ui.sh needs Node.js on PATH (scripts/proto-shot.mjs)" >&2
  exit 2
fi

CHROME="${CHROME_PATH:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
if [ ! -x "$CHROME" ]; then
  echo "check-ui.sh needs Google Chrome; set CHROME_PATH to its executable (looked at $CHROME)" >&2
  exit 2
fi
export CHROME_PATH="$CHROME"

cargo run -p tact-desktop --example ui_capture --features ui-capture -- target/ui-capture
node scripts/proto-shot.mjs target/ui-proto/prototype-light.png light
"$PYTHON" scripts/ui-check.py target/ui-capture target/ui-proto/prototype-light.png
"$PYTHON" scripts/ui-diff.py target/ui-capture target/ui-proto/prototype-light.png 01-shell-light

node scripts/proto-shot.mjs target/ui-proto/prototype-dark.png dark
"$PYTHON" scripts/ui-check.py target/ui-capture target/ui-proto/prototype-dark.png
"$PYTHON" scripts/ui-diff.py target/ui-capture target/ui-proto/prototype-dark.png 02-shell-dark
