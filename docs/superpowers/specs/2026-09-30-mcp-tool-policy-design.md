# MCP server tool policy — honour Codex's per-entry fields

Status: **approved** (user asked for A+B: fix the local config, then implement the Codex
per-server fields Tact currently only reports as unmodelled).

## Problem

Tact reads Codex-compatible `.mcp.json` entries and **names** the per-entry fields it does
not model (`enabled_tools`, `omit_tools_from`, `startup_timeout_sec`, `tools`) in `mcp list`
and in the log. Naming them is honest, but three of them are pure loss for the user:

1. **Token cost.** Every tool a server exposes is sent on every request. Basic Memory
   exposes 21 tools ≈ 34.6 KB of JSON schema ≈ 8.7k tokens; a user who needs the seven
   recall tools still pays for `delete_project`, `schema_diff`, `create_memory_project`, …
2. **Startup failure.** The stdio handshake timeout is the fixed `MCP_INIT_TIMEOUT` (60s,
   `crates/tact/src/mcp/mod.rs`). A cold `uvx basic-memory mcp` measured ~100s, so the
   server is reported as failed with no way to say "this one is slow".
3. **Approval friction.** `normalize_mcp_capability` returns `CapabilityRisk::High` for
   every MCP tool, so a read-only recall tool prompts on every call. The only escape today
   is a `settings.json` allow rule naming each tool.

## Design

### Fields modelled (Codex semantics, snake_case keys as Codex writes them)

| Field | Type | Behaviour |
|---|---|---|
| `enabled_tools` | `[string]` | Allow list. When present, only these tool names are exposed. |
| `disabled_tools` | `[string]` | Deny list, applied **after** `enabled_tools` (Codex's documented order). |
| `startup_timeout_sec` | number | Overrides the handshake timeout for this server. |
| `startup_timeout_ms` | number | Codex's millisecond alias; `sec` wins when both are present. |
| `default_tools_approval_mode` | `auto \| prompt \| approve` | Server-wide default approval behaviour. |
| `tools.<tool>.approval_mode` | `auto \| prompt \| approve` | Per-tool override, wins over the default. |
| `tools.<tool>.output_token_limit` | number | Per-tool result budget; an oversized result is spilled to disk with a preview. |

Because `McpProjectConfig` is `rename_all = "camelCase"` with `#[serde(flatten)] extra`,
each Codex field needs an explicit `#[serde(rename = "…")]`; the names stay snake_case
exactly as Codex writes them (and as the existing `unmodelled` test fixture writes them).

### Approval modes

Tact has three risk buckets (`Read` / `Write` / `High`), and only `Read` is auto-allowed —
but `Read` is also allowed **before** plan mode is consulted, so mapping a server-policy
`auto` onto `Read` would silently let a write tool run in plan mode.

Instead the policy is a **separate axis**: `PermissionManager::check_with_auto(…, auto_approved)`
consults it after the deny rules, after plan mode, and after `settings.json` allow rules:

```
1. Read                          → allow
2. Plan mode                     → deny          (a server policy must not unlock plan mode)
3. Auto mode                     → allow
4. settings.json deny            → deny          (an explicit local denial always wins)
5. settings.json allow           → allow
6. server policy auto            → allow         ← new
7. High risk                     → ask
8. in-session always-allowed     → allow
9. otherwise                     → ask
```

Only `auto` changes behaviour. `prompt`, `approve`, an unknown value, and an absent field
all keep today's `High` → ask, so the default is unchanged; an unknown value is logged.

### Measurement is approximate, deliberately

`output_token_limit` counts tokens with the same `approx_text_tokens` estimator the
compaction path already uses, and spills through the existing `.tact/tool-results` mechanism
(unconditional-spill helper extracted from `persist_large_output`). It is documented as
Tact's interpretation: the field is present in Codex's binary (`McpServerToolConfig`) but is
**not** in Codex's published configuration reference, so there is no behaviour to copy.

### Reporting stays honest

Hiding a tool is the same class of silence `unmodelled` exists to prevent, so filtering is
reported, never silent:

- `McpLoadReport.filtered: Vec<(server, hidden tool names)>`, filled from the connected client.
- `mcp list` prints a **Filtered tools** section (the shape of *Overridden declarations*).
- `mcp get` prints one line for the inspected server.
- `filtered` is deliberately **not** part of `is_quiet()`, so a deliberate configuration does
  not turn every startup into a notice.

`omit_tools_from` and `tool_timeout_sec` stay unmodelled and reported: the first is Codex's
code-mode concept with no counterpart here, the second is a per-call timeout Tact does not
yet thread through.

## Non-goals

- No per-tool **risk** setting: `auto` is an explicit auto-approve, not a re-classification,
  so `mcp list`, hooks, and the TUI keep reporting `High`.
- No `omit_tools_from` (no code-mode / deferred-tool concept in Tact).
- No change to Tact's 60s default handshake timeout: Codex's default is 10s, and adopting it
  would break slow servers that work today. Only the per-server override is added.
- No new CLI flags on `mcp add`: the fields are written by hand in `.mcp.json` (as in Codex).
- No change to how plugin-contributed servers are named or merged.

## Verification

- Unit tests: filter order (`disabled_tools` beats `enabled_tools`), timeout parsing
  (`sec` beats `ms`), per-tool override beats the server default, unknown mode ignored,
  hidden tools reported.
- Permission tests: `auto` allows in default mode; still denied in plan mode; a `deny` rule
  still wins; `prompt` is unchanged.
- Compaction tests: a result over the per-tool budget spills, one under it does not.
- End-to-end: `tact-ui mcp get basic-memory` against a real `enabled_tools` entry shows the
  filtered list and the hidden names.
