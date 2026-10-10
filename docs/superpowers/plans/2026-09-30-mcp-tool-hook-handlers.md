# Plan: `mcp_tool` hook handlers

Spec: [2026-09-30-mcp-tool-hook-handlers-design.md](../specs/2026-09-30-mcp-tool-hook-handlers-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

1. **Naming** (`crates/tact_extensions/src/mcp/mod.rs`)
   - `mcp_tool_name(server, tool)` free fn + `McpToolName::full_name`; `build_tool_specs` uses it.
2. **Schema** (`crates/tact_extensions/src/plugin/hooks.rs`)
   - `HookCommand` gains `server`, `tool`, `arguments`; `type` is camel/snake tolerant for the two
     spellings Tact accepts.
   - `HookCommand::kind() -> HookKind` with `Command` / `McpTool { server, tool }` / `Invalid(reason)`.
3. **Runner**
   - `run_hook(command, dirs, input, agent)` dispatches; the nine production call sites switch to it.
   - `run_mcp_tool_hook` calls the router under the hook's own timeout and normalizes through
     `parse_output`.
   - `finish_hook_output` / `report_hook_failure` extracted so both kinds share the post-processing.
4. **Tests**
   - `cargo test -p tact --lib plugin::hooks::` , then `--lib mcp::`.
5. **Docs sync**
   - `book/09_chapter_hook_zh.md`: the second hook kind, the code map, and the §13 row.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
