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
patch_llm=(
  --config
  'patch.crates-io.tact_llm.path="crates/tact_llm"'
)
patch_protocol=(
  --config
  'patch.crates-io.tact_protocol.path="crates/protocol"'
)
patch_macros=(
  --config
  'patch.crates-io.tool_refactor_macros.path="crates/tool_refactor_macros"'
)

cargo package -p tact_agent_api --allow-dirty --no-verify
cargo package -p tact_agent_core --allow-dirty --no-verify "${patch_api[@]}"
cargo package -p tact_agent_sdk --allow-dirty --no-verify "${patch_api[@]}" "${patch_core[@]}"
cargo package -p tact_llm --allow-dirty --no-verify "${patch_api[@]}"
cargo package -p tact_protocol --allow-dirty --no-verify "${patch_api[@]}"
cargo package -p tool_refactor_macros --allow-dirty --no-verify
cargo package -p tact --allow-dirty --no-verify \
  "${patch_api[@]}" "${patch_core[@]}" "${patch_llm[@]}" \
  "${patch_protocol[@]}" "${patch_macros[@]}"
cargo package -p tact_host --allow-dirty --no-verify \
  "${patch_api[@]}" "${patch_core[@]}" "${patch_llm[@]}" "${patch_protocol[@]}" \
  "${patch_macros[@]}" --config 'patch.crates-io.tact.path="crates/tact"'

cargo check --manifest-path fixtures/tact-llm-consumer/Cargo.toml --locked
cargo check --manifest-path fixtures/tact-llm-agent-sdk-consumer/Cargo.toml --locked
cargo check --manifest-path fixtures/tact-host-consumer/Cargo.toml --locked
