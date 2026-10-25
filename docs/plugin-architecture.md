# Plugin architecture

This page records the **Phase 0** boundary that the plugin-first migration has
actually implemented. The approved target design is
[`docs/superpowers/specs/2026-10-25-plugin-first-architecture-design.md`](superpowers/specs/2026-10-25-plugin-first-architecture-design.md);
the first implementation plan is
[`docs/superpowers/plans/2026-10-25-plugin-kernel-foundation.md`](superpowers/plans/2026-10-25-plugin-kernel-foundation.md).

## Implemented in Phase 0

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
- The production Agent still dispatches through the old path. Phase 1c is
  planned in
  [`docs/superpowers/plans/2026-10-25-agent-dispatch-cutover.md`](superpowers/plans/2026-10-25-agent-dispatch-cutover.md)
  and is **not started**: it moves tool ownership into the executor, which
  touches 76 `Agent::new` call sites and the 2 600-line dispatch module, so it
  needs its own review cycle.
- `tui` still depends on `tact` for view services. The target has no such edge;
  Phase 4 removes it and its test/fixture dependencies.

## Not implemented yet

- The Agent dispatch cutover (Phase 1c): `Agent` still owns a `ToolRouter`, and
  `into_kernel_catalog` consumes it, so the swap has to move tool ownership
  into the executor in one change.
- `AgentService`, `ChatService`, `SessionService` and view execution.
- Redaction, output persistence and streaming progress are still driven by the
  legacy dispatch path; the kernel pipeline emits `ToolEvent`s but nothing
  consumes them yet.
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
