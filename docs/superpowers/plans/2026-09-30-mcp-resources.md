# Plan: MCP resources

Spec: [2026-09-30-mcp-resources-design.md](../specs/2026-09-30-mcp-resources-design.md)

1. **Service surface** (`crates/tact/src/mcp/mod.rs`)
   - `MCP_RESOURCE_TIMEOUT`.
   - `McpService::{list_resources, read_resource}` — required, so an implementation that cannot
     answer must say so; `RealMcpService` delegates to rmcp, `MockMcpService` gains
     `with_text_resource` and returns a real `resource_not_found`.
2. **New module** (`crates/tact/src/mcp/resource.rs`)
   - `McpResourceTool { List, Read }` + names, `from_name`, `spec`.
   - `McpClient::{list_resources, read_resource}` with the timeout and named errors.
   - `MCPToolRouter::{resource_tool_specs, list_resources, read_resource}`.
   - `render_resource_listing` / `render_resource_contents`.
3. **Dispatch** (`crates/tact/src/agent/tool_dispatch.rs`)
   - `ResolvedTool::McpResource`; a guarded arm in the resolver; arms in the three exhaustive
     matches (`tool_resources_for`, `stable_name`, `risk`); `run_mcp_resource_tool` on the
     execution path.
4. **Agent** (`crates/tact/src/agent/mod.rs`)
   - `rebuild_cached_tool_specs` chains `resource_tool_specs()`.
5. **Reporting** (`crates/tact/src/mcp/mod.rs`, `crates/tact-ui/src/mcp_cli.rs`)
   - `McpServerInspection::resources: Option<usize>`; one line in `render_server_detail`; fixtures.
6. **Tests**
   - `cargo test -p tact --lib mcp::resource`, `--lib mcp::tests::mcp_client`,
     `--lib agent::tool_dispatch`, `--lib agent::tests::the_resource_tools`;
     `cargo test -p tact-ui --lib mcp_cli::`.
7. **Docs sync**
   - `book/08_chapter_mcp_zh.md`: Step 9 rewritten around the two tools, the code map gains
     the module, and the Gaps row splits templates/prompts from resources.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
