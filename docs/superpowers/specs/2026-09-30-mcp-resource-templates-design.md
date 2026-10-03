# Design: `resources/templates/list`

Date: 2026-09-30
Status: approved for implementation

Closes the remaining half of the gap in [Ch 8 §10](../../../book/08_chapter_mcp_zh.md): *"Resource templates
/ prompts | Resources are wired (`list_mcp_resources` / `read_mcp_resource`); `resources/templates/list`
is not, and neither are Prompts"*.

## 1. Problem

Tact exposes two of Codex's three native resource tools. The third,
`list_mcp_resource_templates`, is missing, and its absence is not neutral: a server whose resources are
*template*-addressed publishes nothing through `resources/list`, so it looks like a server with no
resources at all. Tact even says so out loud — the empty listing reads *"A server may still expose
resource templates, which Tact does not list yet"* — and then offers no way to find them. A URI
template is not guessable: `memory://{topic}` needs the placeholder vocabulary before the model can
substitute anything and call `read_mcp_resource`.

So a template-only server is a dead end of exactly the kind the previous change removed for tools.

## 2. Design

### 2.1 A third tool, Codex's name

`McpResourceTool` gains `Templates`, named `list_mcp_resource_templates`, with the same optional
`server` argument as `list_mcp_resources`. `McpResourceTool::ALL` becomes three, in the order
list → templates → read, so the listing that names a template is adjacent to the read that consumes
it. Everything downstream (resolution in `agent::tool_dispatch`, `stable_name`, `risk`,
`tool_resources_for`) follows the existing two entries; a template listing, like a resource listing,
is a **barrier** because it touches every configured server.

### 2.2 The listing teaches substitution

The rendered listing names each template's `uri_template` and says, once, that the `{…}` placeholders
must be filled before reading. Without that sentence a template looks like a URI that simply fails —
the model copies `memory://{topic}` verbatim and gets an error it cannot interpret. `mime_type` and
`description` are carried through the same way `render_resource_listing` carries them, and a
multi-line description is flattened so it cannot break the bullet.

### 2.3 The empty listing stops lying

`render_resource_listing` currently tells the model that templates "are not listed yet". That sentence
becomes false the moment this ships, so an empty `resources/list` now names
`list_mcp_resource_templates` as the next call instead. The old wording is deleted rather than left as
a stale note.

### 2.4 Reporting

`McpServerInspection` gains `resource_templates: Option<usize>`, for the same reason `resources`
exists: "publishes no templates" and "did not answer `resources/templates/list`" are different facts
about a server, and `mcp get` is where a user finds out which one they have.

### 2.5 Service surface

`McpService::list_resource_templates()` is **required**, not defaulted — mirroring `list_resources`.
A defaulted method returning an empty list would make "this transport cannot ask" indistinguishable
from "the server publishes none", which is the one distinction the resources work was careful about.
`RealMcpService` delegates to rmcp's `list_all_resource_templates` (already paginated to the end);
`MockMcpService` gains `with_resource_template` and answers honestly.

## 3. Non-goals

- **Prompts** (`prompts/list`, `prompts/get`). Still unimplemented, and the reason stands: a prompt
  template has no consumer in Tact's turn structure — it is a server-authored message sequence, not a
  tool call. The gap row keeps saying so, now naming Prompts alone.
- Resource *subscriptions* (`resources/subscribe` + `notifications/resources/updated`).
- Any change to `read_mcp_resource`, including the `{…}` substitution itself: Tact reports the
  template, the model performs the substitution. That keeps the read path byte-exact and avoids
  inventing a URI-expansion rule the spec does not define here.

## 4. Tests

- `McpResourceTool::ALL` is three, in order, and every name round-trips through `from_name`.
- The tools appear only while a server is connected, and `list_mcp_resource_templates` is among them.
- A listing carries the `uri_template`, its `name`, its `mime_type`, a flattened `description`, and the
  substitution instruction.
- An empty `resources/list` names `list_mcp_resource_templates` and no longer claims templates cannot
  be listed.
- The router lists templates from a connected server, and an unknown server names the connected ones.
- A template-only server no longer looks empty end to end: `list_mcp_resources` returns the "no
  resources" line, and `list_mcp_resource_templates` returns the template.
- `mcp get` distinguishes "no templates" from "did not answer".
- `agent::tool_dispatch` executes the templates tool.

## 5. Docs sync

- `book/08_chapter_mcp_zh.md`: Step 9 gains templates, the code map and Quick Reference rows
  follow, and the §10 gap row is narrowed to Prompts.
- `book/26_chapter_issue_zh.md`: newest-first entry.
