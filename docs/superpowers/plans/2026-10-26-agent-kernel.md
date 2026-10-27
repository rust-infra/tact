# Agent Kernel Implementation Plan

> **For agentic workers:** Follow the tasks in order. Keep each task independently
> reviewable and do not start the next phase until its gate is green. Use checkbox
> syntax (`- [ ]`) to track implementation.

**Goal:** Extract a provider-neutral `tact_agent_core` kernel and assemble Tact's
existing runtime through plugin-style dependency injection, without changing
user-visible behavior.

**Architecture:** `tact_agent_api` owns neutral contracts and capability traits;
`tact_agent_core` owns `KernelDeps` and one bounded agent activation; `tact`
constructs and injects product adapters and remains the compatibility facade;
`tact_llm` implements the model port; `tact_protocol` remains the host/TUI
protocol.

**Tech stack:** Rust 2024, async-trait/object-safe service traits, Tokio
`sync`/`time` for kernel mechanics, serde for contracts, existing Tact tests and
the current `AgentUpdate` bridge.

**Spec:** [`docs/superpowers/specs/2026-10-26-agent-kernel-design.md`](../specs/2026-10-26-agent-kernel-design.md)

**Current status (2026-10-27):** Tasks 0-7 are complete. Production activation
is kernel-only and enters `run_kernel_activation` directly after
`ensure_session`; the activation selector, legacy control helpers,
`agent/tool_dispatch.rs`, legacy wave scheduling, and `LegacyModelBridge` are
deleted. `agent/tool_schedule.rs` retains only the product `ToolResources`
contract.

Task 4's gate, tool, store, usage, event, composition, prompt, and compaction
adapters are implemented. `TactToolGate` is the only policy path, so sensitive
scanning, hook mutation, reclassification, permission approval, and post-tool
redaction have one owner. `TactContextPolicy` owns lifecycle hooks, queued
context, prompt assembly, and auto/micro decisions; `TactCompactor` handles
local/native replacement and the post-persistence commit callback, and
`/compact` delegates to the same kernel-owned helper.

Task 6 is closed. Durable origin/call-id storage, hook/context migration, prompt
ownership, the full compaction protocol, cursor commits, Goal final-turn
semantics, max-token yield/manual-compaction parity, mid-turn MCP refresh,
mixed-image persistence, resource scopes, Goal Runner ownership, sessionless
composition, direct public-state mutation, resumed TUI bootstrap, usage
visibility, recovery-counter export, nested-subagent evidence,
local/Responses crash recovery, the permission-authority matrix, and the
`AgentUpdate` sequence matrix all have named evidence below. All ten F7
comparisons pass.

Task 8 is closed as not triggered because no concrete second consumer exists.
The implemented boundary stops at `tact_agent_api`, `tact_agent_core`, and
`tact`.

## Global Constraints

- Do not commit or push unless explicitly requested.
- Work on a feature branch. Do not commit directly to `main`.
- Keep the existing public `tact` API and `AgentUpdate` behavior until the
  compatibility gate is green.
- Run one Cargo invocation at a time. Parallel Cargo processes contend on the
  workspace `target/` lock.
- Filter tests first, then widen to the affected crate and workspace checks.
- Keep `tact_agent_api` and `tact_agent_core` free of `sqlx`, `rmcp`, `git2`,
  `reqwest`, `ratatui`, `cpal`, `tera`, and `tact_protocol`.
- Do not put marketplace plugin installation, MCP transport, SQLite, or TUI
  rendering into the kernel.
- A provider adapter must not emit `AgentUpdate`.
- Injected context and Goal context must not become user turns.
- Provider-hosted activity such as web search must never become a user turn or a
  client `ToolCall`; it remains `ProviderActivity` plus the final model response.
- Tool result messages are persisted in assistant source order even when tool
  completion events arrive in a different order.
- One assistant tool-use message is followed by exactly one grouped user-role
  result message, never one message per tool.
- `AgentRuntime` plus `Agent`'s turn/tool counters remain the in-memory state
  authority; `AgentKernel` is stateless across calls and receives
  `&mut AgentState`.
- Hooks, permissions, UI traffic, session metadata, stats, MCP hook services,
  and queued context live in product-owned shared services for each logical
  agent. A subagent may receive explicit parent queue/update handles, but owns
  its own session and permission authority. Hooks receive a read-only
  `LoopState` activation view, never `Agent`, `Arc<Mutex<Agent>>`, or a writable
  transcript.
- The facade, gate, and mode controls for one logical agent share one
  `PermissionAuthority`. The subagent factory creates a child authority from an
  explicitly inherited `PermissionSnapshot`, never a second live parent
  manager.
- The facade calls `ensure_session`, then transfers `AgentState` and the
  complete behavioral sidecar from `&mut Agent`. Restore them through one guard
  on success, cancellation, error, and unwind.
- `AgentRuntime.cancel_flag` remains the only public cancellation authority.
  Mirror it into one activation-scoped `CancelToken`; adapters only observe the
  token.
- Cancellation is monotonic and idempotent for one activation. It drains current
  work, retains committed session state, and emits exactly one terminal event.
- Every model call must produce exactly one terminal `ModelResponse` carrying
  complete message, stop reason, usage, request diagnostics, and cursor update.
- Authoritative usage and request-body persistence goes through one awaited
  `UsageSink`; `AgentEvent::Usage` is display/observer traffic, not the durable
  write barrier.
- `AgentState.total_turns` counts dispatched main-loop request attempts only.
  Compaction calls and transport retries do not consume a turn; a
  context-overflow retry does.
- On load, reconcile a durable assistant tool-use message with missing grouped
  results before any resumed model request.
- A tool result carries typed success/failure and the original error; progress
  and host interactions must remain observable through their existing paths.
- Hook mutations must be re-resolved and fully reclassified before permission,
  scheduling, or execution. Failed results clear terminal/compaction directives.
- `ToolGate::before` receives only `ToolResolver`; the kernel is the sole caller
  of `ToolPort::execute`, and only for the final `Allow` after all rewrites.
- Tool adapters redact live/final output; the gate redacts hook-rewritten visible
  output before returning. One policy covers held-back tails and split secrets.
- Preparation returns `Continue`, `Compact`, or `Block`; the kernel alone runs
  compaction and persists typed replacements.
- One activation has exactly one kernel state owner and control loop. A kernel
  error is never converted into a mid-run legacy fallback.
- Refresh dynamic tool specs, including MCP list changes, before every model
  request.
- MCP refresh and full router replacement share one interior-synchronized
  catalog authority; the active activation holds one cheap handle clone.
- Existing persisted sessions must load with correct message origins and turn
  semantics; a synthetic user-role message is not a real user turn. The SQLite
  migration is additive: nullable `messages.origin`, nullable
  `token_usages.call_id`, and a partial unique `(session_id, call_id)` index.
- Kernel-path usage rows are upserted by non-empty `call_id`; legacy NULL-call-id
  rows remain readable.
- Use typed enums for closed sets such as message origin, stop reason, model
  error kind, gate decision, and run outcome.
- `EventObserver` is the only first-slice extension: read-only, registration
  ordered, and unable to claim, veto, or short-circuit an event. Do not add
  conflict resolution for extension phases that do not exist.
- Treat plugin style as a design principle, not as runtime infrastructure. Do
  not add a `RuntimePlugin` trait, plugin registry, plugin id, installer
  lifecycle, or dynamic loader in this plan.
- The legacy helpers, activation selector, temporary model bridge, and
  superseded dispatch/scheduling code are deleted after all ten F7 scenarios
  and the Task 6 recovery/parity matrix went green.

## Review Focus

- `cargo tree -p tact_agent_api` and `cargo tree -p tact_agent_core` expose no
  product or infrastructure dependency.
- The kernel can run with fakes and no filesystem/network/database.
- `tact_llm` no longer imports `tact_protocol`.
- The existing `tact` integration and TUI tests remain behaviorally unchanged.
- Required capabilities are non-optional fields in `KernelDeps`; the first slice
  defines only the observer extension. Later phases introduce another extension
  only when a migrated behavior has a real caller and no narrower capability
  boundary fits.
- Product capabilities implement contracts directly as adapters; there are no
  `*Plugin` wrappers or service registries.
- Hook compatibility is provided by the read-only activation view and shared
  product services, not by cloning `Agent` or exposing kernel transcript state.
- The gate may hold `Arc<TactSharedServices>` and its narrow hook/permission/
  UI/hook-MCP handles. It has no `ToolPort`, executor, `TranscriptStore`,
  `UsageSink`, or `EventPort`.
- Gate resolution is read-only through `ToolResolver`; compile-time and fake
  port tests prove that approval cannot be bypassed by calling execution.
- Facade state mutations, tool input rewrites/status, progress, model terminal
  events, mid-run context persistence, MCP refresh, Goal final turns, max-token
  tool continuation, cancel bridge, redaction, account DTO mapping, and legacy
  session origins have explicit parity cases.
- Usage durability, interrupted-tool reconciliation, turn accounting, and MCP
  catalog ownership have explicit kernel or facade parity tests.
- Injected context and hosted provider activity preserve their non-user,
  non-client-tool semantics in transcript and `AgentUpdate` golden tests.
- Cancellation races have timeout-bounded tests for idempotence, terminal-event
  cardinality, drained work, and retained session state.

### Original Act On Reconciliation (R1-R11)

The first adversarial review produced eleven Act On findings. They remain
distinct from the later kernel-contract findings F1-F7. `Closed` below means
the plan contains the implementation and evidence named by the row; a complete
contract with unfinished product cutover stays open.

| ID | Finding | Plan location | Delivery status |
|---|---|---|---|
| R1 | Hook rewrites bypassed final security classification. | Task 4 gate work; [ToolGate spec](../specs/2026-10-26-agent-kernel-design.md#toolgate) | Closed. Shared production gate re-resolves and reclassifies both hook stages; adapter and legacy cases are checked in Task 4. |
| R2 | Compaction could not express auto/manual/recovery/native, no-op, or cursor replacement. | Task 4 context ownership; Task 6 compaction cutover | Closed. `TactContextPolicy`/`TactCompactor` cover every typed kind, native cursor-only replacement, veto/skip, kernel-owned persistence, and post-commit bookkeeping. |
| R3 | The activation `CancelToken` was not bridged from `AgentRuntime.cancel_flag`. | Task 4 `cancel_bridge`; Task 5 cancellation gate | Closed. The public-facade matrix covers stream, approval, bash, other in-flight tools, and wave boundaries. |
| R4 | Facade transfer had the wrong receiver and omitted mutable sidecar state. | Task 5 facade-state transfer | Closed. The typed envelope, receipt ranges, sidecar restore, and exhaustive transfer tests are checked. |
| R5 | `initialized = true` could skip SQLite history for resumed TUI sessions. | Task 5 common bootstrap | Closed. `ensure_session` runs before path selection; empty-context resume and direct mutation parity are checked. |
| R6 | `CompleteRun` would change non-final Goal behavior. | Task 6 Goal work | Closed. `TactContextPolicy` adds the final-turn reminder only at `remaining_turns == Some(1)`, and the product matrix proves final completion, non-final continuation, truncated-final refusal, Goal-origin preservation, and host yield after `MaxTokens + ToolUse`. |
| R7 | `MaxTokens + ToolUse` had no defined pending-call ordering. | Task 2 kernel contract; Task 6 product parity | Closed. The kernel pairs and executes pending calls before continuation; product tests cover manual compaction before continuation and `YieldToHost` suppressing a later tool wave before another model call. |
| R8 | Task 3 could not remove `tact_protocol` while account DTOs still lived there. | Task 3 account migration | Closed. Neutral account DTOs, UI mapping, dependency checks, and account tests are checked. |
| R9 | Failed post-tool processing could retain terminal directives. | Task 4 gate/kernel failure handling | Closed. Failed results clear completion, yield, and compaction directives while preserving the original error. |
| R10 | Legacy inference would turn synthetic user rows into real user turns. | Task 6 origin migration | Closed. Deterministic inference and the fresh/pre-migration SQLite round-trip matrix are checked. |
| R11 | Live, final, hook-rewritten, and split-chunk redaction was missing from adapter contracts. | Task 4 adapter tests; Task 6 redaction parity | Closed. `ToolPort` holds back split live chunks and redacts the final result; `ToolGate` redacts hook rewrites and rejects a split secret joined across rewrite stages. |

All eleven findings are closed. The F1-F7 contract table remains separate
evidence and does not substitute for the product test names in this matrix.

### Kernel Contract Findings (F1-F7)

Checked items below mean the design or contract action is complete. A finding
is fully closed only after its named implementation gate is green;
`Kernel contract` and `Adapter parity` are deliberately separate evidence.

- [x] **F1 contract** Keep gate resolution read-only through `ToolResolver`;
  execution remains kernel-only. **Evidence:**
  `gate_rewrite_is_re_resolved_before_execution`,
  `gate_rewrite_uses_the_resolved_access_for_wave_scheduling`,
  `denied_tool_never_executes_but_gets_a_result`, and
  `resolving_a_write_does_not_execute_it` pass.
- [x] **F1 closure** Implement the production `TactToolGate` over the hook,
  permission, popup, reclassification, and post-tool pipeline, then run adapter
  authority and rewrite tests. `pre_tool_rewrite_is_fully_re_resolved_and_reclassified`,
  `permission_request_rewrite_is_rechecked_before_allow`,
  `post_tool_redacts_rewritten_output_and_preserves_the_original_error`,
  `cancellation_wins_while_the_permission_popup_is_pending`, and the adapter
  source guards pass. `original_credential_call_fails_closed_before_any_hook`
  and `post_tool_hooks_cannot_join_a_secret_across_rewrites` cover the removed
  legacy path's security cases through production code. Full user-visible
  cutover parity is covered by the closed F7 gate and the Task 6 sequence
  matrix.
- [x] **F2 contract** Give injected context and hosted provider activity
  explicit non-user, non-client-tool contracts. **Evidence:**
  `injected_context_is_persisted_but_is_not_a_user_turn`,
  `injected_context_and_hosted_activity_keep_separate_non_user_channels`, and
  `provider_activity_is_not_treated_as_a_tool_call` pass.
- [x] **F2 closure** Implement `TactContextPolicy`, the read-only hook/context
  projection, and the production SessionStart/UserPromptSubmit/request-context
  injection path. `successful_session_start_and_user_prompt_context_keep_non_user_origins`,
  `queued_context_is_appended_once_before_the_request`, and
  `injected_context_and_hosted_activity_keep_separate_non_user_channels` pass,
  and `TactKernelComposition` injects the policy into the kernel.
- [x] **F3 contract and public-facade bridge** Define monotonic activation-scoped
  cancellation, current-wave draining, committed-state retention, and one
  terminal event. The `cancellation_*` matrix,
  `repeated_cancellation_has_one_terminal_event_and_bounded_drain`, and
  `public_cancel_flag_keeps_one_terminal_event_and_committed_state` pass.
- [x] **F3 facade wiring** `Agent::run_kernel_activation` creates and drops one
  `CancelBridge` around the real kernel activation and retains committed state.
- [x] **F3 closure** Prove direct `AgentRuntime.cancel_flag` writes through the
  public facade during streaming, approval, bash, other in-flight tools, and
  between waves. **Evidence:**
  `agent_loop_detailed_reports_cancelled_when_the_flag_arrives_mid_response`,
  `cancel_before_tool_execution_pairs_every_tool_use`,
  `cancel_mid_preflight_answers_every_tool_use_once`,
  `cancel_mid_preflight_answers_an_approved_but_unrun_call`,
  `facade_cancel_while_bash_is_in_flight_drains_and_pairs_the_result`,
  `facade_cancel_while_another_tool_is_in_flight_drains_without_another_turn`,
  and `facade_cancel_between_waves_suppresses_later_side_effects_and_pairs_calls`
  pass. They cover the public flag, terminal-event cardinality, drained work,
  suppressed later waves, and durable tool pairing.
- [x] **F4 kernel contract** Make kernel authority explicit: adapters cannot
  write kernel-managed transcript/cursor/usage state or emit kernel events on
  the kernel's behalf. The kernel authority tests pass.
- [x] **F4 closure** Extend
  `capability_adapters_do_not_hold_kernel_state_or_persistence_authority` to
  activation view, gate, context, store, usage, services, permission, hook,
  and composition modules. The companion guards
  `policy_adapters_cannot_execute_tools_or_own_persistence_or_events` and
  `persistence_adapters_cannot_execute_tools_or_emit_kernel_events` pass.
- [x] **F5** Replace direct-only dependency inspection with an all-features
  resolved graph guard and a tested forbidden-transitive negative fixture. The
  guard and fixture are green.
- [x] **F6** Keep `EventObserver` read-only and registration-ordered after
  `EventPort`; it cannot claim, veto, or short-circuit an event. **Evidence:**
  the kernel ordering tests and the Task 4 source matrix are green.
- [x] **F7a** Freeze
  `crates/tact/testdata/agent_kernel/legacy_baseline.json` before adapter
  cutover, using the normalization rules in the spec and including image
  companion blocks plus `usage.total` accounting. The fixture covers all ten
  required scenarios.
- [x] **F7b** Compare every cutover scenario with the frozen baseline before
  deleting the legacy loop. Nine fixture-backed comparators pass:
  `simple_text_kernel_run_matches_frozen_legacy_baseline`,
  `tool_use_image_companion_kernel_run_matches_frozen_legacy_baseline`,
  `permission_denial_kernel_run_matches_frozen_legacy_baseline`,
  `transient_retry_kernel_run_matches_frozen_legacy_baseline`,
  `cancellation_during_stream_matches_frozen_legacy_baseline`,
  `cancellation_during_tool_wave_matches_frozen_legacy_baseline`,
  `injected_context_kernel_run_matches_frozen_legacy_baseline`,
  `hosted_web_search_kernel_run_matches_frozen_legacy_baseline`, and
  `resumed_sqlite_history_kernel_run_matches_frozen_legacy_baseline`.
  `kernel_updates_leave_no_unowned_blank_tui_cells` exercises the tenth
  scenario on the real facade kernel path and verifies that every blank cell
  belongs to an intentional UI surface. The compact regression
  `responses_compact_surfaces_usage_persistence_failure` runs on the direct
  kernel model path: the neutral boundary preserves the serialized
  `MessageContent::Text` representation required by the Responses context hash.
  This closes F7; Task 6 closure and Task 7 deletion are recorded below.

**Final verification (2026-10-27, run serially):**

- `scripts/check-agent-kernel-deps.sh tact_agent_api tact_agent_core tact_llm`
  passed, including its forbidden-transitive fixture.
- `cargo test -p tact_agent_api -p tact_agent_core --quiet` passed 15 API tests
  and 64 kernel tests.
- `cargo test -p tact` passed 1494 tests with 1 ignored; the live MCP target
  passed 1 test and ignored 4 network-required tests.
- `scripts/check-rust.sh` passed with the repository proxy and `no_proxy`
  settings: dependency boundaries, `cargo fmt -- --check`,
  `cargo clippy --all-targets -- -D warnings`, and the `tact-ui`, `tui`,
  `tact`, and `tact_llm` suites were green. `tui` passed 750 tests;
  `tact_llm` passed 293 tests with 17 ignored.
- `cargo tree -p tact_agent_api`, `cargo tree -p tact_agent_core`, and
  `cargo tree -p tact_llm` showed no forbidden dependency edge.
- The kernel baseline comparators, `responses_compact_surfaces_usage_persistence_failure`,
  and `kernel_updates_leave_no_unowned_blank_tui_cells` ran within those green
  suites after the neutral `MessageContent` shape and cancellation bridge fixes.

---

## File Map

### New

- `crates/agent_api/Cargo.toml`
- `crates/agent_api/src/lib.rs`
- `crates/agent_api/src/message.rs`
- `crates/agent_api/src/state.rs`
- `crates/agent_api/src/model.rs`
- `crates/agent_api/src/context.rs`
- `crates/agent_api/src/tool.rs`
- `crates/agent_api/src/event.rs`
- `crates/agent_api/src/usage.rs`
- `crates/agent_api/src/store.rs`
- `crates/agent_api/src/extension.rs`
- `crates/agent_api/src/test_support.rs`
- `crates/agent_core/Cargo.toml`
- `crates/agent_core/src/lib.rs`
- `crates/agent_core/src/kernel.rs`
- `crates/agent_core/src/state.rs`
- `crates/agent_core/src/scheduling.rs`
- `crates/agent_core/src/recovery.rs`
- `crates/agent_core/src/model_scope.rs`
- `crates/agent_core/src/deps.rs`
- `crates/agent_core/src/test_support/`
- `crates/tact/src/runtime/composition.rs`
- `crates/tact/src/runtime/mod.rs`
- `crates/tact/src/runtime/model_adapter.rs`
- `crates/tact/src/runtime/tool_adapter.rs`
- `crates/tact/src/runtime/gate_adapter.rs`
- `crates/tact/src/runtime/context_adapter.rs`
- `crates/tact/src/runtime/compaction_adapter.rs`
- `crates/tact/src/runtime/prompt_adapter.rs`
- `crates/tact/src/runtime/store_adapter.rs`
- `crates/tact/src/runtime/services.rs`
- `crates/tact/src/runtime/activation_view.rs`
- `crates/tact/src/runtime/permission_authority.rs`
- `crates/tact/src/runtime/hook_adapter.rs`
- `crates/tact/src/runtime/noop_transcript.rs`
- `crates/tact/src/runtime/event_adapter.rs`
- `crates/tact/src/runtime/tool_presentation.rs`
- `crates/tact/src/runtime/facade_state.rs`
- `crates/tact/src/runtime/cancel_bridge.rs`
- `crates/tact/src/runtime/usage_adapter.rs`
- `crates/tact/src/runtime/baseline.rs`
- `crates/tact/src/mcp/catalog.rs`
- `crates/tact/testdata/agent_kernel/legacy_baseline.json`
- `scripts/check-agent-kernel-deps.sh`
- `scripts/check-agent-kernel-deps.py`
- `docs/superpowers/specs/2026-10-26-agent-kernel-design.md` (this design)

### Modified

- `Cargo.toml`
- `.github/workflows/rust.yml`
- `crates/tact/Cargo.toml`
- `crates/tact/src/agent/mod.rs`
- `crates/tact/src/agent/tool_schedule.rs`
- `crates/tact/src/agent/outcome.rs`
- `crates/tact/src/hook/mod.rs`
- `crates/tact/src/permission/mod.rs`
- `crates/tact/src/plugin/hooks.rs`
- `crates/tact/src/mcp/mod.rs`
- `crates/tact/src/lib.rs`
- `crates/tact/src/store/session_store/mod.rs`
- `crates/tact/src/store/session_store/sqlite.rs`
- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/src/client.rs`
- `crates/tact_llm/src/content.rs`
- `crates/tact_llm/src/types.rs`
- `crates/tact_llm/src/provider_state.rs`
- `crates/tact_llm/src/account.rs`
- `crates/tact_llm/src/anthropic/`, `openai/`, `mock.rs` as required by the
  adapter migration
- `crates/protocol/src/agent.rs` only for compatibility re-exports or event
  mapping; no kernel types are moved into protocol
- `crates/tact-ui/src/session_bootstrap.rs`
- `crates/tact-ui/src/account.rs`
- `crates/tact-ui/src/goal_runner.rs`
- `ARCHITECTURE.md`
- Tests colocated with the files above

### Deleted

- `crates/tact/src/agent/tool_dispatch.rs`
- `crates/tact/src/runtime/activation.rs`
- `crates/tact/src/runtime/legacy_model_bridge.rs`

### Later

- `crates/tact_store/` or `crates/tact_capabilities/` only if the capability
  extraction has a concrete second consumer. Do not create these in Phase 1.

---

## Task 0: Baseline and Invariant Freeze

**Files:**

- Read: `crates/tact/src/agent/mod.rs`
- Historical read (deleted in Task 7): `crates/tact/src/agent/tool_dispatch.rs`
- Read: `crates/tact/src/agent/tool_schedule.rs`
- Read: `crates/tact/src/tool/bash.rs`
- Read: `crates/tact/src/hook/mod.rs`
- Read: `crates/tact/src/mcp/mod.rs`
- Read: `crates/tact/src/store/session_store/mod.rs`
- Read: `crates/tact/src/store/session_store/sqlite.rs`
- Read: `crates/tact_llm/src/client.rs`
- Read: `crates/protocol/src/agent.rs`
- Read: `crates/tact-ui/src/goal_runner.rs`
- Read: `AGENTS.md`

**Work:**

- [x] Record the current behavioral invariants in the new design doc:
  cancellation points, max-turn behavior, tool result ordering, permission
  preflight order, mutable PreToolUse input, provider cursor commit order,
  injected-context handling, Goal final turns, in-flight tool cancellation,
  usage write ordering, and interrupted-tool reconciliation.
- [x] Build an `AgentUpdate` source matrix: kernel-bridged variants versus
  adapter-owned host traffic (`RequestSelect`, hooks, tasks, goals, background,
  subagents, and transcripts).
- [x] Record the existing cancellation/abandoned-tool pairing matrix for
  cancellation, refusal, max-token exhaustion, truncation, and unknown stop
  reasons.
- [x] Add a dependency allowlist check for `tact_agent_api` and
  `tact_agent_core` before those crates contain production code.
- [x] Upgrade the guard from direct manifest names to direct and transitive
  dependency edges using the resolved graph or `cargo tree`; add a synthetic
  forbidden-transitive fixture so the parser itself is tested.
- [x] Run the baseline focused tests serially:
  `cargo check -p tact`, `cargo test -p tact --lib agent::`, then
  `cargo test -p tact_llm --lib`.
- [x] Record the exact commands and test counts in the implementation notes.

**Gate:** Baseline commands pass; no production file changed.

---

## Task 1: Add `tact_agent_api`

**Files:**

- Create: `crates/agent_api/Cargo.toml`
- Create: `crates/agent_api/src/lib.rs`
- Create: `crates/agent_api/src/message.rs`
- Create: `crates/agent_api/src/state.rs`
- Create: `crates/agent_api/src/model.rs`
- Create: `crates/agent_api/src/tool.rs`
- Create: `crates/agent_api/src/event.rs`
- Create: `crates/agent_api/src/usage.rs`
- Create: `crates/agent_api/src/extension.rs`
- Create: `crates/agent_api/src/test_support.rs`
- Modify: root `Cargo.toml`
- Test: colocated tests in each module

**Interfaces:**

- `AgentRole`, `AgentMessage`, `MessageOrigin`, `AgentContentBlock`,
  `AgentImageSource`, and `MessageId`; origins include `User`, `Assistant`,
  `ToolResult`, `InjectedContext`, `CompactionSummary`, `GoalContext`,
  `SubagentResult`, and `Continuation`.
- `AgentState { transcript, cursor, total_turns, last_token_total, initialized }`,
  `RunInput`, `RunOutcome`, `RunStop`, `HostYieldReason`, `RunPhase`, `RunId`,
  `CancelToken`, and `RuntimeErrorInfo`.
- `ModelPort::stream/complete/compact`, `ModelRequest`, `ModelEvent`,
  `ModelStream`, `ModelResponse`, `StopReason`, `ModelCallKind`, `ModelCursor`,
  `CursorUpdate`, `ModelCapabilities`, `ModelError`, `ModelErrorKind`,
  `MessageDelta`, `ThinkingDelta`, `ProviderActivity`,
  `ProviderActivityKind`, and `ProviderActivityPhase`.
- `ToolResolver::resolve`, `ToolPort: ToolResolver` with
  `refresh/execute`, `ToolSnapshot`, `ToolSpec`, `ToolCall`,
  `ToolDescriptor`, `ResolvedToolCall`, `ResourceKey`, `ResourceScope`,
  `ResourceAccess`, `ToolOutcome`, `ToolDirective`, `ToolExecutionContext`,
  `ToolProgressSink`, `ToolExecutionStatus`, `ToolExecutionResult`,
  `ToolExecutionError`, `ToolExecutionErrorKind`, `ToolOutputChunk`, and
  `ToolOutputStream`.
- `ToolExecutionContext.remaining_turns` uses inclusive current-turn semantics:
  `Some(1)` means the current request/turn is the final permitted call.
- `ToolGate`, `GateDecision`, `ToolGateContext`, and `ToolGateError`.
  `ToolGateContext` carries session, cumulative turn, remaining turns, and
  opaque metadata; it carries no transcript or mutable activation state.
- `ContextPolicy`, `ContextView`, `PreparationDecision<T>`,
  `ContextPreparation`, `RequestPlan`, `PreparedRequest`, `CompactionRequest`,
  `CompactionKind`, `CompactionOutcome`, `ContextReplacement`, and
  `ContextError`.
- `TranscriptStore::replace(session, transcript: Option<&[AgentMessage]>,
  cursor: CursorUpdate)`, `TranscriptSnapshot`, `MessageReceipt`,
  `ReplaceReceipt`, `MessageRange`, `SessionRef`, and `StoreError`.
- `UsageSink::record/flush`, `UsageRecord`, and `UsageError`.
- `EventPort`, `EventObserver`, `AgentEvent`, and `AgentNotice`. The first slice
  does not define `RunHook` or `ContextContributor`.

**Work:**

- [x] Define the API with no product imports. Use `serde` for persistence/event
  contracts and `serde(skip)` only where the existing transcript semantics
  require it.
- [x] Make required service traits object-safe. Use `async_trait` where a trait
  must be stored as `Arc<dyn Trait>`.
- [x] Split gate-time resolution into `ToolResolver` and make
  `ToolGate::before` accept only `&dyn ToolResolver`; `ToolPort` remains the
  kernel-owned supertrait that adds refresh and execute. Update fakes and the
  object-safety coverage before Task 4 adapter work.
- [x] Keep the API dependency allowlist to `async-trait`, `serde`, `serde_json`,
  `thiserror`, `futures-core`, and `tokio-util` with only its `rt` feature.
- [x] Define `CancelToken` as a cloneable wrapper over
  `tokio_util::sync::CancellationToken`, expose cancellation observation, and
  keep token creation/cancellation ownership at the host boundary.
- [x] Define `ModelStream` as an owned, `Send + 'static` boxed stream and add a
  compile-time object-safety test for every required capability.
- [x] Encode the model contract so a stream has zero or more progress events and
  exactly one terminal `Finished(ModelResponse)`; any error item is terminal.
- [x] Include message identity, message, stop reason, authoritative usage,
  request-body diagnostics, and cursor update in `ModelResponse`.
- [x] Define the complete neutral content/delta/activity/progress contracts:
  the `AgentContentBlock` variants, text and thinking deltas with synthesized
  thinking boundaries, provider-activity identity/order/phase/detail/duration,
  and `ToolOutputChunk { stream, text }` with `Stdout`/`Stderr`/`Other`.
- [x] Own `ToolOutputChunk` in `tact_agent_api`; `tact_protocol` keeps its
  host-facing type and converts at the event bridge. The API crate must not
  depend on protocol.
- [x] Define the asynchronous `UsageSink` contract and the idempotent
  `UsageRecord { call_id, kind, usage, request_body, message_range }`.
  `record` must accept a durable write without loss and `flush` is a run-end
  acknowledgement barrier.
- [x] Specify that one assistant tool-use message is followed by one grouped
  user-role `ToolResult` message with blocks in assistant source order, including
  mixed real and synthetic results.
- [x] Define `CursorUpdate::{Unchanged, Replace}`, including an explicit
  replacement with no cursor.
- [x] Define typed preparation decisions with a fallback for skipped
  compaction, and typed replacement/skip outcomes. `Some(&[])` clears a stored
  transcript, `None` retains it for native cursor-only compaction, and
  `CursorUpdate` distinguishes unchanged state.
- [x] Keep `EventObserver` as the only first-slice extension trait and make it
  independently object-safe. Do not add a universal plugin trait, registry, id,
  installer, or dynamic loader.
- [x] Model tool progress, typed success/failure, original errors, and
  pre-tool input rewrites explicitly. Do not reduce tool execution to a string
  outcome.
- [x] Add inclusive `remaining_turns` to tool execution contexts so Goal
  completion adapters can distinguish the final permitted turn without the
  kernel matching a Goal tool name.
- [x] Model resource conflicts with `ResourceScope { key,
  covers_descendants }` so namespaces, components, and ancestor/descendant path
  conflicts remain distinct.
- [x] Keep `ToolPresentationInfo`, popup kinds, and display labels out of the
  API crate.
- [x] Define an in-memory `TranscriptStore` under a `test-support` feature; it
  must not be compiled into the production default.
- [x] Add compile-time tests that the public contracts are usable as dynamic
  trait objects.
- [x] Add serde tests for `ModelCursor`, `CursorUpdate`, every `MessageOrigin`,
  message deltas, provider activity, tool-output chunks, the
  preparation/compaction types, and the persisted transcript boundary.

**Gate:** `cargo test -p tact_agent_api --lib` and
`scripts/check-agent-kernel-deps.sh tact_agent_api` are green with only the
allowed dependencies.

---

## Task 2: Build the Kernel Skeleton with Fakes

**Files:**

- Create: `crates/agent_core/Cargo.toml`
- Create: `crates/agent_core/src/lib.rs`
- Create: `crates/agent_core/src/kernel.rs`
- Create: `crates/agent_core/src/state.rs`
- Create: `crates/agent_core/src/scheduling.rs`
- Create: `crates/agent_core/src/recovery.rs`
- Create: `crates/agent_core/src/model_scope.rs`
- Create: `crates/agent_core/src/deps.rs`
- Create: `crates/agent_core/src/test_support/`
- Modify: root `Cargo.toml`
- Test: colocated unit tests and fake-port integration tests

**Interfaces:**

- `AgentKernel::new(KernelDeps) -> Self`
- `AgentKernel::with_extensions(KernelDeps, KernelExtensions) -> Self`
- `AgentKernel::run(&self, state: &mut AgentState, input: RunInput, cancel:
  CancelToken) -> Result<RunOutcome, KernelError>`
- `AgentKernel::compact(&self, state: &mut AgentState, session: &SessionRef,
  request: CompactionRequest, cancel: CancelToken) ->
  Result<CompactionOutcome, KernelError>` is the host-command path and shares the
  run loop's observed execution and atomic persistence.
- `AgentState { transcript, cursor, total_turns, last_token_total, initialized }`
- `KernelDeps { model, tools, gate, context, transcript, usage, events }`
- `KernelExtensions { observers }`; hook and contributor vectors are deferred
  until their migration phase has concrete callers.
- Internal `ObservedModelPort` wraps every ordinary, non-streaming, local
  compaction, and native compaction model call.

**Work:**

- [x] Implement the bounded run state machine without persistence, provider,
  UI, or OS dependencies.
- [x] Load transcript and cursor only when `state.initialized` is false. When
  true, use the caller-owned transcript as the authority and never reload behind
  the host.
- [x] After load, reconcile every unanswered assistant `ToolUse` into one
  grouped interrupted-result message before `prepare_run` or applying new
  inputs. Persist through `replace` with an unchanged cursor and make healthy
  transcripts a no-op.
- [x] Implement absolute turn-budget checks against `state.total_turns`,
  per-activation `RunOutcome.turns`, and the prospective
  `ContextView.remaining_turns`. Increment only when a prepared main-loop
  request passes the pre-dispatch cancellation check and is about to call the
  model. Do not increment for compaction calls or transport retries.
- [x] Run `prepare_run`, persist transformed inputs and appended messages with
  their origins, then run `prepare_request`, persist its appended messages, and
  refresh tools before building each request. Handle `Continue`, `Compact`, and
  `Block` explicitly.
- [x] Implement one compaction protocol for auto, manual, recovery, command, and
  micro requests: call the observed policy, persist `Replaced` atomically,
  emit `ContextCompacted`, and repeat preparation. Use `if_skipped` for a
  skipped decision so the same preparation cannot loop; return the original
  overflow error when recovery compaction is skipped.
- [x] Implement observed model calls and stream aggregation for text, thinking,
  provider activity, usage progress, and tool calls. Require exactly one terminal
  `Finished(ModelResponse)` and reject any event after it.
- [x] Commit the assistant message and cursor atomically before dispatching any
  tool from that response. Await `UsageSink::record` with the committed message
  range before emitting `Usage` or executing a tool.
- [x] Record compaction usage before consuming the compaction result, and await
  `UsageSink::flush` before any terminal `RunFinished` or `RunFailed`; a record
  or flush failure becomes `RunFailed`.
- [x] Move the pure conflict/wave logic from `tool_schedule.rs` behind opaque
  `ResourceScope` values, including namespace, component, descendant coverage,
  barrier, and independent-worktree semantics.
- [x] Preflight raw `ToolCall`s sequentially through `ToolGate::before`; schedule
  and execute only the final re-resolved `ResolvedToolCall` returned by `Allow`.
- [x] Execute parallel waves, forward ordered progress through the provided
  sink, preserve completion-order events, apply `ToolGate::after`, and assemble
  persisted results into one grouped user-role message in assistant source
  order.
- [x] Preserve typed tool success/failure and the original error through
  post-tool processing.
- [x] Evaluate directives inside the wave loop. `YieldToHost` and `CompleteRun`
  prevent later waves; tools already running in the current wave finish.
- [x] Clear `CompleteRun`, `YieldToHost`, and `RequestCompaction` when the
  final tool status is `Failed`, including failure introduced by
  `ToolGate::after`, while retaining the original operational error.
- [x] Implement cooperative cancellation at every defined boundary, including
  post-response switching, gate approval, in-flight tools, and between waves.
  Continue waiting for the current wave and synthesize paired results for every
  unexecuted pending call.
- [x] Implement typed model error recovery, compaction retry, transient retry,
  max-token continuation, `RunOutcome`, and exactly one terminal run event.
- [x] Preserve `MaxTokens + ToolUse`: execute pending tools while continuation
  budget remains, persist paired results, process yield/manual compaction,
  append the continuation, and verify the next request shape. When budget is
  exhausted, synthesize truncated results and return `MaxTokens`.
- [x] Keep required capabilities as non-optional `KernelDeps` fields.
- [x] Send each event to `EventPort` first and then invoke observers in vector
  order; never use a `HashMap` for extension iteration.
- [x] Do not add a plugin registry, installer, ids, dynamic loading, `Any`, or
  downcasts.
- [x] Add fake model, fake tool, fake gate, fake context policy, in-memory
  transcript, recording usage sink, and recording event port.

**Required tests:**

- [x] End-turn text stream completes with no tool execution.
- [x] A tool-use stream executes tools and appends results in source order.
- [x] Two tools complete out of order but their persisted results preserve
  source order in one grouped user-role message with one block per call.
- [x] Read-only calls share a wave; writes conflict with overlapping reads and
  writes; ancestor/descendant scopes conflict without merging `/src` and
  `/src2`; barriers run alone; independent worktree scopes can run in parallel.
- [x] A gate rewrite is re-resolved and reclassified for sensitive access,
  risk, stable name, MCP auto-approval, prompt policy, and scheduling; a
  benign-to-credential rewrite fails closed and a low-risk-to-high-risk MCP
  rewrite does not inherit the original approval.
- [x] Mutation during `PermissionRequest` follows the same final
  reclassification path before execution.
- [x] A denied or blocked call never reaches `ToolPort::execute` but still
  produces the expected failed result/event.
- [x] Tool success, ordinary failure, adapter error, and post-tool rewriting
  preserve status and original error.
- [x] A final failed status clears `CompleteRun`, `YieldToHost`, and
  `RequestCompaction`, with one test per directive, while preserving the
  operational error.
- [x] Tool progress reaches `AgentEvent::ToolProgress` in emission order.
- [x] Thinking deltas arrive as `Started`/`Delta`/`Finished` with one stable
  message id, and text deltas never become authoritative tool calls.
- [x] Cancellation before streaming and during a stream that ends without a
  terminal response returns `RunStop::Cancelled`, retries nothing, and commits no
  partial assistant message.
- [x] Cancellation requested during a stream that still completes commits the
  assistant/cursor and returns `RunStop::Cancelled`; cancellation after a
  complete tool-use response also persists paired results for pending calls.
- [x] Cancellation during gate approval, during an in-flight tool, and between
  waves returns `RunStop::Cancelled`, waits for the current wave, and pairs every
  persisted assistant tool call.
- [x] Cancellation repeated multiple times is idempotent, every waiting test has
  a bounded timeout, exactly one terminal event is emitted, no kernel event
  follows it, and committed transcript/cursor state is retained.
- [x] `max_total_turns` returns `RunStop::MaxTurns` at the absolute cap, while
  `RunOutcome.turns` reports only the current activation's dispatched main-loop
  attempts.
- [x] Turn accounting counts dispatched main-loop attempts: cancellation before
  dispatch consumes none, transport retries consume one attempt total, an
  overflow retry is a second dispatched attempt, and compaction itself consumes
  none.
- [x] A loaded durable assistant tool-use message with no result is reconciled
  into one grouped interrupted-result message before any model request; a
  healthy transcript is unchanged.
- [x] Injected `prepare_run` and `prepare_request` context is persisted before
  the request that consumes it and does not become a real user turn.
- [x] A transcript commit failure emits `RunFailed` and never calls
  `ToolPort::execute`; assistant/cursor commit still precedes tools on success.
- [x] Usage record failure prevents tool execution, compaction records usage
  before its result is consumed, and run-end flush failure produces
  `RunFailed`.
- [x] A stream with no terminal response, two terminal responses, or an event
  after the terminal response fails the run.
- [x] A terminal message id that conflicts with `MessageStarted` fails the run.
- [x] Local non-streaming and native compaction calls emit observed
  usage/diagnostics, and context overflow invokes compaction and retries.
- [x] Auto, manual, recovery, command, and micro requests each exercise the
  typed compaction protocol; successful replacement repeats preparation.
- [x] Skipped compaction uses the precomputed fallback without rerunning the
  same preparation, and skipped recovery returns the original overflow error.
- [x] Native cursor-only replacement retains the transcript; explicit clear and
  unchanged replacement remain distinguishable through `TranscriptStore`.
- [x] `MaxTokens + ToolUse` executes pending tools, persists paired results,
  appends the continuation, and constructs a structurally valid next request.
- [x] Exhausted max-token continuation synthesizes truncated results and returns
  `MaxTokens`; a truncated final-turn response cannot newly complete a Goal.
- [x] A Goal completion directive on the final permitted turn returns
  `Completed`, while the same directive with turns remaining continues.
- [x] `YieldToHost` suppresses later waves but lets the current wave finish;
  `CompleteRun` returns `RunStop::Completed`.
- [x] Conflicting directives in one wave use fixed precedence independent of
  completion timing: cancellation, `CompleteRun`, `YieldToHost`, then
  `RequestCompaction`.
- [x] Hosted provider activity never appears as a `ToolCall`.
- [x] Injected context and hosted provider activity are tested together:
  injected context retains a non-`User` origin, hosted search emits activity
  without a transcript message or client tool call, and neither replaces
  `RunInput.inputs`.
- [x] `EventPort` receives each event before observers, observers run in
  deterministic registration order, and both receive exactly one terminal
  `RunFinished` or `RunFailed` event.

**Gate:** `cargo test -p tact_agent_core --lib` and
`scripts/check-agent-kernel-deps.sh tact_agent_core` pass; the kernel manifest
has no forbidden dependency.

---

## Task 3: Make the Provider Layer UI-Neutral

**Files:**

- Modify: `crates/tact_llm/Cargo.toml`
- Modify: `crates/tact_llm/src/client.rs`
- Modify: `crates/tact_llm/src/content.rs`
- Modify: `crates/tact_llm/src/types.rs`
- Modify: `crates/tact_llm/src/provider_state.rs`
- Modify: `crates/tact_llm/src/anthropic/`
- Modify: `crates/tact_llm/src/openai/`
- Modify: `crates/tact_llm/src/mock.rs`
- Modify: `crates/tact_llm/src/account.rs` (required provider-neutral account
  DTO migration)
- Modify: `crates/tact/Cargo.toml`
- Modify: `crates/tact/src/agent/mod.rs`
- Historical create (deleted in Task 7):
  `crates/tact/src/runtime/legacy_model_bridge.rs`
- Modify: `crates/protocol/src/lib.rs` only for compatibility re-exports
- Test: provider unit tests and adapter parity tests

**Interfaces:**

- `tact_llm` implements `tact_agent_api::ModelPort`.
- Provider code emits `ModelEvent`, not `AgentUpdate`.
- Account balance, balance entries, usage quota, and usage windows are
  provider-neutral DTOs under `tact_llm`; `tact-ui` maps them to
  `tact_protocol::biz` for host updates.
- Provider conversation state is wrapped as an opaque `ModelCursor`.
- Hosted web search is represented as `ProviderActivity` with a stable id,
  response-local step ordinal, started/completed/failed phase, query, failure
  detail, and optional duration.
- `TactModelPort` now feeds the kernel directly. The short-lived
  `LegacyModelBridge` used only while unreachable helpers remained was deleted
  with those helpers in Task 7.
- Existing `tact_llm::Message`/`Tool`/`TokenUsageInfo` paths remain available
  through aliases to neutral types or thin conversions while call sites migrate;
  they do not reintroduce the protocol dependency.

**Work:**

- [x] Define provider-neutral balance/quota DTOs in `tact_llm::account` and
  switch `query_deepseek_balance`, `query_kimi_balance`, and
  `query_kimi_code_usage` to return them.
- [x] Update `crates/tact-ui/src/account.rs` to map those DTOs into
  `tact_protocol::biz::{BalanceInfo, UsageQuotaInfo}` and adapt its tests.
- [x] Prove `cargo check -p tact_llm` is green with the neutral account types
  before removing `tact_protocol` from `tact_llm/Cargo.toml`.
- [x] Remove `tact_protocol` from `tact_llm/Cargo.toml` only after all other
  provider-side protocol imports are gone.
- [x] Replace `UpdateSink` arguments and direct `AgentUpdate` construction with
  a `ModelStream` return path.
- [x] Move provider-independent message/content/request types to
  `tact_agent_api`, or add compatibility aliases until the move is complete.
- [x] Port OpenAI Responses streaming, OpenAI-compatible streaming, Anthropic
  streaming, and mock streaming so each emits progress events followed by
  exactly one terminal `Finished(ModelResponse)`.
- [x] Port non-streaming `complete` and provider-native `compact`, returning the
  same complete response metadata; unsupported native compaction must remain a
  typed `Unsupported` error.
- [x] Preserve request-body diagnostics and provider state through neutral
  response metadata.
- [x] Preserve the `TokenUsageInfo` call-site shape through a neutral alias or
  conversion, while `tact_protocol` keeps its host-facing type and mapping.
- [x] Preserve web-search query, status, error detail, and completion behavior
  through `ProviderActivity`; assign step ordinals in first-seen order, suppress
  duplicate terminal events, and map a non-terminal item completion to `Failed`.
- [x] Bridge the temporary legacy helper period, then remove that bridge after
  the kernel-only cutover. `TactEventBridge` remains the permanent translation
  from kernel events to the host `AgentUpdate` stream.
- [x] Remove the old provider-side TUI mapping code from `tact_llm`; the
  product layer owns all host-protocol mapping.
- [x] Update provider tests to assert model events, terminal cardinality, usage,
  stop reason, request body, and cursor updates rather than TUI updates.
- [x] Add provider tests for non-streaming `complete` and native `compact`
  response metadata and unsupported-provider behavior.
- [x] Add provider-activity tests for stable id/step pairs, synthesized started
  events, duplicate terminal suppression, failure detail, and duration.
- [x] Add account-DTO tests for DeepSeek balance, Kimi balance, and Kimi Code
  quota parsing, plus a product-layer mapping test for `AccountUpdate`.
- [x] Keep the permanent event-bridge tests for text, thinking, web search,
  tool calls, usage ordering, notices, and provider-state replacement/removal;
  duplicate legacy-bridge tests were removed with the bridge.

**Gate:** `cargo tree -p tact_llm` contains no `tact_protocol`;
`cargo test -p tact_llm --lib`,
`cargo test -p tact-ui --lib account::`,
`cargo test -p tact --lib runtime::event_adapter::`,
`cargo check -p tact`, and `scripts/check-agent-kernel-deps.sh tact_llm` pass.

---

## Task 4: Add Tact Capability Adapters and the Event Bridge

**Files:**

- Modify: `crates/tact/Cargo.toml`
- Modify: `crates/tact/src/lib.rs`
- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact/src/hook/mod.rs`
- Modify: `crates/tact/src/permission/mod.rs`
- Modify: `crates/tact/src/plugin/hooks.rs`
- Modify: `crates/tact/src/mcp/mod.rs`
- Modify: `crates/tact/src/runtime/mod.rs`
- Modify: `crates/tact/src/store/session_store/mod.rs`
- Modify: `crates/tact/src/store/session_store/sqlite.rs`
- Modify: `crates/agent_api/src/tool.rs`
- Modify: `crates/agent_core/src/kernel.rs`
- Modify: `crates/agent_core/src/test_support/mod.rs`
- Create: `crates/tact/src/runtime/composition.rs`
- Create: `crates/tact/src/runtime/model_adapter.rs`
- Create: `crates/tact/src/runtime/tool_adapter.rs`
- Create: `crates/tact/src/runtime/gate_adapter.rs`
- Create: `crates/tact/src/runtime/context_adapter.rs`
- Create: `crates/tact/src/runtime/store_adapter.rs`
- Create: `crates/tact/src/runtime/services.rs`
- Create: `crates/tact/src/runtime/activation_view.rs`
- Create: `crates/tact/src/runtime/permission_authority.rs`
- Create: `crates/tact/src/runtime/hook_adapter.rs`
- Create: `crates/tact/src/runtime/noop_transcript.rs`
- Create: `crates/tact/src/runtime/event_adapter.rs`
- Create: `crates/tact/src/runtime/tool_presentation.rs`
- Create: `crates/tact/src/runtime/cancel_bridge.rs`
- Create: `crates/tact/src/runtime/usage_adapter.rs`
- Create: `crates/tact/src/mcp/catalog.rs`
- Create: `crates/tact/testdata/agent_kernel/legacy_baseline.json`
- Test: adapter unit tests and bridge tests

**Interfaces:**

- `TactModelPort` wraps the active `tact_llm` provider.
- `TactToolPort` refreshes and aggregates native/MCP tools, returns a generationed
  `ToolSnapshot`, resolves raw calls, and computes `ResourceScope` values.
- `TactToolPort` implements `ToolPort: ToolResolver`, so adapters can share
  resolution without exposing execution to `TactToolGate`.
- `TactSharedServices` owns the per-logical-agent `UpdateSink`, stats, pending
  hook/subagent queues, hook status ids, `HookMcpHandle`, and
  `McpCatalogHandle` view needed by adapters. It contains no transcript,
  cursor, session store, usage sink, or kernel event port. Session/model
  metadata belongs to the activation view.
- `PermissionAuthority` is one interior-synchronized owner of the existing
  `PermissionManager` for one logical agent. It exposes short synchronous
  reads/mutations for mode, rules, denial state, checks, grants, and snapshots;
  never hold its lock across an awaited popup. A subagent starts from an
  inherited snapshot and owns a separate authority.
- `TactActivationView`/`LoopState` is the read-only hook projection over
  `TactSharedServices`, session/model metadata, cumulative turn counts, and
  borrowed transcript data. It replaces the `pub type LoopState = Agent` alias
  with `pub type LoopState<'a> = TactActivationView<'a>` and cannot mutate
  transcript/cursor state.
- `TactToolGate` owns the sensitive-path guard, pre/post hooks, shared permission
  authority, approval prompts, and decision labels. It accepts a raw `ToolCall`,
  re-resolves mutations from both hook events, reclassifies every policy input,
  and returns the final allowed call. It may hold `Arc<TactSharedServices>` and
  hook/permission/UI/queue/hook-MCP handles; it never holds `ToolPort`,
  `TranscriptStore`, `UsageSink`, or `EventPort`.
- `TactContextPolicy` owns prompt assembly, skills, memory, AGENTS.md,
  micro-compaction, and native/local compaction, including pre-request appended
  context. It drains hook/subagent queues into returned messages and never
  writes the transcript.
- `SqliteTranscriptStore` implements the transcript barrier over the existing
  `SessionStore` and provider-state transactions, including nullable explicit
  origins and origin-aware load. Legacy inference/round-trip parity lands in
  Task 6.
- `SqliteUsageSink` persists idempotent `UsageRecord`s and implements the
  run-end flush barrier through nullable `call_id` plus a partial unique
  `(session_id, call_id)` index; it is not an `EventObserver`.
- `McpCatalogHandle` is one interior-synchronized catalog authority shared by
  `TactToolPort` and full-router replacement.
- `NoopTranscriptStore` is the production sessionless adapter and preserves the
  commit barrier without durable rows.
- `NoopUsageSink` is its sessionless usage counterpart: records and `flush`
  succeed without durable rows, making the loss of usage persistence explicit.
- `TactEventBridge` maps `AgentEvent` to the current `AgentUpdate` stream.
- `TactToolPresentation` computes argument summaries, labels, and popup policy
  outside `tact_agent_api`.
- `HookMcpHandle` preserves the existing hook-side MCP operation without
  exposing the model `ToolPort` or a general tool executor to the gate.
- The adapters implement their capability traits directly; no `*Plugin`
  wrapper or registry entry is introduced.
- `compose_tact_kernel()` constructs fresh `KernelDeps`, adds the ordered observer
  list supported by the first slice, reuses shared handles from `Agent`, and
  creates the kernel.
- `cancel_bridge` creates the activation token from
  `AgentRuntime.cancel_flag`; no adapter creates or cancels an independent
  token.

**Work:**

- [x] Before adding adapters, freeze
  `crates/tact/testdata/agent_kernel/legacy_baseline.json` for simple text, tool
  use including an image-producing result, permission denial, transient retry,
  cancellation during a stream and tool wave, injected context, hosted web
  search, resumed SQLite history, and TUI full-area blank-background painting.
  Normalize volatile ids, timestamps, durations, database ids, and temporary
  paths as specified; preserve `AgentUpdate`/event order and multiplicity,
  transcript origins, cursor, usage, and terminal outcome. Reuse exact existing
  assertions and record only behavior that would otherwise disappear with the
  legacy loop. The fixture and normalizer are present; all per-scenario
  kernel-run comparisons and Task 7 deletion are complete, with the final broad
  verification recorded in Task 7.
- [x] Complete the `ToolResolver` split before adding gate adapters. Change
  `ToolGate::before` to require `&dyn ToolResolver`, update kernel fakes, and add
  compile-time plus recording tests proving resolution cannot execute a tool and
  the kernel executes only the final `Allow`.
- [x] Add kernel-authority tests proving denied and cancelled calls never reach
  `ToolPort::execute`; add a source guard proving the current adapters do
  not hold `AgentState`, `TranscriptStore`, or `UsageSink` authority. Extend
  that guard to gate/context/store/composition adapters as they land.
- [x] Add cancellation race tests with bounded timeouts: repeated cancellation,
  cancellation during stream/gate/wave, current-wave drain, no new work after
  cancellation, exactly one terminal event, no post-terminal events, and
  retained committed transcript/cursor/usage state. Both the public-flag bridge
  and direct `AgentRuntime.cancel_flag` facade writes have coverage.
- [x] Add injection/activity golden tests proving injected context keeps a
  non-`User` origin and hosted provider activity emits no user/tool transcript
  message and no client tool execution.
- [x] Add the production model, tool, event, cancel, no-op transcript, no-op
  usage, and presentation adapters before the kernel-only cutover.
- [x] Add `TactSharedServices`, `PermissionAuthority`, `HookMcpHandle`, and the
  read-only `TactActivationView`. Move hooks, update/status traffic, stats,
  queues, and MCP catalog state onto the per-logical-agent shared handles; keep
  session/model metadata in the activation view. Keep `Agent` as the facade and
  shared services as the control-plane authority: `LoopState` is a projection,
  not an `Agent` clone.
- [x] Change `LoopState` to the lifetime-parameterized `TactActivationView`
  alias and update every hook trait bound to borrow the view for the activation
  lifetime. Migrate closures that explicitly name `&Agent`; do not introduce a
  non-generic projection alias or an owned clone.
- [x] Add cumulative `ToolGateContext.total_turns`, update kernel constructors
  and fakes, and build the hook payload's `turn_id` from that value instead of
  reading `Agent.turns_taken`.
- [x] Migrate every in-repo hook closure that explicitly takes `&Agent` to
  `&LoopState`, and add a compile-time/source test proving the projection cannot
  obtain `AgentState`, `TranscriptStore`, `UsageSink`, or a mutable transcript.
  Document the hook-source migration. A hook that needs facade state cannot run
  through the kernel projection and has no automatic legacy fallback.
- [x] Add a shared permission visibility test: a mode change or always-allow
  grant made through the facade is seen by the next gate call, and gate changes
  to mode/rules/denial counters are visible when the activation returns. Build
  each subagent's separate authority from `PermissionAuthority::snapshot`, not
  a second live parent manager, and prove its session identity is distinct.
  **Evidence:**`facade_mode_and_grant_changes_reach_the_next_gate_call`,
  `gate_denials_are_visible_through_the_shared_facade_authority`,
  `always_allow_popup_selection_is_visible_to_the_next_gate_call`, and
  `subagent_permissions_are_snapshot_derived_and_session_scoped` pass.
- [x] Add the remaining gate, context, SQLite transcript, SQLite usage, and
  composition adapters, then remove the temporary helper scaffold after parity.
- [x] Extract the existing dispatch policy rather than reimplementing it:
  the sensitive guard, hook/permission/popup/reclassification pipeline, and the
  post-tool hook/status/rewrite block belong behind `TactToolGate`. Behavior is
  identical and the deleted legacy dispatch copy has no production successor.
  **Evidence:** `TactToolGate` and `TactToolPort` are the only policy and
  execution paths;
  `pre_tool_rewrite_is_fully_re_resolved_and_reclassified`,
  `permission_request_rewrite_is_rechecked_before_allow`,
  `post_tool_redacts_rewritten_output_and_preserves_the_original_error`,
  `original_credential_call_fails_closed_before_any_hook`, and
  `post_tool_hooks_cannot_join_a_secret_across_rewrites` pass. Scheduling,
  presentation, transcript assembly, and task bookkeeping stay behind product
  adapters rather than moving into the gate.
- [x] Add the first SQLite schema step in `session_store`: nullable
  `messages.origin`, nullable `token_usages.call_id`, and a partial unique
  `(session_id, call_id)` index using `PRAGMA table_info` plus `ALTER TABLE` for
  existing databases. New writes include origins/call ids; historical NULL rows
  remain readable through the origin inference rules completed in Task 6.
- [x] Implement `SqliteUsageSink` with `call_id` upsert, request-body and message
  range retention, and a `flush` that does not return before queued writes are
  visible.
- [x] Implement `NoopUsageSink` for production sessionless composition, where
  usage persistence is intentionally disabled.
- [x] Add `NoopTranscriptStore` with sessionless receipt/clear tests, and keep
  the adapters constructor-injected and free of global registry lookups.
- [x] Keep the composition root as the only wiring authority; adapters must not
  compose or invoke each other's execution paths, and `EventObserver` remains
  read-only with no conflict or short-circuit semantics.
- [x] Give `TactToolGate` no stored `ToolPort`, executor, `TranscriptStore`,
  `UsageSink`, or `EventPort`; its only model-tool resolution handle is the
  per-call `&dyn ToolResolver`. Assert the allowed shared handle set in source
  and constructor tests.
- [x] Do not add a `RuntimePlugin` trait, plugin ids, service slots, or
  install/freeze lifecycle.
- [x] Keep the permission/security order inside `TactToolGate`; the kernel must
  not know about `PermissionManager`, `PermissionAuthority`, or `Scanner`.
- [x] Scan the original call before hooks and fail closed on `Credential`.
  Re-resolve and reclassify after `PreToolUse` before deciding auto-approval or
  a prompt, then repeat after any `PermissionRequest` mutation. Each checkpoint
  recomputes sensitive classification, risk, stable name, MCP auto-approval,
  prompt policy, and resource scopes before permission/execution.
- [x] Refresh MCP/tool state before each request and expose it through
  `ToolSnapshot`; do not reintroduce a synchronous `specs()` shortcut.
- [x] Refactor `MCPToolRouter` mutable catalog/connection state behind
  `McpCatalogHandle`; expose `refresh_changed(&self)`, keep cheap handle clones,
  and replace the handle only between activations while an active activation
  keeps its clone.
- [x] Pass progress through the kernel's `ToolProgressSink`; keep direct task,
  subagent, hook, and transcript traffic in their product adapters.
- [x] Redact live chunks before `ToolProgressSink` and final output before
  returning `ToolExecutionResult`; preserve held-back tail state for a secret
  split across chunks.
- [x] Redact hook-rewritten visible output in `TactToolGate::after` before it
  reaches `ToolFinished`, persistence, or a host.
- [x] Preserve image-bearing tool output as companion `Image` blocks in the one
  grouped result message; do not route it through `render_tool_result` as
  `[image]` text.
- [x] Add kernel tests for image companion preservation and
  `last_token_total == TokenUsage::total_tokens` on ordinary and compaction
  calls; using `input_tokens` is a failing parity test.
- [x] Keep Tera, skill, memory, AGENTS.md, and compaction code inside the
  context policy adapter. SessionStart/UserPromptSubmit hooks enqueue context
  through the activation view; the policy returns it as `appended_messages`,
  and the kernel alone commits it.
- [x] Keep SQL, request-body retention, token usage, and session locks inside
  the SQLite/session or usage adapters. The kernel only sees awaited records and
  opaque receipts/ranges.
- [x] Map `TurnStarted` to `TurnStats`, `ModelCallStarted` to `ModelInfo`,
  message events to stream chunks, and tool events to `StepAdded`/`StepStarted`/
  `ToolProgress`/`StepFinished`/`StepFailed`.
- [x] Map typed tool results, permission labels, usage metadata, notices, and
  compaction events without flattening failures back into success.
- [x] Map provider activity to the existing web-search UI without executing it
  as a kernel tool; preserve stable id, step order, phase, query, failure
  detail, and duration.
- [x] Preserve adapter-owned host traffic outside the kernel event stream:
  `RequestSelect`/`RequestMultiSelect`, `HookStatus`/`HookContext`,
  `TasksChanged`/`GoalsChanged`, `ToolMeta`, background completions,
  subagent updates, and tool transcript streams.
- [x] Route kernel-bridge updates and adapter-owned host traffic through the same
  multiplexed runtime sink without dropping or duplicating either source.
- [x] Ensure `EventPort` and observers enqueue without blocking the model stream
  or tool wave.
- [x] Add recording bridge tests that compare old/new sequences for simple,
  tool-use, denial, progress, permission, provider-activity, and cancellation
  runs. **Evidence:** the nine fixture-backed
  `*_kernel_run_matches_frozen_legacy_baseline` comparators cover simple text,
  image-bearing tool use, denial, retry/progress, web-search activity,
  cancellation, injection, and resumed history; the source-matrix test covers
  the adapter-owned update boundary.
- [x] Add adapter tests for benign-to-credential and permission-request rewrites,
  low-risk-to-high-risk MCP rewrites, live/final/hook-rewritten redaction, and a
  secret split across live chunks or hook output. **Evidence:**
  `pre_tool_rewrite_is_fully_re_resolved_and_reclassified`,
  `permission_request_rewrite_is_rechecked_before_allow`,
  `tool_port_redacts_split_live_chunks_and_the_final_result`,
  `post_tool_redacts_rewritten_output_and_preserves_the_original_error`, and
  `post_tool_hooks_cannot_join_a_secret_across_rewrites` pass.
- [x] Add usage adapter tests for immediate visibility after flush, idempotent
  replay, record failure, legacy NULL-call-id readability, unique
  `(session_id, call_id)` enforcement, and final compaction/continuation drain.
  **Evidence:** `sqlite_usage_is_visible_before_flush_returns`,
  `idempotent_records_leave_one_current_row`,
  `sqlite_usage_record_failure_is_reported`,
  `usage_call_ids_upsert_and_legacy_null_rows_remain_readable`, and the kernel
  compaction/usage flush tests pass.
- [x] Add MCP tests for a mid-turn list change and a full router replacement
  between activations.
- [x] Add source-matrix tests proving every adapter-owned `AgentUpdate` still
  reaches the host while kernel events are mapped exactly once.
- [x] Extend the authority source guard to `gate_adapter`, `context_adapter`,
  `store_adapter`, `usage_adapter`, `services`, `activation_view`,
  `permission_authority`, `hook_adapter`, and `composition`. It must reject
  stored `Agent`, `AgentState`, `ToolPort`, `TranscriptStore`, `UsageSink`, or
  kernel `EventPort` in the gate and reject transcript/cursor ownership in all
  policy adapters.

Task 4's bridge, redaction, usage, and adapter-source gates are complete. The
broader host-visible sequence matrix remains a Task 6 deliverable because it
spans ask-user, hooks, background/subagent, compaction, MCP-refresh, and Goal
yield/completion behavior beyond the frozen F7 scenarios.

**Gate:** adapter tests pass; the read-only hook projection and
shared permission authority are wired; the full adapter source guard covers
gate/context/store/usage/services/activation-view/permission/hook/composition;
the additive SQLite columns and unique call-id index open both fresh and
pre-migration databases;
`cargo test -p tact --lib agent::` and `cargo check -p tact` pass;
`cargo test -p tact --lib mcp::` and usage adapter tests pass;
`cargo test -p tact_agent_api --lib tool::` and the focused
`cargo test -p tact_agent_core --lib` authority/cancellation tests pass;
`scripts/check-agent-kernel-deps.sh` confirms both kernel crates remain clean.

---

## Task 5: Route a Basic Run Through the Kernel

**Files:**

- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact/src/agent/outcome.rs`
- Modify: `crates/tact/src/lib.rs`
- Modify: `crates/tact/src/runtime/composition.rs`
- Create: `crates/tact/src/runtime/facade_state.rs`
- Modify: `crates/tact/src/runtime/cancel_bridge.rs`
- Modify: `crates/tact-ui/src/session_bootstrap.rs`
- Modify: `crates/tact-ui/src/interactive.rs`
- Modify: `crates/tact-ui/src/headless.rs`
- Test: existing agent, driver, and headless tests

**Interfaces:**

- `tact::Agent` keeps its public fields and builder methods while owning the
  current kernel composition used by the next activation.
- Production activation is kernel-only. After `ensure_session`,
  `agent_loop_detailed` enters `run_kernel_activation` directly; there is no
  activation selector or legacy fallback.
- `take_facade_state(&mut Agent) -> FacadeActivationState` and
  `restore_facade_state(&mut Agent, FacadeActivationState)` move the existing
  transcript/cursor/turn authority plus all behavioral sidecar state.
- `FacadeActivationState { state: AgentState, sidecar: FacadeSidecar }`.
  `AgentState` contains transcript, cursor, total turns, last token total, and
  initialization; `FacadeSidecar` contains `first_message_db_id`,
  `last_message_db_id`, `llm_call_last_message_id`, `compact_state`,
  `recovery_state`, `goal_yield_reason`, `max_turns_reached`, and
  `tool_use_counter`.
- An activation guard restores the complete envelope on success, cancellation,
  error, and unwind.
- Shared services, permission authority, MCP catalog, update sink, stats, and
  queues stay outside the envelope. `LoopState` exposes only the read-only
  projection; the kernel and gate never hold `Agent`.
- `cancel_bridge` creates one `CancelToken` from `AgentRuntime.cancel_flag`,
  mirrors direct flag writes while the activation guard is alive, and stops the
  watcher on exit.
- `agent_loop` remains `Result<()>`.
- `agent_loop_detailed` remains the typed bounded-activation entry point.
- The existing `AgentUpdate` stream remains the observable frontend contract.

**Work:**

- [x] Compose dependencies at activation entry from current facade fields and
  the stable per-logical-agent `TactSharedServices`/`PermissionAuthority`
  handles, so replacing the model, MCP router facade, or hooks is visible on
  the next run rather than hidden behind a stale adapter. Keep
  `McpCatalogHandle` shared and each agent's permission state authoritative
  across its activations.
- [x] Run common facade bootstrap and `ensure_session` first. Preserve non-empty
  direct `runtime.context` mutations by relying on `ensure_session`'s
  empty-context load rule.
- [x] After `ensure_session`, enter `run_kernel_activation` directly. Active
  Goal and hook cases use the same kernel path. Required adapters are
  constructor-injected; sessionless hosts explicitly choose no-op transcript
  and usage adapters. A kernel error never falls back mid-run.
- [x] Build `AgentState` with `initialized = true` only after `ensure_session`.
  Move the facade state and complete sidecar into the activation guard and
  restore them through one path. Never keep two writable transcript,
  provider-state, or behavioral-state authorities in use at the same time.
  Shared services and hooks stay outside the envelope and address the same
  handles before, during, and after activation. **Evidence:**
  `FacadeStateGuard`, `FacadeActivationState`, and `FacadeSidecar` are used by
  `run_kernel_activation`; `facade_state_guard_restores_every_owned_sidecar_field`
  and `facade_state_guard_restores_on_unwind_and_keeps_persisted_ranges` pass.
- [x] Update range fields from opaque store receipts rather than exposing DB ids
  through `tact_agent_api`.
- [x] Map `Agent.turns_taken` to `AgentState.total_turns` using dispatched
  main-loop attempts only; do not count compaction calls or transport retries,
  and do not consume a turn when cancellation wins before dispatch.
- [x] Create the activation token from the public cancel flag, cancel
  immediately when it is already set, and mirror later writes until the
  activation guard exits. Nested subagents perform the same bridge for their
  own flags; adapters only observe the token.
- [x] Route all mode reads/writes, always-allow grants, and denial-counter
  updates through `PermissionAuthority`.
- [x] Add a facade-to-gate-to-facade permission visibility test and a subagent
  test proving a separate session identity and a snapshot-derived authority,
  not a copied live parent manager.
- [x] Build hook callbacks from `TactActivationView` and the shared queues.
  Ensure a hook's queued context is returned through the context policy and
  committed once by the kernel; attempted transcript writes have no effect.
- [x] Delegate eligible ordinary activations, including the simple no-tool and
  single-tool paths, through the production facade to the kernel after
  readiness selection.
- [x] Remove the legacy path from production and preserve adapter construction
  order, observation semantics, and event sink behavior on the kernel path.
- [x] Ensure `TaskComplete` remains driver-owned rather than kernel-owned.
- [x] Add parity tests proving direct mutations of `AgentRuntime.context` and
  `provider_state` affect the next activation, and cancellation/error paths
  restore the public facade fields.
- [x] Add an empty-runtime-context plus populated-SQLite parity test proving
  `ensure_session` loads history before `AgentState` construction.
- [x] Add an exhaustive facade-field ownership map and sidecar tests for every
  moved field on success, cancellation, error, and unwind, including row ranges,
  compaction/recovery state, Goal yield, max-turn flag, tool counter, hook
  flags/source, pending SessionStart context, and prompt caches. Verify shared
  queues, stats, cancel flag, and hook status sequence keep one authority.
  **Evidence:** `facade_state_guard_restores_every_owned_sidecar_field`,
  `facade_state_guard_restores_on_unwind_and_keeps_persisted_ranges`, and the
  receipt tests in `runtime::facade_state` pass.
- [x] Add direct `AgentRuntime.cancel_flag` writes during streaming, approval,
  bash, other in-flight tools, and between waves; each must reach the kernel
  token and preserve tool pairing. The F3 closure evidence above covers every
  case.
- [x] Add parity tests for model info, thinking, text streaming, tool progress,
  cancellation, and whole-activation path selection.
- [x] Add a kernel-only matrix covering active Goal and hook cases, asserting
  activation enters the kernel once before state transfer.

**Gate:** focused agent tests, `tact-ui` driver tests, and headless tests pass;
simple no-tool and single-tool activations run through the production facade
kernel path; the production cancel bridge, readiness matrix, permission
authority, readonly hook projection, `cargo check -p tact`, and the dependency
guard pass; no test expectation is weakened.

**Current gate state (2026-10-27):** dependency composition, bootstrap ordering,
direct kernel activation, public-flag bridge wiring, the complete facade sidecar
guard, store-receipt transfer, turn transfer, hook projection, driver-owned
`TaskComplete`, the full public-cancellation matrix, the facade/gate/subagent
permission visibility cases, direct public-state mutation, empty-context SQLite
resume, event parity, and replacement-model composition are implemented and
tested. `TactToolGate` is the only tool-policy path, so no duplicated
policy/hook/permission block remains.

---

## Task 6: Migrate Durable and Policy-Heavy Behavior

**Files:**

- Modify: `crates/agent_api/src/state.rs`
- Modify: `crates/agent_core/src/kernel.rs`
- Modify: `crates/agent_core/src/kernel/tests.rs`
- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact/src/agent/tool_schedule.rs`
- Modify: `crates/tact/src/tool/subagent.rs`
- Modify: `crates/tact/src/tool/worktree.rs`
- Modify: `crates/tact/src/mcp/`
- Modify: `crates/tact/src/compact/`
- Modify: `crates/tact/src/recovery.rs`
- Modify: `crates/tact/src/hook/`
- Modify: `crates/tact/src/plugin/`
- Modify: `crates/tact/src/permission/`
- Modify: `crates/tact/src/subagent.rs`
- Modify: `crates/tact/src/store/session_store/mod.rs`
- Modify: `crates/tact/src/store/session_store/sqlite.rs`
- Modify: `crates/tact/src/runtime/store_adapter.rs`
- Modify: `crates/tact/src/runtime/usage_adapter.rs`
- Modify: `crates/tact/src/runtime/event_adapter.rs`
- Modify: `crates/tact/src/runtime/services.rs`
- Modify: `crates/tact/src/runtime/activation_view.rs`
- Modify: `crates/tact/src/runtime/permission_authority.rs`
- Modify: `crates/tact/src/runtime/tool_presentation.rs`
- Modify: `crates/tact-ui/src/goal_runner.rs`
- Test: focused and cross-crate integration suites

**Work:**

- [x] Move provider cursor loading and atomic assistant-message/cursor commit to
  the transcript adapter and add a test that tools cannot run before a
  successful commit.
- [x] Preserve image-bearing result blocks through transcript persistence and
  provider conversion, including the companion-block position for a mixed
  multi-tool batch. **Evidence:**
  `mixed_tool_result_images_keep_their_companion_order_through_sqlite` and
  `tool_use_image_companion_kernel_run_matches_frozen_legacy_baseline` pass.
- [x] Complete the additive `messages.origin` migration. Store the serialized
  `MessageOrigin` explicitly for new append/replace calls; return an
  origin-bearing stored record on load; decode non-null values strictly; and
  infer NULL rows only from complete frames and exact content:
  compaction handoff/summary prefix, complete hook frame, complete Goal frame,
  complete subagent-finished frame, exact continuation prompt, tool-result-only
  blocks, assistant role, then real user fallback. Reconstruct legacy
  `MessageKind` from the resolved origin for old callers.
- [x] Test a pre-column SQLite file opens and migrates without losing rows.
  Repeated loads preserve the same inferred origins; replacement writes explicit
  origins; and those origins survive reopen. Cover user text, hook context, Goal
  context, subagent results, continuation prompts, compaction summaries,
  assistant output, and tool results. Synthetic user-role rows must never become
  real user turns, and genuine marker-like user text stays `User`.
- [x] Persist one grouped user-role result message in assistant source order and
  synthesize missing paired results into that same message on every terminal
  path after a persisted tool-use assistant message. Preserve each image as a
  companion block immediately after its `ToolResult`; never split a dispatch
  batch into one user message per tool or flatten image content.
- [x] Move SessionStart and UserPromptSubmit behavior into `prepare_run`;
  persist transformed inputs plus session-start context before the first model
  call.
- [x] Move pending subagent results and hook context into
  `RequestPlan.appended_messages`; persist them with explicit non-user
  origins before the request that consumes them.
- [x] Move PreToolUse, PermissionRequest, and PostToolUse behavior into
  `TactToolGate`; preserve sensitive scanning first, hook mutation and
  re-resolution, final sensitive/risk/name/MCP/prompt/resource
  reclassification, permission mode, approval prompt, post-processing, status,
  and original errors.
- [x] Keep Interrupt, Stop, SessionEnd, and their host traffic in the facade or
  driver adapters. Do not add speculative `RunHook` infrastructure merely to
  route them.
- [x] Move micro, auto, manual, recovery, command, local non-streaming summary,
  and native compaction behind the typed `ContextPolicy` protocol, always using
  the kernel's observed model port and kernel-owned atomic persistence.
  **Evidence:** `compaction_commit_callback_runs_after_the_replacement_is_persisted`,
  `failed_replacement_never_acknowledges_compaction_commit`,
  `replacement_commits_and_posts_only_after_kernel_acknowledgement`,
  `every_full_compaction_kind_reaches_precompact_with_its_trigger`,
  `native_compaction_returns_only_a_replacement_cursor`,
  `micro_replacement_keeps_recent_results_without_full_post_state`,
  `skipped_prepare_run_compaction_does_not_replay_stale_inputs`,
  `skipped_request_compaction_does_not_replay_queued_context`,
  `replaced_prepare_run_compaction_reuses_hook_prepared_inputs_once`,
  `micro_replacement_still_allows_auto_before_queued_context`, and
  `command_compaction_uses_kernel_owned_replacement_and_events` pass.
- [x] Preserve transient-backoff and continuation notices, request-body
  diagnostics, and message ranges through neutral events and `UsageRecord`, and
  persist token usage through `UsageSink` rather than an event observer.
  **Evidence:** the transient-retry baseline, event-adapter notice mapping,
  usage-adapter request-body/message-range tests, and kernel usage-flush tests
  pass.
- [x] Export retry counters as typed kernel data instead of leaving them local
  to `run_model_attempt`/the activation loop. Add `RecoveryStats` to
  `RunOutcome`, count transport retries, continuation requests, and
  overflow-compaction attempts, then assign the returned values to
  `AgentRuntime.recovery_state` before `agent_loop_detailed` returns. Tests must
  cover recovered retry, exhausted continuation, overflow compaction, and
  cancellation during backoff without inferring counters from `Info` text.
  **Evidence:** `kernel_recovery_counts_are_visible_on_the_facade_after_success`,
  `kernel_recovery_counts_are_visible_on_the_facade_after_cancellation`, and
  `agent_loop_detailed_reports_max_tokens_after_continuations_exhausted` pass.
- [x] Extend `record_token_usage` with `call_id: Option<&str>`. Kernel records
  pass a non-empty id and use `INSERT ... ON CONFLICT(session_id, call_id) DO
  UPDATE`; legacy callers keep NULL rows. The partial unique index must allow
  multiple NULL call ids, reject duplicate non-NULL ids, and body trimming must
  run after both insert and update. Test replay leaves one current row and
  pre-migration NULL rows remain readable.
- [x] Preserve `last_token_total` from `TokenUsage::total_tokens` on ordinary and
  compaction calls, matching legacy `usage.total`; never substitute
  `input_tokens`.
- [x] Preserve the kernel `MaxTokens + ToolUse` ordering: execute and pair
  pending tools before the continuation, append the continuation prompt, and
  assert that the next provider request has no orphaned tool calls.
  `max_tokens_with_tools_executes_pairing_then_continues` and
  `exhausted_max_tokens_synthesizes_truncated_results` cover the kernel path.
- [x] Preserve product `MaxTokens + ToolUse` parity for real `YieldToHost`,
  manual compaction, and future-wave suppression after truncated responses.
  **Evidence:** `max_tokens_tool_use_runs_manual_compaction_then_continues`,
  `max_tokens_yield_suppresses_later_tool_waves_before_continuation`,
  `agent_loop_max_tokens_goal_request_input_yields_without_continuation`, and
  `max_tokens_with_tools_runs_manual_compaction_before_continuation` pass.
- [x] Refresh MCP tools before every model request and test a mid-turn list
  change reaching the next request with an incrementing snapshot generation.
  **Evidence:** `kernel_refreshes_a_mid_turn_mcp_catalog_before_the_next_request`
  proves the next kernel request sees a tool announced while the preceding
  tool call is in flight.
- [x] Move `MCPToolRouter` mutable catalog state behind `McpCatalogHandle`, make
  refresh use `&self`, and test a full router replacement between activations.
- [x] Compute native path, MCP server, bash, unknown-tool, subagent, and worktree
  resource scopes in `TactToolPort`; preserve hierarchical path conflicts and
  allow only `spawn_subagent(worktree=true)` to run independently. **Evidence:**
  `port_classifies_native_mcp_and_subagent_resource_scopes` passes.
- [x] Prove nested-subagent composition through the product-owned child
  construction path. `spawn_subagent` must continue to build a child `Agent`,
  child session, cancellation flag, and snapshot-derived permission authority
  in `tact`, then invoke the same bounded kernel activation. Add a test that
  observes the child kernel path and child-owned services. Do not add a
  subagent branch, subagent factory trait, registry, or kernel plugin slot.
  **Evidence:** `nested_subagent_uses_the_kernel_with_child_owned_services`
  passes.
- [x] Keep the Goal Runner outside the kernel and let the facade continue to
  own the public turn counter. **Evidence:** `GoalRunner` remains in
  `crates/tact-ui/src/goal_runner.rs`; `run_once_bounds_a_single_activation_by_the_turn_budget`,
  `run_once_never_overspends_the_cumulative_turn_budget`, and the facade
  turn-delta tests pass.
- [x] Preserve the Goal final-turn reminder through
  `ContextView.remaining_turns`, `goal_request_input` as `YieldToHost`, and
  `goal_propose_completion` as `CompleteRun` only when
  `ToolExecutionContext.remaining_turns == Some(1)`. Non-final proposals
  continue; a final completion returns `Completed`, not `MaxTurns`; a
  truncated response cannot newly complete the Goal. **Evidence:**
  `agent_loop_reminds_a_goal_activation_to_checkpoint_on_its_last_turn`,
  `goal_completion_on_the_final_turn_returns_completed`,
  `non_final_goal_completion_continues_the_activation`,
  `truncated_final_goal_turn_cannot_complete_the_goal`, and
  `goal_context_input_keeps_its_non_user_origin_on_the_kernel_path` pass.
- [x] Test that `YieldToHost` lets the current wave finish while suppressing all
  later side-effecting waves.
- [x] Add the complete parity matrix for full `AgentUpdate` sequences:
  simple, tool use, permission denial/approval, progress, ask-user, hooks,
  background/subagent, cancellation, retry, compaction, MCP refresh, and Goal
  yield/completion. **Evidence:** the event-adapter, baseline, Goal, subagent,
  cancellation, compaction, and MCP tests named in this plan cover each row;
  the behavior table is recorded in the spec's
  `AgentUpdate Sequence Matrix`.
- [x] Run a kernel-path comparison for every F7 scenario before Task 7:
  `simple_text`, `tool_use_image_companion`, `permission_denial`,
  `transient_retry`, `cancellation_during_stream`,
  `cancellation_during_tool_wave`, `injected_context`, `hosted_web_search`,
  `resumed_sqlite_history`, and `tui_full_area_blank_background`. The TUI case
  must execute the full-area/blank-background assertion on the kernel path, not
  only cite its legacy test. **Evidence:** the nine
  `*_kernel_run_matches_frozen_legacy_baseline` / `*_matches_frozen_legacy_baseline`
  comparators pass. The required TUI check,
  `kernel_updates_leave_no_unowned_blank_tui_cells`, also passes and verifies
  that every blank cell belongs to an intentional surface.
- [x] Add redaction parity for live progress, final results, and hook-rewritten
  visible output, including a secret split across chunks. **Evidence:**
  `tool_port_redacts_split_live_chunks_and_the_final_result`,
  `post_tool_redacts_rewritten_output_and_preserves_the_original_error`, and
  `post_tool_hooks_cannot_join_a_secret_across_rewrites` pass.
- [x] Test sessionless execution through `NoopTranscriptStore` and
  `NoopUsageSink`. **Evidence:**
  `sessionless_composition_runs_a_simple_text_activation` passes.
- [x] Test direct mutation of `AgentRuntime.context` and `provider_state` across
  activations. **Evidence:**
  `kernel_activation_uses_direct_facade_context_mutations` and
  `kernel_activation_uses_direct_facade_provider_state_mutations` pass.
- [x] Test resumed TUI-style bootstrap with empty runtime context plus populated
  SQLite history. **Evidence:**
  `empty_facade_context_loads_sqlite_history_before_kernel_transfer` passes.
- [x] Test that usage is durable before the sink's flush barrier and that a
  record failure is surfaced. **Evidence:**
  `sqlite_usage_is_visible_before_flush_returns` and
  `sqlite_usage_record_failure_is_reported` pass.
- [x] Test crash recovery for a durable unmatched tool call on both local and
  Responses sessions, proving one grouped interrupted result reaches the
  product facade before the resumed request. **Evidence:**
  `local_session_reconciles_a_durable_unmatched_tool_call_before_the_request`
  and
  `responses_session_reconciles_a_durable_unmatched_tool_call_before_the_request`
  pass.
- [x] Add a permission-authority parity test covering mode changes, allow-once,
  always-allow, denial streak, and subagent inheritance while hooks receive
  only the read-only activation view. **Evidence:**
  `permission_authority_matrix_covers_grants_denials_modes_and_inheritance`
  passes.

**Gate:** targeted `tact`, `tact-ui`, Goal, and protocol parity tests pass;
the origin and call-id migration tests pass on fresh and pre-migration
databases; all ten F7 scenarios have kernel-path comparisons; the full adapter
source guard and `cargo check -p tact` pass; every production activation stays
on the kernel path before Task 7.

**Current gate state (2026-10-27):** cursor/transcript commits, origin and
call-id migration, grouped tool results, mixed image-batch persistence,
hook/context preparation, gate pipeline, lifecycle-hook ownership, max-token
continuation, MCP catalog ownership, resource scopes, Goal Runner ownership,
sessionless composition, direct facade mutation, resumed TUI bootstrap, usage
visibility, host-yield semantics, recovery-state export, nested-subagent
composition, the `AgentUpdate` sequence matrix, local/Responses crash recovery,
the consolidated permission matrix, and nine fixture-backed F7 comparisons plus
the TUI full-frame assertion all have focused kernel or adapter coverage.

---

## Task 7: Remove the Legacy Control Loop

Task 7 is complete. It was deletion and compatibility cleanup after production
activations had already moved to the kernel exclusively.

**Files:**

- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact/src/runtime/composition.rs`
- Keep only `ToolResources`: `crates/tact/src/agent/tool_schedule.rs`
- Delete: `crates/tact/src/agent/tool_dispatch.rs`
- Delete: `crates/tact/src/runtime/activation.rs`
- Delete: `crates/tact/src/runtime/legacy_model_bridge.rs`
- Keep the kernel helper: `crates/tact/src/runtime/compaction_adapter.rs`
- Modify: `crates/tact/src/lib.rs`
- Modify: `ARCHITECTURE.md`
- Modify: any re-export/test support references

**Work:**

- [x] Run the full parity matrix from Task 6 and confirm every row is green
  before deleting code. In particular, include facade mutations, tool rewrite and
  status, image companions, failed-directive clearing, progress/redaction, model
  terminal events, mid-run persistence, MCP refresh, max-token tool
  continuation, cancellation bridge, usage flush, `last_token_total`,
  interrupted-tool reconciliation, turn accounting, early yield, Goal final
  turns, legacy origins, read-only hook projection, shared permission authority,
  resumed sessions, full MCP router replacement, and sessionless runs. The
  serial `scripts/check-rust.sh` run and `cargo test -p tact` covered this
  matrix; the final verification block above records the counts.
- [x] Compare the Task 4 normalized legacy baseline for every frozen scenario
  before deleting the old path, including injected-context authorship, hosted
  provider activity without a user/client-tool fabrication, cancellation
  terminal cardinality, resumed history, approval denial, and TUI full-area
  blank-background paint. The required IDs are `simple_text`,
  `tool_use_image_companion`, `permission_denial`, `transient_retry`,
  `cancellation_during_stream`, `cancellation_during_tool_wave`,
  `injected_context`, `hosted_web_search`, `resumed_sqlite_history`, and
  `tui_full_area_blank_background`. All nine fixture-backed comparators and the
  TUI full-frame surface assertion pass. The first checklist item above also
  requires the broader Task 6 parity matrix before code deletion.
- [x] Remove the unreachable legacy control helpers after the kernel parity gate
  is green. This includes the old stream/compaction/tool-loop paths such as
  `CompactRebuildMode::CodexStyle`, `compact_responses_native`,
  `compact_history_local*`, `apply_user_prompt_hooks`, legacy `compact_id_display`,
  and the unused `TOOL_*`/`unexecuted_tool_results` block. Keep the kernel-path
  `runtime::compaction_adapter::compact_id_display` used for host-visible
  compaction ids.
- [x] Remove `ActivationPath`, `ActivationFallbackReason`, `ActivationSelection`,
  `select_activation_path`, `last_fallback_reason`, and
  `last_activation_path_for_test` once no test needs the scaffold. Remove the
  temporary `LegacyModelBridge` after moving any still-useful parity assertions
  to the event adapter or deleting duplicate tests.
- [x] Audit `crates/tact/src/agent/tool_schedule.rs` and remove only legacy
  scheduling helpers. Do not delete the module: `ToolResources` remains the
  product resource-scope contract used by `tool/metadata.rs` and
  `runtime/tool_adapter.rs`.
- [x] Remove any remaining legacy `LoopState = Agent` adapter, facade-state
  compatibility shim, or transcript-write shim. `LoopState` remains the
  read-only projection and no production hook path receives `Agent`.
- [x] Keep `AgentLoopOutcome`/`AgentLoopStop` as compatibility aliases or thin
  conversions to the kernel outcome types.
- [x] Remove unused imports and dependencies from `tact` only after a separate
  dependency audit.
- [x] Update `ARCHITECTURE.md` to describe the target dependency graph and
  kernel/capability boundaries.
- [x] Add a short migration note explaining that `AgentUpdate` is a host
  protocol, not a kernel type.

**Gate:** full `tact`, `tact-ui`, `tui`, `tact_llm`, protocol, and kernel tests
pass; `cargo check -p tact` and `scripts/check-agent-kernel-deps.sh` pass; no
unreachable legacy helper, activation selector scaffold, or temporary model
bridge remains outside explicit test support.

---

## Task 8: Optional Capability Extraction

**Status (2026-10-27): closed as not triggered.** No concrete second consumer
exists in the workspace. The kernel boundary is complete enough to stop at
`tact_agent_api`, `tact_agent_core`, and the `tact` composition root; splitting
capabilities merely to mirror Pi's package count would add speculative
maintenance surface.

**Files:**

- Only create new capability crates when a concrete second consumer exists.
- Candidate boundaries: `tact_store`, `tact_tools`, `tact_mcp`,
  `tact_plugin`, `tact_prompt`.

**Work:**

- [x] Audit the workspace manifests, imports, and dependency edges for an
  external consumer of `tact_store`, `tact_tools`, `tact_mcp`, `tact_plugin`, or
  `tact_prompt`. No such consumer exists.
- [x] Do not write speculative consumer notes or start with a hypothetical
  `tact_store`/`tact_tools` split while the trigger condition is absent.
- [x] Keep every capability in `tact`; no moved public type requires a
  transitional re-export because no capability type moved.
- [x] Create no crate solely to make the directory tree look modular.

**Gate:** closed only because the required second-consumer trigger is absent.
If a real consumer appears, reopen this as a new design task; each extraction
must then remove a real dependency or enable that documented consumer, and all
existing tests must remain green.

---

## Verification and Documentation

- [x] Run one Cargo invocation at a time. Start with focused tests, then run
  `cargo test -p tact_agent_api`, `cargo test -p tact_agent_core`,
  `cargo test -p tact_llm`, focused `cargo test -p tact-ui --lib account::`,
  `cargo test -p tact`, `cargo test -p tact-ui`, and `cargo test -p tui`.
  `tact_agent_api`/`tact_agent_core` passed 15/64 tests, and the broad package
  suites passed through `scripts/check-rust.sh`.
- [x] Run the `ToolResolver` object-safety/authority tests, timeout-bounded
  cancellation tests, and injected-context/provider-activity golden tests before
  the broad suites. Keep every channel-waiting async test bounded. These tests
  are part of the green kernel and product suites listed above.
- [x] Run `scripts/check-agent-kernel-deps.sh` for `tact_agent_api`,
  `tact_agent_core`, and `tact_llm`. Also run the three `cargo tree` commands and
  verify the forbidden-dependency table from the spec. All three trees were
  clean.
- [x] Wire the dependency guard into `.github/workflows/rust.yml` so it runs on
  every interface-changing phase and pull request, not as a one-off local check.
- [x] Invoke the same dependency guard from `scripts/check-rust.sh` and verify
  its forbidden-transitive fixture fails as expected before accepting a green
  local check.
- [x] Run `cargo fmt --all` and `./scripts/check-rust.sh` with the repository's
  proxy rules when preparing to push. Set
  `no_proxy=127.0.0.1,localhost,::1` alongside the configured proxy. The full
  check passed.
- [x] Update `ARCHITECTURE.md` and the relevant book chapter when the target
  behavior lands. `ARCHITECTURE.md` now describes the kernel, adapters, and
  host-event boundary.
- [x] Add a book/chapter note if the crate split changes a public import path.
  No public import path changed: `tact_llm` kept its existing root re-exports,
  and `tact::LoopState`, `AgentUpdate`, and `Agent` remain compatibility
  surfaces. No book HTML was edited.
- [x] Run `python3 scripts/book-check.py book` if any book HTML is touched. No
  book HTML was touched, so this check was not required.
- [x] Run `git diff --check`, then re-read the final diff for accidental edits to
  unrelated dirty or untracked files.

## Definition of Done

- `tact_agent_api` owns the neutral contracts and `tact_agent_core` owns the
  control loop with no product, provider, storage, MCP, or TUI dependency.
- `AgentKernel` retains no conversation state across calls; the facade state and
  one borrowed `AgentState` never expose concurrent writable authorities. The
  activation guard restores every sidecar field on success, cancellation, error,
  and unwind.
- Hooks, permission authority, UI traffic, stats, MCP hook services, and queues
  are product-owned shared handles; session/model metadata belongs to the
  read-only activation view. No hook receives `Agent`, `Arc<Mutex<Agent>>`, or
  a second writable transcript. Each logical agent's facade and gate see the
  same permission state; subagents own a separate session and snapshot-derived
  authority.
- The facade calls `ensure_session` and selects a path before state transfer; a
  resumed session with an empty runtime context therefore cannot skip SQLite
  history.
- Direct writes to `AgentRuntime.cancel_flag` reach one activation-scoped token,
  whose watcher stops with the guard; nested subagents use the same bridge for
  their own flags.
- Every model call, including local and native compaction, has exactly one
  terminal `ModelResponse` with complete message, stop reason, usage,
  diagnostics, and cursor update.
- Authoritative usage and request-body metadata are written through an awaited
  `UsageSink`, flushed before return, and do not depend on an event observer.
- `AgentState.last_token_total` follows provider `total_tokens`, matching legacy
  `usage.total`; input-only accounting is a parity failure.
- Auto, manual, recovery, command, and micro preparation use typed
  `Continue`/`Compact`/`Block` decisions; replacement is atomic, native
  cursor-only compaction retains the transcript, and skipped decisions do not
  loop.
- Hook-mutated calls are fully re-resolved and reclassified before permission or
  execution; failed results clear all terminal/compaction directives while
  retaining the operational error.
- `ToolGate::before` receives only `ToolResolver`; the kernel is the sole caller
  of `ToolPort::execute`, and denied, blocked, or cancelled calls never execute.
- Tool rewrite/re-resolution, typed status and original errors, ordered
  redacted progress, permission labels, and immediate `YieldToHost` behavior
  survive. Live, final, and hook-rewritten visible output is redacted. Image
  results remain companion `Image` blocks beside their `ToolResult`, never
  `[image]` placeholder text.
- `MaxTokens + ToolUse` persists paired results before continuation and produces
  structurally valid next requests; exhausted continuation returns `MaxTokens`.
- Goal completion fires only on `remaining_turns == Some(1)`, non-final
  proposals continue, and truncated responses cannot newly complete a Goal.
- Mid-run context is persisted before its request, assistant/cursor commits
  precede tools, results persist in source order, and every terminal path closes
  pending tool calls.
- A loaded durable orphan tool call is reconciled into one grouped interrupted
  result before any resumed request.
- MCP refresh, hierarchical `ResourceScope` conflicts, worktree subagent
  parallelism, absolute turn budgets, and Goal final-turn behavior are unchanged.
  MCP mutation is owned by one interior-synchronized catalog handle shared with
  whole-router replacement.
- Main-loop turn accounting counts dispatched attempts only; compaction and
  transport retries do not add turns, while context-overflow retry dispatches do.
- New sessions persist `MessageOrigin` in nullable `messages.origin`; legacy
  SQLite sessions infer NULL rows only from complete marker frames and exact
  continuation prompts, leaving genuine marker-like user text unchanged.
  Inference is deterministic, and later replacements persist explicit origins.
- Kernel usage records upsert by non-empty `call_id` through a partial unique
  `(session_id, call_id)` index; legacy NULL-call-id rows remain readable.
- Provider activity and message progress use closed neutral types; one assistant
  tool-use message is followed by one grouped result message, and provider
  activities with duplicate terminal events produce one terminal UI step.
- Injected context retains a non-`User` origin, hosted provider activity never
  fabricates a user turn or client tool call, and neither replaces activation
  inputs.
- Cancellation is idempotent, drains already-running work, retains committed
  transcript/cursor/usage state, and emits exactly one terminal run event with
  no post-terminal kernel events.
- Every mutable facade field has an explicit ownership mode; the activation
  guard preserves moved sidecar values and shared handles on success,
  cancellation, error, and unwind.
- `tact` remains the compatibility facade and composition root. Production
  activation is kernel-only; the legacy selector was deleted before this plan
  completed.
- `tact_llm` implements `ModelPort`, returns provider-neutral account DTOs, and
  never imports `tact_protocol` or emits `AgentUpdate`; `tact-ui` owns the
  mapping to `tact_protocol::biz`.
- `AgentUpdate` remains stable for hosts; kernel-bridged and adapter-owned
  traffic are each emitted exactly once.
- The legacy production loop and temporary model bridge were removed after the
  full parity gate.
- All ten F7 scenarios have executable kernel-path comparisons, including
  TUI full-area blank-background paint, before the legacy loop was removed.
- The kernel runs entirely with fakes and an in-memory transcript, the dependency
  guard detects direct and transitive forbidden edges through a tested negative
  fixture and runs from both `scripts/check-rust.sh` and CI, and all existing
  plus new tests pass.
