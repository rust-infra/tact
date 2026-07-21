#!/usr/bin/env bash
# Install tact-ui on Linux or macOS (prefer GitHub release, else build from source).
set -euo pipefail

REPO="${TACT_INSTALL_REPO:-rust-infra/tact}"
GIT_REF="${TACT_INSTALL_GIT_REF:-main}"
BINARY_NAME="tact-ui"
CRATE_PACKAGE="tact-ui"
DEFAULT_VERSION="0.19.0"
# Matches workspace.package.rust-version (edition 2024).
MIN_RUSTC_VERSION="1.85.0"

INSTALL_DIR="${TACT_INSTALL_DIR:-}"
USE_SYSTEM=0
FROM_SOURCE=0
RELEASE_ONLY=0
NO_MODIFY_PATH=0
SKIP_DEPS=0

usage() {
  cat <<'EOF'
Usage: install.sh [OPTIONS]

Install the tact-ui binary on Linux or macOS.

By default the installer downloads a matching GitHub release asset when one
exists, otherwise it builds from source (Rust 1.85+ / edition 2024 required).

Options:
  --install-dir DIR   Install to DIR (default: ~/.local/bin)
  --system            Install to /usr/local/bin (may require sudo)
  --from-source       Build from source only (skip release download)
  --release           Prefer a GitHub release binary; fall back to source build
  --release-only      Require a GitHub release binary (no source fallback)
  --git-ref REF       Git branch/tag when cloning (default: main)
  --skip-deps         Skip OS package / rustup dependency installation
  --no-modify-path    Do not append the install directory to shell PATH
  -h, --help          Show this help

Environment:
  TACT_INSTALL_DIR       Override install directory
  TACT_INSTALL_REPO      GitHub repo (owner/name)
  TACT_INSTALL_GIT_REF   Git ref when cloning from source

Examples:
  curl -fsSL https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.sh | bash
  ./scripts/install.sh --from-source
  ./scripts/install.sh --release --install-dir ~/.local/bin
EOF
}

log() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)
      [[ $# -ge 2 ]] || die "--install-dir requires a path"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --system) USE_SYSTEM=1; shift ;;
    --from-source) FROM_SOURCE=1; shift ;;
    --release) FROM_SOURCE=0; shift ;;
    --release-only) RELEASE_ONLY=1; FROM_SOURCE=0; shift ;;
    --git-ref)
      [[ $# -ge 2 ]] || die "--git-ref requires a value"
      GIT_REF="$2"
      shift 2
      ;;
    --skip-deps) SKIP_DEPS=1; shift ;;
    --no-modify-path) NO_MODIFY_PATH=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Linux|Darwin) ;;
  *) die "unsupported OS: $OS (use scripts/install.ps1 on Windows)" ;;
esac

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

# True when $1 >= $2 (dotted numeric versions).
version_ge() {
  printf '%s\n%s\n' "$2" "$1" | sort -V -C
}

detect_target_triple() {
  case "$OS-$ARCH" in
    Linux-x86_64|Linux-amd64) echo "x86_64-unknown-linux-gnu" ;;
    Linux-aarch64|Linux-arm64) echo "aarch64-unknown-linux-gnu" ;;
    Darwin-x86_64) echo "x86_64-apple-darwin" ;;
    Darwin-arm64|Darwin-aarch64) echo "aarch64-apple-darwin" ;;
    *) die "unsupported platform: $OS $ARCH" ;;
  esac
}

default_install_dir() {
  if [[ "$USE_SYSTEM" -eq 1 ]]; then
    echo "/usr/local/bin"
  else
    echo "${HOME}/.local/bin"
  fi
}

INSTALL_DIR="${INSTALL_DIR:-$(default_install_dir)}"

install_linux_deps() {
  [[ "$SKIP_DEPS" -eq 1 ]] && return 0
  [[ "$OS" == "Linux" ]] || return 0

  if command -v apt-get >/dev/null 2>&1; then
    log "Installing Linux build dependencies (apt)..."
    if [[ "$(id -u)" -eq 0 ]]; then
      apt-get update
      apt-get install -y build-essential pkg-config libsqlite3-dev clang libclang-dev curl git
    elif command -v sudo >/dev/null 2>&1; then
      sudo apt-get update
      sudo apt-get install -y build-essential pkg-config libsqlite3-dev clang libclang-dev curl git
    else
      warn "run as root or install: build-essential pkg-config libsqlite3-dev clang libclang-dev"
    fi
  elif command -v dnf >/dev/null 2>&1; then
    log "Installing Linux build dependencies (dnf)..."
    local pkgs=(gcc gcc-c++ make pkg-config sqlite-devel clang clang-devel git curl)
    if [[ "$(id -u)" -eq 0 ]]; then
      dnf install -y "${pkgs[@]}"
    elif command -v sudo >/dev/null 2>&1; then
      sudo dnf install -y "${pkgs[@]}"
    else
      warn "install manually: ${pkgs[*]}"
    fi
  elif command -v pacman >/dev/null 2>&1; then
    log "Installing Linux build dependencies (pacman)..."
    local pkgs=(base-devel pkgconf sqlite clang git curl)
    if [[ "$(id -u)" -eq 0 ]]; then
      pacman -Sy --noconfirm "${pkgs[@]}"
    elif command -v sudo >/dev/null 2>&1; then
      sudo pacman -Sy --noconfirm "${pkgs[@]}"
    else
      warn "install manually: ${pkgs[*]}"
    fi
  elif command -v apk >/dev/null 2>&1; then
    log "Installing Linux build dependencies (apk)..."
    if [[ "$(id -u)" -eq 0 ]]; then
      apk add --no-cache build-base pkgconf sqlite-dev clang llvm-dev git curl
    elif command -v sudo >/dev/null 2>&1; then
      sudo apk add --no-cache build-base pkgconf sqlite-dev clang llvm-dev git curl
    else
      warn "install manually: build-base pkgconf sqlite-dev clang llvm-dev"
    fi
  else
    warn "unknown Linux package manager; ensure sqlite, pkg-config, clang, and a C compiler are installed"
  fi
}

install_macos_deps() {
  [[ "$SKIP_DEPS" -eq 1 ]] && return 0
  [[ "$OS" == "Darwin" ]] || return 0

  if ! xcode-select -p >/dev/null 2>&1; then
    warn "Xcode Command Line Tools not found; install with: xcode-select --install"
  fi

  if command -v brew >/dev/null 2>&1; then
    log "Ensuring macOS build dependencies (Homebrew)..."
    brew list sqlite >/dev/null 2>&1 || brew install sqlite
    brew list pkg-config >/dev/null 2>&1 || brew install pkg-config
  fi
}

rustc_meets_minimum() {
  command -v rustc >/dev/null 2>&1 || return 1
  local ver
  ver="$(rustc -V 2>/dev/null | awk '{print $2}')"
  [[ -n "$ver" ]] || return 1
  version_ge "$ver" "$MIN_RUSTC_VERSION"
}

ensure_rust() {
  [[ "$SKIP_DEPS" -eq 1 ]] && return 0

  if rustc_meets_minimum && command -v cargo >/dev/null 2>&1; then
    return 0
  fi

  if command -v rustup >/dev/null 2>&1; then
    log "Updating stable Rust toolchain (need >= ${MIN_RUSTC_VERSION} for edition 2024)..."
    rustup toolchain install stable --profile minimal
    rustup default stable
  elif ! command -v cargo >/dev/null 2>&1; then
    log "Rust toolchain not found; installing via rustup (stable >= ${MIN_RUSTC_VERSION})..."
    need_cmd curl
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  else
    die "rustc $(rustc -V 2>/dev/null || echo '?') is too old; install Rust >= ${MIN_RUSTC_VERSION} (https://rustup.rs)"
  fi

  # shellcheck disable=SC1091
  [[ -f "${HOME}/.cargo/env" ]] && source "${HOME}/.cargo/env"
  command -v cargo >/dev/null 2>&1 || die "cargo still not found after rustup install"
  rustc_meets_minimum || die "rustc $(rustc -V) is still below ${MIN_RUSTC_VERSION}; run: rustup update stable"
}

repo_root() {
  local dir="$1"
  [[ -f "${dir}/Cargo.toml" && -d "${dir}/crates/tact" ]]
}

resolve_version() {
  local root="$1"
  if [[ -f "${root}/Cargo.toml" ]]; then
    # Prefer [workspace.package] version; fall back to first bare version = "..." line.
    awk '
      /^\[workspace\.package\]/ { in_pkg=1; next }
      /^\[/ { in_pkg=0 }
      in_pkg && /^version = "/ {
        gsub(/"/, "", $3); print $3; found=1; exit
      }
      END {
        if (!found) exit 1
      }
    ' "${root}/Cargo.toml" \
      || awk -F'"' '/^version = "/ { print $2; exit }' "${root}/Cargo.toml"
  else
    echo "$DEFAULT_VERSION"
  fi
}

try_install_release() {
  local version="$1"
  local triple="$2"
  local tmp archive url asset_name

  asset_name="${BINARY_NAME}-v${version}-${triple}.tar.gz"
  url="https://github.com/${REPO}/releases/download/v${version}/${asset_name}"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  log "Trying release asset: ${asset_name}"
  if ! curl -fsSL -o "${tmp}/${asset_name}" "$url"; then
    warn "release asset not found at ${url}"
    return 1
  fi

  tar -xzf "${tmp}/${asset_name}" -C "$tmp"
  if [[ -f "${tmp}/${BINARY_NAME}" ]]; then
    install_binary "${tmp}/${BINARY_NAME}"
    return 0
  fi
  if [[ -f "${tmp}/${BINARY_NAME}-${triple}/${BINARY_NAME}" ]]; then
    install_binary "${tmp}/${BINARY_NAME}-${triple}/${BINARY_NAME}"
    return 0
  fi

  warn "release archive did not contain ${BINARY_NAME}"
  return 1
}

build_from_source() {
  local root="$1"
  local -a cargo_args=(build --release -p "$CRATE_PACKAGE")
  log "Building ${BINARY_NAME} from source (rustc >= ${MIN_RUSTC_VERSION})..."
  need_cmd cargo
  if [[ -f "${root}/Cargo.lock" ]]; then
    cargo_args+=(--locked)
  fi
  (
    cd "$root"
    cargo "${cargo_args[@]}"
  )
  local built="${root}/target/release/${BINARY_NAME}"
  [[ -f "$built" ]] || die "build succeeded but binary missing: ${built}"
  install_binary "$built"
}

install_binary() {
  local src="$1"
  mkdir -p "$INSTALL_DIR"
  if [[ "$USE_SYSTEM" -eq 1 && ! -w "$INSTALL_DIR" ]]; then
    need_cmd sudo
    sudo install -m 0755 "$src" "${INSTALL_DIR}/${BINARY_NAME}"
  else
    install -m 0755 "$src" "${INSTALL_DIR}/${BINARY_NAME}"
  fi
  log "Installed ${BINARY_NAME} -> ${INSTALL_DIR}/${BINARY_NAME}"
}

ensure_path() {
  [[ "$NO_MODIFY_PATH" -eq 1 ]] && return 0
  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) return 0 ;;
  esac

  local line="export PATH=\"${INSTALL_DIR}:\$PATH\""
  local updated=0
  for rc in "${HOME}/.bashrc" "${HOME}/.zshrc" "${HOME}/.profile"; do
    if [[ -f "$rc" ]] && ! grep -Fq "$INSTALL_DIR" "$rc"; then
      printf '\n# tact-ui installer\n%s\n' "$line" >>"$rc"
      log "Added ${INSTALL_DIR} to PATH in ${rc}"
      updated=1
    fi
  done

  if [[ "$updated" -eq 0 ]]; then
    warn "add ${INSTALL_DIR} to your PATH manually"
  else
    warn "restart your shell or run: export PATH=\"${INSTALL_DIR}:\$PATH\""
  fi
}

main() {
  local src_root="" work="" version="" triple=""

  if repo_root "$(pwd)"; then
    src_root="$(pwd)"
    log "Using current repository: ${src_root}"
  else
    need_cmd git
    work="$(mktemp -d)"
    trap '[[ -n "${work:-}" ]] && rm -rf "$work"' EXIT
    log "Cloning https://github.com/${REPO}.git (${GIT_REF})..."
    git clone --depth 1 --branch "$GIT_REF" "https://github.com/${REPO}.git" "$work"
    src_root="$work"
  fi

  version="$(resolve_version "$src_root")"
  triple="$(detect_target_triple)"

  install_linux_deps
  install_macos_deps

  if [[ "$FROM_SOURCE" -eq 1 ]]; then
    ensure_rust
    build_from_source "$src_root"
    ensure_path
    log "Done. Run: ${BINARY_NAME} --help"
    return 0
  fi

  if try_install_release "$version" "$triple"; then
    ensure_path
    log "Done. Run: ${BINARY_NAME} --help"
    return 0
  fi

  if [[ "$RELEASE_ONLY" -eq 1 ]]; then
    die "no release asset found for v${version} (${triple}); publish a release or omit --release-only"
  fi

  warn "falling back to source build"
  ensure_rust
  build_from_source "$src_root"
  ensure_path
  log "Done. Run: ${BINARY_NAME} --help"
}

main "$@"
