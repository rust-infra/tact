# MCP `env_vars` and `tool_timeout_sec` — two more Codex fields honoured

Status: **approved** (remaining TODO; both fields were previously parsed only to be reported).

## Problem

`unmodelled_keys` names the Codex per-entry fields Tact ignores. Two of them cost the user real
functionality:

1. **`tool_timeout_sec`.** A single `tools/call` is bounded by one fixed ceiling
   (`MCP_CALL_TOOL_TIMEOUT`, 600s) for every server. A server with one genuinely long tool forces
   the user to accept 600s for *all* of them; a server whose tool should fail fast cannot ask for
   that either. The field is documented by Codex and was already being reported, so Tact looked
   like it honoured a budget it never read.
2. **`env_vars`.** Tact reads credentials out of `env`, a literal map — so a token has to be
   written into `.mcp.json` to reach a server. That makes the file unshareable and puts a secret on
   disk. Codex's `env_vars` copies named variables out of the host environment instead, and 0.157
   accepts both `["TOKEN"]` and `[{"name": "TOKEN", "source": "local"}]`.

## Design

### `env_vars`

Modelled as an untagged `McpEnvVar` (`Name(String)` | `Explicit { name, source }`), so both Codex
spellings parse. Resolution happens in `McpClient::connect`, **before** the child is spawned:

- `source` absent or `"local"` → the variable is read from Tact's own environment.
- `source: "remote"` → refused, naming the missing remote-stdio executor.
- any other `source` → refused, naming the allowed set (`local` / `remote`).
- the variable is not set → this server fails with `env var \`X\` is not set`.

Failing rather than skipping is the point: a server that starts without the credential it expects
fails later, somewhere unrelated, with an authentication error. Codex makes the same choice. The
failure is per server, so the load report already has a channel for it — one broken server still
does not stop the agent.

Precedence: a literal `env` entry of the same name wins and the `env_vars` entry is skipped (with a
debug log). A value someone wrote down meant it; a pass-through must never override it.

`env_vars` is a **stdio** field. A remote (`url`) entry has no child process, so declaring it there
is added to the `unmodelled_keys` report — the existing mechanism for "parsed, deliberately not
honoured here".

### `tool_timeout_sec`

`McpProjectConfig.tool_timeout_sec` → `McpServerPolicy::tool_timeout()` →
`McpClient::call_tool` uses `policy.tool_timeout().unwrap_or(MCP_CALL_TOOL_TIMEOUT)`. The timeout
error names the effective budget, so a custom value and the default are distinguishable from the
message alone.

Tact keeps its own default (600s) rather than adopting Codex's, for the same reason it keeps 60s
for the handshake: an entry that works today must not start failing after an upgrade.

### Reporting

`tool_timeout_sec` leaves the unmodelled set entirely. `env_vars` joins it only for a remote entry.
The doc comments on `UnmodelledKeys` and `unmodelled_keys` are reworded to say both things: a field
Tact does not implement, and a field Tact implements somewhere this entry cannot use.

## Non-goals

- **No `omit_tools_from`.** It is Codex's code-mode concept; Tact has no deferred-tool layer, so
  there is nothing to map it onto.
- **No remote stdio executor.** `source: "remote"` stays refused by name rather than approximated.
- **No `${VAR}` interpolation in config values.** `env_vars` is an explicit allowlist; a template
  engine over every value is a different, and riskier, feature.
- **No per-tool `tool_timeout_sec`** (`tools.<name>.tool_timeout_sec`): Codex declares the timeout
  per server.

## Verification

- `mcp::` tests: both `env_vars` spellings parse; `local` resolves from the process environment; a
  literal `env` entry wins; an unset variable fails naming itself; `remote` and unknown sources are
  refused with their reasons; a remote entry's `env_vars` is reported as unmodelled while a stdio
  entry's is not; resolution is proven to run **before** the spawn (a nonexistent command still
  reports the missing variable, not a spawn failure); `tool_timeout_sec` sets the policy budget,
  defaults to `None`, and leaves the unmodelled set.
- The existing `codex_only_keys_are_reported_with_their_source` fixture now asserts
  `["omit_tools_from"]` only, and that the timeout it declares reached the policy.
