# Plan: MCP server instructions

Spec: [2026-09-30-mcp-server-instructions-design.md](../specs/2026-09-30-mcp-server-instructions-design.md)

Each step compiles and is tested before the next one starts. `cargo test` runs one invocation at a
time (AGENTS.md — parallel runs contend on `target/`).

1. **Capture** (`crates/tact_extensions/src/mcp/mod.rs`)
   - `MCP_INSTRUCTIONS_MAX_CHARS` next to the other MCP ceilings.
   - `McpService::instructions` (default `None`); `RealMcpService` reads `peer_info()`.
   - `MockMcpService::with_instructions`; `McpClient.instructions` + accessor, filled in
     `assemble`.
2. **Block** (`MCPToolRouter::instructions_block`)
   - Sorted `## <server>` sections, skip blank and skip all-tools-filtered, per-server cap with a
     truncation marker. Returns `String` (empty = nothing to inject).
3. **Prompt surface** (`crates/tact_extensions/src/prompt/mod.rs` + both templates)
   - `SystemPrompt.mcp_instructions` field, builder setter, all the hand-written `From`/`Default`
     impls (they are exhaustive — missing one is a compile error), `Into<Prompt>` context value.
   - New fenced section in `system_prompt_template.md` and `responses_system_prompt_template.md`,
     placed after `additional` and before `DYNAMIC_BOUNDARY`.
4. **Wire into the agent** (`crates/tact_extensions/src/agent/mod.rs`)
   - `.mcp_instructions(self.mcp_router.instructions_block())` in `build_system_prompt`.
5. **Report** (`crates/tact_extensions/src/mcp/mod.rs`, `crates/tact_ui/src/mcp_cli.rs`)
   - `McpServerInspection.instructions_chars: Option<usize>` filled by `inspect_server`; one line in
     `render_server_detail`; fixture updates for the new field.
6. **Tests**
   - `cargo test -p tact --lib mcp::` , then `--lib prompt::`, then `--lib agent::`.
   - `cargo test -p tact-ui --lib mcp_cli::`.
7. **Docs sync**
   - `book/08_chapter_mcp_zh.md`: a "Server instructions" subsection (capture, fence, cap,
     boundary placement) and the Gaps row for `InitializeResult.instructions` flipped to done.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
   - `ARCHITECTURE.md` only if the prompt-composition description drifts.
