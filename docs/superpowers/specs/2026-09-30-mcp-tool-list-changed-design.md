# Design: `notifications/tools/list_changed`

Date: 2026-09-30
Status: approved for implementation

Closes the gap recorded in [Ch 8 §10](../../../book/08_chapter_mcp.md): *"No `tools/list_changed`
handling — Tool list fixed at connect; no `ClientHandler` or loop refresh."*

## 1. Problem

Tact snapshots a server's tools once, at connect: `McpClient::assemble` fills `tools` / `tool_specs`,
the agent copies them into `cached_tool_specs`, and every request is built from that snapshot. MCP
defines `notifications/tools/list_changed` for exactly the case where the list moves afterwards, and
Tact is deaf to it, because the connection is created as `RunningService<RoleClient, ()>` — the
handler is the unit type, so `ClientHandler::on_tool_list_changed` is rmcp's no-op default.

The cost is concrete. A server that finishes an authorization or an indexing pass and then exposes
more tools is stuck at whatever it advertised during the handshake; the model never learns the tool
exists, and `mcp list` agrees with the stale snapshot. The only fix today is a restart, or
`reload_mcp_router` (which re-dials every server, including the healthy ones). A server that *removes*
a tool is worse: the model keeps being offered a name that now fails.

## 2. Design

### 2.1 Capture — a handler that only records

`ToolListChangedSignal` is `Clone + Default`, holds an `Arc<AtomicBool>`, and implements
`ClientHandler::on_tool_list_changed` by setting it. It is installed with `signal.clone().serve(transport)`,
and the clone is handed to `RealMcpService` so the client can read it.

The handler does **not** re-list. Re-listing needs `&mut McpClient` and it must not run inside the
service's notification task, where it would contend with the very transport it is driven by. The
notification is a *hint*: it says "ask again", not what the answer is.

`McpService::take_tools_changed(&self) -> bool` exposes the flag, defaulted to `false` so every test
double keeps compiling and so a service that cannot send notifications can never become stale.

### 2.2 Refresh — one path, shared with connect

`McpClient::refresh_tools_if_stale(&mut self) -> Result<bool>` returns `false` without touching the
server when the flag is clear. When it is set, it re-lists and re-derives everything the client keeps:
the policy filter, `tool_specs`, and `declared_read_only`.

Those derivations move into one helper that `assemble` also calls. Two code paths that decide "which
tools does this server expose" would eventually disagree, and the disagreement would be invisible —
exactly the class of bug this file's `filtered` / `unmodelled` reporting exists to prevent.

The entry filter is re-applied to the **new** list, so a tool that appears later and is named in
`disabled_tools` is hidden and reported, not silently exposed.

### 2.3 Reporting and failure

`MCPToolRouter::refresh_changed(&mut self) -> Vec<ToolListChange>` walks the clients, refreshes the
flagged ones, and reports `{ server, before, after }` for the ones whose count actually moved.

A **failed re-list keeps the previous list**. Tools are lost only if the server says so; a transient
transport error must not turn a working server into a server with no tools. The failure is reported as
its own line.

The agent drains this at the top of each `agent_loop` iteration — before the request is built — and
emits one `Info` line per changed server, then rebuilds `cached_tool_specs`. Two reasons for that
placement:

- The tool list is sent **per request**, so a change that lands mid-turn is picked up on the next LLM
  call rather than waiting for the next user turn.
- `cached_tool_specs` is otherwise only rebuilt on construction and on `reload_mcp_router`, so without
  this the refresh would be invisible to the model.

A silent tool-list change would be the same failure mode `filtered` exists to avoid, so the line is
not optional.

### 2.4 What is deliberately not rebuilt

`instructions_block()` — the system prompt's `## <server>` sections — is built once per turn and is not
re-derived on refresh. Instructions come from the `initialize` result and cannot change for the life of
a connection, so re-deriving them would be a no-op that invites the reader to think otherwise.

## 3. Non-goals

- `notifications/resources/list_changed` and `notifications/prompts/list_changed`. Resources are read
  on demand by a tool call, so a stale count is not load-bearing; `mcp get` is a one-shot inspection
  that connects, reads and disconnects. (Resource *templates* are a separate gap.)
- Server-initiated requests (`sampling`, `elicitation`, `roots`). The handler leaves rmcp's defaults
  for those, so they are refused the way they are today.
- Any change to how tools are ordered or named, and to `tools/list` pagination.

## 4. Observability

One `AgentUpdate::Info` line per server whose list moved, naming the old and new counts. A server that
was asked and failed gets its own line naming the reason.

## 5. Tests

- A real in-process rmcp server whose `call_tool` adds a tool and calls `notify_tool_list_changed()`:
  the client picks the new tool up (polled with a deadline — the notification arrives on the service
  task, so it is genuinely asynchronous), and the rebuilt specs and `declared_read_only` reflect it.
- A quiet server: `refresh_tools_if_stale` returns `false` and the list is untouched.
- The entry filter and `declared_read_only` are re-derived from the new list, not the old one.
- The router reports `before → after` for a changed server only, and keeps the previous list when the
  re-list fails.
- The agent's next request offers the newly added tool.

## 6. Docs sync

- `book/08_chapter_mcp.md` + `_zh.md`: Step 10 (Notifications) gains the handling, Step 4 the refresh
  point, and the §10 gap row is replaced.
- `book/26_chapter_issue.md` + `_zh.md`: newest-first entry.
