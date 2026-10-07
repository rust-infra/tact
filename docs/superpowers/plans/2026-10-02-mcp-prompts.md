# Plan: MCP prompts

Spec: [2026-10-02-mcp-prompts-design.md](../specs/2026-10-02-mcp-prompts-design.md)

1. **Rename the fetch ceiling.** `crates/tact/src/mcp/mod.rs`: `MCP_RESOURCE_TIMEOUT` →
   `MCP_FETCH_TIMEOUT`, doc comment widened to `resources/*` and `prompts/*`; update the three uses in
   `crates/tact/src/mcp/resource.rs`.

2. **New module `crates/tact/src/mcp/prompt.rs`**, mirroring `resource.rs`:
   - `LIST_PROMPTS_TOOL` / `GET_PROMPT_TOOL`, `McpPromptTool` with `ALL` / `from_name` / `name` /
     `spec`. The `Get` schema requires `server` and `name`, and takes an optional `arguments` object
     of strings.
   - `prompt_tool_risk` / `prompt_tool_risk_with` over `mcp.prompt_list_risk` / `prompt_get_risk`,
     `High` when unset.
   - `McpClient::list_prompts` / `get_prompt` on `MCP_FETCH_TIMEOUT`.
   - `MCPToolRouter::prompt_tool_specs` (empty router → none), `list_prompts(server: Option<&str>)`,
     `get_prompt(server, name, arguments)`.
   - `render_prompt_listing`, `render_prompt_messages`, `get_prompt_params`.
   - `crates/tact/src/mcp/resource.rs`: `known_servers` becomes `pub(super)` so the sibling module
     reuses it rather than copying the lookup.

3. **Service surface.** `crates/tact/src/mcp/mod.rs`: `mod prompt; pub use prompt::*;`;
   `McpService::list_prompts` / `get_prompt` required; `RealMcpService` delegates to rmcp's
   `list_all_prompts` / `get_prompt`; `MockMcpService` gains `prompts`, `prompt_messages`,
   `prompt_calls`, `with_prompt`, `with_prompt_messages`, `prompt_calls()`.

4. **Reporting.** `McpServerInspection` + `InspectionFacts` gain `prompts: Option<usize>`;
   `inspect_server` fills it with `.ok().map(|list| list.len())`; `crates/tact-ui/src/mcp_cli.rs`
   prints the line and its "did not answer" counterpart.

5. **Config.** `crates/tact/src/config/types.rs`: `prompt_list_risk` / `prompt_get_risk` on
   `McpTomlConfig`, `McpSettings` and the hand-written `Default`. `resolve.rs` parses both through the
   existing `parse_mcp_tool_risk`.

6. **Agent wiring.** `crates/tact/src/agent/mod.rs` chains `prompt_tool_specs()`.
   `crates/tact/src/agent/tool_dispatch.rs`: `ResolvedTool::McpPrompt`, `is_mcp_prompt_tool`,
   `run_mcp_prompt_tool`, `prompt_arguments`, the resolve branch, `tool_resources_for`, `stable_name`,
   the risk arm, and the execution branch.

7. **Tests.** The battery in `prompt.rs`; `prompt_arguments` and the dispatch paths in
   `tool_dispatch.rs`; the resolve test in `config/resolve.rs`; `EchoServer` grows `list_prompts` /
   `get_prompt` plus `mcp_client_gets_prompts_from_a_real_in_process_server`.

8. **Docs.** `config.example.toml`, `book/08_chapter_mcp_zh.md` (Step 9, code map, §10),
   `book/26_chapter_issue_zh.md`.

9. **Gate.** `cargo fmt` → `cargo fmt -- --check` → `cargo clippy --all-targets -- -D warnings` →
   `cargo test -p tact-ui -p tui -p tact -p tact_llm --quiet` (i.e. `scripts/check-rust.sh`).
