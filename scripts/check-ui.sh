#!/usr/bin/env bash
# Check the desktop shell on the real engine, headless.
#
# The GPUI build rendered the shell off-screen and compared the pixels with
# the HTML prototype. The Native Shell build drives the real app with
# `nsh-testing`: every test opens the engine, presses and types like a user,
# and asserts what the screen shows, so the same checks run on any machine
# without a window server or Screen Recording permission.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo test -p tact-desktop --test shell --test parity
