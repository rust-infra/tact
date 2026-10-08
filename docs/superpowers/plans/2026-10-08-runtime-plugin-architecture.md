# Runtime Kernel and Plugin Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure Tact so the final code dependency graph matches the approved Runtime Kernel, Plugin Protocol, extension, and View Adapter architecture while preserving every current business capability.

**Architecture:** Extract protocol-neutral Kernel services for lifecycle, capabilities, permission, events, trajectory, storage, cancellation, interaction, and errors. Adapt current Agent, Session, native tools, MCP, hooks, and TUI behind those boundaries; then add Rust, Node.js, and WASM hosts using the same protocol. Each task leaves a working compatibility path until the final removal task.

**Tech Stack:** Rust workspace, Tokio, serde/serde_json, SQLite existing store, existing MCP transport, ratatui, Node.js stdio RPC, WASI/Wasmtime host, Cargo unit/integration tests.

**Spec:** `docs/superpowers/specs/2026-10-08-runtime-plugin-architecture-design.md`

## Global Constraints

- Preserve all current business behavior listed in the spec's Business-function preservation matrix.
- Kernel code must not depend on `tui`, `tact-ui`, Chat, or a specific Agent implementation.
- All plugin languages use the same serialized protocol concepts and centralized permission path.
- Every new async test that waits on a channel uses a bounded timeout.
- Run only one Cargo test/build/clippy command at a time in this workspace.
- Keep `book/` Chinese-only; sync `ARCHITECTURE.md`, protocol/trajectory docs, and `book/26_chapter_issue_zh.md` when behavior becomes user-visible.
- Do not commit implementation changes unless the user explicitly requests commits.

## Review Focus

- A disconnected or slow client must resume from an event sequence without losing Trajectory facts; test sequence gaps, replay boundaries, and backpressure.
- A plugin must not forge reserved host events or bypass permission checks; test hostile capability declarations and reserved trajectory event types.
- Cancellation, timeout, plugin crash, and Runtime shutdown must terminate in-flight work without affecting unrelated runs; test each terminal outcome with timeouts.
- Existing tool metadata, permissions, typed effects, MCP namespacing, and UI presentation must remain unchanged; test native and MCP calls through the unified router.
- Compaction, recovery, session resume, sub-agents, background tasks, and headless execution must retain their observable behavior; add focused compatibility tests before deleting legacy paths.

### Task 1: Freeze protocol IDs, envelopes, and neutral runtime messages

**Files:**
- Create: `crates/protocol/src/ids.rs`
- Create: `crates/protocol/src/envelope.rs`
- Create: `crates/protocol/src/runtime.rs`
- Create: `crates/protocol/src/capability.rs`
- Create: `crates/protocol/src/interaction.rs`
- Create: `crates/protocol/src/error.rs`
- Modify: `crates/protocol/src/lib.rs`
- Test: `crates/protocol/tests/runtime_protocol.rs`

**Interfaces:**
- Produces `PluginId`, `RequestId`, `SessionId`, `RunId`, `TrajectoryId`, `StepId`.
- Produces `PluginRequest`, `PluginResponse`, `RuntimeEvent`, `RuntimeCommand`, `InteractionRequest`, `InteractionResponse`, `CapabilityDeclaration`, and `ProtocolError`.
- Every envelope carries protocol version, request ID, plugin ID, optional session/run/trajectory IDs, and optional deadline.

- [ ] Define serde-stable IDs as newtypes with string serialization and equality/hash behavior.
- [ ] Define protocol envelopes and explicit error categories, including retryability and origin.
- [ ] Define runtime-neutral event variants for run lifecycle, model boundaries, tool calls, permission, plugin lifecycle, interaction, and terminal outcomes.
- [ ] Define capability declarations for Tools, Commands, EventHandlers, Apps, Views, and runtime services.
- [ ] Write serialization round-trip tests, unknown variant tests, and reserved-field validation tests.
- [ ] Run `cargo test -p tact_protocol --test runtime_protocol`.

### Task 2: Add Kernel service interfaces and invocation context

**Files:**
- Create: `crates/tact/src/kernel/mod.rs`
- Create: `crates/tact/src/kernel/context.rs`
- Create: `crates/tact/src/kernel/capability.rs`
- Create: `crates/tact/src/kernel/error.rs`
- Create: `crates/tact/src/kernel/cancellation.rs`
- Modify: `crates/tact/src/lib.rs`
- Test: `crates/tact/src/kernel/tests.rs`

**Interfaces:**
- Produces `RuntimeContext`, `CapabilityRouter`, `InvocationContext`, `CancellationService`, and `KernelError`.
- `CapabilityRouter::register`, `CapabilityRouter::describe`, and `CapabilityRouter::invoke` are the only generic capability entry points.
- `InvocationContext` contains request identity, actor, deadline, cancellation token, and access to event, trajectory, permission, and storage services.

- [ ] Define traits with object-safe boundaries so in-process and remote hosts share the same invocation path.
- [ ] Implement deadline and cancellation propagation using Tokio cancellation primitives.
- [ ] Ensure invocation errors preserve origin, retryability, request ID, and plugin ID.
- [ ] Add tests for missing capabilities, duplicate registrations, cancelled calls, expired deadlines, and error conversion.
- [ ] Run `cargo test -p tact --lib kernel::`.

### Task 3: Extract Permission Engine behind the Kernel boundary

**Files:**
- Modify: `crates/tact/src/permission/mod.rs`
- Modify: `crates/tact/src/permission/settings.rs`
- Create: `crates/tact/src/kernel/permission.rs`
- Modify: `crates/tact/src/kernel/context.rs`
- Test: `crates/tact/src/kernel/permission_tests.rs`

**Interfaces:**
- Produces `PermissionService::check`, `PermissionService::request`, and `PermissionDecision`.
- Consumes existing `PermissionPolicy`, `CapabilityRisk`, settings, and UI-independent `InteractionRequest`.

- [ ] Move the decision boundary so native tools, MCP tools, and plugins all call the same service.
- [ ] Preserve Auto, Ask, Deny, always-allowed, sensitive-path, shell, and fail-closed behavior.
- [ ] Emit permission request, decision, and denial events through RuntimeEvent and Trajectory.
- [ ] Test hostile plugin declarations, sensitive paths, denied writes, approval responses, and missing responders.
- [ ] Run `cargo test -p tact --lib kernel::permission`.

### Task 4: Implement Event Transport and Trajectory Recorder

**Files:**
- Create: `crates/tact/src/kernel/event.rs`
- Create: `crates/tact/src/kernel/trajectory.rs`
- Create: `crates/tact/src/trajectory/model.rs`
- Create: `crates/tact/src/trajectory/recorder.rs`
- Create: `crates/tact/src/trajectory/query.rs`
- Create: `crates/tact/src/trajectory/replay.rs`
- Modify: `crates/tact/src/store/mod.rs`
- Test: `crates/tact/src/trajectory/tests.rs`

**Interfaces:**
- Produces `EventTransport::publish`, `subscribe`, `replay_from`, and `close`.
- Produces `TrajectoryRecorder::append`, `query`, and `replay`.
- Produces `TrajectoryEvent` with sequence, actor, event type, parent step, payload, and sensitivity.

- [ ] Implement monotonic per-trajectory sequence allocation and parent-child step relationships.
- [ ] Persist durable events through the existing SQLite store with redaction before persistence.
- [ ] Separate transient EventBus delivery from durable Trajectory append; define what is replayable.
- [ ] Reject plugin attempts to emit reserved host facts while allowing namespaced plugin events.
- [ ] Add bounded channel tests for ordering, replay, sequence gaps, subscriber cancellation, and slow consumers.
- [ ] Run `cargo test -p tact --lib trajectory::`.

### Task 5: Add namespaced Minimal Storage facade

**Files:**
- Create: `crates/tact/src/kernel/storage.rs`
- Modify: `crates/tact/src/store/mod.rs`
- Modify: `crates/tact/src/store/sqlite.rs`
- Test: `crates/tact/src/kernel/storage_tests.rs`

**Interfaces:**
- Produces `Storage::get`, `set`, `delete`, `list`, and `transaction`.
- Produces namespace types for `runtime`, `sessions`, `trajectories`, and `plugins/<plugin_id>`.

- [ ] Wrap existing SQLite access without changing current database compatibility.
- [ ] Enforce namespace ownership at the service boundary.
- [ ] Keep existing session, task, team, worktree, token usage, and trajectory records readable during migration.
- [ ] Test cross-namespace denial, plugin isolation, transaction rollback, and existing database migration behavior.
- [ ] Run `cargo test -p tact --lib store::`.

### Task 6: Define Plugin Lifecycle and Rust Host

**Files:**
- Create: `crates/tact/src/plugin/manifest.rs`
- Create: `crates/tact/src/plugin/registry.rs`
- Create: `crates/tact/src/plugin/lifecycle.rs`
- Create: `crates/tact/src/plugin/transport.rs`
- Create: `crates/tact/src/plugin/supervision.rs`
- Modify: `crates/tact/src/plugin/mod.rs`
- Test: `crates/tact/src/plugin/lifecycle_tests.rs`

**Interfaces:**
- Produces `PluginRegistry::discover`, `register`, `start`, `stop`, `restart`, `health`, and `unregister`.
- Produces a Rust host that uses the same manifest, protocol, capability, permission, event, and trajectory paths as remote hosts.

- [ ] Validate plugin IDs, protocol versions, dependencies, capability declarations, and reserved names.
- [ ] Implement lifecycle events and health state transitions.
- [ ] Ensure in-process Rust extensions do not bypass permission or trajectory recording.
- [ ] Test duplicate IDs, version mismatch, dependency failure, crash state, restart, shutdown drain, and in-flight request failure.
- [ ] Run `cargo test -p tact --lib plugin::`.

### Task 7: Unify native tools and MCP through Capability Router

**Files:**
- Modify: `crates/tact/src/tool/registry.rs`
- Modify: `crates/tact/src/tool/metadata.rs`
- Modify: `crates/tact/src/agent/tool_dispatch.rs`
- Modify: `crates/tact/src/mcp/mod.rs`
- Modify: `crates/tact/src/mcp/prompt.rs`
- Create: `crates/tact/src/capability/native_tool.rs`
- Create: `crates/tact/src/capability/mcp_tool.rs`
- Test: `crates/tact/src/capability/router_tests.rs`

**Interfaces:**
- Produces one `CapabilityRouter::invoke` path for native, MCP, and future plugin tools.
- Preserves `ToolMetadata`, `PermissionPolicy`, `ResourcePolicy`, `ToolPresentation`, `ToolEffect`, and stable external names.

- [ ] Adapt native `Tool` implementations without changing their public LLM names or metadata.
- [ ] Adapt `mcp__<server>__<tool>` and MCP prompt/resource routing through protocol capabilities.
- [ ] Move permission, resource, presentation, output, and trajectory hooks to the shared invocation boundary.
- [ ] Test native tool success/failure/effect application, MCP namespacing, unknown tools, and fail-closed privileges.
- [ ] Run `cargo test -p tact --lib capability::`.

### Task 8: Move Agent and Session behind extension interfaces

**Files:**
- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact/src/agent/tool_schedule.rs`
- Modify: `crates/tact/src/compact/mod.rs`
- Modify: `crates/tact/src/store/session_store/mod.rs`
- Create: `crates/tact/src/extensions/agent.rs`
- Create: `crates/tact/src/extensions/session.rs`
- Test: `crates/tact/src/extensions/agent_tests.rs`
- Test: `crates/tact/src/extensions/session_tests.rs`

**Interfaces:**
- Produces Agent and Session extension entry points that consume `RuntimeContext` and return protocol events/results.
- Removes direct Agent dependencies on TUI channels and concrete responder types.

- [ ] Move model loop output to RuntimeEvent/EventTransport while preserving streaming, thinking, tool progress, stop reasons, and error handling.
- [ ] Route compaction, transcript persistence, provider recovery, sub-agents, and background work through Kernel services.
- [ ] Preserve session locking, resume semantics, token usage, and existing SQLite records.
- [ ] Test normal run, cancellation, compaction, transport recovery, resume, sub-agents, and headless execution.
- [ ] Run focused tests one invocation at a time: `cargo test -p tact --lib agent::`, then `cargo test -p tact --lib extensions::`.

### Task 9: Convert UI responder and TUI to View / Interaction adapters

**Files:**
- Create: `crates/tact/src/kernel/interaction.rs`
- Modify: `crates/tact/src/ui_responder.rs`
- Modify: `crates/tact-ui/src/session_bootstrap.rs`
- Modify: `crates/tact-ui/src/driver.rs`
- Modify: `crates/tact-ui/src/interactive.rs`
- Modify: `crates/tui/src/lib.rs`
- Modify: `crates/tui/src/state/`
- Test: `crates/tact/src/kernel/interaction_tests.rs`
- Test: existing TUI event/render tests requiring protocol updates

**Interfaces:**
- Produces client-neutral `InteractionRequest` and `InteractionResponse`.
- TUI consumes RuntimeEvent and emits RuntimeCommand without calling Agent methods.

- [ ] Convert select, multi-select, confirm, input, permission, popup, stream, and completion flows to neutral interactions.
- [ ] Preserve every existing TUI rendering and key-binding outcome, including tool cards, popups, themes, i18n, scroll behavior, and bottom-bar usage data.
- [ ] Keep headless mode as an External Client adapter with deterministic responses and exit codes.
- [ ] Test interaction ordering, cancellation, TUI event projection, and headless behavior with bounded timeouts.
- [ ] Run `cargo test -p tui --lib` followed by `cargo test -p tact-ui --lib` sequentially.

### Task 10: Register official Agent, Session, Chat, Tools, and Workflow extensions

**Files:**
- Create: `crates/tact/src/extensions/mod.rs`
- Create: `crates/tact/src/extensions/chat.rs`
- Create: `crates/tact/src/extensions/tools.rs`
- Create: `crates/tact/src/extensions/workflow.rs`
- Modify: `crates/tact/src/tool/mod.rs`, `crates/tact/src/tool/registry.rs`, and tool metadata modules for extension registration
- Test: `crates/tact/src/extensions/official_extensions_tests.rs`

**Interfaces:**
- Produces official extension manifests and registrations using the same Plugin Protocol as third-party extensions.
- Chat owns conversational projection and commands; Tools owns current tool families; Workflow owns orchestration.

- [ ] Register built-in extensions through PluginRegistry and CapabilityRouter.
- [ ] Preserve current command names, tool names, hooks, memory, skills, tasks, teams, worktrees, voice, and background behavior.
- [ ] Test extension enable/disable, replacement, duplicate capability handling, and a Chat instance using a different View adapter.
- [ ] Run `cargo test -p tact --lib extensions::`.

### Task 11: Add Node.js Plugin Host and a minimal Node Chat plugin

**Files:**
- Create: `crates/tact_plugin_node/Cargo.toml`
- Create: `crates/tact_plugin_node/src/lib.rs`
- Create: `crates/tact_plugin_node/src/process.rs`
- Create: `crates/tact_plugin_node/src/transport.rs`
- Create: `crates/tact_plugin_node/tests/fixtures/chat-plugin/index.mjs`
- Create: `crates/tact_plugin_node/tests/node_host.rs`
- Modify: workspace `Cargo.toml`

**Interfaces:**
- Produces `NodePluginHost::start`, `stop`, `restart`, and protocol transport over stdio.
- Fixture plugin registers a Chat capability, starts a run, subscribes to events, answers an interaction request, and exits cleanly.

- [ ] Implement process startup, handshake, manifest validation, stdout/stderr separation, deadlines, cancellation, and shutdown drain.
- [ ] Route Node calls through the same Capability Router, Permission Engine, Event Transport, and Trajectory Recorder.
- [ ] Detect crashes and fail in-flight calls without terminating unrelated Runtime work.
- [ ] Test registration, event replay, permission denial, timeout, cancellation, malformed messages, crash recovery, and protocol mismatch.
- [ ] Run `cargo test -p tact_plugin_node --test node_host`; the test launches the checked-in `index.mjs` fixture directly, so no second test runner is required.

### Task 12: Add WASM Plugin Host with constrained capabilities

**Files:**
- Create: `crates/tact_plugin_wasm/Cargo.toml`
- Create: `crates/tact_plugin_wasm/src/lib.rs`
- Create: `crates/tact_plugin_wasm/src/host_functions.rs`
- Create: `crates/tact_plugin_wasm/tests/wasm_host.rs`
- Modify: workspace `Cargo.toml`

**Interfaces:**
- Produces `WasmPluginHost::instantiate`, `invoke`, `interrupt`, and `drop_instance`.
- Host functions expose only declared and permitted capabilities.

- [ ] Implement manifest and protocol validation before instantiation.
- [ ] Mediate storage, event, trajectory, clock, and declared external capabilities through host functions.
- [ ] Enforce memory, fuel/deadline, cancellation, and permission limits.
- [ ] Test denied filesystem/network/process access, timeout, interruption, namespaced events, and instance cleanup.
- [ ] Run `cargo test -p tact_plugin_wasm --test wasm_host`.

### Task 13: Remove legacy direct paths and update documentation

**Files:**
- Modify: `crates/tact/src/agent/mod.rs`, `crates/tact/src/agent/tool_dispatch.rs`, and `crates/tact/src/agent/tool_schedule.rs` for remaining direct UI sends
- Modify: `crates/tact/src/mcp/mod.rs`, `crates/tact/src/mcp/prompt.rs`, `crates/tact/src/mcp/resource.rs`, and `crates/tact/src/mcp/remote.rs` for remaining special invocation paths
- Modify: `crates/tact/src/ui_responder.rs` or delete once migrated
- Modify: `ARCHITECTURE.md`
- Create: `docs/plugin_protocol.md`
- Create: `docs/trajectory.md`
- Modify: `book/26_chapter_issue_zh.md` if user-visible behavior changed
- Test: repository dependency and protocol integration tests

- [ ] Remove every Agent-to-TUI channel and replace it with RuntimeEvent/EventTransport.
- [ ] Remove MCP-only execution branches and route all external capabilities through Plugin Protocol.
- [ ] Remove compatibility types only after all consumers use neutral protocol types.
- [ ] Update architecture diagrams and module tables to match the final dependency graph.
- [ ] Add integration tests proving Kernel builds without UI, a plugin can run without Chat, and a View can be replaced.
- [ ] Run `cargo test --workspace` once, sequentially, after all focused tests pass.
- [ ] Run `git diff --check` and inspect the final dependency graph before declaring completion.

## Completion gate

The migration is complete only when the acceptance requirements in the spec pass and the following scenarios work end to end:

1. Headless External Client starts an Agent run, observes streamed events, answers permission, and reads the final Trajectory.
2. TUI performs the same run using only View / Interaction APIs and renders the existing experience.
3. A Node Chat plugin starts a run and receives the same Runtime events as TUI.
4. A plugin crash affects its run but leaves the Kernel and another run healthy.
5. A disconnected client reconnects from a Trajectory sequence without duplicated or missing durable facts.
6. Existing MCP and native tools preserve names, permissions, typed effects, output persistence, and user-visible results.
7. Compaction, recovery, sessions, sub-agents, tasks, teams, memory, skills, worktrees, background work, voice, and token accounting remain available through the new extension boundaries.
