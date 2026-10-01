# Plan: `notifications/tools/list_changed`

Spec: [2026-09-30-mcp-tool-list-changed-design.md](../specs/2026-09-30-mcp-tool-list-changed-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

1. **Capture the notification** (`crates/tact/src/mcp/mod.rs`)
   - `ToolListChangedSignal` (`Clone + Default`, `Arc<AtomicBool>`) implementing
     `ClientHandler::on_tool_list_changed`.
   - `connect` installs it (`signal.clone().serve(transport)`) and returns the clone alongside the
     `RunningService`; `RealMcpService` stores it.
   - `McpService::take_tools_changed(&self) -> bool`, defaulted `false`.
2. **Refresh** (`crates/tact/src/mcp/mod.rs`)
   - Extract the filter/spec/annotation derivation out of `assemble` into one helper; `assemble` calls
     it.
   - `McpClient::refresh_tools_if_stale(&mut self) -> Result<bool>` — no-op when the flag is clear;
     otherwise re-list, re-derive through the helper, and report whether it refreshed.
   - `MCPToolRouter::refresh_changed(&mut self) -> Vec<ToolListChange>` with
     `ToolListChange { server, before, after }`; a failed re-list keeps the previous list and is
     reported.
3. **Wire into the agent** (`crates/tact/src/agent/mod.rs`)
   - At the top of each `agent_loop` iteration, before the request is built: refresh, emit one
     `Info` line per changed server (and per failure), and `rebuild_cached_tool_specs()`.
4. **Tests**
   - `cargo test -p tact --lib mcp::` , then `--lib agent::`.
   - The in-process rmcp fixture (`EchoServer`) gains a mutable tool list and a `call_tool` arm that
     adds a tool and calls `peer.notify_tool_list_changed()`; the test polls with a deadline.
5. **Docs sync**
   - `book/08_chapter_mcp_zh.md`: Step 4 names the refresh point, Step 10 gains the handling,
     and the §10 gap row is replaced.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
