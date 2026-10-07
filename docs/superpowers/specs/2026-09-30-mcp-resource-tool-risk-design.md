# Design: declare the risk of Tact's own resource tools

Date: 2026-09-30
Status: approved for implementation

Closes the row the resource work left in [Ch 8 §10](../../../book/08_chapter_mcp_zh.md): *"The two
router-served resource tools (`list_mcp_resources`, `read_mcp_resource`) are **not** addressable this
way — they are not in any server's `tools` map — so they stay `CapabilityRisk::High`."*

## 1. Problem

`tools.<name>.risk` addresses a server's tools, and these three are not a server's: they are
Tact-native names served by the router, so no entry can declare them. They are `High` by construction
— third-party content, and a listing touches every configured server — which is a defensible default
and an unusable one: a user with a single trusted local server cannot say "listing is fine, reading is
not", so every listing prompts and a non-interactive run refuses them outright.

There are three of them now (`list_mcp_resources`, `list_mcp_resource_templates`,
`read_mcp_resource`), so the gap is bigger than when it was recorded.

## 2. Design

### 2.1 Two keys, because listing and reading are not the same act

`[mcp]` in `config.toml` gains:

```toml
[mcp]
# `list_mcp_resources` + `list_mcp_resource_templates`
resource_list_risk = "write"
# `read_mcp_resource`
resource_read_risk = "high"
```

A listing returns metadata — URIs, names, MIME types — and touches every server; a read returns
third-party **content** fetched from one. Collapsing them into one knob would force a user who is
comfortable enumerating to also accept whatever the content is, so they are separate.

`[mcp]` rather than a server entry is the whole point: these tools belong to Tact, not to a server,
which is exactly why `tools.<name>.risk` could not reach them.

### 2.2 The same vocabulary, and the same default

Values are `read` | `write` | `high` — `ToolRisk`, the vocabulary `tools.<name>.risk` already uses, so
there is one way to talk about tool risk in this project. Default is `high` for both, so this change
is invisible until someone declares otherwise. An unknown value is warned about and ignored, exactly
as an unknown `tools.<name>.risk` is: one typo must not fail the whole config.

The semantics are the existing tiers, unchanged: `read` is allowed in every mode **including plan
mode**, `write` blocks in plan mode and asks once, `high` asks and is denied in a non-interactive run.
Declaring `read` for `read_mcp_resource` therefore bypasses plan mode — the same trade the native
`read_file` makes — and the docs say so rather than leaving it to be discovered.

### 2.3 One named place

`mcp::resource_tool_risk(tool)` resolves a [`McpResourceTool`] to a [`CapabilityRisk`], reading
`config::try_settings()` and falling back to `CapabilityRisk::High`. It sits beside
`normalize_mcp_capability`, which is the same kind of single sayer for server tools, and it is what
`agent::tool_dispatch`'s `ResolvedTool::McpResource` arm calls instead of a bare `High`.

`try_settings()` returning `None` — a process that never resolved a config, which includes most tests
— yields `High`, so the unconfigured path is the restrictive one.

## 3. Non-goals

- Per-server granularity for the resource tools. `read_mcp_resource` targets one server, so a
  per-server tier would be finer; it would also need a rule for `list_mcp_resources`, which spans
  every server ("the strictest wins"?) and a second place for the declaration to live. One global
  pair is the whole of what the gap asked for.
- Any change to `ApprovalMode`, to `tools.<name>.risk`, or to the tool *descriptions*.
- A data-flow / egress axis, still: a declared `read` says the tool does not write, not that its
  content is safe to fetch.

## 4. Tests

- All three tools default to `High` with an empty `[mcp]` section.
- Each key applies to its own tools and leaves the other group alone.
- An unknown value is ignored and the tool stays `High`.
- `config` parsing of the two keys.

## 5. Docs sync

- `book/08_chapter_mcp_zh.md`: the §10 row is replaced, and the resource section names the
  keys.
- `config.example.toml`: the `[mcp]` block gains both keys.
- `book/26_chapter_issue_zh.md`: newest-first entry.
