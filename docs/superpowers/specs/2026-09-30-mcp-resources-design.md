# MCP resources — `list_mcp_resources` / `read_mcp_resource`

Status: **approved** (remaining TODO; pairs with the server-instructions change).

## Problem

Tact wired exactly one MCP primitive: Tools. Resources — read-only content addressed by URI — were
listed in the chapter's gap table and nowhere else.

That gap is load-bearing for the integration this work exists to improve. A server that publishes
its real content as a resource tells the model to go read one, and Basic Memory does exactly that:
its `InitializeResult.instructions` say *"read the `memory://ai_assistant_guide` resource"*. After
the instructions change, Tact faithfully delivers that sentence and then offers no way to act on
it — a dead end that is worse than the silence it replaced, because the model now knows it is
missing something.

## Design

### Two native tools, not `mcp__…` entries

A resource is not a tool: it has no input schema and is addressed by a URI, so it cannot be one
`mcp__<server>__<tool>` entry per resource. Tact follows Codex, which exposes resources as native
tools:

| Tool | Arguments | Behaviour |
|---|---|---|
| `list_mcp_resources` | `server` (optional) | `resources/list` for every connected server, or one |
| `read_mcp_resource` | `server`, `uri` (required) | `resources/read`, rendered for the model |

Both are resolved in `agent::tool_dispatch`, in the same place `mcp__…` names are: `McpResourceTool`
is a new [`ResolvedTool`] variant, so the permission check, the step card, and the scheduling
decision all flow through the existing path. Risk is `CapabilityRisk::High` — the content is
third-party, and a listing touches every server the user configured.

### Lifetime and scheduling

`MCPToolRouter::resource_tool_specs()` returns nothing when the router is empty. Advertising the
names unconditionally would hand the model a tool whose only possible answer is "no MCP servers
are connected". `rebuild_cached_tool_specs` chains them, so `/mcp auth` gains them the same way it
gains a server's tools.

`read_mcp_resource` is scoped to the server it names (same-server calls stay serial); a listing is
a barrier, because it may touch all of them.

### Rendering

`render_resource_listing` prints one `## <server>` section per server, `- <uri> — <name> [(mime)]`,
plus the description on an indented line with newlines flattened so a multi-line description cannot
break the bullet. URIs are the thing the model must copy, so they come first on the line.

An empty listing says *why* it might not be the whole truth: a template-only server exposes no
concrete resources, so the message names `resources/templates/list` as unread rather than letting
the server look empty.

`render_resource_contents` returns text as-is and reports a blob by its base64 length instead of
inlining it: unreadable to the model, expensive to carry. Empty contents are stated, not rendered
blank.

### Errors

- Unknown `server` names the connected ones, so a typo is distinguishable from an empty server.
- An empty router is an error from `list_resources`, not an empty listing.
- `read_mcp_resource` without `server`/`uri` fails with the missing argument named, before any
  lookup.
- The mock returns a real `resource_not_found` for a missing URI, so "not found" and "empty" can
  never be confused in a test.

### Reporting

`mcp get <server>` gains `resources  N available to \`list_mcp_resources\``, or
`(the server did not answer \`resources/list\`)` when the request failed. "Publishes none" and
"cannot answer" are different facts, and only the first is about the server's contents; a server
that was never contacted (pending authorization, disabled, failed) prints neither.

## Non-goals

- **`list_mcp_resource_templates`.** Codex has it; Tact does not. A URI template can be read once
  its URIs are known, and the concrete listing is the part that goes stale fastest. The empty
  listing names the gap.
- **Prompts.** `prompts/list` / `prompts/get` have no consumer in Tact's turn structure.
- **Subscriptions** (`resources/subscribe`), so a resource can change mid-session.
- **Auto-injection.** A resource is fetched only when the model asks, which keeps a 30 KB guide
  from arriving in every conversation.

## Verification

- `mcp::resource::tests`: tool existence gated on a connected server; `read` declares both required
  arguments; name round-trip; listing wording (URIs, mime, flattened description) and the empty
  case; blob not inlined; empty contents stated; a real round trip through the router; unknown
  server names the connected ones; an empty router refuses.
- `mcp::tests::mcp_client_reads_resources_from_a_real_in_process_server`: the rmcp call shapes
  (`list_all_resources`, `read_resource`) against a real `ServerHandler`.
- `agent::tool_dispatch::tests`: the two tools produce `Success`/`Failed` results, and the argument
  guards name the missing field.
- `agent::tests::the_resource_tools_are_offered_only_with_a_connected_server`: present in
  `all_tool_specs` with a server, absent after the router is emptied.
- `mcp_cli::tests::the_detail_view_separates_no_resources_from_no_answer`.
- End to end: `tact-ui mcp get basic-memory` reports `resources  1 available`.
