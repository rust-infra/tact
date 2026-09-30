# Plan: MCP server tool policy (Codex per-entry fields)

Spec: [2026-09-30-mcp-tool-policy-design.md](../specs/2026-09-30-mcp-tool-policy-design.md)

Each step compiles and is tested before the next one starts. `cargo test` is run one
invocation at a time (see AGENTS.md — parallel runs contend on `target/`).

1. **Config surface** (`crates/tact/src/mcp/mod.rs`)
   - `McpToolConfig { approval_mode, output_token_limit }`, `ApprovalMode { Auto, Prompt, Approve }`.
   - New `McpProjectConfig` fields with explicit snake_case `rename`s and `Default` updates.
   - `McpServerPolicy` (enabled/disabled lists, startup timeout, default + per-tool approval,
     per-tool output limit) + `expose_tool()`; `McpServerPolicy::from_config`.
   - `unmodelled_keys` keeps reporting only `omit_tools_from` / `tool_timeout_sec`; update the
     doc comments and the existing fixture test.
2. **Exposure filtering** (`McpClient`)
   - `McpClient::try_new` / `with_service` take the policy, filter `tool_specs`, and keep the
     hidden names; `hidden_tools()` accessor; `tool_count()` keeps meaning "exposed".
   - `ResolvedServers` carries `policies: HashMap<String, McpServerPolicy>`; the connect loop
     and `inspect_server` pass it through.
   - `McpLoadReport.filtered` + `mcp list` section + `mcp get` line.
3. **Startup timeout**
   - `McpClient::connect` takes `Option<Duration>`; `MCP_INIT_TIMEOUT` stays the default.
   - Timeout errors name the effective budget.
4. **Approval policy**
   - `PermissionManager::check_with_auto(…, auto_approved)`; `check` delegates with `false`.
   - `MCPToolRouter::is_auto_approved(server, tool)`.
   - `tool_dispatch` computes the flag and calls `check_with_auto`; `normalize_mcp_capability`
     is untouched (risk stays `High`).
5. **Output budget**
   - Extract an unconditional spill helper from `persist_large_output`; add
     `persist_large_output_over_tokens`.
   - `MCPToolRouter::output_token_limit(server, tool)`; `run_mcp_tool` uses it.
6. **Tests**
   - `cargo test -p tact --lib mcp::` , then `--lib permission::`, then `--lib compact::`.
   - `cargo test -p tact-ui --lib mcp_cli::` for the new `mcp list` section.
7. **Docs sync**
   - `book/08_chapter_mcp.md` + `_zh.md`: Step 1 field table, a new "Per-server tool policy"
     section, and the Current Gaps row for `enabled_tools` / `omit_tools_from` /
     `startup_timeout_sec`.
   - `config.example.toml` — an `[mcp]` comment block pointing at the new fields.
   - `book/26_chapter_issue.md` + `_zh.md` — one newest-first entry (date, type, symptom,
     decision, observable behaviour, pointers).
