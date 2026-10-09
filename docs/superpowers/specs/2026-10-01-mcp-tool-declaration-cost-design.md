# `mcp get` prices each tool's declaration in the request

Status: **shipped** (`baa1a4f9`, 2026-10-01).
Retroactive record, written 2026-10-02 from the shipped code, that commit's message, and the
Ch 26 entry.

## Problem

Every MCP tool re-sends its name, description and input schema on **every request**, and the only
lever against it (`enabled_tools`) had no feedback: `mcp list` said "21 tools", never what that
costs. Trimming the tool list was a decision made blind.

Measured before choosing anything: Basic Memory's 21 tools are **34.6 KB ≈ 8.6k tokens per request**,
while collapsing the pydantic-style parts of the schemas (`anyOf: [X, null]` → `X`, drop `title`,
drop `default: null`) saves **2%**.

## Design

### Visibility, never auto-trimming

The 2% figure is the decision. The bytes are legitimate structure, and the real lever is *how many
tools are declared* — so the fix is to make that number visible, not to shave the schemas.
Auto-trimming is a capability loss, and which tools belong to a workflow is the user's call. Nothing
here changes what is sent.

### Where the number comes from

`derive_exposed` measures each tool's payload **as it rebuilds the specs that are actually sent** —
not from the raw server response, so the number describes the request rather than the server's
self-description of it. `McpServerInspection` carries the per-tool bytes to the CLI, sorted by tool
name, and is empty when the server is not connected.

### How it is shown

`render_server_detail` sums the per-tool bytes into a per-request estimate, appends each tool's own
size, and names `enabled_tools` on the header line — so the lever sits next to the number it acts on.

Two formatters, both deliberate:

- `format_bytes` renders `34.6 KB` — one decimal, because the difference between 34.6 and 35 is not
  the point and the difference between 4 and 34 is.
- `format_tokens` renders `≈8.6k tokens`, labelled an **estimate** at four bytes per token. The real
  count comes from the provider (`ctx` in the status bar); this exists only to rank servers and tools
  against each other.

### Bundled in the same commit

The `suggested risk policy` draft's own test and the Ch 08 prose describing the paste-ready block had
been left uncommitted, and land here. The draft lists what the server claimed read-only and nothing
else, so pasting it can only ever make things stricter than the claim, never looser — see
[2026-09-30-mcp-resource-tool-risk-design.md](2026-09-30-mcp-resource-tool-risk-design.md) for the
policy it drafts.

## Non-goals

- **No schema compaction.** Measured at 2%; rejected on the number, not on taste.
- **No automatic `enabled_tools` selection.** The estimate makes the choice informed; it does not
  make it.
- **No tokenizer.** Four bytes per token, and the output says so.

## Verification

- `crates/tact_ui/src/mcp_cli.rs`: `the_detail_view_reports_what_the_tools_cost` ("21 tools" says
  nothing about what a server costs), `the_detail_view_drafts_a_risk_policy_from_the_servers_own_claim`
  (the draft is a suggestion and cannot loosen anything).
- `crates/tact_extensions/src/mcp/mod.rs`: the measurement rides on `derive_exposed`, covered through the detail
  view rather than by a direct unit test.
- Docs: [Ch 08](../../../book/08_chapter_mcp_zh.md), [Ch 26](../../../book/26_chapter_issue_zh.md)
  2026-10-01 entry.
