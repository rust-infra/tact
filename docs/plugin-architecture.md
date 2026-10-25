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
  - `ContextualToolAdapter` (then called `LegacyToolAdapter`) +
    `ToolRouter::into_kernel_catalog` — the existing native tool set adapts into a
    kernel catalog with no handler rewritten.
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
  - (Superseded in Phase 2.) Tact's own MCP resource/prompt tools and regular
    `mcp__server__tool` calls kept a hand-written ladder at this point because
    their risk lived in `[mcp]` settings rather than a kernel descriptor. They are
    kernel tools now and that ladder is deleted — see the MCP section below.
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
  - `PermissionPlugin`, `SecurityPlugin`, `HooksPlugin`, `ProvidersPlugin`,
    `MarketplacePlugin` and `ConfigPlugin` provide their ports through it. `ProvidersPlugin` needed
    a non-panicking `tact_llm::try_get_provider` first — the assembly has to be
    able to ask "is there a provider?" without the question aborting the process.
    `Agent::provider()` now answers from the plugin bundle, so a consumer that
    already has an agent no longer re-reads the process global.
  - `ConfigPlugin` does the same for the resolved configuration, which is the
    prerequisite for the frontend decoupling: `tui` has 45 direct `settings()`
    calls and `tact-ui` 18, and none of them is a dependency the type system can
    see. The plugin does not remove the global — it gives new code a way to
    receive the configuration, and the host one place to record which snapshot it
    assembled with. The Agent's `ToolExecutor` now resolves the policy pair from the
    registry, so a host that forgets a policy plugin gets a named error instead
    of a silently different permission policy.
  - `crates/tact/src/host.rs` is the composition root: `assemble` installs the
    policy plugins (and optionally hooks/storage) in dependency order and seals;
    `executor_from_families` builds the catalog and resolves the ports. The
    Agent's executor goes through it, so the production path is assembled the
    same way the tests are.
  - The optional infrastructure travels in `HostExtras` (`hooks`, `storage`,
    `marketplace`) and `assemble_with(options, extras)` is the entry point that
    takes it. `assemble` stays a two-line wrapper so the Agent's own assembly —
    which has none of it — reads as one call instead of three `None`s.
  - `Runtime::plugin_ids` exposes the install order, and tests fix what it
    means: `assemble` installs security before permission, and shutdown runs the
    reverse — a plugin's `register` may only assume what was installed before it.
  - The production bootstrap now hands the session store over **through the
    bundle**, not beside it: `Agent::new_with_host` takes the `HostExtras` with
    `StorageOptions`, and the bootstrap then calls `with_installed_session`.
    `HostOptions::storage` was previously exercised only by tests while the real
    path passed a store straight to `with_session` — two answers to "where does
    the store come from". A source-level guard in `session_bootstrap` fails if
    the direct handoff comes back.
  - `Agent::with_installed_session` pulls the store from the registry, naming
    `builtin.session_store` when storage was never installed. `with_session` still
    exists for a host that has no bundle (a unit test, and the subagent path that
    opens its own fallback store); it is no longer what the shared bootstrap uses.
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

- Phase 5 has a structural first step: `Tool` gained a defaulted
  `call_in_workspace(Workspace, Value)`. `read_file`, `read_image`, `edit_file`
  and `apply_patch` only ever read `work_dir` and `sleep` reads nothing; those
  four now skip the full context, and a test counts context builds to prove it.
  The `#[tool]` macro chooses the narrower impl from the handler's signature, so
  a tool cannot declare one shape and receive the other.
  - **Measuring changed the justification.** `ToolContext` has 25 fields and only
    two are deep copies (`work_dir`, and the `tool_id` inside
    `progress_reporter`, rebuilt per call anyway); the rest are `Arc` clones. So
    the saving is one `PathBuf` and one `String` — the reason to do this is that
    a handler reading `work_dir` should not be *handed* a session handoff, a
    permission snapshot and a sandbox it can never consult, not the allocation.
  - The remaining tools each need a *different* field, so instead of one narrow
    method per shape there is now a per-domain path: `Tool::domain_needs()`
    declares the handles a tool reads, the adapter fills exactly those into
    `DomainHandles`, and an undeclared handle arrives as `None` — the tool has to
    say so rather than silently seeing a partial context. `load_skill` (skill
    registry) and all eight team tools (teammate manager) are converted, and the
    `#[tool]` macro derives the declaration from the handler's own field access,
    so declaring one handle and reading another cannot compile past the macro.
    The four `task` tools and the four `goal` tools are converted too, which grew
    the handle set (`task_manager`, `goal_manager`, `session_id`,
    `active_goal_id`, `events`). `DomainHandles`
    is `Default` with a `none()` constructor for that reason: a flat struct that
    has to be spelled out in full at every site is exactly how it would turn back
    into `ToolContext`.
  - The macro's detection table is hand-maintained, and forgetting an entry does
    not fail to compile — the tool declares fewer handles than it reads, gets
    `None`, and reports "unavailable" at runtime. That happened with
    `goal_manager` and cost four agent-level tests to find. A guard now asserts
    every `DomainHandle` variant names a field `DomainHandles` declares, plus an
    exhaustive count so a new variant has to be listed.
  - Converted so far: `read_file`, `read_image`, `edit_file`, `apply_patch`
    (workspace only), plus `load_skill`, every team / task / goal / worktree tool,
    `write_file`, `ask_user`, the memory tools, the background tools and `bash`
    (declared handles, 17 variants).
  - `bash` needed a `ShellPolicy` handle rather than four loose fields: the four
    shell settings are resolved once at startup and never change, so they are one
    value answering "how may this command run". The timeout rule lives on
    `ShellPolicy::timeout_for`, including the case that matters — a per-call `0`
    still disables the limit, so a model cannot grant itself more time than the
    user configured.
  - **`subagent` is now the only tool that takes a `ToolContext`**, and it is the
    one whose input genuinely is the whole run: session handoff, permission
    snapshot, the parent's hooks, its own store. `compact` and `sleep` came off
    last: the macro's pure-tool branch expands *parameters* into fields, so a
    handler that reads nothing can drop the parameter and let the macro generate
    the input struct — the hand-written one had no outside references and its
    `#[schemars]`/`#[serde(alias)]` attributes move onto the parameters verbatim.
  - Per-call state is *rebuilt*, not carried: `ToolProgressReporter` is built from
    `tool_use_id` + `events` at the point of use, because it is not a domain
    handle. Trying to carry it would have put a per-call object into a set that
    describes a run.
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
  `StoragePlugin` and `SessionStorePlugin` are installed by the production
  bootstrap now (through `HostExtras`), so the store is no longer a second path.
  `HostOptions::marketplace` is still unset by the production bootstrap: the TUI
  builds its marketplace service where it needs one.
- The guard's facade rule (`mcp::lifecycle` may be named without counting as an
  edge to `mcp`) is only as good as its self-check: it strips the sanctioned path
  and re-checks, and asserts that a second direct use on the same line is still
  rejected. A new facade has to be added deliberately, with the same proof.
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
