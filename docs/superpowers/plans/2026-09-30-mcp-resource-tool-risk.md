# Plan: declare the risk of Tact's own resource tools

Spec: [2026-09-30-mcp-resource-tool-risk-design.md](../specs/2026-09-30-mcp-resource-tool-risk-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

1. **Config surface** (`crates/tact/src/config/types.rs`, `resolve.rs`)
   - `McpTomlConfig::{resource_list_risk, resource_read_risk}: Option<String>`.
   - `McpSettings` gains the same two as `Option<CapabilityRisk>`, parsed with `mcp::ToolRisk` in
     `resolve_mcp`; update `Default` and the resolved literal.
   - `config.example.toml`: the `[mcp]` block gains both keys.
2. **Resolution** (`crates/tact/src/mcp/mod.rs`)
   - `resource_tool_risk(tool: McpResourceTool) -> CapabilityRisk`, reading `config::try_settings()`
     and defaulting to `CapabilityRisk::High`; a warning on an unknown value happens at parse time.
3. **Dispatch** (`crates/tact/src/agent/tool_dispatch.rs`)
   - The `ResolvedTool::McpResource { tool }` arm calls it instead of a bare `High`.
4. **Tests**
   - `cargo test -p tact --lib mcp::` , `--lib config::` , `--lib agent::tool_dispatch`.
5. **Docs sync**
   - `book/08_chapter_mcp_zh.md`: the §10 row and the resource section.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
