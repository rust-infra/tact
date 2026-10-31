#!/usr/bin/env bash
# CI-style Rust checks: formatting + clippy with warnings denied (+ integration tests).
set -euo pipefail

# Do not inherit GIT_DIR / GIT_WORK_TREE from a calling git hook (pre-push,
# pre-commit, ...): tests must run git commands against their own temp repos.
unset GIT_DIR GIT_WORK_TREE

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found on PATH" >&2
  exit 1
fi

# macOS gives each process `ulimit -n 256`. The workspace test binary opens a
# SQLite pool, tokio runtimes and a skill registry per desktop-core test, so
# that budget runs out part-way through and libtest reports it as unrelated
# failures ("Too many open files (os error 24)" raised inside controller
# startup). Raise the soft limit for this script only, up to the hard limit,
# so a red run always means a real defect. Builtins only: this must not depend
# on the caller's shell config.
soft_limit="$(ulimit -Sn 2>/dev/null || true)"
hard_limit="$(ulimit -Hn 2>/dev/null || true)"
if [ -z "$hard_limit" ] || [ "$hard_limit" = "unlimited" ]; then
  target_limit=8192
else
  target_limit="$hard_limit"
fi
if [ -n "$soft_limit" ] && [ "$soft_limit" != "unlimited" ] \
  && [ "$target_limit" != "unlimited" ] && [ "$soft_limit" -lt "$target_limit" ]; then
  if ulimit -n "$target_limit" 2>/dev/null; then
    raised_limit="$(ulimit -Sn)"
    echo "==> raised the open-file limit from $soft_limit to $raised_limit for this run"
  else
    echo "==> warning: could not raise the open-file limit above $soft_limit" >&2
    echo "    (desktop-core tests may fail with 'Too many open files')" >&2
  fi
fi

echo "==> agent kernel dependency boundaries"
scripts/check-agent-kernel-deps.sh tact_agent_api tact_agent_core tact_llm tact_agent_sdk

echo "==> desktop dependency boundaries"
scripts/check-desktop-deps.sh

echo "==> packaging manifests"
python3 scripts/check-packaging.py

echo "==> package and consumer fixtures"
scripts/check-package.sh

echo "==> cargo fmt -- --check"
cargo fmt -- --check

echo "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

# `--quiet`: one character per test instead of one line per test, and no rustc
# command lines. A green run then costs ~2 KB of terminal instead of ~200 KB —
# which matters because this script's stdout is read by humans *and* pasted into
# agent transcripts. libtest still prints the full failure block and the
# `test result:` summary in quiet mode, so a red run stays diagnosable.
echo "==> cargo test --workspace"
cargo test --workspace --quiet

echo "==> book checks"
python3 scripts/book-check.py book

echo "Rust checks passed."
