# Tact Runtime Kernel and Plugin Architecture Design

## Status

Approved design baseline for implementation planning.

## Goal

Evolve Tact into a minimal Runtime Kernel with a stable language-neutral Plugin Protocol, while preserving all current user-visible business capabilities. Agent, Session, Chat, Tools, Commands, and Workflow become replaceable extensions; TUI, Web, Desktop, and external clients become protocol-driven view and interaction adapters.

The final code dependency graph must match this architecture:

```text
Runtime Kernel
  -> Plugin Protocol
  -> Rust / Node.js / WASM Plugin Hosts
  -> Extension Capability API
  -> Agent / Session / Chat / Tools / Commands
  -> Views / Interaction API
  -> TUI / Web / Desktop / External Client
```

The Kernel must not depend on TUI, Chat, a particular Agent implementation, or a plugin language.

## Non-goals

- Do not redesign the model-provider behavior, prompt semantics, or existing tool user experience as part of the architecture migration.
- Do not remove existing capabilities merely to make the Kernel smaller.
- Do not require all plugins to provide a custom UI for every client.
- Do not expose Rust internal structs as the cross-language API.
- Do not treat MCP as a permanent special case outside the Plugin Protocol.

## Current implementation baseline

The current workspace already contains most business capabilities:

| Current area | Responsibility to preserve | Target owner |
|---|---|---|
| `crates/tact_extensions/src/agent/` | Streaming model loop, tool dispatch, compaction, recovery | Agent extension using Kernel services |
| `crates/tact_extensions/src/tool/` | Native tools, metadata, tool effects, task/team/memory/worktree tools | Tools extensions through Capability Router |
| `crates/tact_extensions/src/mcp/` | MCP discovery, connection, tool and prompt routing | Plugin Protocol adapter / external plugin host |
| `crates/tact_extensions/src/permission/` | Permission modes, risk classification, user approval | Kernel Permission Engine |
| `crates/tact_extensions/src/hook/` | Session, pre-tool, post-tool and lifecycle hooks | Event Handler capability |
| `crates/tact_extensions/src/store/` | SQLite persistence, sessions, tasks, teams, worktrees, token usage | Kernel Storage plus extension namespaces |
| `crates/tact_extensions/src/compact/` | Micro-compaction, transcript persistence, Codex-style recovery | Agent extension using Trajectory and Storage |
| `crates/tact_extensions/src/ui_responder.rs` | Select, multi-select and user interaction requests | Kernel Interaction API |
| `crates/tact_protocol/` | Shared wire types and UI update types | Language-neutral Runtime / Plugin Protocol types |
| `crates/tui/` | Ratatui event loop, state and rendering | TUI View Adapter |
| `crates/tact_ui/` | CLI startup, host wiring, interactive and headless modes | Runtime host and client wiring |

Existing behavior remains the acceptance baseline: streaming output, thinking and tool progress, permissions, compaction, recovery, MCP tools, hooks, sub-agents, teams, tasks, background work, memory, skills, worktrees, voice support, token usage, sessions, themes, internationalization, popups, command palette, headless execution, and all existing safety checks.

## Target architecture

### 1. Runtime Kernel

The Kernel contains mechanisms that must be consistent for every extension and client:

```text
crates/tact/src/
  lifecycle.rs       # plugin registration, start, stop, restart
  capability.rs      # capability handles and invocation context
  permission.rs      # centralized permission decisions
  event.rs           # event transport, subscriptions, sequencing
  trajectory.rs      # recording and querying execution facts
  storage.rs         # namespaced storage facade
  protocol.rs        # version negotiation and envelope validation
  cancellation.rs    # cancellation tokens and deadlines
  error.rs           # stable error categories and conversion
  interaction.rs     # client-neutral user interaction requests
```

The Kernel owns no Chat message layout, TUI widget, Agent implementation, or concrete business tool. It provides the following services:

- `PluginLifecycle`: manifest validation, dependency checks, startup, shutdown, restart, health and crash reporting.
- `CapabilityRouter`: capability declaration, discovery, authorization and invocation.
- `PermissionEngine`: uniform decisions for host and plugin capabilities.
- `EventTransport`: publish, subscribe, sequence, replay and backpressure policy.
- `TrajectoryRecorder`: append-only execution facts with stable IDs and ordering.
- `MinimalStorage`: namespaced key/value and durable record access.
- `CancellationService`: cancellation propagation and deadline enforcement.
- `ErrorModel`: structured, serializable errors with retryability and origin.
- `ProtocolVersioning`: major/minor negotiation and feature discovery.

### 2. Plugin Protocol

The protocol is the only stable boundary between the Kernel and a Plugin Host. It must be serializable over stdio, IPC, or an in-process transport and must not contain Rust-specific types.

Core envelope fields:

```text
protocol_version
request_id
plugin_id
session_id (optional)
run_id (optional)
trajectory_id (optional)
deadline (optional)
```

Core messages:

```rust
enum PluginRequest {
    Handshake(HandshakeRequest),
    Register(RegisterRequest),
    Invoke(InvokeRequest),
    Subscribe(SubscribeRequest),
    Cancel(CancelRequest),
    InteractionResponse(InteractionResponse),
    Shutdown,
}

enum PluginResponse {
    HandshakeAccepted(HandshakeAccepted),
    Registered(RegistrationResult),
    Result(InvocationResult),
    Event(RuntimeEvent),
    Error(ProtocolError),
}
```

The protocol must define:

- manifest and capability discovery;
- request/response correlation;
- structured input and output values;
- streaming event delivery;
- cancellation and timeout;
- protocol and capability version negotiation;
- reconnect and replay from an event sequence;
- plugin-originated events with origin metadata;
- explicit error categories;
- shutdown and crash semantics.

`AgentUpdate`, `UserCommand`, and TUI-specific responder variants are migration inputs, not final protocol types. They are replaced by `RuntimeEvent`, `RuntimeCommand`, and `InteractionRequest`.

### 3. Plugin Hosts

Every host implements the same lifecycle and protocol contract:

```text
Plugin Host
  discover -> validate -> handshake -> register -> serve -> drain -> stop
```

#### Rust Plugin Host

The Rust host may support built-in in-process extensions first, but it must use the same manifest, capability registry, permission checks, events, and trajectory hooks as external hosts. In-process status must not create a privileged bypass.

#### Node.js Plugin Host

Node plugins run in an independent Node process connected through stdio RPC or IPC. The host owns process startup, environment setup, protocol transport, deadlines, cancellation, crash detection, restart policy and capability filtering. Node code never receives a Rust object reference.

#### WASM Plugin Host

WASM plugins run under a WASI/Wasmtime-style host boundary. File, network, process, clock and storage access are host functions mediated by capabilities and permissions. WASM must not access Runtime memory directly.

> **Implementation note (2026-10-10).** The shipped `crates/tact_plugin_wasm` is a
> *subprocess* host: it spawns a caller-configured runner executable over the same
> stdio envelope protocol and passes fuel / linear-memory / timeout / WASI
> restrictions as runner arguments. No WASM engine is linked into this
> repository, so those limits are enforced only by the configured runner, and
> host functions are reached through the negotiated `HostCall` protocol rather
> than a Wasmtime `Linker`. Set `TACT_WASM_RUNNER` to a real runner (e.g. a
> `wasmtime` CLI) to get the boundary described here.

### 4. Extension Capability API

Capabilities are divided into contributions and services.

Plugin contributions:

```text
Tool
Command
EventHandler
App
View
```

Kernel services:

```text
runs.start / runs.cancel
sessions.read / sessions.write
events.subscribe / events.publish
trajectory.read / trajectory.append_plugin_event
storage.get / storage.set
permission.request
interaction.request
```

All invocations follow one path:

```text
Plugin request
  -> Capability Router
  -> Permission Engine
  -> Invocation Context
  -> implementation
  -> Runtime Event
  -> Trajectory Recorder
  -> response
```

Capabilities are named by semantic authority rather than implementation language, for example:

```text
filesystem.read
filesystem.write
network.request
process.spawn
session.read
session.write
trajectory.read
```

### 5. Official extensions

Agent, Session, Chat, Tools, and Workflow are official extensions using the same API as third-party extensions.

- **Agent** owns model calls, context construction, tool selection, streaming, retry, compaction and transport recovery.
- **Session** owns session identity, resume, message association and session lifecycle.
- **Chat** owns conversational input, message projection, chat commands and chat-specific interaction semantics.
- **Tools** contribute filesystem, shell, task, team, memory, skill, worktree, background and other existing capabilities.
- **Workflow** owns multi-step orchestration and future task graphs.

The Kernel may ship default extensions for a complete product, but their APIs must remain replaceable and independently testable.

### 6. Views and Interaction API

Views consume structured Runtime events and submit Runtime commands or interaction responses. A View never calls Agent internals or tool handlers.

```rust
enum InteractionRequest {
    Permission(PermissionRequest),
    Select(SelectRequest),
    Confirm(ConfirmRequest),
    Input(InputRequest),
}

enum InteractionResponse {
    Approved,
    Rejected,
    Selected(Vec<String>),
    Text(String),
    Cancelled,
}
```

The adapters are:

```text
TUI             -> ratatui projection and input adapter
Web             -> browser/API projection and input adapter
Desktop         -> native/webview projection and input adapter
External Client -> headless/API protocol client
```

All adapters must support the common minimum: start a run, observe progress, answer interaction requests, cancel a run, inspect results, and resume from a trajectory sequence.

### 7. Trajectory

Trajectory is a first-class Kernel capability and the source of truth for execution history. It is separate from transient EventBus delivery.

```rust
struct TrajectoryEvent {
    trajectory_id: TrajectoryId,
    run_id: RunId,
    sequence: u64,
    timestamp: DateTime<Utc>,
    actor: ActorId,
    event_type: TrajectoryEventType,
    parent_step_id: Option<StepId>,
    payload: JsonValue,
    sensitivity: Sensitivity,
}
```

Trajectory must record user input, model-call boundaries, tool calls and results, permission decisions, hook execution, plugin lifecycle, errors, retries, cancellation, timeout, compaction, recovery, and parent/child relationships for parallel work.

The Kernel may reject plugin attempts to emit reserved host facts such as `permission.granted` or `tool.completed`. Plugins may append namespaced facts such as `plugin.<plugin_id>.progress`.

Trajectory storage must support append, query by run/session, sequence-based resume, and replay. Sensitive payloads use the existing redaction/security policy before persistence.

### 8. Storage

The existing SQLite store remains the initial implementation. Access is exposed through a namespaced facade:

```text
runtime/
sessions/
trajectories/
plugins/<plugin_id>/
```

Runtime-owned records cannot be written through a plugin namespace. Plugin data cannot be written into Runtime, Session, or Trajectory namespaces except through explicit capability methods.

## Business-function preservation matrix

The migration is complete only when the following behavior remains available through the new boundaries:

| Capability | Final extension or service | Preservation requirement | Covered by (test that would fail if this regressed) |
|---|---|---|---|
| Streaming agent loop | Agent extension | Same provider behavior, streamed text, thinking and stop/error handling | `crates/tact_ui/tests/driver_integration.rs` (a turn driven through `chat.submit` → `runs.start`); streaming/thinking assertions in `crates/tact_extensions/src/agent/mod.rs` |
| Native tools | Tools extensions | Same names, metadata, permission/resource policies and typed effects | `crates/tact_ui/tests/tool_integration.rs` |
| MCP tools/prompts/resources | Plugin Protocol adapter | Same discovery, namespacing, routing and failures | `crates/tact_ui/tests/mcp_tools.rs`, `mcp_auth_url_progress.rs` |
| Hooks | EventHandler capability | Same lifecycle timing, mutation and veto semantics | `crates/tact_extensions/src/plugin/hooks.rs` tests; hook timing/veto in `crates/tact_extensions/src/agent/mod.rs`; `headless::tests::headless_stop_hook_continuation_runs_a_second_turn` |
| Permissions | Kernel Permission Engine | Same modes, risk decisions, prompts and fail-closed behavior | `crates/tact_ui/tests/permission_integration.rs`; `crates/tact_extensions/src/agent/tool_dispatch.rs` permission-decision tests |
| Sessions/resume | Session extension + Storage | Same persistence, locking, resume and compatibility | `crates/tact_ui/tests/headless_session_integration.rs`; `crates/tact_extensions/src/extensions/session_tests.rs` |
| Compaction/recovery | Agent extension + Trajectory | Same compaction triggers, transcript behavior and transport recovery | `crates/tact_ui/tests/recovery_compaction.rs` |
| Tasks/teams/subagents | Tools/Workflow extensions | Same persistence, background execution and user-visible results | `crates/tact_ui/tests/subsystem_tools.rs` |
| Memory/skills/worktrees | Tools extensions | Same storage, prompt integration and safety checks | `crates/tact_ui/tests/subsystem_tools.rs` |
| Background processes | Tool/Workflow extension | Same cancellation, output and cleanup behavior | `crates/tact_ui/tests/subsystem_tools.rs`, `tool_integration.rs` |
| Voice | Chat/View extension | Same recording/transcription integration where supported | **no end-to-end test** — needs audio hardware. Unit-level coverage only: `crates/tact_extensions/src/voice/` |
| Token/balance statistics | Agent/Session projection | Same accounting and display data | `crates/tact_ui/tests/tool_integration.rs` (token usage), `harness_advanced.rs` |
| TUI rendering | TUI View Adapter | Same visual behavior and interaction outcomes | `crates/tui/src/render/{scene_tests,log_render_tests,render_gap_tests,popup_scene_tests}.rs` |
| Headless mode | External Client adapter | Same non-interactive execution and exit behavior | `crates/tact_ui/tests/headless_session_integration.rs`, `headless_tui_advanced.rs` |

Thirteen of the fourteen rows name a real suite; voice is the one row with no
end-to-end test. Ten of the twelve `tact_ui` integration suites drive the
**production** run path (`chat.submit` → `runs.start`) rather than the driver's
fallback (the other two never drive the command loop), so this map is evidence
about the migrated path and not only about the legacy one.

## Error, timeout and cancellation model

Every invocation has a request ID and may have a deadline. Cancellation is propagated from client to extension to tool or plugin. The Kernel records request, cancellation and terminal outcome in Trajectory.

Error categories are stable and serializable:

```text
InvalidRequest
ProtocolMismatch
CapabilityNotFound
PermissionDenied
Timeout
Cancelled
PluginUnavailable
PluginCrashed
ProviderError
ToolError
StorageError
InternalError
```

Errors must identify origin and retryability. A plugin crash fails its in-flight calls and emits a lifecycle event; it must not terminate the Runtime or unrelated runs.

## Migration strategy

Migration is incremental in implementation order but final in dependency direction. No new feature may add a permanent Agent-to-TUI or MCP-special-case dependency.

1. **Protocol foundation:** add neutral IDs, envelopes, Runtime events, commands, interactions, errors and capability declarations.
2. **Kernel services:** extract permission, event transport, cancellation, storage facade and trajectory recording behind interfaces.
3. **Unified capability routing:** adapt native tools and MCP tools to one registration and invocation path.
4. **Agent and Session boundaries:** move Agent loop and Session lifecycle behind service interfaces; remove direct TUI channels.
5. **Interaction and View adapters:** convert `ui_responder` and TUI wiring to the common interaction protocol; preserve all existing render behavior.
6. **Rust extensions:** register built-in Agent, Session, Chat and Tools using the same protocol and permission path.
7. **Node host:** add independent Node process transport and a minimal Chat plugin proving registration, run start, event subscription and interaction response.
8. **WASM host:** add constrained execution and a minimal capability plugin.
9. **Removal and verification:** delete old direct channels and special paths; verify dependency graph, business-function matrix and failure semantics.

## Final module and crate shape

The intended end state is:

```text
crates/
  tact/                    # Runtime Kernel and host orchestration
  tact_protocol/           # Language-neutral wire and domain types
  tact_plugin_host/        # Lifecycle, transport and supervision
  tact_plugin_node/        # Node.js host transport
  tact_plugin_wasm/        # WASM host transport
  tact_trajectory/         # Trajectory model, recorder, store, replay
  tact_extensions/         # Official Agent/Session/Chat/Tools/Workflow
  tact_ui/                 # Runtime host and external-client wiring
  tui/                     # TUI View Adapter
```

The exact crate split may be delayed until interfaces stabilize, but the dependency direction is mandatory. A temporary module may remain in an existing crate only if it already obeys the final boundary and has a stated extraction task.

## Verification and acceptance

Architecture acceptance requires:

- Kernel builds and runs without `tui`, `tact-ui`, Chat, or a specific Agent implementation.
- Runtime can execute headlessly through the protocol.
- Chat can be stopped, replaced, or run in multiple instances.
- Rust, Node.js and WASM use the same protocol concepts and permission model.
- Every capability invocation passes through the Capability Router and Permission Engine.
- Every execution fact is available through Trajectory query and replay.
- TUI, Web, Desktop and External Client use only Views / Interaction API.
- Plugin crashes, timeouts, cancellation and reconnect do not corrupt unrelated runs.
- Reserved host trajectory and permission facts cannot be forged by plugins.
- Existing business-function preservation matrix passes.
- The code dependency graph has no hidden Agent-to-TUI direct path and no MCP-only execution path.

## Implementation status (2026-10-10)

The target above is unchanged. This section records what the shipping binary
actually does, so the acceptance list is not read as a description of the
current state. It is based on a read-only audit of the whole spec plus the work
landed since; the evidence lives in `ARCHITECTURE.md` §15 and the committed
history. "Reachable, unconsumed" means the mechanism exists on a real path but
no product code invokes it yet.

| Area | Status |
|---|---|
| §1 Kernel module shape and dependency graph | implemented (`crates/tact` depends only on `tact_protocol`) |
| §1 services | implemented; `tact::services::register` runs in the session bootstrap over real event / trajectory / storage / permission backing, so the §4 service capabilities are reachable. Members with no consumer are listed in `ARCHITECTURE.md` §15 |
| §2 envelopes, messages, error categories | implemented; the 12 categories match this document exactly. `ResponseEnvelope` carries no `deadline`, and no production invocation sets one |
| §2 cancellation | `PluginRequest::Cancel` is defined but never sent; cancellation is process termination. On the host path `runs.cancel` deliberately does not publish the `Cancelled` fact itself — the chat turn records it when the cancelled run returns, so one user cancel yields exactly one event. A caller that drives a run through `runs.start` without a chat turn must record its own terminal state |
| §2 reconnect / replay from a sequence | **deferred**: `EventTransport::replay_from` answers from the recorder, but nothing calls it, and `RuntimeCommand::{Subscribe, Resume}` have no consumer |
| §3 Rust host | implemented; no product caller |
| §3 Node host | implemented (stdio, handshake, deadlines, crash detection, shutdown drain); no product caller |
| §3 WASM host | a **subprocess runner, not an embedded engine** — no WASM engine is linked, fuel/memory/WASI limits are runner arguments, host functions travel over the negotiated protocol. See the implementation note in §3 |
| §4 contribution `Tool` | implemented end to end |
| §4 contributions `App` / `Command` | both hosts submit a turn through `chat.submit` (Chat), which invokes `runs.start` (Agent) once per run; cancel goes through `runs.cancel` and `/compact` through `chat.compact`, so both are registered and invoked on the production router; `workflow.run` is registered only in tests |
| §4 contributions `EventHandler` / `View` | **not implemented** — hooks remain the `hook` subsystem and no View capability exists |
| §4 Kernel services | registered and reachable with real backing. **Two of the twelve have an invoking caller** — `runs.start` (through `chat.submit`) and `runs.cancel` (the driver's cancel arms); the other ten are registered but uninvoked. Note the split: `tact::services::register` installs **8** of them (`storage.*`, `events.*`, `trajectory.*`, `permission.request`, `interaction.request`); `runs.*` and `sessions.*` come from the Agent and Session extensions. `permission.request` / `interaction.request` also fail closed (no responder is wired) |
| §4 semantic-authority names (`filesystem.read`, …) | not implemented; the shipped model is one capability per tool |
| §5 Agent / Session / Tools | implemented as extensions and registered in production |
| §5 Chat / Workflow | **Chat owns the conversational turn and its three commands**: `chat.submit` assembles the user message (including `@`-file / image references), resets the turn, drives the run, runs the Stop-hook continuation loop, and emits `TaskComplete` / fires TaskCompleted hooks; `chat.compact` compacts; cancelling goes through `runs.cancel` (the Agent extension). `chat.start_run` is still served by the Agent extension. Workflow has an interface with a test-only executor |
| §6 Interaction API | partial: the TUI consumes Runtime events and both hosts start runs through the protocol, but prompts still go through `tact_extensions::ui_responder::UiResponder`. The Kernel broker cannot back them yet — `InteractionService::request` takes an `InvocationContext` the prompt sites do not have, and the broker itself has no `withdraw`. (The *shipped* prompt path does not leak ghost prompts: `UiResponder::withdraw` exists and the Kernel broker already cleans up a dropped waiter through `PendingRequestGuard`, so the missing `withdraw` is about making the broker the production path, not about a live defect) |
| §6 adapters | TUI and headless exist; **Web and Desktop do not**. Headless drives the in-process router (not an envelope-speaking client), so "protocol client" means "starts runs through the capability protocol", not "speaks `RequestEnvelope` over a transport" |
| §7 Trajectory | recording, ordering, query and redaction are implemented. User input, tool calls, hooks, permissions, errors, retries, cancellation, compaction and recovery all have producers. Plugin-lifecycle facts have none, and timeout facts are unreachable because no production invocation carries a deadline |
| §7 replay | deferred (see §2) |
| §8 Storage | the facade and its namespace guard are real and now sit on the serving router; production session / task / team / worktree persistence still uses the existing stores directly, and Runtime-owned records are not written through the facade |
| Error / timeout / cancellation model | error categories match and carry origin and retryability; cancellation propagates from client to handler; no production deadline exists |
| Business-function preservation matrix | **verified by mapping**: thirteen of the fourteen rows name the suite that would fail if the behaviour regressed (table above), and ten of the twelve `tact_ui` integration suites drive the production path. Voice has no end-to-end test (audio hardware) and is covered at unit level only |

### Acceptance items that are aspirational, not demonstrated

- "Every execution fact is available through Trajectory query and replay" — replay has no consumer.
- "TUI, Web, Desktop and External Client use only Views / Interaction API" — Web and Desktop do not exist, and the TUI's prompt path is the in-process responder.
- "Reserved host trajectory and permission facts cannot be forged by plugins" — the guard is implemented and tested, but no plugin host has a product caller, so it cannot fire in a real session.
- "Plugin crashes, timeouts, cancellation and reconnect do not corrupt unrelated runs" — proven at host-test level only.

(The business-function preservation matrix is no longer on this list: thirteen of its fourteen rows name the suite that would fail if the behaviour regressed, and ten of the twelve `tact_ui` integration suites drive the production path. Voice remains unit-level only, for want of audio hardware. A single matrix runner over those suites would be a convenience, not new coverage.)

### Not planned for now (revisit when a second View or a plugin needs them)

- Web and Desktop View adapters.
- A second Chat implementation and a production Workflow executor.
- Migrating the existing stores onto the `MinimalStorage` facade.
- Semantic-authority capability naming.
- `EventHandler` and `View` contributions.

## Documentation synchronization

Implementation work must update `ARCHITECTURE.md` to match the final dependency graph, add protocol and trajectory documentation under `docs/`, and add a newest-first user-visible migration entry to `book/26_chapter_issue_zh.md` if behavior or configuration changes are observable.
