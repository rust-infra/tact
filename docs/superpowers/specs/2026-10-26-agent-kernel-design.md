# Tact Agent Kernel Architecture

> Date: 2026-10-26
> Status: accepted and implemented; every production activation is kernel-only,
> legacy control code is removed, and optional capability extraction is closed
> as not triggered until a concrete second consumer exists
> Related: `crates/tact/src/agent/`, `crates/tact/src/tool/`, `crates/tact/src/permission/`,
> `crates/tact/src/store/session_store/`, `crates/tact_llm/`, `crates/protocol/`,
> `ARCHITECTURE.md`
> Plan: `docs/superpowers/plans/2026-10-26-agent-kernel.md`

## Goal

Create a small, stable agent kernel that owns the conversation state machine and
uses plugin-style composition as a design principle: the kernel depends on
stable capability contracts, and Tact supplies replaceable adapters without
turning that idea into a runtime plugin framework. The kernel must make the
existing behavior easier to test and harder to accidentally couple to
providers, storage, the TUI, or the marketplace plugin system.

The target is closer to Pi's package split:

- a provider-neutral model API;
- a generic agent core;
- a coding-agent composition layer;
- frontends that consume a host protocol rather than runtime internals.

The project name for the kernel is `tact_agent_core`. Its contract crate is
`tact_agent_api`. The existing `tact` package remains the product facade and
composition root so downstream imports do not have to change at once.

## Non-Goals

- Do not redesign Tact's tool behavior, prompt content, compaction policy, goal
  semantics, or permission policy in this change.
- Do not build a plugin framework. "Plugin style" here means dependency
  inversion, composable capabilities, and open extension points, not a
  `RuntimePlugin` trait, plugin registry, plugin ids, or dynamic loading.
- Do not put marketplace plugins, MCP clients, SQLite, Ratatui, HTTP clients, or
  process execution into `tact_agent_core`.
- Do not require a new UI protocol at the start. The existing `AgentUpdate`
  remains the TUI-facing protocol and is mapped from kernel events.
- Do not split every module into its own crate. The first boundary is API,
  kernel, composition, and host; finer capability crates can follow when they
  have an independent consumer.

## Original Pressure

Before the kernel cutover, the single-crate runtime demonstrated that a module
boundary alone was not enough:

- `tact` was the only agent crate, but it also owned tools, MCP, permissions,
  storage, plugins, voice, goals, worktrees, and session persistence. Its
  dependency list includes `sqlx`, `rmcp`, `git2`, `reqwest`, `image`, `cpal`,
  and `tera`; see `crates/tact/Cargo.toml`.
- `AgentRuntime` (`crates/tact/src/agent/mod.rs`) was a state bag containing the
  model provider, permission manager, session store, update sink, statistics,
  and provider-specific conversation state.
- `Agent` (`crates/tact/src/agent/mod.rs`) owned the native tool router, MCP
  router, hooks, tool context, and system-prompt mode.
- `agent_loop_detailed` directly called persistence, compaction, MCP refresh,
  hooks, permission-aware tool dispatch, Goal state, and UI event emission.
- `tact_llm` imported `tact_protocol` and emitted `AgentUpdate` values. A
  provider adapter therefore knew about TUI steps and tool presentation.
- `tact_protocol::AgentUpdate` includes presentation metadata such as
  `ToolPresentationInfo`; it is a host view model, not a kernel event model.
- The TUI imported runtime internals such as `tact::plugin` and `tact::skill`,
  which prevents a clean host boundary.

The result was not primarily a line-count problem. It was a dependency-direction
problem: the control loop cannot be tested with a fake model, fake tool, fake
authorizer, and fake transcript store without dragging in unrelated product
features.

## Reference Model

Pi separates its system into packages with narrow responsibilities:

| Pi package | Responsibility |
|---|---|
| `pi-ai` | Unified provider API and model streaming |
| `pi-agent-core` | Stateful agent loop, tool execution, and events |
| `pi-coding-agent` | Coding tools, sessions, CLI, and extensions |
| `pi-tui` | Rendering library |

The useful pattern is not the number of packages. It is that:

1. The agent core has a small model boundary.
2. Tool execution is generic; coding tools are supplied by the coding agent.
3. Events are a runtime-to-host stream, not a UI type from the provider.
4. Extensions compose features without changing the core loop.

## Decision Summary

| ID | Decision |
|---|---|
| D1 | Add `tact_agent_api` for provider-neutral messages, tool calls, events, outcomes, cancellation, and capability contracts. |
| D2 | Add `tact_agent_core` for one bounded agent activation and tool scheduling. |
| D3 | Keep `tact` as the product composition and compatibility facade. |
| D4 | Use plugin-style dependency injection: required capabilities are constructor-injected, optional behavior uses narrow ordered extensions only when a real caller exists, and the service graph is fixed at construction without a plugin registry or plugin lifecycle. |
| D5 | Keep the outer Goal Runner outside the kernel. It calls bounded kernel runs and interprets their outcomes. |
| D6 | Treat provider conversation state as an opaque `ModelCursor`; the kernel stores and passes it but never matches on provider-specific variants. |
| D7 | Split logical transcript messages from provider messages. Context policy converts the former to the latter immediately before a model call. |
| D8 | Preserve `AgentUpdate` as a TUI protocol and add an event bridge from `AgentEvent`; never make `AgentEvent` a presentation type. |
| D9 | Move permission, sensitive scanning, native/MCP tool resolution, storage, prompt assembly, and marketplace-plugin subprocesses behind capability adapters. |
| D10 | Keep tool call ordering and resource-conflict scheduling in the kernel; keep the meaning of a permission decision in the tool-gate adapter. |
| D11 | Keep conversation state host-owned: `AgentKernel` is stateless across activations and operates on a caller-owned `AgentState`. |
| D12 | Model tool execution as a typed result with status/error data, an ordered progress sink, and a final resolved call returned by the gate after any hook rewrite. |
| D13 | Require one terminal `ModelResponse` per model call, route every kernel or context-policy model call through one observed call scope, and persist authoritative usage through an awaited `UsageSink`. |
| D14 | Preserve the current absolute turn cap and completion-yield semantics through `AgentState.total_turns`, `RunInput.max_total_turns`, and typed tool directives. |
| D15 | Persist `MessageOrigin` for new sessions and migrate legacy rows from role/content plus existing marker detection. |
| D16 | `ContextPolicy` returns typed `Continue`, `Compact`, or `Block` preparation decisions. The kernel alone executes compaction, persists its replacement atomically, and repeats preparation only after a successful replacement. |
| D17 | Keep `AgentRuntime.cancel_flag` as Tact's public cancellation authority and bridge it into one activation-scoped `CancelToken`; the token is downstream-only inside the kernel. |
| D18 | Redact tool output at the Tact adapter boundary before progress or final results enter the kernel, and redact again after hook rewrites before visible output or persistence. |
| D19 | Reconcile interrupted assistant tool calls at load time and persist deterministic grouped results before any resumed model request. |
| D20 | Count only dispatched main-loop request attempts as turns; compaction calls and transport retries do not consume turns. |
| D21 | Share one interior-synchronized MCP catalog handle between refresh and full-router replacement; an activation holds one cheap clone. |
| D22 | Expose only a read-only `ToolResolver` to `ToolGate::before`; the kernel alone calls `ToolPort::execute`, and only for the final `Allow` returned after re-resolution. |
| D23 | Keep local injected context and provider-hosted activity on separate non-user channels: typed transcript origins or `ProviderActivity`; neither may become a real user turn or a client `ToolUse`. |
| D24 | Treat cancellation as one monotonic, activation-scoped signal: repeated cancellation is idempotent, adapters only observe it, committed state is retained, and exactly one terminal run event follows. |
| D25 | Keep hooks, permission authority, UI traffic, and queues in product-owned shared services per logical agent, with session/model metadata in a read-only activation view. Hook callbacks never receive a second `Agent` or a writable transcript; permission changes within one agent flow through one shared authority, while subagents inherit a snapshot into their own authority. |

### Original Act On Reconciliation

The first adversarial review produced eleven Act On findings. They are tracked
as `R1`-`R11` so they remain distinct from the later kernel-contract review,
whose IDs are `F1`-`F7`. Every row has a normative contract; `Closed` also
requires the named production evidence. All rows below meet both conditions.

| ID | Original finding | Normative contract | Delivery status |
|---|---|---|---|
| R1 | Hook rewrites must rerun the complete security and permission classification. | [ToolGate](#toolgate) | Closed. The production gate re-resolves and reclassifies after both hook stages, with credential-rewrite, permission-rewrite, post-tool, and source-guard tests. |
| R2 | Compaction needs typed requests, replacement, skip/veto, and atomic persistence. | [ContextPolicy](#contextpolicy) | Closed. `TactContextPolicy` and `TactCompactor` deliver typed auto/manual/recovery/command/micro decisions, native cursor-only replacement, skip/veto, and post-persistence bookkeeping. |
| R3 | The public `cancel_flag` must drive the activation token. | [Cancellation Contract](#cancellation-contract) | Closed. `CancelBridge`, the public-facade cancellation matrix, and the nested-subagent ownership rule are implemented. |
| R4 | Facade state transfer must include every mutable field and the correct receiver. | [Compatibility Facade](#compatibility-facade) | Closed. `FacadeActivationState`, `FacadeSidecar`, receipt ranges, guard restore, and exhaustive transfer tests are implemented. |
| R5 | Resumed sessions must load before constructing `initialized = true`. | [Compatibility Facade](#compatibility-facade) | Closed. The common bootstrap calls `ensure_session` before path selection and preserves non-empty direct context mutations. |
| R6 | `CompleteRun` may complete a Goal only on its final permitted turn. | [Goal and Host Yield Semantics](#goal-and-host-yield-semantics) | Closed. `TactContextPolicy` injects the reminder only on the final permitted turn; final completion, non-final continuation, truncated-final refusal, Goal-origin preservation, and `goal_request_input` yield all have product evidence. |
| R7 | `MaxTokens + ToolUse` must pair pending calls before continuation. | [Run Semantics](#run-semantics) | Closed. Kernel pairing and continuation are verified together with product manual-compaction-before-continuation and `YieldToHost` suppression of later tool waves. |
| R8 | `tact_llm` account DTOs must not depend on `tact_protocol`. | [`tact_llm`](#tact_llm) | Closed. Provider-neutral account DTOs and the `tact-ui` mapping landed, and the dependency tree no longer contains `tact_protocol`. |
| R9 | A failed post-tool result must clear all control directives. | [ToolGate](#toolgate) | Closed. The kernel clears `CompleteRun`, `YieldToHost`, and `RequestCompaction` while preserving the original failure. |
| R10 | Known synthetic user-role rows must keep non-user origins after migration. | [Legacy Session Migration](#legacy-session-migration) | Closed. Deterministic legacy inference covers hook, Goal, subagent, continuation, summary, assistant, and tool-result rows. |
| R11 | Redaction must cover live, final, hook-rewritten, and split-chunk output. | [ToolPort](#toolport), [ToolGate](#toolgate) | Closed. `ToolPort` preserves split-chunk held-back state and redacts the final result; `ToolGate` redacts rewritten output and prevents a secret from being joined across rewrite stages. |

All eleven findings have named production evidence and are closed.

### Kernel Contract Findings (F1-F7)

A later kernel-contract review produced seven findings. Five were
implementation-blocking; all seven are normative in this design rather than
deferred notes. These findings do not replace or implicitly close `R1`-`R11`.
“Contract verified” means the kernel/API contract and its focused tests pass,
but the product-adapter gate still has to run. “Implemented” means the current
code has the named production path. “Verified” means every listed guard or test
passes. “Open” is explicit unfinished delivery work and cannot be treated as
closed by documentation alone.

| ID | Finding | Resolution | Status | Authoritative section |
|---|---|---|---|---|
| F1 | A gate could resolve and then execute an unapproved tool through the same capability. | Split read-only `ToolResolver` from executable `ToolPort`; only the kernel executes the final accepted call. | Verified in the production gate and authority guards | [ToolPort](#toolport), [ToolGate](#toolgate), [Kernel Authority](#kernel-authority) |
| F2 | Local injected context and provider-hosted search were at risk of becoming synthetic user turns or client tool calls. | Persist injected context with a non-`User` `MessageOrigin`; represent hosted search only as `ProviderActivity`; never replace activation inputs. | Verified in `TactContextPolicy`, the event bridge, and production composition | [Logical Transcript vs Model Messages](#logical-transcript-vs-model-messages), [Event Contract](#event-contract) |
| F3 | Cancellation races had no single terminal-event or committed-state contract. | Make cancellation monotonic and activation-scoped, drain in-flight work, retain committed state, and emit exactly one terminal run event. | Verified through the public facade, kernel, and bridge | [Cancellation Contract](#cancellation-contract) |
| F4 | Adapters could write kernel-owned transcript, cursor, usage, or events outside the control loop. | Make the kernel the sole writer of those paths and construct product gates without execution or persistence handles. | Verified by the full adapter authority source guard | [Kernel Authority](#kernel-authority) |
| F5 | A direct-manifest dependency check could not detect a forbidden transitive edge. | Parse the resolved all-features graph, test a synthetic forbidden transitive edge, and run the guard from local checks and CI. | Verified | [Dependency Rules](#dependency-rules) |
| F6 | Extension ordering and authority were underspecified, risking claim, veto, or short-circuit behavior. | Keep `EventObserver` read-only: after `EventPort`, in registration order, returning no value and unable to claim, veto, or short-circuit. | Verified in kernel and source-matrix tests | [EventPort](#eventport), [Event Contract](#event-contract) |
| F7 | The design could pass compile-time contracts while changing observable behavior during cutover. | Freeze a normalized legacy baseline before adapter cutover, including image companion blocks and `usage.total` accounting, and compare every later cutover against it before deleting the old loop. | Verified: nine fixture-backed kernel comparators plus the TUI full-frame surface assertion cover all ten scenarios | [Baseline Invariants](#baseline-invariants), [Testing Strategy](#testing-strategy) |

Current finding evidence is:

- F1: `gate_rewrite_is_re_resolved_before_execution`,
  `gate_rewrite_uses_the_resolved_access_for_wave_scheduling`, and
  `denied_tool_never_executes_but_gets_a_result` prove the kernel contract.
  `resolving_a_write_does_not_execute_it`,
  `pre_tool_rewrite_is_fully_re_resolved_and_reclassified`,
  `permission_request_rewrite_is_rechecked_before_allow`, and
  `post_tool_redacts_rewritten_output_and_preserves_the_original_error` prove
  the production `TactToolGate` and `TactToolPort` boundary. The source guards
  also reject a stored executor, `TranscriptStore`, `UsageSink`, or `EventPort`
  in the gate.
- F2: `injected_context_is_persisted_but_is_not_a_user_turn`,
  `injected_context_and_hosted_activity_keep_separate_non_user_channels`, and
  `provider_activity_is_not_treated_as_a_tool_call`; the event-bridge golden
  `injected_context_and_hosted_activity_keep_separate_non_user_channels`
  verifies the same boundary through `TactEventBridge`.
  `successful_session_start_and_user_prompt_context_keep_non_user_origins` and
  `queued_context_is_appended_once_before_the_request` verify the production
  `TactContextPolicy`; production composition wires that policy into the
  kernel activation.
- F3: the `cancellation_*` matrix and
  `repeated_cancellation_has_one_terminal_event_and_bounded_drain`; the Tact
  bridge test
  `public_cancel_flag_keeps_one_terminal_event_and_committed_state` verifies
  the bridge. The public-facade matrix is also green:
  `agent_loop_detailed_reports_cancelled_when_the_flag_arrives_mid_response`,
  `cancel_before_tool_execution_pairs_every_tool_use`,
  `cancel_mid_preflight_answers_every_tool_use_once`,
  `cancel_mid_preflight_answers_an_approved_but_unrun_call`,
  `facade_cancel_while_bash_is_in_flight_drains_and_pairs_the_result`,
  `facade_cancel_while_another_tool_is_in_flight_drains_without_another_turn`,
  and `facade_cancel_between_waves_suppresses_later_side_effects_and_pairs_calls`.
  Together they cover direct `AgentRuntime.cancel_flag` writes during
  streaming, permission approval, bash, another in-flight tool, and between
  waves, including terminal-event cardinality and durable pairing.
- F4: `denied_tool_never_executes_but_gets_a_result`,
  `cancellation_during_gate_approval_never_executes_the_tool`, and
  `capability_adapters_do_not_hold_kernel_state_or_persistence_authority`
  guard the split between policy, execution, and kernel-owned persistence.
  The guard now scans activation view, cancel, composition, context, event,
  gate, hook, model, permission, services, store, tool, presentation, and
  usage modules. Its companion guards reject tool execution or kernel-event
  emission from policy and persistence adapters.
- F5: the resolved all-features dependency guard and its synthetic forbidden
  transitive-edge fixture pass.
- F6: `event_port_runs_before_observers_and_emits_one_terminal_event` and
  `event_port_precedes_observers_in_registration_order`; the product source
  matrix `kernel_and_adapter_updates_share_one_sink_exactly_once` verifies the
  adapter-owned update boundary.
- F7: `fixture_freezes_every_required_legacy_scenario`,
  `fixture_keeps_image_and_usage_baseline_explicit`, and
  `fixture_preserves_non_user_injection_and_hosted_activity_boundaries` prove
  the fixture is complete. Nine fixture-backed kernel runs pass:
  `simple_text_kernel_run_matches_frozen_legacy_baseline`,
  `tool_use_image_companion_kernel_run_matches_frozen_legacy_baseline`,
  `permission_denial_kernel_run_matches_frozen_legacy_baseline`,
  `transient_retry_kernel_run_matches_frozen_legacy_baseline`,
  `cancellation_during_stream_matches_frozen_legacy_baseline`,
  `cancellation_during_tool_wave_matches_frozen_legacy_baseline`,
  `injected_context_kernel_run_matches_frozen_legacy_baseline`,
  `hosted_web_search_kernel_run_matches_frozen_legacy_baseline`, and
  `resumed_sqlite_history_kernel_run_matches_frozen_legacy_baseline`.
  The tenth scenario runs through the real facade on the kernel path in
  `kernel_updates_leave_no_unowned_blank_tui_cells`; it checks that every blank
  cell in the full frame carries one of the intentional surface backgrounds,
  rather than incorrectly requiring the status, input, bottom, and code
  surfaces to collapse to `theme.bg`. The compact regression
  `responses_compact_surfaces_usage_persistence_failure` now runs on the direct
  kernel model path. The neutral provider boundary preserves the original
  `MessageContent::Text` representation used by the Responses context hash
  instead of rewriting it to `Blocks`; the temporary model bridge and legacy
  control paths have been deleted.

These checks close F1-F7. The production facade routes every activation through
the kernel, directly from `ensure_session` to `run_kernel_activation`.
`ActivationPath`, `ActivationFallbackReason`, `ActivationSelection`,
`select_activation_path`, and their facade fields have been deleted. There is no
selection extension point and no mid-run fallback to reintroduce.

Task 6 is also closed. The evidence is intentionally spread across the layer
that owns each behavior:

- recovery counters are returned in `RunOutcome.recovery` and copied to
  `AgentRuntime.recovery_state`; `kernel_recovery_counts_are_visible_on_the_facade_after_success`,
  `kernel_recovery_counts_are_visible_on_the_facade_after_cancellation`, and the
  exhausted-continuation facade test cover success, cancellation, and
  max-token outcomes;
- nested subagents use the product-owned child constructor and the same bounded
  kernel activation: `nested_subagent_uses_the_kernel_with_child_owned_services`;
- both durable orphan paths reconcile before the resumed request:
  `local_session_reconciles_a_durable_unmatched_tool_call_before_the_request`
  and
  `responses_session_reconciles_a_durable_unmatched_tool_call_before_the_request`;
- permission authority parity is consolidated in
  `permission_authority_matrix_covers_grants_denials_modes_and_inheritance`,
  with facade/gate visibility and child ownership covered by the adjacent
  `facade_mode_and_grant_changes_reach_the_next_gate_call`,
  `gate_denials_are_visible_through_the_shared_facade_authority`,
  `always_allow_popup_selection_is_visible_to_the_next_gate_call`, and
  `subagent_permissions_are_snapshot_derived_and_session_scoped` tests;
- the complete host-visible sequence evidence is indexed in
  [AgentUpdate Sequence Matrix](#agentupdate-sequence-matrix).

Goal final-turn behavior and Goal Runner ownership remain part of the closed
set: the Runner stays in `tact-ui`, and the kernel contains no Goal-specific
runner.

## Plugin-Style Design, Not a Plugin Framework

In this design, "plugin" is a principle, not a public abstraction we must
recreate. It means:

1. The kernel owns the stable control flow.
2. Product behavior enters through small, typed capability contracts.
3. A capability can be replaced without editing the loop.
4. Optional behavior composes additively.
5. Policy and infrastructure remain independently testable.

There is no `RuntimePlugin` trait, `PluginRegistry`, plugin id, service slot,
dynamic loader, install lifecycle, or `freeze()` step in the first design.
Those are plugin-framework concepts, not requirements for plugin-style code.
Adding them without a real third-party extension requirement would add
indirection while making the kernel more complex.

The word "plugin" also already names Tact's marketplace feature. That system
installs skills, commands, hooks, and MCP servers. A marketplace plugin may
contribute a tool or hook adapter to the runtime, but it is not the kernel's
extension abstraction.

### Capability Adapters

A kernel instance receives exactly one implementation of each required
capability:

- `ModelPort`;
- `ToolPort`;
- `ToolGate`;
- `ContextPolicy`;
- `TranscriptStore`;
- `UsageSink`;
- `EventPort`.

`ToolGate` also receives a per-call `&dyn ToolResolver` view of `ToolPort`.
`ToolResolver` exposes only `resolve`; it is not a separate `KernelDeps` slot
and it deliberately cannot execute tools. This narrows the capability surface
available to hook and permission code.

These are ordinary dependency-inversion boundaries. The Tact implementations
remain normal adapter structs (`TactModelPort`, `TactToolPort`,
`TactToolGate`, `TactContextPolicy`, `SqliteTranscriptStore`) and implement the
contract directly. Do not add a `*Plugin` wrapper whose only job is to look like
a plugin.

Optional behavior may use narrow, purpose-specific extension traits:

- `EventObserver` for read-only `AgentEvent` consumption;
- future `RunHook` for pre-run, user-prompt, turn-end, and run-end behavior;
- future `ContextContributor` for ephemeral context or prompt sections.

The implemented slice contains the required capabilities and `EventObserver`.
`RunHook` and `ContextContributor` remain intentionally unavailable until a
caller needs them. Host-owned commands such as Goal control, session switching,
and `/compact` stay in the driver.

This sequencing is deliberate: optional extension traits that have no caller
become speculative framework surface. `RunHook` and `ContextContributor` are
introduced only when a concrete product behavior first needs them.

### Composition

Dependency injection is sufficient; no registry layer is needed:

```rust
pub struct KernelDeps {
    pub model: Arc<dyn ModelPort>,
    pub tools: Arc<dyn ToolPort>,
    pub gate: Arc<dyn ToolGate>,
    pub context: Arc<dyn ContextPolicy>,
    pub transcript: Arc<dyn TranscriptStore>,
    pub usage: Arc<dyn UsageSink>,
    pub events: Arc<dyn EventPort>,
}

pub struct KernelExtensions {
    pub observers: Vec<Arc<dyn EventObserver>>,
}

impl AgentKernel {
    pub fn new(deps: KernelDeps) -> Self;
    pub fn with_extensions(deps: KernelDeps, extensions: KernelExtensions) -> Self;
    pub async fn run(
        &self,
        state: &mut AgentState,
        input: RunInput,
        cancel: CancelToken,
    ) -> Result<RunOutcome, KernelError>;
}
```

`KernelDeps` makes required capabilities compile-time fields rather than
optional slots. The implemented contract exposes only the ordered observer
vector. Hook and contributor collections are added only with a real caller,
because each extension order is observable. The service graph is immutable
after construction; stateful adapters may use synchronization internally, but
the kernel never sees those locks.

The product composition root constructs these values in ordinary Rust and
passes them to the kernel. This provides the useful properties of plugins --
replaceability, dependency inversion, open extension, isolated policy -- without
inventing plugin infrastructure.

If dynamic third-party runtime plugins become a real requirement later, design
that as a separate feature with its own ABI, security, versioning, discovery,
and failure model. It must not leak into the first kernel abstraction.

### Shared Product Services and the Activation View

The kernel must not depend on `tact::Agent`. Before the migration, Tact's hooks
received `&LoopState`, where `pub type LoopState = Agent`. Hooks inspect session
metadata, model identity, turn count, permission mode, MCP services, update
status, and queued context. Copying `Agent` into an adapter, storing
`Arc<Mutex<Agent>>`, or creating a second writable transcript would have
defeated the kernel boundary and made state ownership ambiguous.

Tact therefore owns one shared-services handle and constructs a read-only
activation view for hook callbacks:

```rust
pub struct TactSharedServices {
    pub updates: UpdateSink,
    pub stats: Arc<RwLock<SessionStats>>,
    pub permissions: PermissionAuthority,
    pub pending_subagent_results: Arc<Mutex<VecDeque<SubagentResult>>>,
    pub pending_hook_context: Arc<Mutex<VecDeque<HookContextChunk>>>,
    pub hook_status_seq: AtomicU64,
    pub hook_mcp: HookMcpHandle,
    pub mcp_catalog: McpCatalogHandle,
}

pub struct TactActivationView<'a> {
    pub services: &'a TactSharedServices,
    pub transcript: Option<ReadOnlyTranscript<'a>>,
    pub session: SessionIdentity,
    pub model: ModelIdentity,
    pub turns_taken: u32,
    pub remaining_turns: Option<u32>,
}

pub type LoopState<'a> = TactActivationView<'a>;
```

These types are Tact-only and never enter `tact_agent_api` or
`tact_agent_core`. The exact accessor names may change during extraction, but
the ownership model may not:

- `TactSharedServices` is owned by one logical `Agent` and shared by that
  activation's gate/context adapters. A subagent is a separate logical agent:
  it owns its session identity, stats, cancel flag, and permission authority.
  Async subagents may receive explicit parent-owned queue and update handles,
  but not the parent's activation view or live permission manager.
- `PermissionAuthority` is the single interior-synchronized owner of the
  existing `PermissionManager` for one logical agent. The facade, tool gate, and
  UI-facing mode controls for that agent use the same authority. The subagent
  factory reads the parent's snapshot and constructs a separate child
  authority; it never shares the parent's live manager. Mode changes,
  always-allow grants, denial counters, and snapshots must be visible through
  every handle for the same agent. Do not hold its lock across an awaited host
  prompt.
- `TactActivationView` borrows or snapshots transcript data read-only when the
  caller has it (for example `TactContextPolicy` via `ContextView`); a tool-gate
  call can pass `None` when its hooks do not need transcript data. It exposes
  session and model metadata, cumulative `turns_taken`, remaining turns, update
  methods, hook status ids, queued-context handles, and the narrow hook MCP
  handle. It must not expose `AgentState`, a `TranscriptStore`, `UsageSink`,
  kernel `EventPort`, or model `ToolPort`.
- Hook context writes go only to the explicit pending-context queue. A
  `TactContextPolicy` drains that queue into `RequestPlan.appended_messages`;
  the kernel remains the only transcript writer. The transitional
  transcript-looking compatibility shim has been removed; queued hook context
  can only enter the kernel through `RequestPlan.appended_messages`.
- `HookMcpHandle` preserves the existing ability of plugin hooks to call MCP
  services. It is not the model tool executor, cannot execute a model
  `ToolCall`, and is not `ToolPort`.

The public name `tact::LoopState` remains, but its meaning changed from the
whole `Agent` to this lifetime-parameterized read-only projection. Hook traits
bind the view and its borrow to the same activation lifetime
(`for<'a> Fn(&'a LoopState<'a>, ...)`), rather than using a non-generic alias
or an owned `Agent` clone. In-repo hook closures that explicitly type their
argument as `&crate::Agent` were migrated to `&LoopState`; this was a
deliberate hook-boundary migration. Every in-repo hook now uses that projection;
a hook that requires facade-owned mutable state that the projection cannot
provide is unsupported rather than a reason to bypass the kernel.

`TactToolGate` and `TactContextPolicy` may hold `Arc<TactSharedServices>` and
their narrow hook/prompt/compaction helpers. The shared-services bundle is
limited to the fields shown above; it and the gate must never hold `ToolPort`,
the model tool executor, `TranscriptStore`, `UsageSink`, or `EventPort`.

### Kernel Authority

The composition root wires capabilities together, but the runtime call path is
not a peer-to-peer service graph. The non-bypassable order is:

```text
Kernel -> ToolResolver.resolve -> ToolGate.before
       -> final Allow(call) -> kernel scheduling -> ToolPort.execute
       -> ToolGate.after -> kernel-owned transcript/usage/event commits
```

`AgentKernel` is the only component that invokes `ToolPort::execute`,
`ToolGate::after`, kernel-path `TranscriptStore` writes, or `UsageSink` records.
`ToolGate` may call only the `ToolResolver` passed to `before`; it cannot execute
or schedule a tool, and `TactToolGate` is not constructed with a stored
`ToolPort`, executor, `TranscriptStore`, `UsageSink`, or `EventPort`.
`TactToolPort`, `TactToolGate`, and `TactContextPolicy` may produce their own
host-facing adapter traffic, but they cannot emit kernel `AgentEvent`s or write
the logical transcript/cursor and usage rows on behalf of the kernel. The only
writes to those kernel-managed paths are the awaited `TranscriptStore` and
`UsageSink` calls made by the kernel itself; implementing those ports does not
authorize another adapter to call them. The composition root is the only layer
that wires capabilities together.

Adapters receive the narrow shared product services they actually need. They do
not receive `Arc<Mutex<Agent>>`, a cloned `Agent`, or a second transcript. A
read-only `TactActivationView` is the compatibility boundary for hooks; it can
carry shared infrastructure handles but never transcript ownership.

These are ownership and call-path invariants inside one process. They prevent a
normal adapter from accidentally bypassing approval through an over-broad
trait; they are not a sandbox for hostile Rust code or a replacement for
process/plugin isolation. The first design has no dynamic capability grants,
per-session grant broker, or adapter service locator.

### Adapter Style

- One trait has one reason to exist; do not create a mega-`Plugin` trait.
- Constructors inject dependencies; adapters do not read global registries.
- Closed decisions and failures are typed enums, not string labels.
- Optional behavior is additive and ordered; adding an observer must not edit
  the turn loop.
- Runtime failures become typed errors or `AgentEvent` notices, never panics.
- Prefer ordinary structs and functions over framework-shaped wrappers.

## Target Crates

### `tact_agent_api`

Pure domain contracts. No provider clients, storage, TUI, process execution, or
OS-specific APIs.

Responsibilities:

- logical `AgentMessage`, `MessageOrigin`, and content blocks;
- provider-neutral `ModelRequest`, `ModelEvent`, `ModelResponse`, `ModelCursor`;
- `ToolCall`, `ToolResolver`, `ToolDescriptor`, `ResolvedToolCall`,
  `ResourceAccess`, `ToolOutcome`, and tool directives;
- `AgentEvent`, `RunOutcome`, stop reasons, cancellation, and token usage;
- authoritative `UsageRecord` persistence through the async `UsageSink`;
- required capability traits and optional extension traits;
- serialization for contracts that cross a process or persistence boundary.

The crate may depend on `async-trait`, `serde`, `serde_json`, `thiserror`,
`futures-core`, and the `rt` feature of `tokio-util` for `CancelToken`. It must
not depend on `tact`, `tact_llm`, `tact_protocol`, `sqlx`, `rmcp`, `reqwest`,
`ratatui`, or `cpal`.

### `tact_agent_core`

The kernel. It owns the run state machine and calls only `tact_agent_api`
contracts.

Responsibilities:

- one bounded activation over a caller-owned `AgentState`;
- conversation turn state;
- stream aggregation and tool-use extraction;
- sequential preflight and parallel tool waves;
- tool-result ordering and pairing;
- cancellation and max-turn enforcement;
- retry/compaction decisions based on typed model errors;
- event ordering;
- persistence barrier calls through `TranscriptStore`;
- `KernelDeps` and ordered `KernelExtensions`.

The crate may depend on `tact_agent_api`, `futures-util`, `tracing`, and a small
Tokio surface (`sync`, `time`, and test runtime features). It must not depend on
provider adapters, `tact_protocol`, `sqlx`, `rmcp`, `ratatui`, `cpal`, or
product-level `tact::*` modules.

### `tact`

The product composition root and compatibility facade.

Responsibilities:

- construct Tact's model, native-tool, MCP, permission, prompt, compaction,
  session, marketplace-plugin, goal, subagent, and notification adapters;
- construct one kernel for the main agent and one for each subagent;
- map the existing facade state into `AgentState` and back without creating a
  second mutable transcript;
- expose the existing `tact::Agent` API while delegating the loop to the kernel;
- map kernel events to `AgentUpdate`;
- migrate legacy session origins and provide the sessionless no-op transcript
  adapter;
- keep configuration, paths, and product defaults outside the kernel.

`tact` may remain large at first. Its size is less important than its
dependency direction and the fact that it no longer owns the control loop.
Its facade must call `ensure_session` before constructing `AgentState`, bridge
the public cancel flag into the activation token, transfer all behavioral
sidecar state around the kernel call, and restore that state on every exit path.

### `tact_llm`

Provider adapters implementing `ModelPort`:

- Anthropic;
- OpenAI-compatible chat completions;
- OpenAI Responses;
- DeepSeek and Kimi-compatible variants;
- mock provider.

`tact_llm` may own provider-specific request conversion, auth, retry hints, and
opaque model cursors. It must not emit `AgentUpdate` or import `tact_protocol`.
Provider account balance and usage-quota queries return provider-neutral DTOs
defined by `tact_llm`; the product layer maps those DTOs to
`tact_protocol::biz` for host presentation.

### `tact_protocol`

Host protocol for TUI, headless, RPC, and future frontends. It stays separate
from `tact_agent_api`.

`AgentUpdate`, `UserCommand`, `ToolPresentationInfo`, `PlanStep`, and Goal
snapshots remain here. `tact` maps kernel events and extension events into this
protocol.

## Kernel Types

### Logical Transcript vs Model Messages

The kernel must not use provider messages as its internal transcript. The
distinction is required for injected context, compaction summaries, Goal
context, and UI-only history.

```rust
pub enum MessageOrigin {
    User,
    Assistant,
    ToolResult,
    InjectedContext { source: String },
    CompactionSummary,
    GoalContext,
    SubagentResult,
    Continuation,
}

pub struct AgentMessage {
    pub role: AgentRole,
    pub origin: MessageOrigin,
    pub content: Vec<AgentContentBlock>,
}

pub enum AgentContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AgentImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

pub struct AgentImageSource {
    /// The provider's image-source discriminator, for example `base64`.
    pub type_: String,
    pub media_type: String,
    pub data: String,
}
```

`MessageOrigin` is a closed enum. Provider data that does not fit a known
origin must be adapted into an explicit new variant rather than smuggled
through a string field.

The request pipeline is:

```text
AgentMessage[]
    -> ContextPolicy transform/compaction
    -> AgentMessage[]
    -> convert_to_model_messages
    -> ModelMessage[]
    -> ModelPort
```

`InjectedContext` and `GoalContext` must never increment the user-turn counter.
This preserves the existing invariant that injected context is not a user turn.

Local context injection and provider-hosted activity are separate mechanisms:

- Tact-injected context is a logical transcript message with an explicit
  non-`User` origin. `ContextPolicy` may append it, and the kernel persists it
  before the model request that consumes it.
- Provider-hosted activity, including web search performed by the model server,
  is response provenance represented only by `ProviderActivity`. It is not a
  transcript message and never becomes a client `ToolCall`. An adapter must not
  synthesize a user turn, replace `RunInput.inputs`, or inject a fake tool-use
  or tool-result block for it.

The final `ModelResponse` still owns any model-visible text and any provider
state needed for a later turn. Provider-specific server-side context may remain
inside the opaque `ModelCursor`; Tact does not expose or reinterpret it.

The two counters have separate meanings. `AgentState.total_turns` counts
dispatched main-loop request attempts across the lifetime of the logical agent,
while `MessageOrigin::User` controls which transcript cells count as real user
turns for compaction and prompt policy. A synthetic user-role message must never
be treated as both.

### Opaque Model Cursor

The kernel stores provider state but does not inspect it:

```rust
pub struct ModelCursor {
    pub provider: String,
    pub version: u32,
    pub payload: serde_json::Value,
}
```

The adapter is the only code that understands `payload`. The kernel only
passes it back to the same model port and commits it atomically with the
assistant message.

### Bounded Run

```rust
pub struct AgentState {
    pub transcript: Vec<AgentMessage>,
    pub cursor: Option<ModelCursor>,
    /// Main-loop request attempts that reached model dispatch.
    ///
    /// A context-overflow retry is a new attempt. Compaction calls and
    /// transport retries inside one attempt do not consume another turn.
    pub total_turns: u32,
    /// Last provider-reported total token count (`TokenUsage::total_tokens`),
    /// matching the legacy `runtime.last_token_total = usage.total` behavior.
    /// Compaction policy uses it when provider usage is available; input
    /// tokens alone are not equivalent.
    pub last_token_total: Option<u32>,
    /// False only when the caller wants the kernel to load the session.
    pub initialized: bool,
}

pub struct RunInput {
    pub session: SessionRef,
    /// New inputs for this activation, never a second copy of the transcript.
    pub inputs: Vec<AgentMessage>,
    /// Absolute cap compared with `AgentState.total_turns`, matching the
    /// current process-lifetime `Agent::turns_taken` behavior.
    pub max_total_turns: Option<u32>,
}

pub struct RecoveryStats {
    /// Context-overflow compactions attempted by this activation.
    pub compact_attempts: u32,
    /// Transient model failures retried by this activation.
    pub transport_attempts: u32,
    /// Max-token continuation requests issued by this activation.
    pub continuation_attempts: u32,
}

pub struct RunOutcome {
    pub stop: RunStop,
    /// Main-loop request attempts dispatched by this activation.
    pub turns: u32,
    /// `AgentState.total_turns` after the activation.
    pub total_turns: u32,
    pub tool_calls: u64,
    /// Deterministic recovery counters for this activation.
    pub recovery: RecoveryStats,
}

pub enum RunStop {
    Completed,
    YieldToHost { reason: HostYieldReason },
    MaxTurns,
    MaxTokens,
    Cancelled,
    Blocked { phase: RunPhase, reason: String },
}

pub enum HostYieldReason {
    NeedsUser,
}

pub enum RunPhase {
    Load,
    Reconcile,
    PrepareRun,
    PrepareRequest,
    Stream,
    Gate,
    Execute,
    Commit,
}
```

`RunOutcome` is the kernel's only budget/result contract. The Goal Runner,
headless driver, and interactive driver interpret it independently.
`RecoveryStats` is the authoritative source for the facade's
`AgentRuntime.recovery_state`; host-visible `AgentNotice::RecoveryBackoff` and
`AgentNotice::Continuation` events remain observability traffic and must not be
reconstructed into counters. Request bodies and message ranges remain part of
`UsageRecord`, not recovery counters.

`RunInput.inputs` is the only source of new messages. If `initialized` is false,
the kernel loads `session` through `TranscriptStore` and fills `AgentState`
before applying those inputs. If it is true, the host owns the transcript and
the kernel must not reload it behind the host's back.

Tact always supplies `initialized = true`, but only after the facade has called
`ensure_session`. The TUI deliberately leaves that load to the activation
entry point, so constructing `AgentState` first would silently skip resumed
SQLite history. A standalone host may instead supply `initialized = false` and
let the kernel perform the load.

### State Authority

There is exactly one mutable authority for an activation:

1. A standalone kernel host owns an `AgentState`.
2. During Tact migration, `AgentRuntime.context`,
   `AgentRuntime.provider_state`, `Agent.turns_taken`, and the behavioral
   sidecar fields listed under
   [Compatibility Facade](#compatibility-facade) remain the in-memory
   authority because they are public and already mutated directly by tests and
   drivers.
3. `AgentKernel` retains no transcript or cursor between calls. `run` receives
   `&mut AgentState`, commits through the transcript port, and returns the
   updated outcome.

`tact::Agent` constructs the borrowed `AgentState` view at activation entry and
writes committed changes back to the existing public fields. Because
`agent_loop_detailed` takes `&mut self`, no facade field can change concurrently
during that activation. Kernel dependencies are recomposed at activation entry
when a mutable facade field such as the provider, MCP router, or hooks changed,
or the adapter exposes the same shared handle as that field. The kernel never
keeps a stale clone of mutable product state.

## Required Service Contracts

The exact Rust signatures may change during implementation, but the ownership
and object-safety requirements below are fixed.

All required services are stored as `Arc<dyn Trait>`. Their traits therefore
have no generic methods or return-position `impl Trait`. Async methods use
`#[async_trait]`, and streaming returns an owned boxed stream:

```rust
pub type ModelStream = Pin<
    Box<dyn Stream<Item = Result<ModelEvent, ModelError>> + Send + 'static>,
>;
```

Passing `&dyn ModelPort` into another object-safe service is supported. If an
implementation cannot keep its generated future `Send`, that is an adapter bug,
not a reason to make the kernel generic over every capability.

Every async extension point receives a `CancelToken`. An extension may return
`Cancelled`; it may not swallow cancellation and continue making model or tool
calls.

Cancellation is observed at defined boundaries rather than treated as an
implicit future drop. The model and tool rules are stated under
[Cancellation Contract](#cancellation-contract).

### ModelPort

```rust
#[async_trait]
pub trait ModelPort: Send + Sync {
    fn capabilities(&self) -> ModelCapabilities;

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError>;

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError>;

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::unsupported("native compaction"))
    }
}

pub enum ModelEvent {
    MessageStarted {
        message_id: MessageId,
        role: AgentRole,
    },
    MessageDelta {
        message_id: MessageId,
        delta: MessageDelta,
    },
    ProviderActivity {
        activity: ProviderActivity,
    },
    /// Streaming usage is display/accounting progress only. The terminal
    /// response owns the authoritative usage value.
    Usage {
        usage: TokenUsage,
    },
    Finished(ModelResponse),
}

pub struct ModelResponse {
    pub message_id: MessageId,
    pub message: AgentMessage,
    pub stop_reason: StopReason,
    pub usage: Option<TokenUsage>,
    /// Serialized request body retained for diagnostics when the adapter has it.
    pub request_body: Option<Vec<u8>>,
    pub cursor: CursorUpdate,
}

pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Refusal,
    PauseTurn,
    Unknown { raw: String },
}

pub enum ModelCallKind {
    Stream,
    Complete,
    LocalCompact,
    NativeCompact,
}

pub enum MessageDelta {
    Text {
        delta: String,
    },
    Thinking {
        phase: ThinkingDelta,
    },
}

pub enum ThinkingDelta {
    Started,
    Delta(String),
    Finished,
}

pub struct ProviderActivity {
    /// Stable provider call id used by both the started and terminal events.
    pub id: String,
    /// Zero-based ordinal among hosted activities in this model response.
    pub step: u32,
    pub kind: ProviderActivityKind,
}

pub enum ProviderActivityKind {
    WebSearch {
        phase: ProviderActivityPhase,
        query: Option<String>,
        failure: Option<String>,
        duration: Option<Duration>,
    },
}

pub enum ProviderActivityPhase {
    Started,
    Completed,
    Failed,
}
```

`ModelStream` must yield zero or more progress events and exactly one terminal
`ModelEvent::Finished(ModelResponse)`; any `Err` item is terminal. The kernel
rejects a second terminal item or any event after a terminal item. The response
always carries the message identity, complete logical assistant message, stop
reason, cursor update, authoritative usage, and request-body diagnostics, so
tool dispatch never depends on reconstructed deltas. If `MessageStarted` was
emitted, its id must match `ModelResponse.message_id`.

`MessageDelta::Text` is append-only display progress. A thinking block is a
`Started`, zero or more `Delta`, then `Finished` sequence; adapters that only
receive reasoning deltas synthesize the boundaries. Deltas are never the
authoritative message and cannot add or remove tool calls from the terminal
`ModelResponse`. `MessageStarted` and every delta for one response use the same
`MessageId`.

The kernel wraps the configured `ModelPort` in a per-call `ObservedModelPort`
before passing it to `ContextPolicy`. That scope emits `ModelCallStarted` and
`Usage` events with `ModelCallKind`, records request-body diagnostics, and
collects the response metadata needed by token-usage persistence. This includes
local summary compaction, which currently uses the non-streaming
`create_message` path, and native compaction, which uses the provider-specific
endpoint.

`ModelCapabilities` carries values the kernel needs without provider branching:

```rust
pub struct ModelCapabilities {
    pub model: String,
    pub context_window: usize,
    pub max_output_tokens: u32,
    pub supports_streaming: bool,
    pub supports_native_compaction: bool,
}
```

`ModelCallStarted.model` and `max_output_tokens` come from these capabilities;
opaque request metadata supplies product display fields that the kernel does not
interpret.

`ModelError` carries a typed `ModelErrorKind`:

- `ContextOverflow`;
- `Transient`;
- `Permanent`;
- `Cancelled`;
- `Unsupported`.

The kernel uses these kinds for recovery. It must not inspect provider error
strings or match on an `LlmProvider` enum.

### ToolPort

```rust
pub trait ToolResolver: Send + Sync {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError>;
}

#[async_trait]
pub trait ToolPort: ToolResolver {
    /// Refresh any dynamic catalog (for example MCP list-changed servers) and
    /// return the snapshot used for the next model request.
    async fn refresh(&self, cancel: CancelToken) -> Result<ToolSnapshot, ToolError>;

    async fn execute(
        &self,
        call: ResolvedToolCall,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError>;
}

pub struct ToolSnapshot {
    pub specs: Vec<ToolSpec>,
    pub generation: u64,
}

pub struct ToolExecutionContext {
    pub call_id: String,
    pub progress: Arc<dyn ToolProgressSink>,
    pub cancel: CancelToken,
    /// Number of permitted model calls from the current turn onward, including
    /// the turn that produced this tool call. `None` is unbounded.
    pub remaining_turns: Option<u32>,
}

pub trait ToolProgressSink: Send + Sync {
    fn emit(&self, chunks: Vec<ToolOutputChunk>);
}

pub enum ToolOutputStream {
    Stdout,
    Stderr,
    Other,
}

pub struct ToolOutputChunk {
    pub stream: ToolOutputStream,
    pub text: String,
}

pub enum ToolExecutionStatus {
    Succeeded,
    Failed,
}

pub struct ToolExecutionResult {
    pub status: ToolExecutionStatus,
    pub outcome: ToolOutcome,
    /// The original operational failure. Post-tool policy receives this even
    /// when a later hook rewrites the visible output.
    pub error: Option<ToolExecutionError>,
}

pub struct ToolExecutionError {
    pub kind: ToolExecutionErrorKind,
    pub message: String,
}

pub enum ToolExecutionErrorKind {
    Tool,
    OutputPersistence,
    Mcp,
    HostInteraction,
    Adapter,
}
```

`ToolPort::refresh` is required before each model request. Its snapshot supplies
the request's tool specs, preserving the current mid-turn MCP refresh behavior.
The kernel does not call a synchronous `specs()` accessor.

`ToolResolver` is deliberately the only tool capability exposed to
`ToolGate::before`. The kernel owns the full `ToolPort`, performs refresh and
execution itself, and passes `&dyn ToolResolver` to the gate. A gate must not be
able to bypass the execute path merely because it needs to resolve a hook
rewrite.

MCP ownership must support refresh through `&self` without creating a second
mutable authority. The Tact adapter moves `MCPToolRouter`'s mutable catalog and
connection state behind a shared interior-synchronized catalog handle; the
router itself becomes cheaply cloneable and exposes `refresh_changed(&self)`.
`Agent.mcp_router` remains the public product handle, while an activation takes
one clone. A full reload replaces that handle only between activations, which is
safe because `agent_loop_detailed` already holds `&mut Agent`; the active
activation therefore sees either the old or the new handle, never both. The
kernel path owns this behavior now. Tests cover a mid-turn list change and a
full router replacement between activations.

`ToolPort::execute` returns `Ok(ToolExecutionResult::Failed...)` for an ordinary
tool failure. `Err(ToolError)` is reserved for an adapter or contract failure,
which the kernel records as a failed result unless it invalidates the run.
Progress travels through the kernel-provided sink, which emits the ordered
`AgentEvent::ToolProgress`; adapters must not write tool progress directly to a
host protocol.

Redaction is also an adapter boundary, not kernel policy. `TactToolPort` must
redact live chunks before passing them to `ToolProgressSink` and redact the
final visible output before returning `ToolExecutionResult`. A streaming
redactor keeps its held-back tail across chunks so a secret split across two
events cannot leak; completion flushes that tail exactly once.
`TactToolGate::after` applies the same redaction policy to any hook-rewritten
visible output before returning, event emission, or persistence. Live, final,
and hook-rewritten paths share one policy and parity tests include a secret
split across chunks. The kernel receives only redacted result content and never
needs to know Tact's redaction levels or patterns.

`ToolOutputChunk` is the neutral API type. `tact_protocol` keeps its
host-facing output type and the event bridge converts between them.

`ResolvedToolCall` contains only kernel-relevant data:

- tool name and call id;
- validated input;
- `ResourceAccess` for scheduling;
- optional execution-mode constraints.

`ResourceAccess` uses opaque keys so the kernel stays platform-neutral:

```rust
pub struct ResourceKey {
    pub namespace: String,
    pub components: Vec<String>,
}

pub struct ResourceScope {
    pub key: ResourceKey,
    /// A path directory scope conflicts with its descendants. Exact resources
    /// such as one MCP server name leave this false.
    pub covers_descendants: bool,
}

pub struct ResourceAccess {
    pub reads: Vec<ResourceScope>,
    pub writes: Vec<ResourceScope>,
    pub barrier: bool,
}
```

The tool adapter computes keys from paths, MCP server names, or other domain
resources. The kernel compares namespaces and component prefixes; it never
parses host paths. This preserves the current ancestor/descendant path conflict
rule without making the kernel platform-aware. `spawn_subagent` with
`worktree: true` returns independent access; other subagents remain barriers.

`ToolOutcome` carries only logical result data:

- text content;
- optional image content, which the kernel emits as a companion `Image` block
  immediately after that call's `ToolResult` block; it must never be flattened
  to `[image]` text or dropped;
- typed directives:
  - `RequestCompaction { focus: Option<String> }`;
  - `YieldToHost { reason: HostYieldReason }`;
  - `CompleteRun` for an adapter that has durably delivered a terminal verdict
    inside the allowed turn budget;
- optional structured metadata for an event observer.

`ToolPresentationInfo`, popup policy, and display labels are not part of
`ToolOutcome`.

### ToolGate

Permissions are a pipeline, not a single boolean. The tool-gate adapter owns the
entire policy sequence, including sensitive-path refusal, pre-tool hooks,
permission modes, approval prompts, and post-tool hooks.

```rust
#[async_trait]
pub trait ToolGate: Send + Sync {
    async fn before(
        &self,
        call: ToolCall,
        resolver: &dyn ToolResolver,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<GateDecision, ToolGateError>;

    async fn after(
        &self,
        call: &ResolvedToolCall,
        result: ToolExecutionResult,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<ToolExecutionResult, ToolGateError>;
}

pub struct ToolGateContext {
    pub session: SessionRef,
    /// Cumulative dispatched main-loop request attempts at this gate call.
    pub total_turns: u32,
    pub remaining_turns: Option<u32>,
    pub metadata: serde_json::Value,
}

pub enum GateDecision {
    Allow {
        /// Final call after any PreToolUse mutation and re-resolution.
        call: ResolvedToolCall,
        label: Option<String>,
    },
    Deny { reason: String },
}
```

`ToolGateContext.total_turns` is copied from `AgentState.total_turns` after the
current main-loop request was counted but before tool preflight. It carries the
session, cumulative turn, remaining-turn budget, and opaque call metadata. It
never carries the transcript, `AgentState`, or a mutable activation handle.
`TactToolGate` combines that neutral call context with its shared product
services to build the read-only hook view; the hook payload's `turn_id` comes
from `total_turns`.

The gate receives the raw call because PreToolUse hooks can mutate its name and
input. It resolves the original call and applies the sensitive guard before
hooks. A `Credential` hit on the original call fails closed before any hook can
rewrite it.

The kernel preflights every raw call through `ToolGate::before`, but invokes
`ToolPort::execute` only for the final `Allow` value returned after all hook and
permission rewrites. `Deny`, gate errors, and cancellation never execute a tool.
The narrow resolver argument makes this ordering a type-level boundary rather
than a convention that each gate must remember.

After `PreToolUse`, the gate re-resolves and reclassifies before deciding
whether auto-approval or a permission prompt applies. If a prompt is applicable,
`PermissionRequest` may mutate the call again; the gate repeats the same
re-resolution and classification before accepting an allow or deny and before
scheduling execution. At each checkpoint it recomputes all policy inputs:

- sensitive classification, again failing closed on `Credential`;
- declared risk and secret-tier escalation;
- stable permission/tool name;
- MCP auto-approval for the final server/tool pair;
- permission prompt policy and label;
- `ResourceAccess` used for scheduling.

An original benign call may not be rewritten into a credential read or a
different-risk MCP call that inherits the original approval. Likewise, an
original `Secret` or `Credential` classification cannot be erased by a hook.
The kernel schedules and executes only the final `ResolvedToolCall` returned by
`Allow`.

The gate may await a host approval response. Tact injects its existing
`UiResponder` and update sink into the adapter, so permission prompts,
`RequestSelect`/`RequestMultiSelect`, hook status, and hook context remain
adapter-owned host traffic. They use the same multiplexed sink but are not
kernel events. `TactToolGate` reads and mutates permission state only through
the shared `PermissionAuthority`; it does not own a copied manager. Before each
hook callback it builds a `TactActivationView` from the shared services and the
call-scoped activation metadata. Hook callbacks never receive the facade
`Agent`, an `AgentState`, or a writable transcript. The view's queue handles are
the only writable context channel, and `TactContextPolicy` later turns queued
items into appended messages.

`TactToolGate::after` runs the existing post-tool policy and must redact any
hook-rewritten visible output before returning it. A gate error in `after`
becomes a failed `ToolExecutionResult`; it never silently restores a successful
result. A post-tool rewrite may change visible output or add a stricter failure,
but it cannot turn a failed result into success or discard the original
operational error.

When the final status is `Failed`, the kernel clears `CompleteRun`,
`YieldToHost`, and `RequestCompaction` from that result while preserving
`ToolExecutionResult.error`. This applies when the tool itself failed and when
`ToolGate::after` or a post-tool hook converts an initially successful result
into a failure. A failed call must not complete a Goal, yield the host, or
trigger compaction as a side effect of its pre-rewrite payload.

This design keeps the kernel free of `PermissionManager`, `Scanner`,
`UiResponder`, and Claude hook details while preserving the current security
order inside the Tact gate adapter. It also keeps hook compatibility without
smuggling the facade's mutable state into the kernel.

### ContextPolicy

Context construction, prompt assembly, compaction, and injected context are
product policy:

```rust
#[async_trait]
pub trait ContextPolicy: Send + Sync {
    async fn prepare_run(
        &self,
        input: &RunInput,
        state: &ContextView<'_>,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<ContextPreparation>, ContextError>;

    async fn prepare_request(
        &self,
        state: &ContextView<'_>,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, ContextError>;

    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError>;

    /// Observe one replacement that the kernel has durably committed.
    async fn compaction_committed(
        &self,
        state: &ContextView<'_>,
        request: &CompactionRequest,
    ) -> Result<(), ContextError> {
        Ok(())
    }
}

pub enum PreparationDecision<T> {
    Continue(T),
    /// Try compaction before continuing. `if_skipped` is the already-computed
    /// fallback to use when policy vetoes or makes compaction a no-op.
    Compact {
        request: CompactionRequest,
        if_skipped: T,
    },
    Block(RunStop),
}

pub struct ContextPreparation {
    /// Final inputs after UserPromptSubmit-style transformation.
    pub inputs: Vec<AgentMessage>,
    /// Session-start context and other messages to persist before the first
    /// model request.
    pub appended_messages: Vec<AgentMessage>,
}

pub struct RequestPlan {
    pub request: PreparedRequest,
    /// Hook, subagent, or other context discovered before this request.
    pub appended_messages: Vec<AgentMessage>,
}

pub enum CompactionRequest {
    Auto { focus: Option<String> },
    Manual { focus: Option<String> },
    Recovery { focus: Option<String> },
    Command { focus: Option<String> },
    Micro,
}

#[derive(Clone, Copy)]
pub enum CompactionKind {
    Auto,
    Manual,
    Recovery,
    Command,
    Micro,
}

impl CompactionRequest {
    pub fn kind(&self) -> CompactionKind;
}

pub enum CompactionOutcome {
    Replaced(ContextReplacement),
    /// A policy hook vetoed compaction or there was nothing to replace. This
    /// is not permission to retry the same request immediately.
    Skipped { reason: String },
}

pub struct ContextReplacement {
    /// `None` keeps the logical transcript unchanged, as required by native
    /// cursor-only compaction. `Some(Vec::new())` explicitly clears it.
    pub transcript: Option<Vec<AgentMessage>>,
    pub cursor: CursorUpdate,
}
```

`ContextView` exposes only the transcript, cursor, turn budget, pending input,
model capabilities, and `remaining_turns`. `remaining_turns` counts the current
request/turn when it is already in progress: a value of `Some(1)` means this is
the final permitted model call. `ContextView` does not expose `AgentState`
mutably or let an adapter touch kernel scheduling state.

`PreparationDecision` is the only way policy asks the kernel to compact.
`ContextPreparation` and `RequestPlan` may add transcript messages, but they
cannot mutate or clear history themselves. The kernel persists
`ContextPreparation.appended_messages` before constructing the first request
and persists `RequestPlan.appended_messages` before dispatching the request
returned with them. The kernel validates all origins; a persistence failure
stops the run before that model call.

`TactContextPolicy` may use the same `TactSharedServices` and
`TactActivationView` as the gate. SessionStart, UserPromptSubmit, and compaction
hook callbacks can inspect read-only activation metadata and enqueue context,
but they cannot write the transcript. The policy drains those queues into the
typed `appended_messages` return value, and the kernel performs the commit. This
replaces the former implicit assumption that a hook callback can mutate the
facade's conversation state through `&Agent`.

`PreparedRequest` contains:

- the system prompt;
- provider-neutral model messages;
- the optional model cursor;
- optional request metadata.

The kernel attaches the tool specs from the refreshed `ToolSnapshot` after
`prepare_request`. This guarantees that prompt/hook preparation and the model
request see the same tool generation.

Tact's `ContextPolicy` implementation may use Tera, skills, memory, AGENTS.md,
directory snapshots, and local or native compaction. Those dependencies stay in
`tact` (or a later `tact_capabilities` crate), not in the kernel.

`prepare_run` replaces the current SessionStart hook injection path.
It also applies UserPromptSubmit transformations to the new inputs before they
are persisted. `prepare_request` covers pending subagent results, hook context,
auto/micro-compaction decisions, and request assembly. `compact` covers manual,
automatic, recovery, command, micro, local summary, and native compaction.
Because the context policy receives the kernel's instrumented model port,
compaction calls participate in the same event, usage, request-body, and retry
accounting as ordinary calls.

For a `Compact` decision, the kernel calls `ContextPolicy::compact`, persists a
`Replaced` value through `TranscriptStore::replace`, updates the caller-owned
transcript/cursor, emits `ContextCompacted`, and only then calls
`compaction_committed`; the kernel repeats the same preparation phase.
`TactCompactor` stages summary bookkeeping during `compact`, so
`compaction_committed` can advance `CompactState`, stats, the SessionStart
source, and the one PostCompact invocation only after durable success. A failed
replacement never acknowledges a staged commit.

A `Skipped` outcome uses `if_skipped` and does not rerun the same preparation
immediately, so a veto cannot spin; staged preparation is cleared so stale
inputs or queued context are not replayed. During recovery, a skipped
compaction aborts the retry and surfaces the original `ContextOverflow` error
because the fallback request was the one that already overflowed. A successful
full replacement suppresses the next automatic request-compaction check once.
Micro compaction does not suppress that check because it only trims old tool
results and does not advance full compaction state or run PostCompact.

The same kernel-owned compaction helper backs the facade's explicit
`Command` request, so `/compact` cannot bypass observed usage, atomic
persistence, or the policy veto contract.

### TranscriptStore

The kernel needs a persistence barrier for transcript correctness, but not a
database:

```rust
#[async_trait]
pub trait TranscriptStore: Send + Sync {
    async fn load(&self, session: &SessionRef) -> Result<TranscriptSnapshot, StoreError>;

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError>;

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError>;
}
```

`CursorUpdate` distinguishes `Unchanged`, `Replace(Some(cursor))`, and
`Replace(None)`. `transcript = None` retains the stored logical transcript;
`Some(&[])` explicitly clears it. Together these distinguish native
cursor-only replacement from transcript replacement and from an unchanged
record. The runtime's SQLite adapter keeps the current atomic
message-plus-provider-state replacement semantics inside this port. The kernel
never imports `sqlx` and never learns table names or row ids.

The persistence barrier is explicit:

1. New inputs and prepared context messages are appended before the model call.
2. The assistant message and cursor update are committed atomically before any
   tool from that response may execute.
3. Tool-result messages are appended in assistant source order, even when tool
   completion events are in another order.
4. Every terminal path that already persisted an assistant tool call must
   persist paired synthetic tool results before returning.

Tool-result message cardinality is part of the durable transcript contract.
One assistant response containing N tool-use blocks is followed by exactly one
user-role `AgentMessage` whose origin is `ToolResult`. In assistant source order,
that message contains exactly one `ToolResult` block per call, plus any
companion `Image` block immediately after the result that produced it. It is not
N user messages. This applies when real, denied, cancelled, unknown, truncated,
or synthetic results are mixed: they are assembled into that one message.
Completion order must never reorder its blocks or split it into additional
messages, and image content is never converted to placeholder text.

A sessionless host uses a production no-op `TranscriptStore`; it still returns
receipts and preserves the commit barrier, but retains no durable rows.
`TranscriptSnapshot` and `MessageReceipt` expose opaque receipt/range values so
the kernel can attach message ranges to usage events without learning database
ids. The Tact facade sidecar is updated from those returned ranges; neither the
kernel event nor the API crate carries `first_message_db_id`,
`last_message_db_id`, or `llm_call_last_message_id` directly.

On load, the kernel also reconciles durable interrupted tool calls before
`prepare_run`. For every assistant message containing `ToolUse` blocks, it
collects the directly following grouped `ToolResult` answer, if any, and
synthesizes deterministic "activation interrupted before result commit" blocks
for unanswered call ids. It preserves any real results and replaces the
transcript atomically with `CursorUpdate::Unchanged` before any resumed model
request. A healthy transcript is a no-op. This prevents a crash after the
assistant/cursor commit from leaving an orphaned tool call for either
Anthropic-shaped conversion or Responses state.

Token-usage persistence, request-body retention, session locks, and input
history remain runtime/host concerns. Usage persistence uses the explicit async
`UsageSink` below; these services do not enlarge the kernel transcript port.

### UsageSink

`AgentEvent::Usage` is display/observer progress and cannot carry the durability
barrier because the event port is non-blocking. The kernel therefore receives a
separate async sink:

```rust
#[async_trait]
pub trait UsageSink: Send + Sync {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageError>;

    /// Wait until every previously accepted record is durable.
    async fn flush(&self) -> Result<(), UsageError>;
}

pub struct UsageRecord {
    pub call_id: String,
    pub kind: ModelCallKind,
    pub usage: Option<TokenUsage>,
    pub request_body: Option<Vec<u8>>,
    pub message_range: Option<MessageRange>,
}
```

`record` is idempotent by `call_id`; implementations may enqueue an ordered
write, but the returned success means the record was accepted without loss.
The local SQLite adapter uses an upsert keyed by the call id and includes the
model call kind, request body, usage, and message range. The additive schema is:

- add nullable `token_usages.call_id`;
- create a partial unique index on `(session_id, call_id)` where `call_id IS
  NOT NULL`, so legacy NULL-call-id rows remain insertable and readable;
- change `record_token_usage` (or its new origin-aware sibling) to accept
  `call_id: Option<&str>` and use `INSERT ... ON CONFLICT(session_id, call_id)
  DO UPDATE` when a call id is present;
- keep the existing plain-insert path for legacy callers that still supply no
  call id;
- apply request-body retention after either insert or update, so replaying the
  same call cannot create a second row or bypass body trimming.

Kernel-path records always carry a non-empty `call_id`. Replaying one must leave
one row with the newest complete payload, while pre-migration rows with NULL
call ids continue to load and appear in history queries.

A sessionless host pairs `NoopTranscriptStore` with `NoopUsageSink`. The no-op
usage sink accepts records and `flush` succeeds, but it persists no rows; this
is an explicit "usage is not durable for this host" policy rather than an
omitted dependency.

For an ordinary model call, the kernel awaits `record` after the assistant
message and cursor commit and before any tool from that response executes. A
record failure stops the run before tools and surfaces as `RunFailed`. For a
compaction call, the observed model scope awaits `record` before its result is
used; its message range is `None`. Before returning any terminal outcome, the
kernel awaits `flush`; a flush failure becomes `RunFailed` rather than silently
reporting success. This makes usage rows observable as soon as the facade
returns and preserves the current native-compaction failure behavior.

### Legacy Session Migration

New sessions persist `MessageOrigin` alongside logical messages. The Tact SQLite
adapter uses an additive `messages.origin` column:

- `origin` is nullable and stores the serialized closed `MessageOrigin` value,
  not a free-form inferred label;
- the existing schema-creation path adds the column for new databases, and the
  open path checks `PRAGMA table_info(messages)` then uses `ALTER TABLE
  messages ADD COLUMN origin` for existing databases;
- append and replacement APIs accept an origin-bearing stored message record;
  compatibility readers still accept NULL origins from pre-migration rows;
- load returns an origin-bearing stored record. A non-null origin is decoded
  strictly; a null origin is inferred at read time and is never rewritten in
  place;
- the legacy in-memory `MessageKind` is reconstructed from the decoded origin
  for old callers that still need `Summary`, `HookContext`, or `GoalContext`
  behavior.

Inference is deterministic and idempotent: loading the same legacy row twice
produces the same origin, and a later replacement writes the explicit origin so
future loads no longer depend on content heuristics. A null legacy origin is
inferred only from the message's role and complete content shape, in this order:

1. A complete `<context-handoff>...</context-handoff>` cell or a message
   beginning with the legacy `SUMMARY_PREFIX`:
   `CompactionSummary`.
2. A complete `<hook-context source="...">...</hook-context>` frame:
   `InjectedContext { source }`.
3. A complete `<goal-context>...</goal-context>` frame: `GoalContext`.
4. A complete `<subagent-finished ...>...</subagent-finished>` frame:
   `SubagentResult`.
5. Content exactly equal to a known continuation prompt: `Continuation`.
6. A user-role message containing only tool-result blocks: `ToolResult`.
7. Role assistant: `Assistant`.
8. Otherwise: `User`.

Marker and continuation detection applies to user-role legacy cells. A prefix,
substring, or marker-like sentence inside otherwise genuine user text must not
change its origin. Hook sources are parsed from the complete opening frame.

The inference and round trip are tested against pre-migration rows containing
normal user text, genuine text containing marker-like examples, hook context,
Goal context, subagent results, continuation prompts, summaries, assistant
output, and tool results. A legacy synthetic user-role message must not become
`MessageOrigin::User`. Tests open a pre-column database, migrate it without
losing rows, read it repeatedly for deterministic origins, then replace it and
verify the explicit origins survive a reopen.

### EventPort

```rust
pub trait EventPort: Send + Sync {
    fn emit(&self, event: AgentEvent);
}

pub trait EventObserver: Send + Sync {
    fn observe(&self, event: &AgentEvent);
}
```

The event port is non-blocking and preserves emission order. The runtime
multiplexes it to the TUI, headless JSON output, RPC subscribers, and audit
observers. Event delivery is a control-plane concern; a slow host must not
pause the model stream or tool wave.

For each event, the kernel calls `EventPort::emit` first and then every
`EventObserver` in registered order. Both paths must return without waiting on
host I/O.

The signature cannot enforce this property. `emit` implementations must enqueue
without waiting for a consumer (the default runtime uses an unbounded
host-owned channel); they must not await I/O, hold a lock across delivery, or
synchronously call arbitrary extension code. `EventObserver` implementations
follow the same rule. The recording test port validates order, not wall-clock
behavior.

`EventObserver` is read-only: it receives every event once, in registration
order, after `EventPort`, and returns no replacement or decision. It cannot
claim, veto, or short-circuit an event, so the first slice has no duplicate
claim or conflict-resolution policy. A future extension phase that can affect
control flow must define its own typed ordering and failure semantics in the
migration that introduces it; it must not reuse a generic plugin registry.

## Event Contract

The kernel emits events in a deterministic order:

```text
RunStarted
TurnStarted
ModelCallStarted
MessageStarted / Delta* / Finished
Usage
ToolStarted / Progress* / Finished*
ContextCompacted
Notice*
TurnFinished
RunFinished | RunFailed
```

`Usage` follows the committed assistant message and its awaited `UsageSink`
record, and precedes tool execution, matching the current accounting window.
`ContextCompacted` appears at the point the compaction occurs, including between
a failed request and its retry.
Tool completion events follow actual completion order. Tool-result messages
are appended to the transcript in assistant source order. These two orders are
intentionally different and must remain separately tested.

The event enum has no presentation fields:

```rust
pub enum AgentEvent {
    RunStarted { run_id: RunId },
    TurnStarted {
        run_id: RunId,
        turn: u32,
        activation_turn: u32,
        max_total_turns: Option<u32>,
    },
    ModelCallStarted {
        call_id: String,
        kind: ModelCallKind,
        model: String,
        max_output_tokens: u32,
        metadata: serde_json::Value,
    },
    MessageStarted { message_id: MessageId, role: AgentRole },
    MessageDelta { message_id: MessageId, delta: MessageDelta },
    MessageFinished { message_id: MessageId, message: AgentMessage },
    ProviderActivity { activity: ProviderActivity },
    ToolStarted {
        call: ToolCall,
        /// `None` when resolution fails; the bridge still creates the failed
        /// card, matching the current unknown-tool update sequence.
        descriptor: Option<ToolDescriptor>,
    },
    ToolProgress { call_id: String, chunks: Vec<ToolOutputChunk> },
    ToolFinished {
        call_id: String,
        result: ToolExecutionResult,
        permission_label: Option<String>,
        duration: Duration,
    },
    Usage {
        call_id: String,
        kind: ModelCallKind,
        usage: Option<TokenUsage>,
        request_body: Option<Vec<u8>>,
        message_range: Option<MessageRange>,
    },
    ContextCompacted { kind: CompactionKind },
    Notice { notice: AgentNotice },
    TurnFinished { turn: u32, activation_turn: u32 },
    RunFinished { outcome: RunOutcome },
    RunFailed { error: RuntimeErrorInfo },
}

pub enum AgentNotice {
    TurnLimitReached { max: u32 },
    CancellationRequested,
    RecoveryBackoff {
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        summary: String,
    },
    Continuation {
        attempt: u32,
        max_attempts: u32,
    },
    CompactionStarted { kind: CompactionKind },
}
```

`ModelCallStarted.metadata` is opaque display metadata prepared by the product
runtime, such as thinking budget or reasoning effort. The kernel transports it
but does not interpret it.

`ProviderActivity` represents server-side events such as hosted web search. It
is not a `ToolCall` and must never cause kernel tool execution. The runtime may
map it to the current web-search card.

It also does not create a user-role message or replace the activation's inputs.
Hosted search is injected by the provider into its own model context; on the
Tact side it remains response activity plus whatever final assistant content the
provider returns. The transcript and TUI golden tests assert both halves of the
contract: injected context keeps a non-`User` origin, and hosted search produces
`ProviderActivity` updates without `ToolStarted` or a fabricated user turn.

`ProviderActivity.id` is stable across its lifecycle and `step` is the
zero-based ordinal of hosted activities in one model response, assigned in
first-seen order. If a provider omits `Started`, the adapter synthesizes it
before the terminal event. It must emit at most one terminal `Completed` or
`Failed` activity per id; duplicate terminal events are ignored. A provider item
that ends in a non-terminal search status is reported as `Failed`, carrying the
failure detail rather than silently claiming success. `duration` is set on the
terminal activity when the adapter can measure it.

The TUI bridge maps:

| Kernel event | Existing TUI update |
|---|---|
| `TurnStarted` | `TurnStats` |
| `ModelCallStarted` | `ModelInfo` |
| `MessageDelta` text/thinking | `StreamChunk` / `ThinkingChunk` |
| `ToolStarted` | `StepAdded` + `StepStarted` |
| `ToolProgress` | `ToolProgress` |
| `ToolFinished` | `StepFinished` / `StepFailed` |
| `Usage` | `TokenUsage`; durable call kind/body/range goes through `UsageSink` |
| `ProviderActivity` | The corresponding web-search step events |
| `ContextCompacted` | `Info` or a future dedicated notice |
| `Notice` | `Info` / `MdInfo` |
| `RunFinished` | Driver-level `TaskComplete`, `TaskIdle`, or error state |
| `RunFailed` | Driver-level `Error` and task completion bookkeeping |

The bridge owns argument summaries and `ToolPresentationInfo`. A tool adapter
may expose a separate presentation adapter to the host, but that adapter is
outside the kernel contract.

The following product-owned updates do not originate in the kernel and keep
their existing adapter path into the same multiplexed update sink:

- `RequestSelect` and `RequestMultiSelect`;
- `HookStatus` and `HookContext`;
- `TasksChanged` and `GoalsChanged`;
- `ToolMeta`, `BackgroundTaskFinished`, and `SubagentFinished`;
- `SubagentsChanged`, `ToolTranscript`, and `ToolTranscriptStream`.

Their producers are injected into `TactToolPort`, `TactToolGate`, or the
subagent/background adapters. They are parity-tested alongside kernel events so
the bridge neither drops nor duplicates them. `RequestSelect` requests stay
request/reply control-plane operations, not fire-and-forget `AgentEvent`s.

## Run Semantics

One `AgentKernel::run` call is one bounded activation:

1. Check cancellation, validate the host-owned `AgentState`, load the
   transcript/cursor only when `initialized` is false, and reconcile any
   interrupted tool calls. Reconciliation completes before `RunInput` messages
   are applied.
2. Run `ContextPolicy::prepare_run`. `Continue` carries the transformed inputs
   and session-start context; `Block` selects the terminal outcome without a
   model call; `Compact` follows the kernel compaction protocol below and
   repeats `prepare_run` only after a successful replacement.
3. Persist transformed inputs and prepared context additions before the first
   request, preserving their origins.
4. Check cancellation and the absolute turn cap, then compute the prospective
   `ContextView.remaining_turns` without mutating the counter yet.
5. Run `ContextPolicy::prepare_request` with the same `Continue`/`Compact`/
   `Block` protocol, persist its appended messages, refresh the tool snapshot,
   and build the neutral `ModelRequest`.
6. Re-check cancellation, then increment `total_turns` for this main-loop
   request attempt and emit `TurnStarted` immediately before model dispatch.
   Preparation, compaction calls, and transport retries do not consume a turn.
7. Run the observed model call, aggregate progress, and require one terminal
   `ModelResponse`.
8. Commit the assistant message and cursor through `TranscriptStore`, emit
   `MessageFinished`, await the ordinary call's `UsageSink::record`, and then
   emit usage/diagnostics before executing tools.
9. For `ToolUse`, or for `MaxTokens` with pending calls while continuation
   budget remains, preflight all calls sequentially through `ToolGate`.
10. Schedule the final allowed calls into conflict-free waves and execute them.
11. Process each completion as it arrives; apply post-tool gate processing,
    clear directives if the final status is failed, and append one grouped
    tool-result message with blocks in source order.
12. Apply tool directives inside the wave loop. `YieldToHost` prevents later
    waves from starting; `CompleteRun` ends the activation as `Completed` after
    the current wave only when it survived a successful final result.
13. Continue until a terminal stop reason, cancellation, max-turn, or critical
    error.
14. Await `UsageSink::flush`, emit exactly one terminal event (`RunFinished` or
    `RunFailed`), and return.

`MessageFinished` is emitted only after the assistant message and cursor commit
succeeds. A persistence failure emits `RunFailed` and no tool is executed.
The turn counter increments only after preparation has produced a dispatchable
request and the pre-dispatch cancellation check has passed. A `Block` decision
therefore consumes no turn, while a context-overflow retry is a new main-loop
request attempt and consumes one.

If any terminal path follows a persisted assistant tool-use message, the kernel
first appends a matching synthetic tool result for every pending call. This
includes cancellation, refusal, exhausted max-token continuation, unknown stop
reasons, hook blocks after a response, and user-requested yield. The reason is
stored in the result text so resume and provider validation see a complete
assistant/tool-result pair.

Retry rules remain kernel semantics, but provider classification is supplied by
`ModelErrorKind`:

- `ContextOverflow` may trigger `ContextPolicy::compact` with
  `CompactionRequest::Recovery` and retry; a skipped recovery compaction
  returns the original overflow error;
- `Transient` may back off and retry within the configured budget;
- `MaxTokens` follows the continuation contract below;
- all other errors terminate the run with a typed error.

`MaxTokens + ToolUse` preserves the current recovery order:

1. If continuation budget remains and the truncated response contains tool
   calls, execute the existing tool pipeline rather than abandoning them.
2. Persist one paired tool result for every pending call, apply post-tool
   processing, and honor a real `YieldToHost` or manual compaction request.
3. Append the continuation prompt and issue the next model request with a
   structurally valid assistant/tool-result sequence.
4. If continuation budget is exhausted, persist synthetic truncated results for
   any remaining calls and return `RunStop::MaxTokens`.

A truncated response cannot newly complete a Goal: even if a final-turn
`goal_propose_completion` tool returns `CompleteRun`, continuation recovery
takes precedence. A later successful continuation may propose completion.

The kernel may depend on Tokio `sync` and `time` for these mechanics. It must
not depend on filesystem, process, network, or database APIs.

### Cancellation Contract

Cancellation is cooperative and boundary-specific:

- Before `prepare_run`, before a model request, and between tool waves, the
  kernel checks the token and does not start new work.
- `ModelPort` may observe the token before dispatching an HTTP request or while
  sleeping between retry attempts. Once a response has started, the adapter must
  finish the response or return a typed transport error; it must not manufacture
  a partial terminal response merely because cancellation was requested.
- If cancellation was requested while a stream was in flight, the kernel checks
  the token before recovery classification or retry. A partial stream is never
  converted into a committed assistant message; the run stops as `Cancelled`.
- A cancel observed after a complete model response still commits the assistant
  message and cursor. The kernel then persists cancelled results for every
  pending tool call, emits `Notice::CancellationRequested`, and returns
  `RunStop::Cancelled`.
- A gate may use the token to stop before requesting host approval or before
  allowing execution. A cancelled gate result never reaches `ToolPort`.
- `ToolPort` receives the token. An in-flight tool may terminate cooperatively
  (as `bash` does today), but the kernel does not drop the `FuturesUnordered`
  wave. It waits for the current wave, synthesizes results for calls in later
  waves, and then returns.
- Dropping a provider or tool future is never the cancellation mechanism.

Cancellation has one monotonic state transition per activation:

```text
Running -> cancellation requested -> draining current work -> terminal
```

`CancelToken::cancel` is idempotent. Once cancelled, a token cannot be reset or
uncancelled; repeated writes and repeated calls are harmless. Phase 1 does not
create independent session, turn, gate, or tool cancellation authorities.
Session switching and queued user input remain host concerns. The single
activation token covers its model, gate, and tool scopes, and is passed by clone
only so adapters can observe it. Nested subagents each get a separate activation
token bridged from their own public flag.

Cancellation never rolls back committed transcript, cursor, or usage state.
Uncommitted stream text is discarded. If an assistant tool-use message was
committed, every pending call receives exactly one synthetic or real result
before return. After cancellation is observed, the kernel starts no new model
call, gate approval, or tool; an already-running wave drains. It may emit
`Notice::CancellationRequested`, remaining tool and turn-finish events, and then
exactly one terminal `RunFinished` or `RunFailed`. A mandatory usage flush or
result-persistence failure takes terminal precedence over `RunStop::Cancelled`.
No kernel events follow that terminal event.

`AgentRuntime.cancel_flag: Arc<AtomicBool>` remains Tact's sole public
cancellation authority. At activation entry the facade creates one
`CancelToken`, cancels it immediately if the flag is already true, and starts a
scoped watcher that mirrors subsequent flag writes into the token. The watcher
is tied to the activation guard and stops when the activation exits. The token
is downstream-only inside Tact: `ModelPort`, `ToolGate`, and `ToolPort` observe
it and must not independently cancel it. Each nested subagent activation gets
the same bridge for its own flag. Host code remains responsible for clearing a
stale flag before starting a new activation.

The cancellation tests assert the stop outcome and, whenever an assistant
tool-use message was persisted, durable assistant/tool pairing. They cover
before streaming, during streaming with and without a complete response, after
streaming but before commit, during gate approval, during an in-flight tool, and
between waves. The bridge is constructed inside
`Agent::run_kernel_activation`. The public-facade matrix now covers streaming,
approval, bash, another in-flight tool, and between-wave writes to
`AgentRuntime.cancel_flag`, with one terminal event and committed state. Every
channel-waiting cancellation test has a bounded timeout; repeated cancellation,
one-terminal-event, no-post-terminal-event, and retained-session cases are
explicit assertions rather than timing assumptions.

### Baseline Invariants

Task 0 recorded the following pre-kernel behavior map on 2026-10-26 before the
new crates were added. It is a design-level invariant record, not the
executable F7 baseline. The implementation must preserve these observed
behaviors:

| Invariant | Baseline contract and evidence |
|---|---|
| Cancellation boundaries | The loop checks the flag before a turn, before dispatch, after a completed stream, before tools, during gate preflight, and between waves. It never drops an active wave to emulate cancellation. |
| Turn budget | The cumulative cap is checked before dispatch and the capped turn is included in stats. Compaction calls and transport retries do not consume a turn; a context-overflow retry does. |
| Tool ordering | Tool completion events follow actual completion order, while one grouped user-role `ToolResult` message persists blocks in assistant source order. |
| Tool images | An image-bearing result keeps its companion `Image` block immediately after the corresponding `ToolResult`; it is never flattened to `[image]` text or omitted. |
| Permission order | Sensitive scanning of the original call precedes hooks. `PreToolUse` and `PermissionRequest` mutations force re-resolution and full risk/sensitive/name/MCP/prompt/resource reclassification. |
| Commit before tools | The assistant message and provider cursor commit atomically before any tool from that response executes. |
| Injected context | SessionStart, hook, Goal, subagent, and continuation messages are persisted with non-`User` origins and never count as user turns. |
| Goal final turn | `remaining_turns == Some(1)` is the only turn that may produce `CompleteRun`; non-final proposals continue and truncated responses cannot newly complete a Goal. |
| Usage ordering | Authoritative usage and request bodies are recorded after assistant commit and before tools, compaction usage is recorded before consumption, and the run flushes before returning. |
| Interrupted tools | A loaded assistant tool-use message without its grouped result is reconciled before any resumed request, never left as an orphan. |

The `AgentUpdate` ownership matrix is:

| Source | Updates |
|---|---|
| Kernel bridge | `TurnStats`, `ModelInfo`, `StreamChunk`, `ThinkingChunk`, `StepAdded`, `StepStarted`, `StepFinished`, `StepFailed`, `ToolProgress`, `TokenUsage`, and informational compaction/notice updates. |
| Adapter or product service | `RequestSelect`, `RequestMultiSelect`, `HookStatus`, `HookContext`, `TasksChanged`, `GoalsChanged`, `ToolMeta`, `BackgroundTaskFinished`, `SubagentFinished`, `SubagentsChanged`, `ToolTranscript`, and `ToolTranscriptStream`. |
| Driver | `TaskComplete`, `TaskIdle`, `TaskCancelled`, and terminal `Error` bookkeeping. |

### AgentUpdate Sequence Matrix

`AgentUpdate` is the host/TUI protocol, not a kernel type. A provider emits
`ModelEvent`; the kernel emits `AgentEvent`; `TactEventBridge` maps the latter to
`AgentUpdate`. Adapter-owned traffic uses the same sink without passing through
the kernel. The complete cross-suite sequence evidence is:

| Scenario | Required host-visible evidence | Tests |
|---|---|---|
| Simple text | `TurnStats`/`ModelInfo`/`StreamChunk` before driver-owned `TaskComplete`; no tool step | `simple_text_kernel_run_matches_frozen_legacy_baseline`, `submit_task_emits_task_complete` |
| Tool use | `StepAdded` -> `StepStarted` -> `StepFinished`, preserving image companions and final completion | `tool_use_image_companion_kernel_run_matches_frozen_legacy_baseline`, `submit_task_runs_read_file_tool` |
| Permission approval/denial | `RequestSelect` only when required; exactly one `StepFinished` or `StepFailed` per call | `default_mode_allow_once_runs_edit_file`, `default_mode_deny_blocks_edit_file`, `always_allow_skips_second_permission_prompt`, `permission_denial_kernel_run_matches_frozen_legacy_baseline` |
| Tool progress | Ordered `ToolProgress` before the terminal step update; no duplicate step close | `bash_streams_progress_before_step_finished`, `maps_tool_lifecycle_progress_and_failures_exactly_once` |
| Ask-user | `RequestSelect`/`RequestMultiSelect` plus one successful tool result | `ask_user_with_options_uses_select_popup`, `ask_user_multi_select_uses_multi_popup` |
| Hooks | `HookStatus`/`HookContext` remain adapter-owned and reach the same sink exactly once | `agent_loop_records_tool_hook_context`, `successful_session_start_and_user_prompt_context_keep_non_user_origins`, `kernel_and_adapter_updates_share_one_sink_exactly_once` |
| Background/subagent | `BackgroundTaskFinished`, `SubagentFinished`, and `SubagentsChanged` stay outside kernel events | `parallel_background_runs_each_get_a_sticky_row`, `background_rows_accumulate_across_turns`, `nested_subagent_uses_the_kernel_with_child_owned_services`, `kernel_and_adapter_updates_share_one_sink_exactly_once` |
| Cancellation | One terminal run outcome, drained current work, no later waves, and no `TaskComplete` on a cancelled task | `cancellation_during_stream_matches_frozen_legacy_baseline`, `cancellation_during_tool_wave_matches_frozen_legacy_baseline`, `headless_session_cancel_mid_task_no_done`, `facade_cancel_between_waves_suppresses_later_side_effects_and_pairs_calls` |
| Retry | Recovery notice appears before the eventual assistant stream/completion | `transient_retry_kernel_run_matches_frozen_legacy_baseline`, `mock_timeout_retries_then_succeeds` |
| Compaction | Compaction notice and token usage precede the continuing or terminal response | `context_limit_triggers_auto_compact`, `command_compaction_uses_kernel_owned_replacement_and_events`, `maps_turn_model_deltas_usage_and_notices` |
| MCP refresh | No dedicated refresh update; refreshed tools affect the next request and its normal step sequence | `kernel_refreshes_a_mid_turn_mcp_catalog_before_the_next_request`, `a_server_that_grows_its_tool_list_reaches_the_next_request` |
| Goal yield | Tool step closes, then the activation yields without `TaskComplete` and without a later side-effecting wave | `agent_loop_detailed_reports_goal_yield_and_skips_unexecuted_calls`, `max_tokens_yield_suppresses_later_tool_waves_before_continuation` |
| Goal completion | Final-turn completion returns `Completed`; the driver alone emits `TaskComplete` for a successful task | `goal_completion_on_the_final_turn_returns_completed`, `agent_loop_reminds_a_goal_activation_to_checkpoint_on_its_last_turn`, `submit_task_emits_task_complete` |

The cancellation/abandoned-tool pairing matrix is:

| Stop point | Required result |
|---|---|
| Before model dispatch | No assistant message is persisted; return `Cancelled`. |
| During an incomplete stream | Commit nothing; return `Cancelled`. |
| During a stream that still completes | Commit the assistant/cursor, pair every pending call with cancelled results, return `Cancelled`. |
| After a complete response before tools | Pair every pending call as cancelled and execute none unless an already-running wave must drain. |
| During gate preflight | Answer every assistant call exactly once and execute no unapproved call. |
| During an in-flight wave | Wait for the current wave, synthesize results for later waves, then return `Cancelled`. |
| Refusal, unknown stop, exhausted truncation, or host yield | Persist the assistant response and close every tool call with the matching synthetic result before returning. |

Task 0 baseline commands passed serially:

- `cargo check -p tact`
- `cargo test -p tact --lib agent::` - 203 passed, 0 failed
- `cargo test -p tact_llm --lib` - 281 passed, 17 ignored, 0 failed
- `scripts/check-agent-kernel-deps.sh` syntax check and its workspace-package lookup smoke test.

The completed tree was rechecked serially on 2026-10-27 after the product
adapters, facade state guard, cancellation matrix, and F7 comparisons landed.
`cargo test -p tact_agent_api -p tact_agent_core --quiet` passes 15 API tests
and 64 kernel tests. `cargo test -p tact` passes 1494 tests with 1 ignored; its
live MCP target passes 1 test and ignores 4 network-required tests.
`scripts/check-rust.sh`, with the repository proxy and `no_proxy` settings,
passes the dependency guard, formatting, clippy with warnings denied, and the
`tact-ui`, `tui`, `tact`, and `tact_llm` suites. `tui` passes 750 tests and
`tact_llm` passes 293 with 17 ignored. The three `cargo tree` checks show no
forbidden dependency edge. Nine fixture-backed kernel comparators pass.
The tenth F7 path, `kernel_updates_leave_no_unowned_blank_tui_cells`, also
passes. It renders the real facade's kernel updates through the full TUI and
checks that no blank cell falls outside the status, input, bottom, code, or
theme background surfaces.
The compact regression passes on the direct kernel model path: the neutral
provider boundary preserves the original `MessageContent::Text` representation
used by `context_hash` instead of rewriting it to `Blocks` before Responses
state validation. The Task 6 parity evidence is indexed above, and Task 7
deleted the legacy loop, selector, and temporary bridge.

Passing tests are necessary but not sufficient evidence for the migration.
Before adding product adapters, Task 4 freezes the normalized semantic baseline
required by F7. The baseline covers simple text, tool use including an
image-producing result, permission denial, transient retry, cancellation during
a stream and during a tool wave, injected context, hosted web search, resumed
SQLite history, and TUI full-area blank-background painting. It records the
observable `AgentUpdate`/event sequence, transcript messages and origins, cursor
change, usage records, and terminal outcome where applicable.

The reviewed fixture lives at
`crates/tact/testdata/agent_kernel/legacy_baseline.json`. Tests that already
compare exact sequences remain the authority; the fixture records only behavior
that would otherwise disappear when the legacy loop is removed. Normalization is
part of the contract:

- replace run ids, message ids, call ids, timestamps, durations, database row
  ids, and temporary absolute paths with stable scenario-local placeholders;
- preserve the order and multiplicity of host-visible updates and events,
  including provider activity, tool progress, terminal events, and intentional
  adapter-owned traffic;
- preserve transcript role, origin, content blocks, tool-call pairing, cursor
  state, usage record identity/order, and terminal outcome;
- keep the TUI paint assertion as an exact assertion in its existing test, with
  the baseline manifest recording its test name rather than a screenshot;
- treat a changed fixture as a behavior change requiring explicit approval, not
  as a refresh step.

Every later cutover compares its normalized result with this fixture before the
old path is deleted.

## Tool Scheduling

The current `tool_schedule.rs` logic moves into the kernel, with `PathBuf`
replaced by structured `ResourceScope` values. The guarantees are preserved:

- pure reads may share a wave;
- a write conflicts with reads or writes of the same or overlapping resource;
- a path scope with `covers_descendants` conflicts with descendants while
  namespaces and components keep `/src` distinct from `/src2`;
- barrier calls run alone;
- same-server MCP calls serialize;
- different MCP servers may run in parallel;
- `spawn_subagent` with `worktree: true` is independent; all other subagents are
  barriers;
- assistant source order determines persisted tool-result order.

The tool adapter computes resource keys. For native file tools it uses safe
path resolution; for MCP tools it uses a server-scoped key; for bash,
subagents, and unknown tools it uses a barrier key.

Directives are evaluated at the point a tool completes:

- `RequestCompaction` becomes
  `CompactionRequest::Manual { focus }` and is collected after the current
  dispatch, matching today's manual-compaction behavior.
- `YieldToHost` stops the scheduler before the next wave, marks every later
  unstarted call as not executed, and returns `RunStop::YieldToHost`.
- `CompleteRun` stops before the next wave and returns `RunStop::Completed`.
  This is the generic mechanism behind the final-turn Goal completion special
  case; the kernel never matches the `goal_propose_completion` tool name.
- If the final status of the producing call is `Failed`, all three directives
  are cleared before precedence is evaluated.

Tools already running in the same wave finish, and their events retain actual
completion order. Only later waves are suppressed.

If one wave produces multiple directives, cancellation still wins first;
otherwise `CompleteRun` wins over `YieldToHost`, and either terminal directive
suppresses `RequestCompaction`. This precedence is fixed so a parallel wave
cannot make the stop reason depend on completion timing.

## Goal and Host Yield Semantics

The Goal Runner remains outside the kernel, but the control facts the kernel
currently needs must stay generic:

- `AgentState.total_turns` is the cumulative dispatched main-loop request
  counter; context compaction calls and transport retries do not increment it.
- `RunInput.max_total_turns` is compared with that cumulative value, preserving
  the current absolute cap used by the Goal Runner and subagents.
- `ContextView.remaining_turns` and
  `ToolExecutionContext.remaining_turns` use the same inclusive meaning: the
  number of permitted model calls from the current request/turn onward. Both
  are `Some(1)` on the final permitted call and `None` when unbounded.
- `ContextView.remaining_turns` is computed for the prospective request before
  `total_turns` is incremented; `ToolExecutionContext.remaining_turns` is
  computed after that turn was consumed but still includes the producing turn:
  with a cap, it is `max_total_turns - total_turns + 1`.
- `TactContextPolicy` appends the final-turn Goal checkpoint reminder only when
  `remaining_turns == Some(1)`.
- `goal_request_input` produces `ToolDirective::YieldToHost`; the facade maps
  `RunStop::YieldToHost` back to the existing `GoalYieldReason`.
- `goal_propose_completion` produces `ToolDirective::CompleteRun` after its
  durable mutation only when `ToolExecutionContext.remaining_turns == Some(1)`.
  A non-final proposal returns a normal successful tool result and the
  activation continues.
- On that final permitted turn, the kernel returns `RunStop::Completed`, not
  `MaxTurns`, unless cancellation or a failed tool result wins first.
- If the response carrying the proposal stopped at `MaxTokens`, continuation
  recovery takes precedence and suppresses the new `CompleteRun`.

This preserves the two current Goal special cases without a tool-name branch in
the kernel or moving Goal state into `tact_agent_core`.

## Compatibility Facade

The completed migration preserves all public `tact` call sites:

- `tact::Agent`;
- `AgentRuntime` where still public;
- `AgentLoopOutcome` and `AgentLoopStop`;
- `agent_loop` and `agent_loop_detailed`;
- existing `AgentUpdate` emission;
- existing tests and session data.

`tact::Agent` becomes a facade that owns:

- the current composition inputs for an eligible activation; a fresh
  `AgentKernel` and dependency graph are assembled at activation entry so
  provider, hook, and MCP-router changes are visible on the next run;
- the shared `TactSharedServices` bundle used by the gate, context policy, and
  hook projection, including its `PermissionAuthority` and `McpCatalogHandle`;
- the current marketplace-plugin registry;
- the existing session and plugin registries needed by callers;
- the `AgentUpdate` bridge.

The existing `AgentRuntime.context`, `provider_state`, `Agent.turns_taken`,
client, hooks, MCP router, and tool context remain the authoritative in-memory
product state. `PermissionManager` is reached through the
shared `PermissionAuthority`, not copied into the gate. The facade does not
delegate state ownership to a hidden kernel copy. It transfers one explicit
activation envelope and restores it through an activation guard:

```rust
pub struct FacadeActivationState {
    pub state: AgentState,
    pub sidecar: FacadeSidecar,
}

pub struct FacadeSidecar {
    pub first_message_db_id: i64,
    pub last_message_db_id: i64,
    pub llm_call_last_message_id: i64,
    pub compact_state: CompactState,
    pub recovery_state: RecoveryState,
    pub goal_yield_reason: Option<GoalYieldReason>,
    pub max_turns_reached: bool,
    pub tool_use_counter: usize,
    pub interrupt_hooks_fired: bool,
    pub session_start_hooks_pending: bool,
    pub session_start_source: SessionStartSource,
    pub pending_session_context: Vec<HookContextChunk>,
    pub cached_dir_snapshot: Option<String>,
    pub cached_agents_md: Option<String>,
}

pub fn take_facade_state(agent: &mut Agent) -> FacadeActivationState;
pub fn restore_facade_state(agent: &mut Agent, state: FacadeActivationState);
```

These Tact-only fields stay in the facade; they are not added to
`tact_agent_api`. Every mutable facade field must have exactly one transfer
mode:

| Mode | Fields |
|---|---|
| `AgentState` | transcript, cursor, `total_turns`, `last_token_total`, `initialized` |
| Moved sidecar | row-range anchors, compaction and recovery state, Goal yield, max-turn flag, tool counter, hook flags/source, pending SessionStart context, prompt caches |
| Shared handle | stats, update sink, cancel flag, session store, pending subagent and hook queues, hook status sequence, permission authority, MCP catalog, `ToolContext` |
| Recomposed dependency | model/provider, native tool adapter, MCP router facade, hooks, system prompt, settings, turn cap |

The sidecar snippet covers the activation-mutable owned values; shared handles
stay outside the envelope so detached subagents and hook callbacks continue to
observe the same authority within one logical agent. A subagent owns a separate
services bundle and session; it may receive explicit parent queue/update
handles, but not the parent's live permission manager. An exhaustive
destructuring test must fail to compile when a new mutable facade field is added
without a transfer mode.
`AgentState` owns transcript, cursor, `total_turns`, `last_token_total`, and
`initialized`; the sidecar owns durable range anchors and behavioral state that
must survive a kernel activation. Store receipts update the opaque ranges, and
the guard calls `restore_facade_state` on success, cancellation, error, and
unwind.

The common facade bootstrap runs before path selection. It calls
`ensure_session` so a TUI activation whose `runtime.context` is empty first
loads its SQLite history and provider state. A non-empty direct mutation of
`runtime.context` is preserved because `ensure_session` only restores history
into an empty context. Only after that bootstrap does the typed readiness check
choose a path and `take_facade_state` build `AgentState` with
`initialized = true`. The facade never constructs an empty state and then skips
the resume load.

The activation order is fixed: common bootstrap, path selection, state
transfer, activation execution, guard restore. A resumed-session parity test
starts with an empty runtime context plus a populated SQLite session and proves
the kernel path receives the loaded history. Tests that replace
`agent.runtime.context` or `provider_state` before activation see the mutation
used by the next run.

Kernel dependencies are either rebuilt at activation entry from current facade
fields or hold the same shared product handle. Changing the model, reloading
MCP tools, or attaching hooks cannot leave an immutable stale adapter behind.
The tool adapter and MCP router facade are rebuilt when needed, but both
activations and full-router replacement point at the same `McpCatalogHandle`.
Within one logical agent, the permission authority is never rebuilt, so UI mode
changes, always-allow grants, and denial counters are immediately visible to
the next gate call and the facade. A subagent receives a snapshot into its own
authority and does not observe later parent mutations.

The public `LoopState` hook type is the read-only `TactActivationView`, not
`Agent`. Every in-repo hook now accepts that projection. The facade must never
synthesize an `Agent`, clone the facade, or expose a writable transcript merely
to satisfy a hook.

The original cutover used a per-activation switch while parity tests were being
built. That switch and its selector types were deleted after the parity gates
closed. `agent_loop_detailed` now resets the activation sidecar, calls
`ensure_session`, and immediately enters `run_kernel_activation`. A kernel error
remains a kernel error; there is no fallback path.

The bootstrap order is `ensure_session`, state transfer, activation execution,
then guard restore. Required adapters are constructor-injected, so a missing
permission, transcript, usage, or MCP service is not a runtime "optional slot";
sessionless hosts explicitly choose the no-op transcript and usage adapters.

Each kernel activation mirrors the public `AgentRuntime.cancel_flag` into one
activation-scoped `CancelToken` and stops the watcher when the state guard
exits. The guard restores the facade state on success, cancellation, error, and
unwind.

As of 2026-10-27, `Agent::agent_loop_detailed` calls `ensure_session` and
constructs a real `TactKernelComposition` for every activation.
The gate, context, model, native/MCP tool, SQLite/no-op transcript, SQLite/no-op
usage, event, permission, hook-MCP, catalog, and cancel adapters exist. The
`TactToolGate` is the only policy path, so sensitive scanning, hook mutation,
reclassification, permission approval, and post-tool redaction have one policy
owner. Tool scheduling and transcript assembly are kernel-owned; presentation
and driver-owned task bookkeeping stay in product adapters. The current
`TactContextPolicy` owns prompt assembly, lifecycle hooks, queued-context
injection, and auto/micro decisions; `TactCompactor` implements local, native,
recovery, and command replacement, and `/compact` delegates to the same
kernel-owned helper. The facade conversion uses
`FacadeActivationState`, `FacadeSidecar`, `take_facade_state`, and a
`FacadeStateGuard` that restores the transcript, cursor, turn counters, receipt
ranges, recovery/compaction state, Goal yield state, hook flags, and prompt
caches on success, cancellation, error, and unwind.

`RunOutcome.recovery` is the typed source for `AgentRuntime.recovery_state`;
the facade does not reconstruct counters from `Info` text or events.
`tool_schedule.rs` contains only `ToolResources` and its product-scope helpers.
The old wave scheduler, `ToolScheduleSummary`, the legacy tool-dispatch module,
the activation selector, and `LegacyModelBridge` have been removed.

## Dependency Rules

| Crate | Forbidden dependencies |
|---|---|
| `tact_agent_api` | `tact`, `tact_llm`, `tact_protocol`, provider SDKs, SQL, TUI, OS/process APIs |
| `tact_agent_core` | `tact`, `tact_llm`, `tact_protocol`, provider SDKs, SQL, MCP, TUI |
| `tact_llm` | `tact_agent_core`, `tact_protocol`, TUI |
| `tact_protocol` | runtime internals, storage, provider clients |

The following checks are required:

- `cargo tree -p tact_agent_api` contains no forbidden crate;
- `cargo tree -p tact_agent_core` contains no forbidden crate;
- `cargo tree -p tact_llm` contains no `tact_protocol`;
- a test-only dependency guard runs
  `cargo metadata --format-version 1 --all-features`, parses the resolved graph,
  and fails when any guarded crate gains a forbidden direct or transitive
  dependency; direct manifest-name checks are insufficient;
- parser tests inject a synthetic forbidden transitive edge so the guard cannot
  silently regress to checking only direct dependencies;
- the guard runs from both `scripts/check-rust.sh` and CI after every
  interface-changing phase, not only once.

## Testing Strategy

Kernel tests use fakes only:

- fake model streams text, thinking, tool calls, usage, hosted activity, and
  typed errors;
- fake tools implement `ToolResolver` separately from `ToolPort`, record calls,
  and can complete out of order;
- fake gate can rewrite name/input, mutate again during a permission request,
  re-resolve resources, deny, or delay a host decision;
- fake context policy injects context and returns every
  `PreparationDecision`/`CompactionRequest`/`CompactionOutcome` variant;
- in-memory transcript store records commits;
- recording usage sink tracks record order and flush barriers;
- recording event port captures an ordered event log.

Required kernel cases:

- simple end-turn run;
- tool-use run with source-order result persistence and completion-order events,
  including one grouped user-role result message with one block per call and
  companion image blocks immediately after image-bearing results;
- parallel reads and conflict serialization;
- gate rewrite is re-resolved and reclassified for sensitive access, risk,
  stable name, MCP auto-approval, prompt policy, and scheduling before execution;
- a benign-to-credential rewrite fails closed, and a low-risk-to-high-risk MCP
  rewrite cannot inherit the original risk or auto-approval;
- denied or blocked tool never executes but produces the failed tool result/event;
- a gate receives only `ToolResolver`; a compile-time object-safety test and a
  recording fake prove that resolution cannot be used to execute a tool before
  the kernel accepts the final `Allow`;
- tool success, ordinary failure, adapter error, and post-tool rewriting preserve
  status and original error;
- failed final status clears `CompleteRun`, `YieldToHost`, and
  `RequestCompaction` while preserving the operational error;
- tool progress reaches `AgentEvent::ToolProgress` in emission order;
- cancellation before streaming, during streaming, after streaming, during
  approval, during a tool, and between waves, including durable pairing,
  idempotent repeated cancellation, one terminal event, no post-terminal event,
  and timeout-bounded channel waits;
- absolute max-turn outcome and per-activation delta accounting;
- turn accounting across context-overflow compaction, transport retry, and
  cancellation before dispatch: only dispatchable main-loop attempts consume
  turns, and the final Goal reminder sees the same budget;
- injected context is persisted but does not become a real user turn, while
  hosted provider activity produces no transcript message and no `ToolCall`;
- loading a durable assistant `ToolUse` without its grouped result synthesizes
  deterministic interrupted results before any resumed model request;
- assistant message and cursor commit before tool execution;
- `last_token_total` follows `TokenUsage::total_tokens` on ordinary and
  compaction calls, matching legacy `usage.total` rather than input-only usage;
- pre-request hook/subagent context is persisted before that model call;
- context-overflow compaction and retry;
- auto, manual, recovery, command, and micro compaction requests; successful
  replacements repeat preparation, skipped decisions use the fallback without
  spinning, and skipped recovery preserves the original overflow error;
- the commit callback runs only after replacement persistence and in-memory
  transcript/cursor update; a failed replacement cannot acknowledge staged
  compaction state or run PostCompact;
- local non-streaming and native compaction calls emit usage/diagnostics;
- usage records are awaited after assistant commit and before tools, compaction
  usage is recorded before its result is consumed, record failure stops the run,
  and run-end flush is a terminal barrier;
- `MaxTokens + ToolUse` executes and pairs pending tools before continuation,
  leaves the next provider request structurally valid, and returns `MaxTokens`
  only when the continuation budget is exhausted;
- a truncated final-turn response cannot newly complete a Goal;
- non-final Goal completion proposals continue while a final-turn proposal
  returns `Completed`;
- hosted provider activity is forwarded in stream order and never creates a
  `ToolCall`.

Tact parity tests cover:

- full `AgentUpdate` sequences for simple, tool-use, permission, progress,
  ask-user, hook, background/subagent, cancellation, retry, compaction, and
  Goal-yield runs;
- the Task 4 normalized legacy baseline is unchanged across the adapter and
  kernel cutover for injected-context authorship, hosted-search activity,
  approval denial, stream/tool cancellation, resumed history, and TUI blank
  background painting;
- every F7 scenario has a kernel-path comparison before deletion:
  `simple_text`, `tool_use_image_companion`, `permission_denial`,
  `transient_retry`, `cancellation_during_stream`,
  `cancellation_during_tool_wave`, `injected_context`, `hosted_web_search`,
  `resumed_sqlite_history`, and `tui_full_area_blank_background`;
- hook callbacks receive only the read-only activation view, queued context is
  committed by the kernel, and attempted transcript-looking writes cannot
  change `AgentState`;
- permission mode changes, always-allow grants, and denial counters use one
  authority per logical agent visible to its facade and gate; subagents start
  from a parent snapshot but own a separate session and authority;
- a nested subagent activation runs through the product-owned child
  composition and reports the kernel path while retaining its own session,
  cancellation flag, and permission authority;
- transport, continuation, and compaction attempts returned in
  `RunOutcome.recovery` are visible through `AgentRuntime.recovery_state` after
  success, cancellation, and max-token outcomes;
- a pre-column SQLite database migrates in place; origin inference is stable
  across repeated loads, explicit origins survive replacement/reopen, and
  kernel-path `call_id` replay upserts one usage row while legacy NULL call ids
  remain readable;
- every production activation stays on the kernel path; the legacy activation
  selectors have been removed;
- mid-turn MCP list changes reaching the next request;
- a full MCP router replacement between activations uses the new shared catalog
  handle without nesting two mutable authorities;
- worktree-isolated subagent parallelism and hierarchical path conflicts;
- `YieldToHost` suppressing later side-effecting waves;
- the final-turn Goal reminder and completion-proposal outcome;
- facade cancel-flag writes reaching the scoped token during streaming,
  approval, bash, other in-flight tools, and between waves;
- facade state and every sidecar field restored on success, cancellation, error,
  and unwind;
- resumed SQLite sessions with an empty runtime context loading before
  `AgentState` construction;
- legacy SQLite sessions with every inferred origin, complete marker frames,
  exact continuation prompts, and genuine user text containing marker-like
  examples;
- direct public-field mutation of `AgentRuntime.context` and `provider_state`;
- sessionless runs through the production no-op transcript adapter;
- a crash-simulated durable orphan tool call receives one grouped interrupted
  result message before the next request, for both local and Responses paths;
- usage rows exist when the facade returns, a record failure prevents tool
  execution, and flush drains the final compaction or continuation record;
- live progress, final tool output, and hook-rewritten output remain redacted,
  including a secret split across stream chunks;
- provider activity keeps one stable id and step ordinal, suppresses duplicate
  terminal events, reports a non-terminal search completion as failure with its
  detail, and preserves duration when available;
- the full adapter source guard covers gate, context, store, usage,
  composition, cancel, event, model, tool, and presentation modules; none emits
  kernel `AgentEvent`s, executes after denial, holds `Agent`, or writes logical
  transcript/cursor and usage state outside the kernel-owned sinks;
- `tact_llm` account DTOs map to `tact_protocol::biz` without restoring the
  dependency edge.

Tact parity tests use the existing integration suite. No test expectation may be
weakened to make the kernel pass.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| The new `tact_agent_core` becomes a renamed monolith | Forbid product/runtime dependencies in its manifest and CI dependency guard |
| Plugin-style abstraction turns into a framework | Use dependency injection and small traits; no registry, installer lifecycle, or `PluginContext` |
| Provider state is accidentally flattened | Keep `ModelCursor` opaque and test atomic commit before tools |
| UI behavior regresses during event mapping | Build a recording bridge, run existing TUI and driver tests at each phase |
| Facade and kernel state diverge | Transfer `AgentState` plus the complete facade sidecar, keep production selection kernel-only, and restore through one guard on every exit |
| Hooks force a second `Agent` or stale permission copy | Share product services, expose only the read-only activation view, and keep all permission mutations in one `PermissionAuthority` |
| Tool behavior changes behind an apparently clean port | Typed execution results, full post-hook reclassification, adapter-side redaction, progress tests, and security-order parity |
| Two loops drift in production | Production has one kernel loop; the legacy helpers, selector types, and temporary bridge are deleted |
| Legacy sessions change turn/compaction meaning | Persist origin for new rows and test explicit inference for old rows |
| Marketplace plugins and kernel composition are confused | Describe kernel capabilities as adapters; reserve "plugin" for the existing product feature |
| Dynamic extension loading creates ABI/security problems | Do not add loading, discovery, or ABI in this design |

## Completed Delivery Record

1. Added contracts and the dependency guard.
2. Added the kernel skeleton and fake-port tests.
3. Moved neutral message/model/tool types and removed provider-to-TUI emission.
4. Added Tact adapters and the event bridge.
5. Routed every production activation through the kernel behind the existing
   `Agent` facade.
6. Exported recovery state and closed the product parity matrices.
7. Deleted the legacy helpers, selector scaffold, temporary model bridge, and
   superseded tool-dispatch/scheduling code.
8. Closed candidate capability-crate extraction as not triggered; no second
   consumer exists.

The detailed task order and gates are in
`docs/superpowers/plans/2026-10-26-agent-kernel.md`.

## Acceptance Criteria

- A model-only run can execute with an in-memory transcript and no SQLite,
  MCP, network, filesystem, or TUI dependency.
- `AgentKernel` retains no conversation state between calls; facade and kernel
  never expose two mutable transcript authorities, and every facade sidecar
  field is restored through one activation guard.
- Hooks receive only the read-only activation view, queued context is committed
  by the kernel, and each logical agent's facade and gate share one permission
  authority.
- A tool-use run preserves source-order results and completion-order events.
- Each assistant tool-use message is followed by exactly one grouped user-role
  result message with one block per call in assistant source order.
- Tool status/error, full hook-rewrite reclassification, adapter-side redaction,
  progress, permission labels, and immediate host-yield behavior survive the
  port.
- `ToolGate::before` can resolve rewrites through `ToolResolver` but cannot
  invoke execution; only the kernel executes the final allowed call.
- Failed tool results cannot complete a Goal, yield the host, or request
  compaction while retaining the original operational error.
- Every model call has one terminal response with complete usage/cursor
  metadata, including local and native compaction calls.
- Authoritative token usage is recorded through an awaited `UsageSink`, flushed
  before return, and never depends on a non-blocking event observer. Kernel
  records upsert by non-empty `call_id`; legacy NULL-call-id rows remain
  readable.
- Auto, manual, recovery, command, and micro compaction have typed decisions;
  full replacements commit bookkeeping and PostCompact only after persistence,
  skipped preparation cannot loop, micro compaction does not suppress the next
  auto check, and native cursor-only replacement is distinguishable from
  unchanged or cleared transcripts.
- Mid-run injected context is persisted before the model call that consumes it.
- A durable interrupted tool call is reconciled into the same grouped result
  message before a resumed model request.
- Dynamic MCP refresh, hierarchical resource conflicts, worktree subagent
  parallelism, and Goal final-turn behavior remain unchanged. MCP refresh shares
  one interior-synchronized catalog authority with full router replacement.
- Main-loop turn accounting counts dispatched request attempts only; compaction
  and transport retries do not consume extra turns, and cancellation before
  dispatch does not consume one.
- `MaxTokens + ToolUse` preserves paired results and launches structurally valid
  continuation requests; truncated final turns cannot newly complete a Goal.
- Direct writes to `AgentRuntime.cancel_flag` reach an activation-scoped token,
  and the facade runs `ensure_session` before constructing `AgentState`.
- Cancellation is idempotent, drains already-running work, preserves committed
  session state, and emits exactly one terminal run event before the activation
  returns.
- The kernel has no provider-specific branch and no `AgentUpdate` reference.
- `tact` can assemble the existing runtime through constructor-injected
  capability adapters without changing user-visible behavior.
- `tact_llm` no longer imports `tact_protocol`; its provider-neutral account
  DTOs are mapped to host protocol types in the product layer.
- The TUI consumes a mapped event stream and does not need kernel internals for
  ordinary run rendering.
- Existing SQLite sessions load with correct message origins and turn
  semantics, including complete marker frames, exact continuation prompts, and
  genuine user text that only resembles a marker. Explicit origins survive
  replacement and reopen, and NULL-origin inference is stable.
- Injected context never becomes a real user turn, and provider-hosted activity
  never becomes a transcript user/tool message or a client tool execution.
- All ten frozen F7 scenarios have kernel-path comparisons, and the legacy loop
  has been deleted.
- Existing Rust checks and the full test suite remain green.
