#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

patch_api=(
  --config
  'patch.crates-io.tact_agent_api.path="crates/agent_api"'
)
patch_core=(
  --config
  'patch.crates-io.tact_agent_core.path="crates/agent_core"'
)

cargo package -p tact_agent_api --allow-dirty --no-verify
cargo package -p tact_agent_core --allow-dirty --no-verify "${patch_api[@]}"
cargo package -p tact_agent_sdk --allow-dirty --no-verify "${patch_api[@]}" "${patch_core[@]}"
cargo package -p tact_llm --allow-dirty --no-verify "${patch_api[@]}"

cargo check --manifest-path fixtures/tact-llm-consumer/Cargo.toml --locked
cargo check --manifest-path fixtures/tact-llm-agent-sdk-consumer/Cargo.toml --locked
