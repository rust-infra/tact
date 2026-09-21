#!/usr/bin/env bash
# Build the Tact desktop installers for the current platform.
#
# `cargo packager` reads `[package.metadata.packager]` from
# `crates/tact-gui/Cargo.toml` and writes the platform's installers into
# `dist/desktop`: a .deb and an AppImage on Linux, a .dmg and an .app on macOS,
# an NSIS .exe and a WiX .msi on Windows.
#
# Signing is opt-in through the packager's own environment variables; when
# `CARGO_PACKAGER_SIGN_PRIVATE_KEY` is set, every artifact gets a `.sig`
# beside it, which is what the in-app updater verifies.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FORMATS="${1:-}"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
log() { printf '==> %s\n' "$*"; }

command -v cargo >/dev/null 2>&1 || die "cargo not found"

if ! cargo packager --version >/dev/null 2>&1; then
  die "cargo-packager is not installed; run: cargo install cargo-packager --locked"
fi

case "$(uname -s)" in
  Linux) default_formats="deb,appimage" ;;
  Darwin) default_formats="dmg,app" ;;
  MINGW*|MSYS*|CYGWIN*) default_formats="nsis,wix" ;;
  *) default_formats="" ;;
esac

if [[ -z "$FORMATS" ]]; then
  FORMATS="$default_formats"
fi
[[ -n "$FORMATS" ]] || die "cannot infer installers for $(uname -s); pass a format list, e.g. deb"

log "Packaging crates/tact-gui as ${FORMATS}"
cd "${ROOT}/crates/tact-gui"
cargo packager --release --formats "${FORMATS}"

log "Artifacts in ${ROOT}/dist/desktop:"
ls -1 "${ROOT}/dist/desktop" 2>/dev/null || true
