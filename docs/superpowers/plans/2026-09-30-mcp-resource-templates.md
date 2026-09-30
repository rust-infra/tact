# Plan: `resources/templates/list`

Spec: [2026-09-30-mcp-resource-templates-design.md](../specs/2026-09-30-mcp-resource-templates-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

1. **Service surface** (`crates/tact/src/mcp/mod.rs`)
   - `McpService::list_resource_templates()` — required, mirroring `list_resources`.
   - `RealMcpService` delegates to `list_all_resource_templates`; `MockMcpService` gains
     `with_resource_template` and answers from it.
   - `McpServerInspection::resource_templates: Option<usize>`, filled by `inspect_server`; the empty
     `InspectionFacts` gains the field.
2. **The tool** (`crates/tact/src/mcp/resource.rs`)
   - `McpResourceTool::Templates`, `LIST_RESOURCE_TEMPLATES_TOOL`, `from_name`, `name`, `spec`
     (optional `server`), `ALL` = list → templates → read.
   - `McpClient::list_resource_templates` with the resource timeout and named errors.
   - `MCPToolRouter::list_resource_templates(server: Option<&str>)`.
   - `render_resource_template_listing` — `uri_template`, name, mime, flattened description, and the
     "substitute the `{…}` placeholders" instruction.
   - `render_resource_listing`'s empty case now names `list_mcp_resource_templates` instead of
     claiming templates are unlistable.
3. **Dispatch** (`crates/tact/src/agent/tool_dispatch.rs`)
   - The new arm in `run_mcp_resource_tool`, and `is_mcp_resource_tool` recognises the name.
4. **Tests**
   - `cargo test -p tact --lib mcp::resource` , then `--lib mcp::` , then `--lib agent::tool_dispatch`
     and `agent::tests`.
   - `cargo test -p tact-ui --lib mcp_cli::` for the new inspection field.
5. **Docs sync**
   - `book/08_chapter_mcp.md` + `_zh.md`: Step 9, the §6 code map row, the Quick Reference row, and the
     §10 gap row narrowed to Prompts.
   - `book/26_chapter_issue.md` + `_zh.md`: newest-first entry.
