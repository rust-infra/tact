# Agent Dispatch Cutover Implementation Plan

Status: **implemented (Phase 1c), committed**. Deviations from the plan as
written:
- The executor does not own the only handler copy. `ToolRouter` keeps the
  handlers as `Arc<dyn Tool>` and the catalog wraps the same instances, because
  the router must keep emitting model-facing specs (`compact` filtering for the
  Responses provider, `set_tool_description` overrides, cached specs).
- `preflight` is not used by the Agent. Hooks take `&Agent`, so the Agent runs
  its own and drives `resolve` → `authorize_policy` → `finish_authorization`
  instead; those three are the seam the kernel exposes for exactly this.
- MCP tools (including regular `mcp__server__tool` calls) still use the legacy
  ladder. They have no kernel descriptor, so their `[mcp]`-settings risk and
  popup had to stay until `McpToolSource` is registered by the Agent.
- Three bugs surfaced only under the full workspace gate, not in the kernel
  suites: the legacy context factory must stamp the per-call progress reporter
  (background rows collapsed to one), `CancelToken` must wrap the live cancel
  flag (a mid-call cancel was invisible), and the MCP ladder had to be restored
  verbatim. Recorded in the Phase 1c ledger.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ToolExecutor` the only path from a model `ToolUse` to a handler, and delete the legacy dispatch pipeline.

**Architecture:** `Agent` stops owning a `ToolRouter` for dispatch and owns a `ToolExecutor` instead. The executor holds the catalog (`LegacyToolAdapter`-wrapped native tools plus `McpToolSource`), the ports built in Phase 1b, and the ticket store. The existing wave scheduler stays: it consumes `ToolResources` produced from kernel descriptors, not legacy metadata.

**Tech Stack:** Rust 2024, `tact_kernel`, `tact`, `tokio`, `futures`.

**Spec:** `docs/superpowers/specs/2026-10-25-plugin-first-architecture-design.md`

## Global Constraints

- Work on `feat/plugin-first-architecture`; never commit to `main`.
- No user-visible behaviour change: same prompts, same option order, same messages, same tool results, same hook semantics.
- One Cargo invocation at a time. Watch each test fail before implementing it.
- `scripts/check-rust.sh` must pass before the checkpoint commit.
- Keep `tests/execution_parity.rs` green throughout; it is the cutover's safety net.

## Review Focus

1. **Ownership** — the executor must own every handler; no second copy reachable.
2. **Ordering** — prompt → hook → permission → tool, unchanged.
3. **Events** — TUI step updates must still fire exactly once per call.
4. **Redaction and persistence** — result redaction, output budget and session storage must stay on the single choke point.
5. **Subagents** — a child's catalog and permission context must be derived, not shared mutably.
6. **MCP resources/prompts** — the three MCP families must still route.

## File Structure

Modify:
- `crates/tact/src/agent/mod.rs` — `Agent` field swap.
- `crates/tact/src/agent/tool_dispatch.rs` — delete `preflight_tool_calls`' permission/hook body; call the executor.
- `crates/tact/src/agent/tool_schedule.rs` — accept kernel descriptors.
- `crates/tact/src/tool/legacy.rs` — expose `ToolEvent` → `AgentUpdate` mapping.
- `crates/tact/src/tool/mcp_source.rs` — carry MCP resource/prompt tools as kernel tools.
- `crates/tact-ui`, `crates/tact/src/headless*` — construction sites.

### Task 1: Move tool ownership into the executor

- [ ] Add `Agent::with_tool_executor(...)`, keep construction sites building a catalog from `toolset()`.
- [ ] Delete `Agent.tools` and every `self.tools.resolve(...)` call.
- [ ] Parity test: a `bash` call through `Agent` reaches the same handler as before (spy on a wrapper tool).
- [ ] Gate: focused agent tests, then `cargo test -p tact --lib agent::`.

### Task 2: Replace the hand-written preflight with `ToolExecutor::preflight`

- [ ] Delete the sensitive guard, `invoke_hooks!(PreToolUse, …)`, `check_with_auto` and the permission popup block from `preflight_tool_calls`.
- [ ] Build `ToolRequest` with the snapshot generation the model just saw.
- [ ] Map `ToolPreflight` back into `PreparedTool`.
- [ ] Keep the display formatting, step events and `permission_label` writes.
- [ ] Gate: `tests/execution_parity.rs`, `agent::tool_dispatch::tests`, then full `-p tact --lib`.

### Task 3: Replace execution with `ToolExecutor::execute`

- [ ] Route native tools through `execute`; keep MCP resource/prompt tools on their existing helpers.
- [ ] Keep redaction, output budget, `ExecResult` assembly and the PostToolUse hook result rewriting.
- [ ] Gate: `agent::tests`, `tool::` suites.

### Task 4: Delete the legacy dispatch code and re-gate

- [ ] Remove `run_native_tool`, the unused `ResolvedTool` variants and dead helpers.
- [ ] `cargo clippy -D warnings` must be clean with no `#[allow(dead_code)]` added.
- [ ] Full `scripts/check-rust.sh`, `book-check.py`, then checkpoint commit.

## Self-Review Notes

- Scope: one behaviour-preserving cutover; no new features.
- Risk: Tasks 2 and 3 touch the hottest code path; they are separate commits so a regression is bisectable.
- Rollback: each task is independently revertable because the executor is additive until Task 4.
