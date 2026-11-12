#!/usr/bin/env bash
# Reclaim `target/` space that cargo will never reuse.
#
# Why this exists: cargo never deletes stale build directories. Every time a
# unit's hash changes (a dependency bump, a feature flip, a `profile` edit) the
# old `target/debug/build/<pkg>/<hash>/` tree stays forever. Measured in this
# repo: `build/` was 24 GB of the 39 GB total, with 423 variants for a single
# package and a 131 MB binary inside one of them. That is cargo's own
# behaviour, not a workspace setting — `incremental` is a separate, smaller
# 2-3 GB working set that genuinely pays for itself.
#
# What is safe to delete, and why:
#
#   * `target/ui-capture*` scratch dirs — written by `scripts/check-ui.sh` and
#     the ad-hoc `ui-capture-rNN` comparisons. Nothing reads an old one.
#   * `target/{package,doc,flycheck0,ui-proto,icon-preview,...}` — regenerable
#     tool output, not build state.
#   * `target/<triple>/` for non-host triples — cross-target artifacts for
#     Windows and Linux that a macOS/Linux/Windows dev loop never links.
#   * `target/release/` — opt-in, because a release build is expensive and
#     `--release` is usually intentional.
#   * every `target/debug/incremental/<crate>-<hash>` dir — opt-in only. The
#     next build re-creates the ones it needs; the first edit-build after that
#     is a full rebuild of touched crates (11.8s instead of 2.8s for `tact`).
#
# What is NOT deleted, deliberately: `target/debug/build/<pkg>/<hash>` entries.
# There is no reliable way to tell a live variant from a dead one — cargo keys
# them by a hash that does not appear in `.fingerprint/`, mtimes do not move on
# a no-op build, and keeping "the newest N" was measured to force a 75s full
# rebuild. A wholesale `rm -rf target/debug/build` costs 75s once and then
# settles at 1.4 GB; that is a judgement call, so it is behind `--build`.
#
#   scripts/clean-target.sh                 # scratch + cross targets (safe)
#   scripts/clean-target.sh --dry-run       # print the plan, touch nothing
#   scripts/clean-target.sh --build         # also drop all of debug/build (75s rebuild)
#   scripts/clean-target.sh --incremental   # also drop incremental state
#   scripts/clean-target.sh --release       # also drop target/release
#   scripts/clean-target.sh --all           # all of the above

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DRY_RUN=0
WANT_BUILD=0
WANT_INCREMENTAL=0
WANT_RELEASE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --build) WANT_BUILD=1 ;;
    --incremental) WANT_INCREMENTAL=1 ;;
    --release) WANT_RELEASE=1 ;;
    --all)
      WANT_BUILD=1
      WANT_INCREMENTAL=1
      WANT_RELEASE=1
      ;;
    -h | --help)
      sed -n '2,45p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
  shift
done

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ ! -d "$TARGET_DIR" ]; then
  echo "no $TARGET_DIR/ — nothing to clean"
  exit 0
fi

# BSD `du` on macOS has no `-b`; `-sk` is portable to both.
size_kb() { du -sk "$1" 2>/dev/null | cut -f1 || echo 0; }

total_kb=0
removed=0

drop() {
  local path="$1"
  [ -e "$path" ] || return 0
  local kb
  kb="$(size_kb "$path")"
  total_kb=$((total_kb + kb))
  removed=$((removed + 1))
  if [ "$DRY_RUN" -eq 1 ]; then
    printf 'would remove  %8s  %s\n' "$(du -sh "$path" 2>/dev/null | cut -f1)" "${path#"$TARGET_DIR"/}"
  else
    printf 'removed       %8s  %s\n' "$(du -sh "$path" 2>/dev/null | cut -f1)" "${path#"$TARGET_DIR"/}"
    rm -rf "$path"
  fi
}

# --- always safe: scratch output that nothing reads again -------------------

# `scripts/check-ui.sh` owns `target/ui-capture`; keep that one, drop the rest.
for dir in "$TARGET_DIR"/ui-capture*; do
  [ -d "$dir" ] || continue
  [ "$(basename "$dir")" = "ui-capture" ] && continue
  drop "$dir"
done

for dir in package doc flycheck0 skill-review ui-compare icon-preview ui-proto ui-parity appicon; do
  drop "$TARGET_DIR/$dir"
done

# Cross-target artifacts: keep the host triple only.
host_triple="$(rustc -vV 2>/dev/null | awk '/^host:/ { print $2 }')"
for dir in "$TARGET_DIR"/*-unknown-*/; do
  [ -d "$dir" ] || continue
  dir="${dir%/}"
  [ "$(basename "$dir")" = "$host_triple" ] && continue
  drop "$dir"
done

# --- opt-in ----------------------------------------------------------------

if [ "$WANT_RELEASE" -eq 1 ]; then
  drop "$TARGET_DIR/release"
fi

if [ "$WANT_INCREMENTAL" -eq 1 ]; then
  for dir in "$TARGET_DIR"/debug/incremental/*/ "$TARGET_DIR"/release/incremental/*/; do
    [ -d "$dir" ] || continue
    drop "${dir%/}"
  done
fi

if [ "$WANT_BUILD" -eq 1 ]; then
  # One `rm -rf` instead of 2000 variant walks: same outcome, one line of
  # output, and the following build restores exactly the live set (1.4 GB).
  # `release/build` is included in `--release` above, so it is skipped here
  # when that pass already handled it.
  if [ "$WANT_RELEASE" -eq 0 ]; then
    drop "$TARGET_DIR/release/build"
  fi
  drop "$TARGET_DIR/debug/build"
fi

if [ "$removed" -eq 0 ]; then
  echo "nothing to reclaim under $TARGET_DIR/ (use --build / --incremental / --release for the opt-in passes)"
  exit 0
fi

printf '\n%d entries, %.1f GiB %s\n' \
  "$removed" \
  "$(awk -v kb="$total_kb" 'BEGIN { print kb / 1048576 }')" \
  "$([ "$DRY_RUN" -eq 1 ] && echo 'would be freed' || echo 'freed')"

if [ "$DRY_RUN" -eq 0 ]; then
  echo "the next build recompiles only what it needs"
fi
