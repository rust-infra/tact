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
- Phase 2 is in progress. What has moved so far:
  - The built-in tools are a `ToolFamily` table (`shell`, `files`,
    `interaction`, `task`, `goal`, `team`, `worktree`, `background`,
    `subagent`, `memory`, `skill`, plus a restricted `subagent_basics`), and
    `toolset()`/`subagent_toolset()` are folds over it. The family table is
    pinned to the previous 40-tool set, so a family edit that changes the
    advertised tools fails a test.
  - The Agent's catalog is assembled by *installing* those families as plugins
    (`FamilyPlugin` → `PluginHost` → `Runtime::tools`), over the same
    `Arc<dyn Tool>` handlers the router holds. A disabled capability is skipped
    and reported, not silently dropped.
  - The system prompt's parts are kernel prompt sections
    (`prompt:role`, `prompt:skills`, `prompt:guidelines`, `prompt:constraints`,
    `prompt:memory_guidance`, `prompt:additional`, `prompt:mcp_instructions`,
    `prompt:memory`, `prompt:dynamic_context`). The Tera template still owns the
    framing, and a test pins the assembled prompt byte-for-byte against the old
    builder chain.
  - `ToolCatalog` can be reached through a shared handle, so a host registers
    dynamic sources (MCP) without invalidating live tickets, and `handler_for`
    resolves a handler whether a native insert or a source provided it.
- Phase 3 has started. What has moved:
  - The kernel has a typed `ServiceRegistry`: a plugin provides a handle at
    registration (`Registrar::service`), the host commits it with the same
    transaction as the four catalogs, and `Runtime::services()` exposes it after
    sealing. The key is the Rust type, so two plugins cannot quietly provide two
    different things under one name; a second provider is an error naming the
    first. It is not a locator — the composition root pulls the specific handle
    and passes it to the plugins built after it.
  - `PermissionPlugin`, `SecurityPlugin`, `HooksPlugin`, `ProvidersPlugin` and
    `MarketplacePlugin` provide their ports through it. `ProvidersPlugin` needed
    a non-panicking `tact_llm::try_get_provider` first — the assembly has to be
    able to ask "is there a provider?" without the question aborting the process.
    `Agent::provider()` now answers from the plugin bundle, so a consumer that
    already has an agent no longer re-reads the process global. The Agent's `ToolExecutor` now resolves the policy pair from the
    registry, so a host that forgets a policy plugin gets a named error instead
    of a silently different permission policy.
  - `crates/tact/src/host.rs` is the composition root: `assemble` installs the
    policy plugins (and optionally hooks/storage) in dependency order and seals;
    `executor_from_families` builds the catalog and resolves the ports. The
    Agent's executor goes through it, so the production path is assembled the
    same way the tests are.
  - `Runtime::plugin_ids` exposes the install order, and tests fix what it
    means: `assemble` installs security before permission, and shutdown runs the
    reverse — a plugin's `register` may only assume what was installed before it.
  - `Agent::with_installed_session` can pull the store from the registry instead
    of taking one directly, naming `builtin.session_store` when storage was never
    installed. The production bootstrap still passes a store to `with_session`,
    because it resolves the session id before the agent exists and the store
    handoff has to wait for that id.
  - The installed bundle is owned by the Agent and shut down in
    `dispatch_session_end_hooks`, not dropped at construction: a plugin that
    owns a resource is released on the real exit path, and a second teardown is
    a no-op.
  - The hook engine no longer takes `&Agent`. A neutral `HookHost` port carries
    exactly what a hook payload reads (session id, model, permission mode, turn
    count, session-start source) plus emit/status/queue-context/mcp-call, and
    `AgentHookHost<&Agent>` adapts the existing call sites. That was the
    prerequisite for `HooksPlugin`; without it the hook engine could not live
    behind a port at all.
- Phase 4 has started, and the command half now has a real contributor:
  - `Command` gained a defaulted `run_async`, so an async command (one that asks
    a manager owning running processes, or a server) is one type rather than a
    second registry. A pure command still implements `run` alone.
  - `CommandOutcome` gained `Emit(Emitted)` for a read-out that drives a panel
    (`GoalsChanged`) or opens a typed popup: flattening that into `Message` would
    make the host parse text back into the shape it already had. `Emitted` boxes
    the frame, and the enum is deliberately not `PartialEq` because an
    `AgentUpdate` is a transport frame rather than a value.
  - `CommandOutcome` gained `Failed`. Collapsing a manager error into `Usage` was
    how a failure used to surface as a help message; collapsing it into
    `Message` made it open a read-out popup instead of an error line.
  - A command is now a *plugin contribution*: `CommandPlugin` registers through
    `Registrar::command`, so it gets name validation, the capability filter (a
    disabled `/stats` is absent from the catalog and recorded in `skipped`) and a
    transactional install. One plugin per command keeps the plugin id stable
    while the command keeps its own name.
  - `/stats` and `/background` are catalog commands
    (`crates/tact/src/command/`), each owning its metadata, argument grammar and
    answer. The driver holds one catalog per loop and routes both of its
    `/background` paths through it, so the two cannot word the answer
    differently. `/stats` refuses an argument it does not take by name, and
    reads through a poisoned lock.
  - `/goal [list [--all] | show <id>]` is a catalog command. Getting it there
    required moving the read path first: `goal_snapshots_with`, its three
    status/phase/criterion mappers and `GoalLimits` now live in
    `crates/tact/src/goal/` (`snapshots.rs`, `limits.rs`), and `tact-ui`
    delegates. Keeping the projection in the frontend would have made the
    extension crate depend on the view.
  - Two behaviours the migration had to preserve, both caught by existing tests:
    `show` still emits the panel refresh first (the panel never lists archived
    Goals, but `show` reaches them — that is what keeps archiving from being a
    dead end), and an empty list still says `(no goals)` in the log. The command
    produces only the projection; the log line is now the host's job.
  - A command that has to drive the agent loop (`/model`, `/compact`,
    `create`/`resume`/`pause`/`cancel` for Goals, …) is *not* in the catalog yet:
    it cannot own a `&mut Agent` through one, so those arms stay on the driver's
    side until the loop is a plugin.
- Phase 4's view half on the kernel side:
  - `Command` carries `run()`/`complete()`, and `CommandCatalog` routes by name
    (resolving aliases) and completes by either listing names or delegating to
    the command's own grammar. The kernel never parses arguments: a command's
    verbs belong to whoever contributes it, and there is no unknown-command
    error — an unknown name is a `Usage` line the host prints verbatim.
  - `prompt:skills` and `prompt:memory` no longer reach into `skill`/`memory`
    from `agent/`; the module guard's allowlist lost both, so the only remaining
    entry is `mcp` on the two files that still call the router.
- `tui` still depends on `tact` for view services. The target has no such edge;
  Phase 4 removes it and its test/fixture dependencies.

## Not implemented yet

- ~~MCP tools as kernel descriptors.~~ **Done.** Every MCP family — regular
  tools, resource tools and prompt tools — is a catalog source now, and all three
  run through the kernel pipeline. The legacy permission ladder is deleted.
  - Sharing the router was solved without a lock: `McpClient` is `Clone` over
    `Arc`ed services and immutable data (and `shutdown` consumes `self`, so a
    clone cannot tear a connection down), so `MCPToolRouter::call_handles()`
    yields lock-free `McpCallHandle`s. `SnapshotMcpBackend` snapshots those plus
    the `[mcp]` policy; `SnapshotMcpControlBackend` snapshots the router. A call
    in flight keeps working when the host swaps a new snapshot in, and a reload
    can neither block nor be blocked by a descriptor read.
  - The advertised tool list is now built from
    `ToolCatalog::source_descriptors()`, so "what the model sees" and "what the
    executor can resolve" are one set by construction, pinned by a test.
  - Two MCP behaviours were silently dropped by the first version of the cutover
    and are restored: `approval_mode: auto` (carried as
    `ToolPolicy::auto_approved`, consulted *after* plan mode so it cannot unlock
    it) and `tools.<name>.output_token_limit` (carried as
    `ToolPolicy::output_token_limit`).
- Phase 4's view half is untouched: `tui` still depends on `tact` and `tact_llm`,
  and slash commands still dispatch through the `SlashCommand` enum rather than a
  catalog. That is the next large change.
- Phase 3's remaining half: `CompactionPlugin`, `AgentPlugin`, `ChatPlugin`.
  Compaction cannot be extracted as it stands: the whole trigger/threshold/
  recovery path is methods on `Agent`, so a plugin would have to own the loop.
  `StoragePlugin` and `SessionStorePlugin` exist and are installable, but the
  production bootstrap still opens the store itself and passes it to
  `Agent::with_session`, so `HostOptions::storage` is exercised only by tests.
  `HostOptions::marketplace` is likewise unset by the production bootstrap: the
  TUI still builds its marketplace service where it needs one.
- Domain families beyond tools and prompt sections: Goal/Task/Team/Worktree/
  Background/Subagent are tool families but not yet plugins with their own
  lifecycle. The remaining `agent/` → `mcp` edge is only the router's lifecycle
  (load/reload, the prompt/spec queries, and the name-resolver predicates), which
  Phase 3's `McpPlugin` owns; skill and memory no longer have an edge.
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
