# MCP Through the Executor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the last legacy permission path: MCP tools, MCP resource/prompt tools and their refresh go through `ToolExecutor`, and `legacy_authorize` is deleted.

**Architecture:** The `MCPToolRouter` becomes shared (`Arc<tokio::sync::Mutex<..>>`) and is exposed to the kernel as three `ToolSource`s — regular MCP tools, MCP control tools, and (unchanged) nothing else. The Agent registers them on the catalog it already holds, advertises the resulting specs, and dispatches every family through `resolve` → `authorize_policy` → `execute`. `default_approval_mode: auto` becomes a `PermissionService` input instead of a branch in dispatch.

**Tech Stack:** Rust 2024, `tact_kernel`, `tact`, `tokio` (sync), `async-trait`.

**Spec:** `docs/superpowers/specs/2026-10-25-plugin-first-architecture-design.md` (Phase 2)

## Why this needs its own change

`legacy_authorize` is the last hand-written permission ladder. It exists because
an MCP call has **no kernel descriptor**: its risk comes from `[mcp]` settings
(`tools.<name>.risk`, `default_tool_risk`), its "auto approve" bit comes from
`approval_mode`, and its resource budget comes from `output_token_limit`. The
adapters (`McpToolSource`, `McpControlSource`, `McpControlTool`) already exist
and are tested; what is missing is (a) shared ownership of the router, (b) making
the `[mcp]` policy reachable from a descriptor, and (c) advertising exactly what
the catalog resolves.

## Global Constraints

- Work on `feat/plugin-first-architecture`; never commit to `main`.
- No user-visible behaviour change: same prompts, same option order, same risk
  per tool, same `output_token_limit` handling, same refresh notices.
- `Agent.mcp_router` is `pub` and used by the TUI (`/mcp` commands), so its
  ownership change is the risky part; do it in one commit that compiles.
- One Cargo invocation at a time. Watch each test fail before implementing it.
- `scripts/check-rust.sh` must pass before the checkpoint commit. The module
  guard's `tool_dispatch.rs: mcp` allowlist entry must be deleted in the last
  task, not before.

## Review Focus

1. **Risk parity** — the descriptor's permission must equal what
   `mcp_router.risk_for` returned, including the `tools.<name>.risk` override and
   the `High` default.
2. **Auto-approve** — `approval_mode: auto` must still skip the prompt without
   lowering risk, so it cannot unlock plan mode.
3. **Output budget** — `output_token_limit` must still apply per server entry.
4. **Refresh** — a `notifications/tools/list_changed` must move the generation,
   the advertised specs, and the executor's resolvable set together.
5. **No double registration** — the same tool must not appear both as a native
   insert and as a source.
6. **Deadlock** — `McpToolBackend::tools()` uses `try_lock`; a refresh holds the
   lock across `await`.

## File Structure

Modify:
- `crates/tact/src/mcp/mod.rs` — `MCPToolRouter` shared accessors; expose the
  per-tool policy needed to build a descriptor.
- `crates/tact/src/tool/mcp_source.rs` — descriptor carries risk + output budget;
  backend over the shared router.
- `crates/tact/src/tool/mcp_control.rs` — same for the control tools.
- `crates/tact/src/permission/service.rs` — accept a per-tool "auto approved"
  predicate.
- `crates/tact/src/agent/mod.rs` — register the two sources after every MCP load
  and refresh; advertise from the catalog.
- `crates/tact/src/agent/tool_dispatch.rs` — delete `legacy_authorize`, the
  legacy popup constants, and the `ResolvedTool::Mcp*` variants.
- `scripts/check-module-boundaries.sh` — drop the `mcp` allowlist entry.

### Task 1: Share the MCP router

Rejected approaches, recorded so they are not retried:

- `#[derive(Clone)]` on `MCPToolRouter` — needs `McpClient: Clone`, and a cloned
  client would `shutdown` a service it shares with the original
  (`McpClient::shutdown` consumes `self`).
- `Arc<HashMap<..>>` for `clients` — breaks the three mutating methods
  (`register_client`, `refresh_changed`, `disconnect_all`).

Chosen: an `Arc<tokio::sync::Mutex<MCPToolRouter>>` owned by the Agent, with
**synchronous** read-only accessors added for the queries the prompt and specs
need (`instructions_block`, `all_tools`, `resource_tool_specs`,
`prompt_tool_specs`, `risk_for`, `is_auto_approved`, `output_token_limit`,
`server_summaries`) that `block_on` nothing and instead take the lock with
`try_lock`, falling back to "no servers" for the one prompt-building caller that
cannot await. Verify: the spec list and the prompt block must be identical before
and after, or the KV-cache prefix changes and every turn is slower.

- [ ] Add the `Arc<Mutex<..>>` field and the accessors; keep async call sites
      (`call`, `list_resources`, `get_prompt`, `refresh_changed`,
      `disconnect_all`, `read_resource`) taking the lock.
- [ ] Gate: `cargo test -p tact --lib mcp::` and `-p tact-ui` must pass unchanged.
- [ ] Commit.

### Task 2: Put the `[mcp]` policy on the descriptor

- [ ] `McpToolEntry` gains `risk: CapabilityRisk` and `output_token_limit:
      Option<usize>`; `McpTool::descriptor()` uses them instead of asking the
      backend on every call.
- [ ] Add `ToolPolicy::output_token_limit` (kernel) or carry it in
      `OutputPolicy::PersistLargeOutput { token_limit: Option<usize> }`.
- [ ] Test: a descriptor for `mcp__srv__t` has the risk the server entry declares,
      and `High` when it declares none.
- [ ] Commit.

### Task 3: Auto-approve through the permission port

- [ ] `ManagerPermissionService::with_auto_approved` already exists; feed it a
      predicate over the descriptor, so `approval_mode: auto` skips the prompt
      without changing the risk.
- [ ] Test: an auto-approved MCP write in `Default` mode is `Allow`; the same tool
      in `Plan` mode is still `Deny`.
- [ ] Commit.

### Task 4: Register, dispatch, delete

- [ ] Register both sources on the Agent's catalog after every load/refresh, and
      advertise `catalog.descriptor_snapshot` instead of `mcp_router.all_tools()`.
- [ ] Route every `ToolUse` through `resolve`/`authorize_policy`/`execute`; delete
      `legacy_authorize`, `ResolvedTool::Mcp*`, and the legacy popup constants.
- [ ] Delete the `tool_dispatch.rs: mcp` guard entry; `check-module-boundaries.sh`
      must still pass, which proves the Agent no longer calls MCP directly.
- [ ] Full `scripts/check-rust.sh`, then checkpoint commit.

## Follow-on: Phase 3 blocks this for the hook half

- MCP through the executor is **done** (see `docs/plugin-architecture.md`): the
  router is shared through lock-free `McpCallHandle`s rather than a mutex, so
  Tasks 2-4 above were completed by a different route than the one sketched.
- What remains of Phase 3 is the *hook* port: `Agent` calls `invoke_hooks!` at
  12 sites because every callback takes `&Agent`, which the kernel cannot see.
  `NoKernelHooks` is the empty port that fact forces. Turning hooks into a
  plugin service requires the hook engine to stop taking `&Agent` — a change to
  ~30 callbacks across `hook/mod.rs`, `plugin/hooks.rs` and the dispatch path.
  It is a prerequisite for `HooksPlugin`, not something to bolt on beside it.

## Self-Review Notes

- Risk: Task 1 touches every `mcp_router` call site (16 methods, 4 files).
- Rollback: Tasks 1–3 are additive; Task 4 is the only behaviour-moving commit.
- The `output_token_limit` refactor touches the kernel's `OutputPolicy`, which
  `tact::tool::metadata` re-exports; keep the old shape if that ripples too far
  and read the limit from the descriptor's policy instead.
