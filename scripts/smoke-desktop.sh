#!/usr/bin/env bash
# Starts the desktop and checks it stays up, then stops it.
#
#   smoke-desktop.sh <binary>
#
# A packaged NSH runner has no smoke flag, so this opens the real window (under
# xvfb on Linux) and reads what the process said.
set -euo pipefail

bin="${1:?binary}"
log="$(mktemp)"
trap 'rm -f "${log}"' EXIT

uname_s="$(uname -s)"
if [[ "${uname_s}" == MINGW* || "${uname_s}" == CYGWIN* || "${uname_s}" == MSYS* ]]; then
  export APPDATA="${HOME}/AppData/Roaming"
  export LOCALAPPDATA="${HOME}/AppData/Local"
  mkdir -p "${APPDATA}" "${LOCALAPPDATA}"
fi

if [[ "${uname_s}" == "Linux" ]]; then
  HOME="${HOME}" XDG_DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}" \
    TACT_REDUCED_MOTION=1 xvfb-run -a "${bin}" >"${log}" 2>&1 &
else
  TACT_REDUCED_MOTION=1 "${bin}" >"${log}" 2>&1 &
fi
pid=$!

status=0
if kill -0 "${pid}" 2>/dev/null; then
  sleep 8
  if ! kill -0 "${pid}" 2>/dev/null; then
    echo "the desktop exited during startup" >&2
    status=1
  fi
  kill "${pid}" 2>/dev/null || true
  wait "${pid}" 2>/dev/null || true
else
  echo "the desktop exited at once" >&2
  status=1
fi

cat "${log}"
if grep -Eq 'panicked at|failed to start' "${log}"; then
  echo "the desktop reported a startup failure" >&2
  status=1
fi
exit "${status}"
