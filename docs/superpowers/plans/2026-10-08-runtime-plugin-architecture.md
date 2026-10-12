# Runtime Kernel and Plugin Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure Tact so the final code dependency graph matches the approved Runtime Kernel, Plugin Protocol, extension, and View Adapter architecture while preserving every current business capability.

**Architecture:** Extract protocol-neutral Kernel services for lifecycle, capabilities, permission, events, trajectory, storage, cancellation, interaction, and errors. Adapt current Agent, Session, native tools, MCP, hooks, and TUI behind those boundaries; then add Rust, Node.js, and WASM hosts using the same protocol. Each task leaves a working compatibility path until the final removal task.

**Tech Stack:** Rust workspace, Tokio, serde/serde_json, SQLite existing store, existing MCP transport, ratatui, Node.js stdio RPC, WASI-style subprocess runner (no engine linked — see the implementation note in the spec's §3), Cargo unit/integration tests.

**Spec:** `docs/superpowers/specs/2026-10-08-runtime-plugin-architecture-design.md`

## Status correction — 2026-10-10 audit

The `[x]` marks below record the state when each task was written. A read-only
audit of the whole spec against the shipping binary found several of them
**false**. Treat the boxes as "the code exists", not "production-wired", and
check the evidence before relying on one.

| Task | Box that is false | Evidence (2026-10-10) |
|---|---|---|
| 2/5 | Kernel services reachable | `tact::services::register` has **zero callers**; the 12 §4 service capabilities (`storage.*`, `events.*`, `trajectory.*`, `permission.request`, `interaction.request`) are unreachable in the shipping binary |
| 6 | "Implement lifecycle events and health state transitions" | `RuntimeEvent::PluginStarted/Stopped` have **no producer**; `PluginRegistry::{discover,restart,health,health_all,unregister}` have no production caller |
| 9 | "Convert select, multi-select, confirm, input, permission, popup, stream, and completion flows to neutral interactions" | production interaction is still `tact_extensions::ui_responder::UiResponder` and the prompt sites still do not have an `InvocationContext`; `InteractionRequest::{Permission,Confirm,Input}` are never constructed in production. **Partly corrected 2026-10-11**: the Kernel `InteractionBroker` is no longer test-only (`withdraw` exists, the interactive host installs it and the driver answers through it — see the progress note below), but it backs the *capability* surface, not the prompt sites |
| — | §7 "timeout" fact | `TimedOut`/`TrajectoryEventType::Timeout` now **has a producer** (`CapabilityRouter::guarded`, `crates/tact/src/capability.rs`) and it **lands in a real session**: a hosted plugin's *tool* call is the host ↔ external-participant boundary and is the one invocation that carries both a deadline and a run, so the wave router puts the host's `DEFAULT_REQUEST_TIMEOUT` on it (`Agent::tool_call_deadline`) and a hung plugin publishes the fact for the run. A plugin's capability call and the host calls it makes are bounded too, but their deadlines sit below `guarded` (`HostCapabilityHandler`) or are run-less (`GrantedHostCalls`), and a `TimedOut` fact needs a run (the type's `RunId` is required), so those do not publish. A user's turn and native/MCP tools are deliberately never bounded. Provider timeouts appear as `Retry`/`Recovery` facts |
| 10 | "Register built-in extensions through PluginRegistry and CapabilityRouter" | only the **manifests** register (`register_official_manifests`); `AgentExtension`/`SessionExtension`/`WorkflowExtension::register` are test-only; `Agent::runtime_plugins` is written and never read |
| 11 | "a fixture plugin registers Chat, starts a run, subscribes to events, answers an interaction" | the fixture fabricates all three inside `index.mjs`; the host has no event transport or interaction broker; `runs.start` is never invoked from the Node path |
| 12 | "Enforce memory, fuel/deadline, cancellation, and permission limits" | no engine dependency (`Cargo.lock` has 0 hits for `wasmtime`/`wasmi`/`wasmer`); fuel/max-memory are runner argv nobody interprets in-repo; the fixture "module" is an 8-byte placeholder |
| 13 | "Remove every Agent-to-TUI channel" / "Remove MCP-only execution branches" | `agent.agent_loop` is still called directly by the interactive and headless hosts; `plugin/hooks.rs` still calls `mcp_router.call(...)` outside the router; legacy `ui_tx`/`UiResponder` remain live |
| 6/11 | file lists | `tact_extensions/src/plugin/{manifest,registry,lifecycle,transport,supervision}.rs` and `tact_plugin_node/src/{process,transport}.rs` do not exist (the code moved to `crates/tact/src/lifecycle.rs` + `tact_plugin_host/src/*` in Task 14; the boxes were never corrected) |

Progress since the audit: `RunFinished` and the user-role `Text` fact now have
producers, and `EventTransport::close` has a real caller (commit `f342883f`);
the **headless** host starts its run through `runs.start`, so
`AgentExtension::register` and the run capability have a production caller
(commit `d68c3a0b`); the session bootstrap now builds a serving
`RuntimeContext` that calls `tact::services::register` over real backing, and
`SessionExtension::register` runs in production, so the Task 2/5 and Session
rows above are addressed; **both** hosts now start their run through
`runs.start` (the interactive driver registers the Agent extension on the same
serving context), so the Task 13 "Agent-to-TUI" row no longer describes the run
path either; and **Chat now has a real implementation** — `chat.submit` owns the
conversational turn (assembly, the Stop-hook continuation loop, `TaskComplete`,
TaskCompleted hooks) and both hosts submit through it, which also fixed a real
asymmetry (headless used to ignore continuation-requesting `Stop` hooks and never
fire `TaskCompleted`). **Workflow now has a production executor**: both hosts
register `WorkflowExtension` with `ChatTurnWorkflow`, which sequences steps by
invoking `chat.submit` (spec
`docs/superpowers/specs/2026-10-11-production-workflow-executor-design.md`); no
product code invokes `workflow.run` yet. **The plugin-host boundary now has a
product caller**: an installed plugin whose `.codex-plugin/plugin.json` declares
a `runtime` half is spawned through `StdioPluginHost` at session bootstrap, its
handshake capabilities register on the serving router, its manifest registers in
the session's `PluginRegistry`, and both hosts stop it at teardown (spec
`docs/superpowers/specs/2026-10-11-hosted-runtime-plugins-design.md`). Guest
host calls are denied unless the manifest grants them (`grants` plus the `host_calls`
handshake feature), so the Kernel services stay reserved; and a hosted capability of
kind `Tool` **is** a model-visible tool, served by the Agent's per-wave tool router
rather than the serving router (the "not a model-visible tool" non-goal in
`2026-10-11-hosted-runtime-plugins.md` is superseded). Still open: 10
of the 12 service capabilities have no invoking caller, and nothing reads
extension state back (no health surface). The submit paths are
all routed; `ARCHITECTURE.md` §15 lists what remains unconsumed. **The
interaction surface is now live too**: the interactive host creates a
`tact::InteractionBroker`, `bootstrap_session` installs it as the session's
interaction service *and* as the serving policy's permission responder (so
`interaction.request` and `permission.request` are answerable through the same
broker, and the broker gained the `withdraw` the old note asked for), and the
driver routes `RuntimeCommand::RespondInteraction` into it before the local
`UiResponder`; headless stays fail-closed by design. What that slice does not do
is convert the *prompt* sites — the TUI popup still reconciles from
`UiResponder::snapshot()` with decimal ids and renders only the select family,
so nothing subscribes to the broker yet and a Kernel prompt is refused rather
than displayed (spec
`docs/superpowers/specs/2026-10-11-kernel-service-consumers-design.md`
§ *Update 3*). **The §7 "timeout" fact now has a producer**: the Kernel's
`guarded` deadline path publishes `RuntimeEvent::TimedOut`
(`crates/tact/src/capability.rs`), and the deadline is placed at the boundary
the decision names — the host ↔ external participant one — rather than on the
user's turn: `chat.submit` / `runs.start` stay unbounded on purpose, while a
hosted plugin's capability call keeps its own bound and the host calls it makes
are bounded by the host's `DEFAULT_REQUEST_TIMEOUT` (`GrantedHostCalls`, so a
Kernel service that never answers fails the host call instead of hanging the
plugin). **The fact now lands**: a hosted plugin's *tool* call is that same
host ↔ external-participant boundary and is the one invocation that is both
deadline-bearing and run-carrying — the wave router puts the host's
`DEFAULT_REQUEST_TIMEOUT` on the invocation that enters `guarded`
(`Agent::tool_call_deadline`) — so a hung plugin tool both fails the call and
publishes `TimedOut` for its run. A plugin's *capability* deadline still lives
inside `HostCapabilityHandler`, below the router's `guarded`, and a host call is
run-less, so neither of those publishes; `TimedOut` requires a `RunId`.

Verification note (2026-10-10): the `tact_ui` integration suites now attach a
serving context, so 10 of the 12 of them drive the **production** run path
(`chat.submit` → `runs.start`) rather than the driver's fallback; the other two
never drive the command loop. No existing assertion needed changing, and the
routed chain proved behaviourally equivalent to the fallback for everything the
suites check. `build_test_agent_without_serving` keeps the fallback covered.

The spec's business-function preservation matrix is now a **map** rather than an
unverified list: each of the fourteen rows names the test that would fail if the
behaviour regressed (twelve do; voice needs audio hardware and the balance half
of token/balance has no end-to-end test, and both rows say so). That closes the
acceptance item "the preservation matrix passes" without adding a duplicate
suite. The driver's fallback path — which the shared builders no longer exercise
— is covered end to end by
`crates/tact_ui/tests/driver_integration.rs::an_agent_without_a_serving_context_still_completes_a_turn`.

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
- Create: `crates/tact_protocol/src/ids.rs`
- Create: `crates/tact_protocol/src/envelope.rs`
- Create: `crates/tact_protocol/src/runtime.rs`
- Create: `crates/tact_protocol/src/capability.rs`
- Create: `crates/tact_protocol/src/interaction.rs`
- Create: `crates/tact_protocol/src/error.rs`
- Modify: `crates/tact_protocol/src/lib.rs`
- Test: `crates/tact_protocol/tests/runtime_protocol.rs`

**Interfaces:**
- Produces `PluginId`, `RequestId`, `SessionId`, `RunId`, `TrajectoryId`, `StepId`.
- Produces `PluginRequest`, `PluginResponse`, `RuntimeEvent`, `RuntimeCommand`, `InteractionRequest`, `InteractionResponse`, `CapabilityDeclaration`, and `ProtocolError`.
- Every envelope carries protocol version, request ID, plugin ID, optional session/run/trajectory IDs, and optional deadline.

- [x] Define serde-stable IDs as newtypes with string serialization and equality/hash behavior.
- [x] Define protocol envelopes and explicit error categories, including retryability and origin.
- [x] Define runtime-neutral event variants for run lifecycle, model boundaries, tool calls, permission, plugin lifecycle, interaction, and terminal outcomes.
- [x] Define capability declarations for Tools, Commands, EventHandlers, Apps, Views, and runtime services.
- [x] Write serialization round-trip tests, unknown variant tests, and reserved-field validation tests.
- [x] Run `cargo test -p tact_protocol --test runtime_protocol`.

### Task 2: Add Kernel service interfaces and invocation context

**Files:**
- Create: `crates/tact/src/mod.rs`
- Create: `crates/tact/src/context.rs`
- Create: `crates/tact_extensions/src/capability.rs`
- Create: `crates/tact/src/error.rs`
- Create: `crates/tact/src/cancellation.rs`
- Modify: `crates/tact/src/lib.rs`
- Test: `crates/tact/src/tests.rs`

**Interfaces:**
- Produces `RuntimeContext`, `CapabilityRouter`, `InvocationContext`, `CancellationService`, and `KernelError`.
- `CapabilityRouter::register`, `CapabilityRouter::describe`, and `CapabilityRouter::invoke` are the only generic capability entry points.
- `InvocationContext` contains request identity, actor, deadline, cancellation token, and access to event, trajectory, permission, and storage services.

- [x] Define traits with object-safe boundaries so in-process and remote hosts share the same invocation path.
- [x] Implement deadline and cancellation propagation using Tokio cancellation primitives.
- [x] Ensure invocation errors preserve origin, retryability, request ID, and plugin ID.
- [x] Add tests for missing capabilities, duplicate registrations, cancelled calls, expired deadlines, and error conversion.
- [x] Run `cargo test -p tact --lib kernel::`.

### Task 3: Extract Permission Engine behind the Kernel boundary

**Files:**
- Modify: `crates/tact_extensions/src/permission/mod.rs`
- Modify: `crates/tact_extensions/src/permission/settings.rs`
- Create: `crates/tact_extensions/src/permission.rs`
- Modify: `crates/tact/src/context.rs`
- Test: `crates/tact/src/permission_tests.rs`

**Interfaces:**
- Produces `PermissionService::check`, `PermissionService::request`, and `PermissionDecision`.
- Consumes existing `PermissionPolicy`, `CapabilityRisk`, settings, and UI-independent `InteractionRequest`.

- [x] Move the decision boundary so native tools, MCP tools, and plugins all call the same service.
- [x] Preserve Auto, Ask, Deny, always-allowed, sensitive-path, shell, and fail-closed behavior.
- [x] Emit permission request, decision, and denial events through RuntimeEvent and Trajectory.
- [x] Test hostile plugin declarations, sensitive paths, denied writes, approval responses, and missing responders.
- [x] Run `cargo test -p tact --lib kernel::permission`.

### Task 4: Implement Event Transport and Trajectory Recorder

**Files:**
- Create: `crates/tact/src/event.rs`
- Create: `crates/tact_trajectory/src/service.rs`
- Create: `crates/tact_trajectory/src/model/event.rs`
- Create: `crates/tact_trajectory/src/model/recorder.rs`
- Create: `crates/tact_trajectory/src/model/sqlite.rs`
- Modify: `crates/tact_extensions/src/store/mod.rs`
- Test: `crates/tact_trajectory/src/model/sqlite.rs` (`#[cfg(test)]` module)

**Interfaces:**
- Produces `EventTransport::publish`, `subscribe`, `replay_from`, and `close`.
- Produces `TrajectoryRecorder::append`, `query`, and `replay`.
- Produces `TrajectoryEvent` with sequence, actor, event type, parent step, payload, and sensitivity.

- [x] Implement monotonic per-trajectory sequence allocation and parent-child step relationships.
- [x] Persist durable events through the existing SQLite store with redaction before persistence.
- [x] Separate transient EventBus delivery from durable Trajectory append; define what is replayable.
- [x] Reject plugin attempts to emit reserved host facts while allowing namespaced plugin events.
- [x] Add bounded channel tests for ordering, replay, sequence gaps, subscriber cancellation, and slow consumers.
- [x] Run `cargo test -p tact --lib trajectory::`.

### Task 5: Add namespaced Minimal Storage facade

**Files:**
- Create: `crates/tact/src/storage.rs`
- Modify: `crates/tact_extensions/src/store/mod.rs`
- Modify: `crates/tact_extensions/src/store/sqlite.rs`
- Test: `crates/tact/src/storage_tests.rs`

**Interfaces:**
- Produces `Storage::get`, `set`, `delete`, `list`, and `transaction`.
- Produces namespace types for `runtime`, `sessions`, `trajectories`, and `plugins/<plugin_id>`.

- [x] Wrap existing SQLite access without changing current database compatibility.
- [x] Enforce namespace ownership at the service boundary.
- [x] Keep existing session, task, team, worktree, token usage, and trajectory records readable during migration.
- [x] Test cross-namespace denial, plugin isolation, transaction rollback, and existing database migration behavior.
- [x] Run `cargo test -p tact --lib store::`.

### Task 6: Define Plugin Lifecycle and Rust Host

**Files:**
- Create: `crates/tact_extensions/src/plugin/manifest.rs`
- Create: `crates/tact_extensions/src/plugin/registry.rs`
- Create: `crates/tact_extensions/src/plugin/lifecycle.rs`
- Create: `crates/tact_extensions/src/plugin/transport.rs`
- Create: `crates/tact_extensions/src/plugin/supervision.rs`
- Modify: `crates/tact_extensions/src/plugin/mod.rs`
- Test: `crates/tact_extensions/src/plugin/lifecycle_tests.rs`

**Interfaces:**
- Produces `PluginRegistry::discover`, `register`, `start`, `stop`, `restart`, `health`, and `unregister`.
- Produces a Rust host that uses the same manifest, protocol, capability, permission, event, and trajectory paths as remote hosts.

- [x] Validate plugin IDs, protocol versions, dependencies, capability declarations, and reserved names.
- [x] Implement lifecycle events and health state transitions.
- [x] Ensure in-process Rust extensions do not bypass permission or trajectory recording.
- [x] Test duplicate IDs, version mismatch, dependency failure, crash state, restart, shutdown drain, and in-flight request failure.
- [x] Run `cargo test -p tact --lib plugin::`.

### Task 7: Unify native tools and MCP through Capability Router

**Files:**
- Modify: `crates/tact_extensions/src/tool/registry.rs`
- Modify: `crates/tact_extensions/src/tool/metadata.rs`
- Modify: `crates/tact_extensions/src/agent/tool_dispatch.rs`
- Modify: `crates/tact_extensions/src/mcp/mod.rs`
- Modify: `crates/tact_extensions/src/mcp/prompt.rs`
- Create: `crates/tact_extensions/src/capability/native_tool.rs`
- Create: `crates/tact_extensions/src/capability/mcp_tool.rs`
- Test: `crates/tact_extensions/src/capability/router_tests.rs`

**Interfaces:**
- Produces one `CapabilityRouter::invoke` path for native, MCP, and future plugin tools.
- Preserves `ToolMetadata`, `PermissionPolicy`, `ResourcePolicy`, `ToolPresentation`, `ToolEffect`, and stable external names.

- [x] Adapt native `Tool` implementations without changing their public LLM names or metadata.
- [x] Adapt `mcp__<server>__<tool>` and MCP prompt/resource routing through protocol capabilities.
- [x] Route execution through one `CapabilityRouter::invoke` path, consume one-use preflight authorization in `PermissionService`, carry output/effects through the adapter result, and record tool lifecycle through event/trajectory services. Keep Agent resource-wave scheduling and TUI presentation projection in their current compatibility owners until Tasks 8/9 replace those paths.
- [x] Test native tool success/failure/effect application, MCP namespacing, unknown tools, and fail-closed privileges.
- [x] Run `cargo test -p tact --lib capability::`.

### Task 8: Move Agent and Session behind extension interfaces

**Files:**
- Modify: `crates/tact_extensions/src/agent/mod.rs`
- Modify: `crates/tact_extensions/src/agent/tool_schedule.rs`
- Modify: `crates/tact_extensions/src/compact/mod.rs`
- Modify: `crates/tact_extensions/src/store/session_store/mod.rs`
- Create: `crates/tact_extensions/src/extensions/agent.rs`
- Create: `crates/tact_extensions/src/extensions/session.rs`
- Test: `crates/tact_extensions/src/extensions/agent_tests.rs`
- Test: `crates/tact_extensions/src/extensions/session_tests.rs`

**Interfaces:**
- Produces Agent and Session extension entry points that consume `RuntimeContext` and return protocol events/results.
- Removes direct Agent dependencies on TUI channels and concrete responder types.

- [x] Move model loop output to RuntimeEvent/EventTransport while preserving streaming, thinking, tool progress, stop reasons, and error handling.
- [x] Route compaction, transcript persistence, provider recovery, sub-agents, and background work through Kernel services.
- [x] Preserve session locking, resume semantics, token usage, and existing SQLite records.
- [x] Test normal run, cancellation, compaction, transport recovery, resume, sub-agents, and headless execution.
- [x] Run focused tests one invocation at a time: `cargo test -p tact --lib agent::`, then `cargo test -p tact --lib extensions::`.

### Task 9: Convert UI responder and TUI to View / Interaction adapters

**Files:**
- Create: `crates/tact/src/interaction.rs`
- Modify: `crates/tact_extensions/src/ui_responder.rs`
- Modify: `crates/tact_ui/src/session_bootstrap.rs`
- Modify: `crates/tact_ui/src/driver.rs`
- Modify: `crates/tact_ui/src/interactive.rs`
- Modify: `crates/tui/src/lib.rs`
- Modify: `crates/tui/src/state/`
- Test: `crates/tact/src/interaction_tests.rs`
- Test: existing TUI event/render tests requiring protocol updates

**Interfaces:**
- Produces client-neutral `InteractionRequest` and `InteractionResponse`.
- TUI consumes RuntimeEvent and emits RuntimeCommand without calling Agent methods.

- [x] Convert select, multi-select, confirm, input, permission, popup, stream, and completion flows to neutral interactions.
- [x] Preserve every existing TUI rendering and key-binding outcome, including tool cards, popups, themes, i18n, scroll behavior, and bottom-bar usage data.
- [x] Keep headless mode as an External Client adapter with deterministic responses and exit codes.
- [x] Test interaction ordering, cancellation, TUI event projection, and headless behavior with bounded timeouts.
- [x] Run `cargo test -p tui --lib` followed by `cargo test -p tact-ui --lib` sequentially.

### Task 10: Register official Agent, Session, Chat, Tools, and Workflow extensions

**Files:**
- Create: `crates/tact_extensions/src/extensions/mod.rs`
- Create: `crates/tact_extensions/src/extensions/chat.rs`
- Create: `crates/tact_extensions/src/extensions/tools.rs`
- Create: `crates/tact_extensions/src/extensions/workflow.rs`
- Modify: `crates/tact_extensions/src/tool/mod.rs`, `crates/tact_extensions/src/tool/registry.rs`, and tool metadata modules for extension registration
- Test: `crates/tact_extensions/src/extensions/official_extensions_tests.rs`

**Interfaces:**
- Produces official extension manifests and registrations using the same Plugin Protocol as third-party extensions.
- Chat owns conversational projection and commands; Tools owns current tool families; Workflow owns orchestration.

- [x] Register built-in extensions through PluginRegistry and CapabilityRouter.
- [x] Preserve current command names, tool names, hooks, memory, skills, tasks, teams, worktrees, voice, and background behavior.
- [x] Test extension enable/disable, replacement, duplicate capability handling, and a Chat instance using a different View adapter.
- [x] Run `cargo test -p tact --lib extensions::`.

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

- [x] Implement process startup, handshake, manifest validation, stdout/stderr separation, deadlines, cancellation, and shutdown drain.
- [x] Route Node calls through the same Capability Router, Permission Engine, Event Transport, and Trajectory Recorder.
- [x] Detect crashes and fail in-flight calls without terminating unrelated Runtime work.
- [x] Test registration, event replay, permission denial, timeout, cancellation, malformed messages, crash recovery, and protocol mismatch.
- [x] Run `cargo test -p tact_plugin_node --test node_host`; the test launches the checked-in `index.mjs` fixture directly, so no second test runner is required.

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

- [x] Implement manifest and protocol validation before instantiation.
- [x] Mediate storage, event, trajectory, clock, and declared external capabilities through host functions.
- [x] Enforce memory, fuel/deadline, cancellation, and permission limits.
- [x] Test denied filesystem/network/process access, timeout, interruption, namespaced events, and instance cleanup.
- [x] Run `cargo test -p tact_plugin_wasm --test wasm_host`.

### Task 13: Remove legacy direct paths and update documentation

**Files:**
- Modify: `crates/tact_extensions/src/agent/mod.rs`, `crates/tact_extensions/src/agent/tool_dispatch.rs`, and `crates/tact_extensions/src/agent/tool_schedule.rs` for remaining direct UI sends
- Modify: `crates/tact_extensions/src/mcp/mod.rs`, `crates/tact_extensions/src/mcp/prompt.rs`, `crates/tact_extensions/src/mcp/resource.rs`, and `crates/tact_extensions/src/mcp/remote.rs` for remaining special invocation paths
- Modify: `crates/tact_extensions/src/ui_responder.rs` or delete once migrated
- Modify: `ARCHITECTURE.md`
- Create: `docs/plugin_protocol.md`
- Create: `docs/trajectory.md`
- Modify: `book/26_chapter_issue_zh.md` if user-visible behavior changed
- Test: repository dependency and protocol integration tests

- [x] Remove every Agent-to-TUI channel and replace it with RuntimeEvent/EventTransport.
- [x] Remove MCP-only execution branches and route all external capabilities through Plugin Protocol.
- [x] Remove compatibility types only after all consumers use neutral protocol types.
- [x] Update architecture diagrams and module tables to match the final dependency graph.
- [x] Add integration tests proving Kernel builds without UI, a plugin can run without Chat, and a View can be replaced.
- [x] Run `cargo test --workspace` once, sequentially, after all focused tests pass.
- [x] Run `git diff --check` and inspect the final dependency graph before declaring completion.

### Task 14: Align the crate taxonomy with the layered architecture

**Files:**
- Create: `crates/tact_trajectory/`, `crates/tact_plugin_host/`, `crates/tact_extensions/`
- Rename: `crates/protocol` → `crates/tact_protocol`, `crates/tact-ui` → `crates/tact_ui`
- Repurpose: `crates/tact` as the Runtime Kernel

**Interfaces:**
- `tact` (Kernel) depends on `tact_protocol` only; no frontend, no extension.
- `tact_plugin_host` and `tact_trajectory` depend on the Kernel; `tact_plugin_node`
  and `tact_plugin_wasm` depend on `tact_plugin_host` and never on
  `tact_extensions`.

- [x] Move the Kernel services out of the extension crate: `tact` is now the
      Runtime Kernel (capability router, permission boundary, events, minimal
      storage, cancellation/timeout/error, plugin registry, redaction, path
      matching).
- [x] Extract `tact_trajectory` for the execution-fact model, in-memory and
      SQLite recorders, and ordered replay; it implements the Kernel's
      `TrajectoryService`.
- [x] Extract `tact_plugin_host` for the lifecycle boundary, stdio transport,
      and supervision; `tact_plugin_node` and `tact_plugin_wasm` are thin
      entry points above it and no longer depend on each other's crate.
- [x] Rename the extension crate to `tact_extensions` (official Agent, Session,
      Chat, Tools, Workflow extensions plus the in-process Rust host).
- [x] Delete the first-pass duplicate hosts (`tact/src/plugin/{node,wasm}.rs`)
      that the dedicated host crates replaced.
- [x] Update the push gate to run `cargo test --workspace` so every crate in the
      taxonomy is covered.
- [x] Sync `ARCHITECTURE.md` §0/§15, `docs/plugin_protocol.md`, `docs/trajectory.md`,
      and every `crates/` path referenced from `book/`.

## Completion gate

The migration is complete only when the acceptance requirements in the spec pass and the following scenarios work end to end:

1. Headless External Client starts an Agent run, observes streamed events, answers permission, and reads the final Trajectory.
2. TUI performs the same run using only View / Interaction APIs and renders the existing experience.
3. A Node Chat plugin starts a run and receives the same Runtime events as TUI.
4. A plugin crash affects its run but leaves the Kernel and another run healthy.
5. A disconnected client reconnects from a Trajectory sequence without duplicated or missing durable facts.
6. Existing MCP and native tools preserve names, permissions, typed effects, output persistence, and user-visible results.
7. Compaction, recovery, sessions, sub-agents, tasks, teams, memory, skills, worktrees, background work, voice, and token accounting remain available through the new extension boundaries.
