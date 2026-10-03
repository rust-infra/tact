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

echo "==> cargo fmt -- --check"
cargo fmt -- --check

echo "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

# `--quiet`: one character per test instead of one line per test, and no rustc
# command lines. A green run then costs ~2 KB of terminal instead of ~200 KB —
# which matters because this script's stdout is read by humans *and* pasted into
# agent transcripts. libtest still prints the full failure block and the
# `test result:` summary in quiet mode, so a red run stays diagnosable.
echo "==> cargo test -p tact-ui -p tui -p tact -p tact_llm"
cargo test -p tact-ui -p tui -p tact -p tact_llm --quiet

echo "Rust checks passed."
