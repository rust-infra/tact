# Design: `prompts/list` and `prompts/get`

Date: 2026-10-02
Status: approved for implementation

Closes the last row of [Ch 8 §10](../../../book/08_chapter_mcp_zh.md). The predecessor spec
([`2026-09-30-mcp-resource-templates-design.md`](./2026-09-30-mcp-resource-templates-design.md) §3)
declared Prompts a non-goal with a reason: *"a prompt template has no consumer in Tact's turn
structure — it is a server-authored message sequence, not a tool call."* This spec does not overturn
that sentence so much as notice what it was scoped to.

## 1. Problem

That reason is about **pushing** prompts at the model, which Tact still does not do. It says nothing
about a **tool result**. `get_mcp_prompt` returns the composed messages as text, and the model reads
them exactly the way it reads a resource. The "no consumer" objection dissolves at the moment the
primitive becomes a tool the model can call — which is the same move the resources work already made
for a primitive the MCP spec calls application-driven.

The cost of leaving it out is not neutral, and it is measurable. A probe of the server this branch
has been testing against (`initialize` + the four list calls over stdio, `basic-memory mcp`):

| Primitive | Advertised | Tact consumed before |
|---|---|---|
| `tools/list` | 21 | yes |
| `resources/list` | 1 | yes |
| `resources/templates/list` | 1 | yes |
| `prompts/list` | **4** | **no** |

Its `capabilities` declares `prompts: {listChanged: false}` — so this is not a server that might have
prompts. It has four, named `continue_conversation`, `getting_started`, `recent_activity` and
`search_knowledge_base`, and `tact-ui mcp get basic-memory` said nothing about them. A model told to
start with `getting_started` had no way to fetch it, exactly as it had no way to read
`memory://ai_assistant_guide` before resources were wired.

## 2. Design

### 2.1 Two tools, the resource pair's shape

`crates/tact_extensions/src/mcp/prompt.rs` mirrors `resource.rs`: an `McpPromptTool` enum (`List` → `Get`),
`LIST_PROMPTS_TOOL` / `GET_PROMPT_TOOL` constants, `ALL` / `from_name` / `name` / `spec`, and the same
`MCPToolRouter` methods. Everything downstream follows the existing entries — resolution in
`agent::tool_dispatch` (`is_mcp_prompt_tool`, `run_mcp_prompt_tool`), `stable_name`, `risk`,
`tool_resources_for`.

The names are the resource pair's convention (`list_mcp_resources` / `read_mcp_resource`) applied to
the second primitive; there is no third-party name being copied here.

Scheduling splits the same way: `get_mcp_prompt` is scoped to the server it names, a listing spans
every connected server and is a **barrier**.

Both tools exist only while a server is connected (`MCPToolRouter::prompt_tool_specs`), for the reason
the resource trio does: with an empty router the names are unresolvable, so the model is never handed
a tool whose only possible answer is "no MCP servers are connected".

### 2.2 The listing must show the argument vocabulary

A prompt's `arguments` are what make it a template rather than a fixed string. The rendered listing
carries each argument name and marks the required ones — `arguments: topic (required), project`. A
placeholder the model cannot see is one it will not fill, and `get_mcp_prompt` would then reach the
server with an unfilled template. A prompt with no arguments gains no argument line at all.

### 2.3 Messages render by role, payloads by size

`render_prompt_messages` emits one `## user` / `## assistant` section per message **in the server's
order** — the order is the instruction, so it is never sorted or merged. Images and embedded
resources are reported by size rather than inlined, reusing `render_resource_contents` for the
embedded case, for the reason `read_mcp_resource` already gives: a base64 payload is unreadable to the
model and expensive to carry. A `resource_link` is reported as its URI and name.

### 2.4 Arguments: stringify scalars, refuse the rest by name

The protocol's prompt arguments are a **string** map. A number or a bool is stringified — a model that
writes `{"count": 3}` means `"3"` — but an object, array or null is refused by name:
*"argument \`topic\` must be a string"*. Silently dropping it would reach the server as an unfilled
placeholder and still report success, which is the failure mode this whole section exists to prevent.
An absent or empty `arguments` object is `None`, not an empty map: they are the same request.

### 2.5 Risk: two more `[mcp]` keys

`mcp.prompt_list_risk` and `mcp.prompt_get_risk`, on the resource keys' terms: these names belong to
Tact, so no server entry's `tools.<name>.risk` can address them; both default to `High`, and so does
an unresolvable config, so the unconfigured path is the restrictive one. Two keys rather than one
because a listing returns metadata while a get returns server-authored content — declaring `read` for
the get key bypasses plan mode, the same trade `read_mcp_resource` and the native `read_file` make.

### 2.6 Reporting

`McpServerInspection` gains `prompts: Option<usize>`, and `mcp get <server>` prints
`prompts  N available to \`list_mcp_prompts\``, or `(the server did not answer \`prompts/list\`)`.
This is the only place prompts are visible without the model asking: without the line, a server's four
prompts are invisible to the user too.

### 2.7 Service surface

`McpService::list_prompts()` and `McpService::get_prompt(params)` are **required**, mirroring
`list_resources` / `read_resource`. A defaulted method returning an empty list would make "this
transport cannot ask" indistinguishable from "the server publishes none". `RealMcpService` delegates
to rmcp's `list_all_prompts` (already paginated) and `get_prompt`. `MockMcpService` gains `with_prompt`
/ `with_prompt_messages`, records every `(name, arguments)` pair it receives — the arguments are the
part a mock must be able to prove — and treats a prompt it *advertised* as gettable, so "not
advertised" and "composed nothing" stay distinguishable.

### 2.8 The timeout constant is renamed, not duplicated

`MCP_RESOURCE_TIMEOUT` becomes `MCP_FETCH_TIMEOUT` and covers `resources/*` and `prompts/*`. The four
requests are the same kind of wait — a listing from the server's own index, or fetching one item — and
a name per primitive would only invite the ceilings to drift apart for no reason.

## 3. Non-goals

- **Prompts as TUI slash commands.** The MCP spec calls prompts *user*-controlled, so a slash command
  is arguably the spec-faithful shape, and Codex-style clients do surface them that way. Deferred, not
  rejected: this change answers "the model cannot reach them at all" without committing to a second
  user-facing surface (completion, i18n, persistence) in the same diff.
- **`prompts/list_changed`.** Still unhandled, and now for prompts the same reason it is unhandled for
  resources: prompts are fetched on demand by a tool call, so a stale count is not a correctness
  problem. Every server we can check declares `listChanged: false`.
- **Prompt *subscriptions*** — not a thing in this protocol version.
- **Pushing a prompt into the conversation automatically.** Tact does not; the model calls a tool.

## 4. Tests

- `McpPromptTool::ALL` is two, in order, and every name round-trips through `from_name` (and does not
  collide with a resource name).
- The tools appear only while a server is connected.
- A listing carries the prompt name, its description and its argument vocabulary, with `(required)`
  marked; a prompt with no arguments gains no argument line.
- Messages render in order with their roles; an image payload is reported by size and not inlined;
  empty messages and an empty listing are stated, not rendered blank.
- The router lists and gets a real prompt; an unknown server names the connected ones; an empty router
  refuses rather than reporting "no prompts"; a prompt the server never listed is an error, not an
  empty result.
- The arguments reach the server, and a non-string argument is refused by name.
- `agent::tool_dispatch` executes both tools and names a missing `server` / `name`.
- **A real in-process rmcp server** (`EchoServer`) advertises a prompt and echoes an argument back, so
  the rmcp call shapes and the argument round-trip are proven over a wire, not only against the mock.
- `mcp get` distinguishes "no prompts" from "did not answer".
- Risk: both keys default to `High` and move independently of each other and of the resource keys.

## 5. Docs sync

- `book/08_chapter_mcp_zh.md`: Step 9 gains the two tools, the prompts paragraph that answers the old
  non-goal, the four risk keys, the code map row, and the §10 gap row is deleted.
- `config.example.toml`: the two keys beside the resource pair.
- `book/26_chapter_issue_zh.md`: newest-first entry.
