# Tact Agent SDK Architecture

> Date: 2026-10-28
> Status: implemented (2026-10-28)
> Related: `crates/agent_api/`, `crates/agent_core/`, `crates/tact_llm/`,
> `ARCHITECTURE.md`
> Plan: `docs/superpowers/plans/2026-10-28-agent-sdk.md`

## Goal

Add `tact_agent_sdk` as a small, ergonomic Rust SDK over the existing
`tact_agent_api` contracts and `tact_agent_core` runtime.

The SDK must make the kernel usable without copying seven capability
implementations into every host:

- a model is required;
- tools, permissions, context policy, persistence, usage, and events have safe
  defaults;
- one builder can create a reusable agent or a ready-to-run session;
- a session owns the host-owned `AgentState` and provides a small run API;
- telemetry is injected through tracing-compatible instrumentation without
  making the kernel depend on `tracing`;
- the design remains plugin-style: capabilities are injected at construction,
  not discovered through a runtime registry.

The SDK is a facade and adapter layer, not a second agent runtime.

## Non-Goals

- Do not change `tact_agent_api` contracts or `tact_agent_core` control flow.
- Do not make the SDK depend on the full `tact` product crate.
- Do not put SQLite, MCP transport, marketplace plugins, TUI rendering, or
  provider HTTP clients into the SDK dependency graph.
- Do not add a runtime `RuntimePlugin` trait, plugin registry, plugin id,
  install/enable/freeze lifecycle, or dynamic loader.
- Do not install a process-global `tracing` subscriber or choose a telemetry
  backend for the host.
- Do not turn `AgentUpdate` into the SDK event contract. `AgentEvent` remains the
  neutral runtime event type.
- Do not implement product compaction, prompt discovery, skills, memory, Goal
  Runner, or permission policy in the SDK.
- Do not bridge the product `ToolRouter` or `#[tool]` macro into the SDK in the
  first version. Product hosts either pass their own `ToolPort` or wrap product
  tools behind the SDK's static `ToolHandler` contract.
- Do not re-export `tact_llm`. A host that wants a concrete provider adds
  `tact_llm` directly and passes the resulting `ModelPort` to the builder.

## Package Boundary

```text
tact_agent_sdk
  -> tact_agent_api
  -> tact_agent_core
  -> async-trait
  -> futures-util
  -> serde_json
  -> thiserror
  -> uuid
  -> tracing        optional, via the telemetry feature
```

The SDK does not depend on:

```text
tact
tact_llm
tact_protocol
tact-ui
tui
sqlx
rmcp
reqwest
ratatui
async-openai
async-openai-local
```

`tact_agent_core` remains the authority for one bounded activation.
`tact_agent_sdk` owns only composition, defaults, session ergonomics, and
instrumentation.

## Public Usage

### Reusable Agent

```rust
let agent = AgentBuilder::new(model)
    .system_prompt("You are a concise research agent.")
    .model_options(ModelCallOptions {
        reasoning_effort: Some(ReasoningEffort::Medium),
        ..ModelCallOptions::default()
    })
    .tools(Arc::new(tools))
    .gate(Arc::new(AllowAllGate))
    .max_total_turns(24)
    .build()?;

let mut first = agent.session("first");
let mut second = agent.session("second");

first.run_text("Summarize the architecture.").await?;
second.run_text("List the open questions.").await?;
```

### Direct Session Construction

```rust
let mut session = AgentBuilder::new(model)
    .system_prompt("You are a helpful agent.")
    .max_total_turns(Some(24))
    .build_session("chat-1")?;

let outcome = session.run_text("hello").await?;
let answer = session.last_assistant_message();
```

### Restored Session

```rust
assert!(saved_state.initialized);

let mut session = AgentBuilder::new(model)
    .build_session_from_state("chat-1", saved_state)?;
```

`build_session_from_state` accepts only initialized state. An uninitialized state
is an error, not a request to merge a partial transcript with durable storage.

### Custom Tools

```rust
let tools = ToolSet::new()
    .add(SearchTool)?
    .add(ReadFileTool)?;

let mut session = AgentBuilder::new(model)
    .tools(Arc::new(tools))
    .gate(Arc::new(AllowAllGate))
    .build_session("tool-chat")?;
```

## Builder

```rust
#[derive(Clone)]
pub struct AgentBuilder {
    model: Arc<dyn ModelPort>,
    tools: Option<Arc<dyn ToolPort>>,
    gate: Option<Arc<dyn ToolGate>>,
    context: Option<Arc<dyn ContextPolicy>>,
    system_prompt: Option<String>,
    model_options: Option<ModelCallOptions>,
    transcript: Option<Arc<dyn TranscriptStore>>,
    usage: Option<Arc<dyn UsageSink>>,
    events: Option<Arc<dyn EventPort>>,
    observers: Vec<Arc<dyn EventObserver>>,
    max_total_turns: Option<u32>,
    run_id_source: Option<Arc<dyn RunIdSource>>,
    telemetry: TelemetryConfig,
}
```

```rust
impl AgentBuilder {
    pub fn new(model: Arc<dyn ModelPort>) -> Self;

    pub fn system_prompt(self, prompt: impl Into<String>) -> Self;
    pub fn model_options(self, options: ModelCallOptions) -> Self;
    pub fn tools(self, tools: Arc<dyn ToolPort>) -> Self;
    pub fn gate(self, gate: Arc<dyn ToolGate>) -> Self;
    pub fn context(self, context: Arc<dyn ContextPolicy>) -> Self;
    pub fn transcript(self, store: Arc<dyn TranscriptStore>) -> Self;
    pub fn usage(self, sink: Arc<dyn UsageSink>) -> Self;
    pub fn events(self, port: Arc<dyn EventPort>) -> Self;
    pub fn observer(self, observer: Arc<dyn EventObserver>) -> Self;
    pub fn max_total_turns(self, max: impl Into<Option<u32>>) -> Self;
    pub fn run_id_source(self, source: Arc<dyn RunIdSource>) -> Self;
    pub fn telemetry(self, config: TelemetryConfig) -> Self;

    pub fn build(self) -> Result<Agent, AgentBuildError>;

    pub fn build_session(
        self,
        session_id: impl Into<String>,
    ) -> Result<AgentSession, AgentBuildError>;

    pub fn build_session_from_state(
        self,
        session_id: impl Into<String>,
        state: AgentState,
    ) -> Result<AgentSession, AgentBuildError>;
}
```

`max_total_turns` accepts all of:

```rust
.max_total_turns(24)
.max_total_turns(Some(24))
.max_total_turns(None)
```

`None` explicitly opts into an unlimited total-turn budget. The builder default
remains `Some(32)`.

The builder is a configuration object. It never owns `AgentState`.

`system_prompt` and `model_options` configure the default
`BasicContextPolicy`. They are mutually exclusive with a custom `context`,
because a custom policy owns prompt assembly and request options:

- `system_prompt` changes only the default policy;
- `model_options` changes only the default policy;
- configuring either one together with `context` returns
  `AgentBuildError::ConflictingContextConfig`.

`model_options` is stored as `Option<ModelCallOptions>` so the builder can
distinguish an unconfigured default from an explicit `.model_options(...)` call.
An explicit default-valued `ModelCallOptions` still conflicts with a custom
context policy.

The default system prompt is the empty string. An explicitly empty prompt is
valid and means no system instructions.

## Internal Configuration

`AgentConfig` is SDK-internal and contains only activation and telemetry policy.
It is shared by `Agent` and `AgentSession`.

```rust
#[derive(Clone)]
pub(crate) struct AgentConfig {
    pub(crate) default_max_total_turns: Option<u32>,
    pub(crate) run_id_source: Arc<dyn RunIdSource>,
    pub(crate) telemetry: TelemetryConfig,
}
```

Capabilities live in `AgentKernel`, not in `AgentConfig`. This keeps one
authoritative dependency container and prevents the SDK from maintaining a
second copy of the capability graph.

## Agent

```rust
#[derive(Clone)]
pub struct Agent {
    kernel: Arc<AgentKernel>,
    config: Arc<AgentConfig>,
}

impl Agent {
    pub fn session(&self, session_id: impl Into<String>) -> AgentSession;

    pub fn session_from_state(
        &self,
        session_id: impl Into<String>,
        state: AgentState,
    ) -> Result<AgentSession, AgentBuildError>;
}
```

`Agent` is shareable and may create multiple independent sessions. Each
`AgentSession` owns its own `AgentState`; the kernel remains stateless across
activations.

Session ids are host identities. A host must not create two live sessions with
the same `SessionRef` against the same durable `TranscriptStore`. The SDK
serializes one `AgentSession`, but it does not maintain a global session registry.

## AgentSession

```rust
pub struct AgentSession {
    kernel: Arc<AgentKernel>,
    session: SessionRef,
    state: AgentState,
    config: Arc<AgentConfig>,
    control: SessionControl,
}
```

```rust
impl AgentSession {
    pub fn session_id(&self) -> &SessionRef;
    pub fn state(&self) -> &AgentState;
    pub fn transcript(&self) -> &[AgentMessage];
    pub fn last_assistant_message(&self) -> Option<&AgentMessage>;
    pub fn control(&self) -> SessionControl;

    pub async fn run_text(
        &mut self,
        text: impl Into<String>,
    ) -> Result<RunOutcome, KernelError>;

    pub async fn run_message(
        &mut self,
        message: AgentMessage,
    ) -> Result<RunOutcome, KernelError>;

    pub async fn run_messages(
        &mut self,
        messages: Vec<AgentMessage>,
    ) -> Result<RunOutcome, KernelError>;

    pub async fn run_with(
        &mut self,
        run_id: RunId,
        messages: Vec<AgentMessage>,
        max_total_turns: impl Into<Option<u32>>,
    ) -> Result<RunOutcome, KernelError>;
}
```

Run behavior:

1. `run_text` creates a user-origin text message.
2. `run_message` accepts one already-built logical message.
3. `run_messages` passes all messages as `RunInput.inputs`.
4. `run_with` uses the supplied run id and turn limit. Its turn cap accepts the
   same `u32`, `Some(u32)`, and `None` forms as the builder.
5. The other methods use the builder's `RunIdSource` and default turn limit.
6. The session installs a fresh activation token before calling the kernel.
7. The session passes `&mut state` to `AgentKernel::run`.
8. The kernel's `RunOutcome` is returned without reinterpretation.

`run_*` requires `&mut self`, so one session cannot run two activations
concurrently. This is the SDK-level serialization boundary.

`last_assistant_message` returns the last transcript message whose role is
`AgentRole::Assistant`. It is a read-only convenience for embedders that do not
install an event observer.

## State Initialization

The kernel treats `initialized == false` as "load durable state before applying
new input." It overwrites `state.transcript` and `state.cursor` from
`TranscriptStore::load`.

The SDK therefore uses these contracts:

- `build_session` creates an uninitialized empty state. The first run loads from
  the transcript store.
- `build_session_from_state` accepts only `state.initialized == true`; otherwise
  it returns `AgentBuildError::UninitializedState`.
- `Agent::session_from_state` applies the same validation and returns the same
  error.
- An initialized state is authoritative and is not reloaded by the kernel.

This avoids silently discarding a caller-supplied partial transcript when the
default no-op store is in use.

## Defaults

| Capability | Default | Behavior |
|---|---|---|
| `ModelPort` | required | `AgentBuilder::new` requires a model |
| `ModelCallOptions` | `Default` | provider-neutral default request options |
| `ToolPort` | `EmptyTools` | no tools are advertised |
| `ToolGate` | `DenyAllGate` | any accidental tool call is denied |
| `ContextPolicy` | `BasicContextPolicy` | system prompt plus transcript conversion |
| `TranscriptStore` | sessionless `NoopTranscriptStore` | commit barriers exist, no durable rows |
| `UsageSink` | `NoopUsageSink` | usage is accepted and flushed, not persisted |
| `EventPort` | `NoopEventPort` | events are discarded |
| `EventObserver` | none | no observers |
| `RunIdSource` | UUID v4 | unique run ids by default |
| `max_total_turns` | `Some(32)` | bounded by default |
| `TelemetryConfig` | enabled | tracing wrappers, no content logging |

`AllowAllGate` is deliberately not a default. Adding tools without configuring
a gate results in denied calls, not silent tool execution.

`EmptyTools` is a zero-sized `ToolPort`, not a mutable empty `ToolSet`.
`resolve` and `execute` return `ToolErrorKind::UnknownTool`; `refresh` returns
an empty snapshot with generation `1`.

The default `NoopTranscriptStore` allocates receipt ranges with a
mutex-protected counter, so concurrent sessions from one `Agent` cannot receive
overlapping ranges. A replacement advances the allocator to at least the
replacement's returned range end; neither a replacement nor an explicit clear
rewinds it.
The store keeps no rows, so a host that needs durability must inject another
store or persist `AgentState`.

`BasicContextPolicy` does not implement compaction. Its `compact` method returns
`CompactionOutcome::Skipped`. A host that expects automatic recovery from context
overflow must inject a policy with compaction support.

## Basic Context Contract

`BasicContextPolicy` is deliberately literal:

```text
prepare_run:
  inputs = input.inputs.clone()
  appended_messages = []
  return Continue(ContextPreparation { inputs, appended_messages })

prepare_request:
  system = configured prompt
  messages = state.transcript converted to ModelMessage
  options = configured ModelCallOptions
  return Continue(RequestPlan {
    request = PreparedRequest { system, messages, options, cursor: None },
    appended_messages = []
  })

compact:
  return Skipped
```

It must not invent messages, copy only selected input messages, or rely on
implicit kernel behavior to append the caller's input. The kernel persists only
the `ContextPreparation.inputs` returned by the policy.

## Session Cancellation

`SessionControl` is a handle to the active activation slot. Its internal shape
is:

```rust
struct ControlState {
    generation: u64,
    active: Option<ActiveActivation>,
}

struct ActiveActivation {
    generation: u64,
    token: CancelToken,
}

#[derive(Clone)]
pub struct SessionControl {
    shared: Arc<Mutex<ControlState>>,
}

struct ActivationGuard {
    shared: Arc<Mutex<ControlState>>,
    generation: u64,
}
```

```rust
impl SessionControl {
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
}
```

The session starts one fresh `CancelToken` per activation. The control handle
can cancel only the active token:

- cancel while idle is a no-op;
- cancel during a run is monotonic and idempotent;
- the next run gets a fresh token;
- a previous cancellation cannot poison a later turn;
- `ActivationGuard::drop` clears only the matching generation, so an unwind or
  late guard can never clear a newer activation;
- the active token is cleared when the run returns, errors, is cancelled, or
  unwinds.

The lock is never held across an `await`. `cancel` only locks, clones or observes
the active token, and unlocks.

Lock poisoning is recovered with `PoisonError::into_inner`; cancellation must
remain usable after a panicking activation. While idle, `is_cancelled()` returns
`false` rather than reporting the previous activation's outcome.

The SDK does not expose the raw token or the guard. This prevents a host from
reusing a token across activations.

## Plugin-Style Tool Composition

The plugin boundary is a static provider contract, not a registry.

```rust
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn metadata(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    fn access(&self, _input: &serde_json::Value) -> ResourceAccess {
        ResourceAccess::barrier()
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError>;
}

pub trait ToolProvider: Send + Sync {
    fn tools(&self) -> Vec<Arc<dyn ToolHandler>>;
}
```

The resource default is conservative. A handler that omits `access` is treated
as a global barrier and cannot run concurrently with another call. A handler
must explicitly return `ResourceAccess::independent()` or a narrower scope when
that is actually safe.

`ToolSet` implements `ToolResolver` and `ToolPort`:

```rust
pub struct ToolSet {
    entries: Vec<ToolSetEntry>,
    by_name: HashMap<String, usize>,
    snapshot_generation: AtomicU64,
}

pub struct ToolSetEntry {
    spec: ToolSpec,
    metadata: serde_json::Value,
    handler: Arc<dyn ToolHandler>,
}

impl ToolSet {
    pub fn new() -> Self;

    pub fn add(self, tool: impl ToolHandler + 'static)
        -> Result<Self, ToolSetError>;

    pub fn add_provider(self, provider: impl ToolProvider + 'static)
        -> Result<Self, ToolSetError>;
}
```

Duplicate tool names are rejected across `add`, across a single
`add_provider()` batch, and between the incoming provider batch and existing
entries. Provider validation is atomic: if any name is duplicated, no handler
from that provider is committed.

`ToolSet` calls `ToolHandler::spec()` and `metadata()` exactly once at insertion
and caches both results. `refresh` returns those cached specs in insertion order,
so a handler cannot change its advertised name or descriptor after composition.
Each `refresh` increments `snapshot_generation` and returns a monotonically
increasing generation; the composition itself remains immutable.

`resolve` looks up the cached entry, calls `handler.access(&call.input)`, and
constructs `ResolvedToolCall` with that value plus the cached descriptor. It
must never synthesize a default or independently computed access value.
`execute` forwards progress through `ToolExecutionContext.progress`.

Composition is fixed:

```text
ToolHandler / ToolProvider
          |
       ToolSet
          |
       ToolPort
          |
    AgentKernel
```

There is no `PluginId`, `install`, `enable`, `freeze`, mutable runtime
registration, dynamic loading, or service locator.

`metadata` is included because `ToolDescriptor.metadata` is already part of the
kernel contract. It is a presentation and policy hint, not a second registry.

## Telemetry

`tracing` is a facade. The host owns the subscriber and backend.

```rust
#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    enabled: bool,
    record_content: bool,
}

impl TelemetryConfig {
    pub fn enabled() -> Self;
    pub fn disabled() -> Self;
    pub fn record_content(self, enabled: bool) -> Self;
}
```

Default behavior:

- `enabled = true`;
- `record_content = false`;
- no process-global initialization;
- no prompt, tool input, or tool output content is logged.

There is no sampling option in the first version. Sampling a span graph requires
a run-level decision, a sampling identity, and child-span propagation rules; a
naive per-span ratio would create orphaned spans and incomplete run records. The
host may still sample at its subscriber or exporter boundary.

When the `telemetry` feature is enabled, the SDK composes internal wrappers:

```text
InstrumentedModelPort
InstrumentedToolPort
InstrumentedToolGate
InstrumentedContextPolicy
InstrumentedTranscriptStore
InstrumentedUsageSink
TelemetryEventObserver
```

The wrappers are transparent. They must forward every method of the wrapped
trait, including default methods:

- `ModelPort`: `capabilities`, `stream`, `complete`, and `compact`;
- `ToolPort`: `resolve`, `refresh`, and `execute`;
- `ToolGate`: `before` and `after`;
- `ContextPolicy`: `prepare_run`, `prepare_request`, `compact`, and
  `compaction_committed`;
- `TranscriptStore`: `load`, `append`, and `replace`;
- `UsageSink`: `record` and `flush`;
- `EventObserver`: `observe` and `observe_with_delivery`.

Omitting a default-method forward would silently disable native compaction or
drop compaction bookkeeping.

`AgentSession` creates the parent run span. Capability spans are children:

```text
agent.run
  agent.model.call
  agent.model.compact
  agent.tool.refresh
  agent.tool.execute
  agent.gate.before
  agent.gate.after
  agent.context.prepare_run
  agent.context.prepare_request
  agent.context.compact
  agent.context.compaction_committed
  agent.store.load
  agent.store.append
  agent.store.replace
  agent.usage.record
  agent.usage.flush
```

Stable fields:

```text
session_id
run_id
turn
call_id
model
tool
kind
status
error.kind
duration_ms
time_to_first_event_ms
input_tokens
output_tokens
total_tokens
```

`time_to_first_event_ms` is measured by the instrumented model stream. It
covers the first successfully yielded model event, not merely the time to
create the stream. A stream whose first item is an error enters `Errored`
without recording `time_to_first_event_ms`; the error keeps its own
`duration_ms` and `error.kind`.

Every instrumented asynchronous span that declares `duration_ms` records its
elapsed time on exit, including `agent.run`, model methods, gate, context,
transcript, and usage wrappers. Tool spans record the same field on their
explicit paths.

`TelemetryEventObserver` records only transitions represented by the neutral
event stream: run, turn, model, tool, usage and context lifecycle,
`RecoveryBackoff` notices, `CancellationRequested` notices, and terminal run
outcomes. It does not invent a recovery-success event or a cancellation source
that the kernel does not expose. It must not log every delta or tool progress
chunk.

Host integration remains explicit:

```rust
tracing_subscriber::registry()
    .with(tracing_subscriber::fmt::layer())
    .init();
```

`tokio-console` and OpenTelemetry are host choices and do not become SDK
dependencies.

Telemetry uses a structural allowlist even when `record_content` is false. It
may record only identities and classifications: `session_id`, `run_id`, `turn`,
`call_id`, `model`, `tool`, event kind, status, stop reason, duration, and token
counts. It must not record `ToolCall.input`, `ToolDescriptor.metadata`,
`ToolExecutionResult`, `MessageFinished.message`, provider activity text, or
full error text by default.

When `record_content` is enabled, the SDK may additionally log explicitly
selected caller input text, assistant message text, and tool payloads. Each
field is truncated to at most 512 Unicode scalar values. A canary test proves
that default telemetry does not emit inputs, metadata, or results, and the
enabled-content test exercises both caller input and assistant output so
truncation is not vacuously true. Content logging is never the default.

The instrumented model stream begins in `AwaitingFirst` immediately after
`stream()` returns, then follows a linear state machine:

```text
AwaitingFirst -> Streaming -> Terminal
              \-> Errored
```

- stream construction is observed by the parent model span before the stream
  exists; a construction error records `duration_ms` and `error.kind`;
- the first successfully yielded event closes `time_to_first_event_ms`;
- `Finished` records the terminal response exactly once;
- an error, including one that is the first item, records one terminal error
  transition without closing `time_to_first_event_ms`;
- an empty or cancelled stream records no fabricated terminal response;
- events are yielded unchanged and in order, including an event after the
  terminal response that the kernel will reject as a contract violation.

When the `telemetry` feature is disabled:

- the wrappers are not constructed;
- the SDK has no `tracing` dependency;
- `TelemetryConfig` remains a no-op configuration type;
- behavior is otherwise identical.

## Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentBuildError {
    #[error("cannot configure a custom context policy with system_prompt or model_options")]
    ConflictingContextConfig,

    #[error("restored session state must be initialized")]
    UninitializedState,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolSetError {
    #[error("duplicate tool name: {0}")]
    DuplicateTool(String),
}
```

Runtime failures remain `tact_agent_core::KernelError`. The SDK does not wrap
them in a lossy string error because callers need to match `RunPhase` and decide
whether to retry or surface a terminal failure.

`Agent::session` is infallible because it starts a fresh empty state.
`Agent::session_from_state`, `build_session`, and `build_session_from_state`
return `AgentBuildError`. `build_session` is fallible only because it shares the
builder validation path.

## Run IDs

```rust
pub trait RunIdSource: Send + Sync {
    fn next_run_id(&self) -> RunId;
}

pub struct UuidRunIdSource;
```

The default source uses UUID v4. `uuid` is a required dependency in the first
version because a process-local counter is not unique across retries or process
restarts, and `RunInput.run_id` is part of the durable usage identity.

Custom sources remain injectable for hosts that already have an idempotency or
trace id.

## Module Layout

```text
crates/agent_sdk/
  Cargo.toml
  README.md
  src/
    lib.rs
    config.rs
    builder.rs
    agent.rs
    session.rs
    control.rs
    error.rs
    run_id.rs
    toolset.rs
    defaults/
      mod.rs
      context.rs
      gate.rs
      store.rs
      usage.rs
      events.rs
    telemetry/
      mod.rs
      model.rs
      tools.rs
      gate.rs
      context.rs
      store.rs
      usage.rs
      events.rs
  examples/
    minimal.rs
    tool_agent.rs
```

The public surface is re-exported from `lib.rs`. Internal wrappers stay private
unless a host needs to inspect the composed graph for tests.

## Dependency Rules

- `tact_agent_sdk` may depend on `tact_agent_api` and `tact_agent_core`.
- `tracing` is behind the `telemetry` feature.
- `uuid` is a required dependency in the first version.
- The SDK does not depend on `tact_llm`; provider adapters remain host-owned.
- The SDK must not depend on `tact`, `tact_protocol`, `tact-ui`, `sqlx`, `rmcp`,
  `reqwest`, `async-openai`, or any UI crate.
- The SDK must not add dependencies to `tact_agent_api` or
  `tact_agent_core`.
- The dependency guard must define direct and transitive forbidden sets and a
  production dependency allowlist for `tact_agent_sdk`.
- `cargo metadata --all-features` is sufficient because the only optional SDK
  feature is `telemetry`; the SDK never enables a provider HTTP stack.

## Testing Strategy

The SDK tests should verify behavior through the public API:

- `builder_defaults_are_safe`: no tools, deny-all gate, bounded turns, and
  no-op persistence.
- `build_session_runs_text_and_persists_state`: text run updates transcript and
  `total_turns`.
- `basic_context_policy_forwards_inputs`: `BasicContextPolicy::prepare_run`
  returns the caller input in `ContextPreparation.inputs`.
- `reused_agent_isolates_sessions`: two sessions do not share `AgentState`.
- `build_session_from_state_skips_reload`: initialized state is authoritative.
- `restored_uninitialized_state_is_rejected`: both builder and agent entry
  points return `UninitializedState`.
- `max_total_turns_accepts_value_option_and_none`: the three accepted forms
  produce the expected `RunInput.max_total_turns` at `ContextPolicy::prepare_run`.
- `model_options_reach_prepared_request`: reasoning effort and temperature are
  preserved by `BasicContextPolicy`.
- `tool_provider_composes_without_registry`: a static provider contributes
  tools and duplicate names fail across `add` and `add_provider`.
- `tool_access_defaults_to_barrier`: a handler that omits `access` is serialized
  by the scheduler.
- `deny_all_gate_prevents_execution`: adding tools without a gate cannot execute.
- `allow_all_gate_runs_a_tool`: the explicit gate reaches `ToolPort::execute`.
- `session_control_cancels_only_the_active_run`: cancel before, after, and while
  idle has the documented behavior.
- `next_run_gets_a_fresh_cancel_token`: a cancelled run does not poison a later
  run.
- `activation_guard_preserves_newer_generation`: a stale guard cannot clear a
  newer activation.
- `last_assistant_message_returns_latest_assistant`: the convenience accessor
  selects the latest assistant message.
- `telemetry_is_optional`: disabled telemetry keeps behavior and does not build
  tracing wrappers.
- `telemetry_does_not_log_content_by_default`: a recording subscriber sees
  metadata spans but no prompt or tool payload.
- `telemetry_wrappers_forward_default_methods`: model compaction and
  `compaction_committed` calls reach the wrapped implementations.
- `telemetry_content_is_bounded_when_explicitly_enabled`: emitted content is
  truncated to 512 Unicode scalar values.
- `custom_context_conflicts_with_system_prompt`: build fails deterministically.
- `custom_context_without_model_options_succeeds`: a custom policy alone is
  valid.
- `custom_context_conflicts_with_model_options`: an explicit
  `.model_options(ModelCallOptions::default())` with a custom policy fails.

The existing kernel test support is a dev-only fixture. It must not become a
public SDK feature or public SDK API.

## Definition of Done

- `tact_agent_sdk` is a workspace member and depends only on the allowed crates.
- `AgentBuilder::build`, `build_session`, and `build_session_from_state` work.
- `AgentBuilder::max_total_turns` accepts `u32`, `Option<u32>`, and `None`.
- Defaults are safe and documented.
- `ToolSet` supports static tools and providers without a runtime registry.
- `ToolHandler::access` defaults to a barrier.
- `AgentSession` owns state and serializes activations through `&mut self`.
- Restored state must be initialized.
- Cancellation is activation-scoped and stale-cancel-free.
- Telemetry wrappers forward complete capability contracts and provide
  run/model/tool/store/usage timings without content logging by default.
- Hosts can install any tracing-compatible subscriber independently.
- The SDK does not depend on the full `tact` product or `tact_llm`.
- Examples compile and the SDK test suite is green.

## Open Design Decisions

None. The first version deliberately keeps `AgentState` under the session,
not under the builder, and keeps telemetry as capability decoration rather than
kernel behavior.
