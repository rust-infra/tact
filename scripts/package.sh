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

# A key generated with an empty password still needs *a* password argument:
# with none, the packager falls back to prompting on the controlling terminal,
# which fails with `ENXIO` in CI (no tty) after the package itself was already
# written. Setting the variable to the empty string is what says "this key has
# no password" rather than "ask me for one".
if [[ -n "${CARGO_PACKAGER_SIGN_PRIVATE_KEY:-}" ]] \
  && [[ -z "${CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD+x}" ]]; then
  export CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD=""
fi

cd "${ROOT}/crates/tact-gui"

# One invocation per format, on purpose. `cargo packager` stops at the first
# format it cannot produce, so asking for `deb,appimage` in one call means a
# failing AppImage also costs you the .deb — and on Windows a failing WiX .msi
# would cost you the NSIS installer that works. Splitting the call keeps every
# format independent: the artifacts that can be built are built, and the exit
# status still reports the failure so CI can warn about the gap.
IFS=',' read -r -a requested <<<"${FORMATS}"
built=()
failed=()
for format in "${requested[@]}"; do
  format="${format// /}"
  [[ -n "$format" ]] || continue
  log "Packaging ${format}"
  if cargo packager --release --formats "${format}"; then
    built+=("${format}")
  else
    failed+=("${format}")
  fi
done

log "Artifacts in ${ROOT}/dist/desktop:"
ls -1 "${ROOT}/dist/desktop" 2>/dev/null || true

if [[ ${#failed[@]} -gt 0 ]]; then
  printf 'warning: these formats failed: %s (built: %s)\n' \
    "${failed[*]}" "${built[*]:-none}" >&2
  exit 1
fi

log "Packaged: ${built[*]}"
