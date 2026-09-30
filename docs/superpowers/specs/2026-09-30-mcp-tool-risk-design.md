# Design: MCP tool risk granularity

Date: 2026-09-30
Status: approved for implementation

Closes the gap recorded in [Ch 8 §10](../../../book/08_chapter_mcp.md): *"Every MCP tool resolves to
`CapabilityRisk::High`; `normalize_mcp_capability` ignores both server and tool. An entry can skip
the **prompt** with `approval_mode: "auto"`, but it cannot lower the reported **risk** — that needs a
capability axis Tact does not have."*

The previous change (`2026-09-30-mcp-tool-policy-design.md`) deliberately stopped at the prompt axis,
for a reason this design must not undo: `Read` is honoured **before** plan mode is consulted, so
mapping an entry's policy onto a lower risk would let a config entry unlock plan mode. Everything
below respects that invariant.

## 1. Problem — one gap, three separable facets

### Facet A — `High` is a dead end, and the UI already pretends otherwise

`PermissionManager::check_with_auto` returns `Ask` for `risk == High` **before** it consults the
session allow-list, so an `always_allowed_tools` entry can never be reached for a High tool. The TUI
does not know this: `agent::tool_dispatch` offers every tool the three options
`Allow once` / `Deny` / **`Always allow this tool`**, and the third resolves to
`allow_tool_with_input`. When no settings store exists — Tact started without a project path — that
function writes the generated rule into the in-memory list only, which step 7 then refuses to read.
The user's click is silently discarded and the next identical call asks again.

With a settings store present the click *does* work, because it persists a project allow rule and
step 4–6 honours `RuleAction::Allow` at every risk. So the observable bug is: **the same gesture works
or does nothing depending on whether Tact has a project path**, and when it does nothing there is no
feedback at all.

This facet is a bug, not a design gap, and it is independent of facets B and C.

### Facet B — there is no per-tool risk axis to declare

`normalize_mcp_capability(_server, _tool)` ignores both arguments. The consequence is not cosmetic:
in a non-interactive run `ask_user` answers `Deny` for `High`, so **every MCP call is refused** in
headless mode unless the entry opted into `approval_mode: "auto"` — a switch that is all-or-nothing
per tool and whose only other effect is to skip the prompt. There is no way to say "this recall tool
is a read; trust it" without also giving up plan mode for it.

`McpToolConfig` has no `#[serde(flatten)] extra`, so an unknown key under `tools.<name>` is dropped
by serde without a word — a `risk` field added to a user's file today would be invisible, not even
reported by `unmodelled_keys` (which only sees `McpProjectConfig::extra`).

### Facet C — the server's own read-only declaration is discarded

MCP defines `Tool.annotations` (`rmcp` 0.17 `model/tool.rs`): `read_only_hint`, `destructive_hint`,
`idempotent_hint`, `open_world_hint`. `build_tool_specs` reads only `name`, `description` and
`input_schema`, so Tact throws the rest away. That is information the user could use — but it is a
third party's self-description, so it must never be authority.

## 2. Design

### 2.1 Facet A — `High` consults the allow-list, and still asks first

`check_with_auto` gains one branch: a High-risk call whose exact tool **and input** the user already
allowed this session is allowed; otherwise High still asks.

What this does **not** change:

- Plan mode is still evaluated first, so a High tool is still blocked there.
- `Auto` mode still short-circuits earlier.
- An explicit `deny` / `ask` rule still outranks everything.
- `ask_user` (non-interactive) still denies High, because the session list is empty in a fresh
  headless run — `always_allowed_tools` is seeded with `read_file` only, never from settings.
- A bare tool name in the list still grants every input. That is pre-existing behaviour of
  `allow_tool` and out of scope; the TUI path writes an input-aware rule.

Order after the change: Read → Plan → Auto → settings Deny/Allow/Ask → server `auto_approved` →
**High + allow-listed → allow** → **High → ask** → session allow-list → default ask.

The High branch is written as an early return that consults the list, rather than by moving step 8
above it, so that a High tool can never fall through to the "default ask" tail with a stale
`permission_label`.

### 2.2 Facet B — a Tact-private `risk` axis, three declared tiers

Two new keys, both **Tact's own** (Codex has no equivalent; a Codex config that lacks them is
unaffected, and one that has them is a Tact config):

```jsonc
{
  "command": "uvx",
  "default_tool_risk": "write",          // entry-level default for tools not named below
  "tools": {
    "search_notes":  { "risk": "read" }, // per-tool override, wins over the default
    "delete_project": { "risk": "high" } // explicit, even if the default is lower
  }
}
```

Contract, spelled out because the tiers are not "levels of the same knob":

| Declared | Plan mode | Default mode | Non-interactive | Session allow-list |
|---|---|---|---|---|
| `read` | **allowed** (this is the plan-mode bypass — declare it only for a genuinely read-only tool) | allowed, never asks | allowed | n/a |
| `write` | blocked | asks once, then the user's allow decision sticks | allowed once | honoured |
| `high` (default when nothing is declared) | blocked | asks; after facet A, an explicit allow sticks | **denied** | honoured |

`write` is the recommended tier for a tool the user wants usable unattended: it buys headless
availability and a sticky allow without touching plan mode. `read` is offered because a genuinely
read-only tool is the one case where bypassing plan mode is defensible — the same trade the native
`read_file` makes — but it is never inferred.

Placement: `ToolRisk` lives in `mcp/mod.rs` beside `ApprovalMode`, parsing the config strings, and
converts to `CapabilityRisk`. `McpServerPolicy::risk_for(tool) -> Option<CapabilityRisk>` resolves
override-then-default. `MCPToolRouter::risk_for(server, tool) -> CapabilityRisk` mirrors the existing
`is_auto_approved`, and defaults to `normalize_mcp_capability` — the free function stays as the
single named place that says "no declaration means High".

An unrecognised `risk` value is warned about and ignored, exactly like `parse_approval_mode`, so one
typo cannot fail a whole entry.

`risk` and `default_tool_risk` are **modelled**, so they never appear as unmodelled keys.

### 2.3 Facet C — annotations are evidence, displayed, never applied

`McpClient` keeps the set of exposed tools whose `annotations.read_only_hint == Some(true)`, exposed
as `declared_read_only()` and surfaced on `McpServerInspection` so `mcp get <server>` can mark those
tools `(server-declared read-only)`.

It does **not** feed `risk_for`. Two reasons, both load-bearing:

1. It is a self-description by the server, and a server that lies is exactly the case a permission
   system exists to survive.
2. Even an honest `read_only_hint: true` can be an exfiltration path when combined with
   `open_world_hint: true`: a tool that reads your files and sends them to a remote endpoint is
   read-only from the permission angle and an egress from the data angle. Tact has no data-flow axis
   to express that, so it must not silently accept the server's framing.

The display exists so the *human* can make the §2.2 declaration with evidence in front of them. A
test asserts the risk is unchanged when a server declares read-only.

## 3. Non-goals

- `list_mcp_resources` / `read_mcp_resource` keep `CapabilityRisk::High`. They are router-served, not
  entry-declared, so `tools.<name>.risk` cannot address them, and the URI they fetch comes from the
  model. Left as-is deliberately; Ch 8's gap table says so.
- No `${VAR}` interpolation, no `env_vars.source: "remote"`, no `omit_tools_from` — still unmodelled.
- No change to `ApprovalMode`: `auto` stays an auto-**approve** on the prompt axis, never a
  re-classification. The two axes now coexist rather than one substituting for the other.
- No data-flow/egress axis. Facet C's second reason above is the boundary of what this change fixes.

## 4. Observability

- `mcp get <server>` prints the effective risk per tool, and `(server-declared read-only)` where the
  server claims it — so an entry that declares a risk is never indistinguishable from one that
  silently kept the default.
- `mcp list`'s unmodelled-keys section is unchanged (the new keys are modelled).

## 5. Tests

- `permission`: an allow-listed High tool is allowed and does not ask; a High tool that is not
  allow-listed still asks; plan mode still blocks it; the non-interactive path still denies it.
- `mcp`: `default_tool_risk` applies and a per-tool `risk` overrides it; an unknown value warns and
  falls back to High; both keys are modelled (absent from unmodelled keys); a declared read-only
  annotation does not change `risk_for`; and the router resolves a declared tier while defaulting
  the rest (and an unknown server) to High — the dispatch site itself is a pure delegation, so it is
  covered there rather than by standing up an agent for it.
- `mcp_cli`: the detail view marks a server-declared read-only tool and separates `(declared)` from
  `(default)` risk.

## 6. Docs sync

- `book/08_chapter_mcp.md` + `_zh.md`: field-table rows, a subsection under "Per-server tool policy"
  for the risk axis and the plan-mode table, and the §10 gap row replaced by an accurate one.
- `config.example.toml`: the `[mcp]` comment block gains both keys with the tier warning.
- `book/26_chapter_issue.md` + `_zh.md`: two newest-first entries — the facet-A bugfix, and the
  facet-B/C feature.
