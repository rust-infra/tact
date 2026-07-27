# Shared helpers for book/scripts/generate.sh

log_info() {
  printf '[book] %s\n' "$*"
}

log_ok() {
  printf '[book] ✓ %s\n' "$*"
}

log_warn() {
  printf '[book] ! %s\n' "$*" >&2
}

log_error() {
  printf '[book] ERROR: %s\n' "$*" >&2
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    log_error "Required command not found: $1"
    exit 1
  fi
}

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

marp_render() {
  local deck="$1"
  local out_dir="$2"
  if have_cmd marp; then
    marp "$deck" --images png -o "$out_dir/" --allow-local-files
    return $?
  fi
  if have_cmd npx; then
    npx --yes @marp-team/marp-cli "$deck" --images png -o "$out_dir/" --allow-local-files
    return $?
  fi
  return 1
}
