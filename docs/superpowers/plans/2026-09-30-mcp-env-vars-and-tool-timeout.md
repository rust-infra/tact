# Plan: MCP `env_vars` and `tool_timeout_sec`

Spec: [2026-09-30-mcp-env-vars-and-tool-timeout-design.md](../specs/2026-09-30-mcp-env-vars-and-tool-timeout-design.md)

1. **`env_vars` config surface** (`crates/tact/src/mcp/mod.rs`)
   - `McpEnvVar` (untagged) + `name()` / `source()`.
   - `env_vars` on `McpServerConfig` and `McpProjectConfig`, carried through `to_stdio` /
     `to_transport` and `Default`.
   - `resolve_env_vars(server_name, explicit, env_vars)` — the whole resolution policy, testable
     without a process.
2. **Spawn wiring**
   - `McpClient::connect` resolves before spawning and merges into the child environment; a literal
     `env` entry wins.
3. **`tool_timeout_sec`**
   - Field on `McpProjectConfig`; `McpServerPolicy::tool_timeout`; `McpClient::call_tool` applies it
     over `MCP_CALL_TOOL_TIMEOUT` and names the effective budget in the error.
4. **Reporting**
   - `unmodelled_keys` stops reporting `tool_timeout_sec`, and adds `env_vars` for remote entries.
     Doc comments on `UnmodelledKeys` / `unmodelled_keys` reworded.
5. **Tests** — `cargo test -p tact --lib mcp::`; fix the two fixtures that assert the unmodelled key
   list.
6. **Docs sync**
   - `book/08_chapter_mcp_zh.md`: the field table gains both rows, the prose explains
     resolution order and the two refusals, the `enabled: false` paragraph drops `tool_timeout_sec`,
     and the Gaps table trades its two rows for accurate ones.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
