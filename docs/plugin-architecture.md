# Plugin architecture

This page records what the plugin-first migration has actually implemented, up
to Phase 1c. The approved target design is
[`docs/superpowers/specs/2026-10-25-plugin-first-architecture-design.md`](superpowers/specs/2026-10-25-plugin-first-architecture-design.md);
the first implementation plan is
[`docs/superpowers/plans/2026-10-25-plugin-kernel-foundation.md`](superpowers/plans/2026-10-25-plugin-kernel-foundation.md).

## Implemented (Phases 0 – 1c)

- `crates/tact_kernel` exists as a leaf crate above `tact_protocol`.
- Plugin identity (`PluginId`, `PluginKind`, `PluginManifest`), capability
  identity/filtering, registration catalogs, and the plugin host lifecycle are
  in the kernel.
- Tool metadata, scheduling resources, permission risk levels, tool results,
  control-plane effects and the neutral `ToolImage` contract live in the
  kernel; `tact` re-exports them so existing imports keep working.
- `scripts/check-architecture.sh` rejects forbidden kernel dependencies.
- `scripts/check-module-boundaries.sh` records the current Agent→Skills/Memory/
  MCP debt as an explicit Phase-0 allowlist; new edges fail immediately.
- Phase 1a adds the kernel execution contracts and a fake-tested
  `ToolExecutor`: `ToolCallContext`, `EventSink`/`InteractionBroker`,
  `ToolSource` descriptor snapshots, and the preflight/execute pipeline with
  one-use tickets.
- Phase 1b adds the production adapters, still without changing user-visible
  behaviour:
  - `LegacyToolAdapter` + `ToolRouter::into_kernel_catalog` — the existing
    native tool set adapts into a kernel catalog with no handler rewritten.
  - `McpToolSource` — MCP tools become a dynamic `ToolSource` with owned
    descriptors and per-tool risk from the server policy.
  - `ManagerPermissionService` — `PermissionManager` stays the only authority
    for modes, rules, always-allow persistence and the plan-mode nudge; the
    adapter only maps its verdict onto the kernel's option-shaped prompt.
  - `SensitivePathGuard` — the credential-tier refusal, unchanged in priority.
  - `KernelHookService` — reuses the existing hook engine and output
    normalization, but runs without an `Agent`.
  - `tests/execution_parity.rs` pins both permission paths to the same verdict
    across every mode × risk, so the cutover cannot silently change behaviour.
- Phase 1c cut the production Agent over to the kernel pipeline. `Agent` owns an
  `Arc<ToolExecutor>` whose catalog wraps the *same* `Arc<dyn Tool>` handlers the
  router holds, so a tool has one instance, not two. Every native call now goes
  resolve → PreToolUse → guard again on the rewritten input →
  `PermissionRequest` hook → permission ladder → one-use ticket → execute. A
  native call that reaches the execution wave without a ticket is an explicit
  error rather than a dispatch.
  - Hooks stay on the Agent side (a callback takes `&Agent`), so the Agent
    drives `resolve` → `authorize_policy` → its own `PermissionRequest` hook →
    `finish_authorization` instead of the executor's `preflight`.
  - Tact's own MCP resource/prompt tools, and regular `mcp__server__tool` calls,
    keep the hand-written ladder: their risk lives in `[mcp]` settings rather
    than a kernel descriptor. That is the last legacy permission path.
  - Redaction level is decided once and travels on `ToolCallContext`;
    `CancelToken` wraps the agent's live flag so a cancel mid-call is seen.
- `tui` still depends on `tact` for view services. The target has no such edge;
  Phase 4 removes it and its test/fixture dependencies.

## Not implemented yet

- MCP tools as kernel descriptors: `McpToolSource` adapts them, but the Agent
  does not yet register it, so MCP calls still bypass the executor and keep their
  `[mcp]`-settings risk and their own permission ladder.
- `AgentService`, `ChatService`, `SessionService` and view execution.
- `ToolEvent`s: the kernel emits them, but nothing consumes them yet — the TUI
  still renders from the legacy `AgentUpdate`s.
- Output persistence and streaming progress are still applied by the Agent after
  the kernel returns, not by the kernel itself (deliberate: the kernel owns no
  session store).
- Domain plugin migration (Agent, Tools, MCP, Skills, Memory, Goal, Task, Team,
  Worktree, Background, Subagent, Voice).
- TUI/headless views as plugins and command/view execution.
- The final documentation/book sync and allowlist removal.

The kernel dependency whitelist is:

```text
tact_protocol, anyhow, async-trait, serde, serde_json, strum, tokio(sync)
```

Forbidden kernel dependencies include `tact`, `tui`, `tact-ui`, `tact_llm`,
`sqlx`, `rmcp` and `ratatui`. The guards run as the first step of
`scripts/check-rust.sh`.
