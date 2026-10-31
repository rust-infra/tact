#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

packages=(tact tact_host tact_protocol tact_catalog tact_desktop_core)
for package in "${packages[@]}"; do
  tree="$(cargo tree -p "$package" --prefix none --edges normal,build)"
  if printf '%s\n' "$tree" | grep -Eq '(^|[[:space:]/])gpui(-[a-z0-9-]+)?([[:space:]]|$)'; then
    echo "error: GPUI dependency leaked into $package" >&2
    exit 1
  fi
done

echo "desktop dependency boundaries passed"
