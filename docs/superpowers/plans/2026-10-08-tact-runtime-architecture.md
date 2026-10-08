# Tact Runtime Architecture Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans for sequential implementation, or superpowers:subagent-driven-development only if the user selects delegation. Checkboxes track deliverables, not authorization to commit or push.

**Goal:** Rebuild Tact into independently developable owner crates while preserving existing package, Hook, protocol, provider and session contracts.
**Architecture:** Capture compatibility, establish shared contracts and ownership, then move production implementations into seven owner crates. Internal module extraction is transitional; the project ends only when the dependency graph and collaboration boundaries in spec §8 are achieved.
**Tech Stack:** Existing Rust 2024 workspace, Tokio, Serde, async-trait, tact_llm, tact_protocol, rmcp and sqlx. No new dependency is required for the module stage.
**Spec:** [Tact runtime architecture](../specs/2026-10-08-tact-runtime-architecture-design.md)
**Status:** In progress on `tact-architecture`. Tasks 1, 2 and the first 9.1/9.2 migration slices are implemented and verified; Tasks 3–8 and the remaining 9.1–9.7 milestones remain.

## Global Constraints

- Preserve current Codex plugin-package support, including marketplace operations and contributed Hooks/MCP/Skills. Only new dynamic plugin ABIs are excluded.
- Preserve existing package names, public Agent/AgentRuntime fields and constructors, legacy Hook signatures, Tool trait and proc-macro paths.
- Keep tui and agent_tui_kit dependent on protocol, not on tact.
- Keep SessionStore and existing transactions/schema; do not create redundant repository abstractions.
- Run Cargo check/build/test/clippy serially. New channel waits use tokio::time::timeout.
- Use the current native/MCP policy and Hook baseline. The final-input security correction is a distinct behavior change in Task 3.
- Hosted provider tools never enter local tool execution, permissions or the scheduler.
- Keep command/query/plugin worker routes and all AgentUpdate payloads supported throughout migration.
- Preserve unrelated changes. Do not commit, push, change package versions or install new runtimes without a user request.

## Review Focus

1. Existing package load/reload and Hook trust must still work after adapters move (Tasks 1, 8).
2. Hook mutation/Allow, stale approvals and original-vs-final resources must remain safe and predictable (Tasks 1, 3, 6).
3. Cancelled prompts, late child/background events and observer lag must not strand the host (Tasks 4, 7).
4. Hosted tool replay and partial persistence must not trigger duplicate local effects (Tasks 5, 7).
5. Public Agent fields, Hook closures and subagent constructors must not force runtime/facade cycles (Tasks 1, 2, 9).

## Execution and verification rules

Execution order is Tasks 1–2, Task 9.1 (the early feasibility gate), Tasks 3–8, Tasks 9.2–9.7, then Task 10. Task numbering groups the physical rebuild milestones; it does not postpone the borrowing proof until after the module migration. Internal modules cannot satisfy the final delivery criteria. Do not stop at a dependency blocker and report the rebuild complete: resolve it through the contracts and resumable execution model in spec §8.3.

Milestone 9.1 establishes the interfaces used by the independent owner lanes. Session and permission work can then be developed independently; tool, MCP and plugin work consumes those contracts. Runtime integration joins the lanes after owner gates pass. Default execution here remains sequential; team ownership is not authorization to delegate to agents.

The complete migration is larger than one reviewable diff. Each milestone must leave the application working through compatibility re-exports/adapters and have its own rollback point. Keep wrappers until consumers move; never run old and new production execution paths simultaneously. No commit is made without a user request.

For each new regression, first create and register its real test module, list its tests, then run the named test. A missing module or unresolved symbol is not a useful behavioral red test. Zero selected tests is a failed gate, even if Cargo exits successfully. Inspect the test count and assertion; do not infer success from command exit alone. Existing baseline tests must pass before moving their owners. A security regression may fail before Task 3 and must pass afterward.

No new test framework or snapshot dependency is needed: use explicit field assertions and serde_json values for externally serialized fixtures. Do not compare Debug output as a stable protocol. The plan provides exact test cases and existing harnesses; executors write tests directly with these objects instead of invented fixture runners.

All listed validation commands are future implementation gates. This documentation-only revision does not run Cargo.

## A. Ownership and interface ledger

### A1. Existing contracts retained

- `Agent::agent_loop(&mut self, Option<tact_llm::Message>) -> anyhow::Result<()>`.
- `Agent::execute_tool_call(&mut self, &[tact_llm::ContentBlock]) -> anyhow::Result<(Vec<tact_llm::ContentBlock>, Option<String>)>`.
- `Tool::call(&self, ToolContext, serde_json::Value) -> anyhow::Result<ToolCallResult>` and metadata/schema methods.
- `DynSessionStore` and the existing SessionStore trait, especially `replace_session_messages_and_provider_state`.
- `LoopState = Agent`, existing Hook variants and builder callbacks.
- `UiResponder::respond(UiResponse) -> bool`, pending request IDs, withdrawal and shutdown.
- `UserCommand`, `AgentUpdate`, and the separate `PluginRequest` worker protocol.

A submitted run uses structured Message, not a new text-only Command type that loses image or provider content. Driver-level commands continue through UserCommand. A completed model turn does not emit TaskComplete.

### A2. New module-stage contracts

Create these in Task 2; keep them pub(crate) until a public API is deliberately needed:

```rust
// runtime/contracts.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RunId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunOutcome { Completed, Cancelled }

pub(crate) struct RunProgress {
    pub id: RunId,
    pub model_turns: u32,
}
// No conversation, provider baseline or permission-rule copies here.

// capability/mod.rs: use existing value types through local imports.
use crate::agent::tool_schedule::ToolResources;
use crate::tool::ToolCallResult;
pub(crate) struct ToolInvocation {
    pub tool_id: String,
    pub name: String,
    pub input: serde_json::Value,
}
pub(crate) enum ToolOrigin {
    Native,
    Mcp { server: String },
    McpResource,
    McpPrompt,
}
pub(crate) struct ToolDescriptor {
    pub name: String,
    pub origin: ToolOrigin,
    pub schema: serde_json::Value,
    pub description: String,
}
#[async_trait::async_trait]
pub(crate) trait ToolExecutor: Send {
    fn describe(&self, name: &str) -> Option<ToolDescriptor>;
    fn resources(&self, call: &ToolInvocation) -> anyhow::Result<ToolResources>;
    async fn execute(&mut self, call: &ToolInvocation)
        -> anyhow::Result<ToolCallResult>;
}
```

ToolResources is the current scheduler value type; ToolCallResult is the current tool result type. Preserve richer internal execution output in an adapter where needed; do not discard images, presentation, spill references or permission labels to fit this trait. Only backend invocation belongs to ToolExecutor; preflight, Hooks, approval, redaction, scheduling and result assembly remain the runtime batch contract. Parallel execution retains the existing wave runner: this trait must not serialize a whole batch merely because execute takes &mut self. The legacy adapter is borrowed at dispatch boundaries; per-wave executable handles retain current ownership/concurrency.

Descriptors are lookups of existing registrations, not a second mutable registry. Connection readiness and generation remain MCP router state. Rebuild exposed specs at existing refresh boundaries; do not freeze descriptor references across reload.

### A3. Extraction seams

| Seam | Module-stage implementation | Gate before physical move |
|---|---|---|
| TurnCoordinator | `async fn run(agent: &mut Agent, input: Option<Message>) -> Result<RunOutcome>`, internal only | Replace concrete Agent dependency with borrowed state/ports; facade keeps public fields; engine suspends before legacy callbacks |
| Tool batch | `async fn execute_batch(agent: &mut Agent, content: &[ContentBlock]) -> Result<(Vec<ContentBlock>, Option<String>)>` | Port-based preflight/execution callbacks, no imported facade |
| HookRunner | Facade-side, invokes current callbacks with &Agent | Built-in package callbacks use owned invocation data; legacy callbacks stay outside extracted runtime |
| Model | Current LlmProvider/MockClient | Reuse current provider interface; do not invent a second provider protocol |
| Store | Current DynSessionStore | Existing async store contract retained |
| Subagent | Existing tool constructs Agent | Outer factory/adapter owns Agent creation, runtime has no concrete subagent-tool dependency |
| Projection | Borrow/classify update, then move original update to host | UI-dependent presentation remains outside pure domain crate |

The temporary Agent parameters are explicit module-stage debt, not a claim of an independent runtime crate. Do not use mem::take to remove state while legacy Hooks can inspect Agent. Borrow the state for one awaited operation, release it, then invoke facade callbacks; no nested runtime locks across callbacks.

## Task 1: Freeze the compatibility baseline before extraction

**Files:** Create `crates/tact-ui/tests/runtime_architecture_contract.rs`; extend `crates/tact-ui/tests/{plugin_cli_tests,mcp_tools,permission_integration,driver_integration}.rs`; create `crates/tact/tests/public_api_compat.rs`; extend existing inline tests in `crates/tact/src/plugin/{install,marketplace,hooks}.rs`.

**Consumes:** Current production types. **Produces:** A baseline contract suite and source-compatibility fixture; no new runtime abstraction.

- [ ] Add an exhaustive match over every AgentUpdate and UserCommand variant in the integration fixture (no catch-all). Use the complete tables in spec §5. Preserve separate PluginRequest coverage.
- [ ] Pin cancellation without TaskComplete; multi-select response IDs; Hook status lifecycle; tasks/metadata; background/subagent completion after parent run end; model/thinking/usage payloads.
- [ ] Reuse `tact_ui::test_support::{build_test_agent, build_test_agent_with_mode, build_test_agent_with_mcp, build_test_agent_with_session}`, `tact_llm::MockClient`, `tact::mcp::MockMcpService` and existing integration harnesses. Keep runtime unit helpers inside tact; never add a dependency from tact to tact-ui.
- [ ] Extend current package fixtures for .codex-plugin/plugin.json, .codex-plugin/marketplace.json, relative marketplace paths, declared MCP files, inline/file Hooks, contributed Skills, update/uninstall/reload and trust hashes. Reuse temporary local repositories; no network marketplace or real credentials.
- [ ] Add a trace Vec<String> shared by recording test callbacks and tool executor; capture pre-Hook Allow/Continue/Block/error, PermissionRequest branches, Notification, post-Hook status/failure and input mutation. Assert baseline first, including current pre-Hook risk calculation.
- [ ] Create compile-only usage coverage in public_api_compat.rs for Agent::new/with_* methods, public runtime fields, LoopState callback types, custom Tool implementation, ToolSpec, protocol types and the existing tool proc macro. Copy working constructor/Tool examples from current unit tests, not a replacement API.
- [ ] Register new tests with the `architecture_` prefix and list before running. Each new await on receive or task join has a five-second timeout; explicit slow process tests may choose a named larger budget.

Concrete mock setup usable inside the new host integration test:

```rust
use tact_llm::{ContentBlock, MockClient, StopReason};
use tact_ui::test_support::build_test_agent;
use std::time::Duration;

#[tokio::test]
async fn architecture_mock_run_has_bounded_completion() {
    let mock = MockClient::new(vec![(
        vec![ContentBlock::Text { text: "done".into() }],
        Some(StopReason::EndTurn),
    )]);
    let (mut agent, _) = build_test_agent(mock, None);
    tokio::time::timeout(Duration::from_secs(5), agent.agent_loop(None))
        .await.expect("run timed out").expect("run failed");
}
```

`StopReason::EndTurn` is the existing terminal variant in `crates/tact_llm/src/types.rs`. Register this fixture as a passing smoke baseline; it is not itself evidence of cancellation, tool execution or persistence correctness, which require the cases listed above.

**Commands, serially:**
```sh
cargo test -p tact-ui --test runtime_architecture_contract -- --list
cargo test -p tact-ui --test runtime_architecture_contract
cargo test -p tact --test public_api_compat
cargo test -p tact --lib plugin::
cargo test -p tact-ui --test plugin_cli_tests
cargo test -p tact-ui --test permission_integration
```
**Exit:** Each new named test listed; baseline passes. Capture actual public/plugin inventory in this plan's execution notes before structural work.

## Task 2: Define ownership and build borrowed module seams

**Files:** Create `crates/tact/src/runtime/{mod,contracts}.rs`, `crates/tact/src/capability/mod.rs`; modify `crates/tact/src/lib.rs`, `agent/tool_schedule.rs`, `tool/mod.rs`; keep old module re-exports.

**Consumes:** A1 baseline. **Produces:** A2 declarations and A3 seams without moving execution behavior.

- [ ] Add RunId, RunProgress and RunOutcome; one in-memory allocator per process, no new database column. Keep all mutable conversation/policy/provider state in AgentRuntime.
- [ ] Add ToolInvocation/ToolOrigin/ToolDescriptor and ToolExecutor imports using existing ToolResources/ToolCallResult. Do not add StartupState snapshots or duplicate permission policies.
- [ ] Introduce one borrowed Native/MCP backend adapter around current routers/context. Describe exact current schemas/names. Use existing mcp_server_resources and resource/prompt route matching.
- [ ] Add architecture_descriptor_preserves_names, architecture_resource_identity and architecture_no_duplicate_context tests. Compare Native and MCP names, same-server keys, barrier behavior and pointer/owner identity where applicable.
- [ ] Enumerate each direct Agent dependency in hook/mod.rs, ToolContext, subagent tool and proc macro. Record which remains facade-side. Preserve legacy paths via aliases/re-exports; avoid exposing new ports before they settle.

**Commands:**
```sh
cargo test -p tact --lib capability:: -- --list
cargo test -p tact --lib capability::
cargo test -p tact --lib runtime::contracts::
cargo test -p tact --test public_api_compat
```
**Exit:** New interfaces compile without changing public struct layouts or creating workspace packages.

## Task 3: Correct final-input validation as a separate behavior change

**Files:** Modify `crates/tact/src/agent/tool_dispatch.rs`; add inline tests there; use existing `permission/mod.rs`, `security/sensitive.rs`, `tool/metadata.rs`; extend `crates/tact-ui/tests/permission_integration.rs`.

**Consumes:** Task 1 baseline and existing policy. **Produces:** Final-input security contract from spec §6.2, before structural extraction.

- [ ] Add failing behavioral tests: PreToolUse changes safe read path to Credential path and returns Allow; PermissionRequest changes approved path; Hook changes executable arguments from read to write; Hook attempts changing tool name. Use local fixture paths recognized by the existing scanner, not real secret files.
- [ ] Assert denied targets never reach the recording executor. Unchanged-input Allow remains as baseline. Changed PermissionRequest input cannot inherit permission for old arguments.
- [ ] After each mutating control Hook, compare final name/input against original resolved call. Reject name mutation. Re-run sensitive scan and risk/resource derivation on changed input. Credential deny has precedence over Hook Allow.
- [ ] For changed PermissionRequest input, call check_with_auto on final input without recursively invoking PermissionRequest; dispatch allow/deny or request a new user decision. Keep notification, scope, labels and denial-counter handling from the existing path.
- [ ] Preserve provider replay of the original model call; execution metadata records the actual effective arguments. Do not mutate the stored provider call JSON to pretend the model emitted Hook-modified input.
- [ ] Mark this task as the intentional behavior delta in execution notes; include the issue-book entry at documentation synchronization.

**Commands:**
```sh
cargo test -p tact --lib agent::tool_dispatch:: -- --list
cargo test -p tact --lib agent::tool_dispatch::
cargo test -p tact --lib permission::
cargo test -p tact --lib plugin::hooks::
cargo test -p tact-ui --test permission_integration
```
**Exit:** New security regressions fail before the correction and pass afterward; unchanged-input baseline remains green.

## Task 4: Add ordered publication without losing host protocol

**Files:** Create `crates/tact/src/runtime/events.rs`, `crates/tact/src/projection/{mod,agent_update}.rs`; modify `agent/mod.rs`, `lib.rs`; extend `runtime_architecture_contract.rs`; inspect `crates/tact/src/{ui_responder,subagent,background}.rs` and `tool/{progress,subagent_ui}.rs` for producer routes.

**Consumes:** Spec §5 inventory and RunId. **Produces:** One update bridge per run and optional observation API.

- [ ] Implement `publish(&mut self, update: AgentUpdate) -> ()` on a crate-private RuntimePublisher owning the existing optional host sender, RunId and next sequence. Classify by borrowing update, then forward the original owned update once.
- [ ] Define observer records as RunId + u64 sequence + an owned observation of the migrated fact. Presentation-only entries remain passthrough. Do not claim the partial observation enum replaces all AgentUpdate data. Full domain conversion requires lossless field assertions before switching that variant's producer.
- [ ] Migrate Agent::emit_update first; then enumerate direct senders and assign exactly one path to each. Preserve legacy Step order, including visible preflight failures. Keep background/child identity rather than assigning late completions to the newest run.
- [ ] Keep the host channel's existing queue semantics. Optional observers use bounded queues and explicit gap notification/snapshot recovery. Slow observers cannot await inside tool execution or act as approval handlers.
- [ ] Add tests for sequence/order, exactly one host forwarding, no double completion, late child result, observer overflow/disconnect and host shutdown with pending select/multi-select. Reuse UiResponder's response/withdraw/shutdown behavior.
- [ ] Keep command handling in driver.rs exhaustive. Do not make all commands submit a task; query/compact/settings/auth/trust routes remain separate.

**Commands:**
```sh
cargo test -p tact --lib projection:: -- --list
cargo test -p tact --lib projection::
cargo test -p tact --lib runtime::events::
cargo test -p tact --lib ui_responder::
cargo test -p tact-ui --test runtime_architecture_contract
cargo test -p tact-ui --test driver_integration
cargo test -p tact-ui --test headless_tui_advanced
```
**Exit:** All inventory rows explicitly convert or pass through; data/terminal behavior unchanged.

## Task 5: Preserve backend and hosted-operation semantics

**Files:** Create `crates/tact/src/capability/{native,mcp}.rs`; modify `capability/mod.rs`, `agent/tool_dispatch.rs`; extend tests in `agent/tool_schedule.rs`, `mcp/{mod,resource,prompt}.rs`, `crates/tact-ui/tests/mcp_tools.rs`, `crates/tact_llm/src/openai/responses/{capabilities,wire,stream}.rs`.

**Consumes:** Task 2 borrowed adapters and Task 4 publisher. **Produces:** Native/MCP lookup and backend execution with preserved batch semantics.

- [ ] Adapt existing ToolRouter lookup and MCPToolRouter calls. Preserve Native metadata and MCP names with embedded double underscores; resources/prompts use existing dedicated routes, not invented server-tool names.
- [ ] Reuse current MCP risk/approval policy, timeouts, output spill, remote auth and list-change refresh. A resolved handle is scoped to the batch; reload cannot retain a stale advertised descriptor indefinitely.
- [ ] Scheduling tests exercise actual concurrent execution: two blocked calls on the same mock server never overlap; different servers may overlap; barriers serialize; path conflicts order; provider results follow original call order. Use timeout-bounded release signals, not sleeps as proof.
- [ ] Test cancellation during a wave and before execution; approved-but-not-run calls receive correctly positioned cancelled results. Preserve actual resource keys after Task 3 mutations.
- [ ] Add hosted-operation assertions alongside existing Responses fixtures: ordinary vs compact request injection, function/hosted name coexistence, first-done deduplication, failed/nonterminal statuses, raw query-array replay preservation. Assert local executor call count remains zero for hosted events.
- [ ] Do not advertise Process/WASM origins with no executor or impose a local sandbox on remote execution.

**Commands (each selects a different owner):**
```sh
cargo test -p tact --lib capability:: -- --list
cargo test -p tact --lib capability::
cargo test -p tact --lib agent::tool_schedule::
cargo test -p tact --lib mcp::
cargo test -p tact-ui --test mcp_tools
cargo test -p tact_llm --lib openai::responses::
```
**Exit:** Real scheduling assertions pass, not merely equality of two metadata values.

## Task 6: Extract the batch runtime and awaited Hook orchestration

**Files:** Create `crates/tact/src/runtime/{tool_execution,hook_runner}.rs`; modify `runtime/mod.rs`, `agent/tool_dispatch.rs`, `hook/mod.rs`; extend inline runtime tests and `runtime_architecture_contract.rs`.

**Consumes:** Tasks 1–5. **Produces:** A3 execute_batch function with Agent::execute_tool_call retaining A1 signature.

- [ ] Move orchestration by phases: sequential preflight, existing conflict-wave runner, original-order result assembly. Preserve batch cancellation and partial results; do not replace the batch with a loop awaiting each ToolExecutor.
- [ ] Keep HookRunner facade-facing and await every control callback. Preserve PreToolUse Allow/Block/error, PermissionRequest/Notification, Rust post-Hook vs package success filtering, PostToolUseFailure and Hook output/context suppression.
- [ ] Preserve result redaction before Hook/UI/transcript observation, image blocks and output spilling. Copy no raw secret results into observer records.
- [ ] Delegate Agent::execute_tool_call to execute_batch without duplicate event publication. Keep existing dispatch module paths as wrappers/re-exports where tests or public users require them.
- [ ] Add architecture_batch_order, architecture_permission_hook_trace, architecture_post_failure_trace and architecture_partial_cancel tests directly in runtime/tool_execution.rs, reusing existing mock-tool construction. Trace both internal Rust callbacks and external package callback visibility.

**Commands:**
```sh
cargo test -p tact --lib runtime::tool_execution:: -- --list
cargo test -p tact --lib runtime::tool_execution::
cargo test -p tact --lib runtime::hook_runner::
cargo test -p tact --lib agent::tool_dispatch::
cargo test -p tact --lib plugin::hooks::
cargo test -p tact-ui --test runtime_architecture_contract
cargo test -p tact-ui --test tool_integration
```
**Exit:** New runtime tests actually run; baseline differs only by the documented Task 3 correction.

## Task 7: Extract run coordination and verify durable recovery

**Files:** Create `crates/tact/src/runtime/{turn_coordinator,context_manager}.rs`; modify `agent/mod.rs`, `runtime/mod.rs`, `compact/mod.rs`, `recovery.rs`; extend `store/session_store/sqlite.rs` tests; modify `crates/tact-ui/src/driver.rs` only for explicit completion ownership; extend `crates/tact-ui/tests/{recovery_compaction,headless_session_integration,runtime_architecture_contract}.rs`.

**Consumes:** A1 provider/store APIs, A3 coordinator seam, batch executor and publisher.
**Produces:** Whole-run coordinator and preserved Agent facade; no new database interface.

- [ ] Start with the existing MockClient and LlmProvider; extract prompt/context operations using the existing Message types and provider baseline. Preserve compaction-before-user-message, incoming-turn reserve, hook context and pending child-result ordering.
- [ ] Move the loop into TurnCoordinator::run with A3 signature. Increment model_turns only for existing counted provider turns. Return Completed/Cancelled; propagate actual failure as anyhow::Error. Agent::agent_loop preserves its old Result<()> interface; driver still owns TaskComplete/TaskCancelled publication.
- [ ] Keep SessionStart lazy dispatch, resume/compact sources, Interrupt once-per-run, Stop continuations, max-turn caps and subagent wake-up deduplication.
- [ ] Keep AgentRuntime fields authoritative. Use short borrows between hooks/provider/store operations; legacy Hook callbacks always observe real state.
- [ ] Add a test-only SessionStore delegator implementing all existing trait methods by forwarding to an in-memory SqliteSessionStore, with a failure switch on append_message after tool execution. Use actual trait signatures from store/session_store/mod.rs; add no product recovery trait.
- [ ] Assert executor counter stays at one when result persistence fails; an error is reported, automatic model/tool continuation stops, raw provider baseline remains inspectable, and resume does not automatically replay the unresolved side effect. If baseline currently violates this, isolate the fix and its book issue entry from the coordinator move.
- [ ] Cover compaction's atomic messages/provider-state replacement, transport recovery, session lock/epoch, cancellation then fresh task, and shutdown with pending approvals. Keep session approval rules in memory.
- [ ] Add runtime tests inside tact using MockClient and existing local Agent construction from agent tests. Do not import tact-ui test helpers into tact.

**Commands:**
```sh
cargo test -p tact --lib runtime:: -- --list
cargo test -p tact --lib runtime::
cargo test -p tact --lib agent::
cargo test -p tact --lib store::session_store::
cargo test -p tact-ui --test recovery_compaction
cargo test -p tact-ui --test headless_session_integration
cargo test -p tact-ui --test runtime_architecture_contract
```
**Exit:** Runs complete/cancel once, facade state remains valid, interrupted persistence cannot cause blind replay.

## Task 8: Consolidate compatibility adapters and close the module stage

**Files:** Create `crates/tact/src/compat/{mod,packages,hooks,skills,config}.rs`; modify `lib.rs`, `plugin/{mod,install,marketplace,hooks,model,store}.rs`, `skill/mod.rs`; preserve existing public paths; adjust `crates/tact-ui/src/{session_bootstrap,hooks_cli,mcp_cli}.rs` only at adapter entry points.

**Consumes:** Task 1 fixtures and stable module seams.
**Produces:** Current external contracts behind explicit compatibility entry points.

- [ ] Keep manifest/marketplace parsing, source precedence, path resolution, installed store, update/uninstall/reload and Hook trust behavior under packages adapter; preserve public plugin functions as re-exports/wrappers.
- [ ] Migrate built-in package Hook implementation to owned invocation data produced by facade-side HookRunner: existing JSON payload, work directory, source/trust identity and current request ID as needed. Reuse current Hook parser/result types. Legacy &Agent callbacks remain in tact.
- [ ] Preserve contributed MCP/Skills/Hook merge order and reload cleanup; adapter tests load actual temporary manifest files, including missing declared files and untrusted hooks.
- [ ] Re-run full command/update inventory and public compile fixture. Verify proc-macro output still resolves old paths and public field users compile.
- [ ] Audit new runtime imports: list every remaining crate::Agent/concrete router/manager dependency and classify it as a temporary facade seam. Do not claim package independence while these remain.

**Commands:**
```sh
cargo test -p tact --lib compat:: -- --list
cargo test -p tact --lib compat::
cargo test -p tact --lib plugin::
cargo test -p tact --lib skill::
cargo test -p tact --test public_api_compat
cargo test -p tact-ui --test plugin_cli_tests
cargo test -p tact-ui --test mcp_tools
cargo test -p tact-ui --test runtime_architecture_contract
```
**Exit:** Module stage is usable with no broken plugin, API or host contract.

## Task 9: Required physical rebuild

The package table and dependency graph in spec §8 are the target, not a menu. Task 9.1 runs immediately after Tasks 1–2 to establish feasibility. Tasks 3–8 then provide passing behavioral seams; milestones 9.2–9.7 remove concrete facade dependencies and move real implementations. New packages inherit workspace version/edition/license and existing library dependency versions. New crate creation includes production code and tests in the same deliverable.

### Task 9.1: Establish shared contracts and prove the legacy callback boundary

**Files:** Create `crates/tact-contracts/{Cargo.toml,src/lib.rs,src/tool.rs,src/hook.rs,src/host.rs,src/services.rs}` and `crates/tact-runtime/{Cargo.toml,src/lib.rs,src/run.rs}`; modify root `Cargo.toml`, `crates/tact/Cargo.toml`, `crates/tact/src/{agent/mod.rs,hook/mod.rs,tool/mod.rs,tool/metadata.rs,agent/tool_schedule.rs}`. Add `crates/tact/tests/runtime_boundary.rs` and runtime inline tests.

**Consumes:** Tasks 1–2 only: existing ToolResources, ToolCallResult, Hook controls/payloads, provider and protocol types. **Produces:** Shared value types/service interfaces with facade re-exports; a resumable engine boundary exercised by the existing Agent facade.

- [ ] Move shared value definitions with their current fields and derives; preserve public old paths with explicit re-exports. Keep serialization unchanged. Move policy metadata below both tools and permission. Split patch target extraction from executable apply-patch behavior so policy does not import native tools.
- [ ] Define the owned effect/reply pairs needed by the vertical slice in runtime/run.rs. Use distinct typed cases and request tokens; reuse current model/Hook/result types. Start with one model response, one legacy PreToolUse callback, one tool execution and one completion. Document the exact concrete signatures in this ledger with their definitions before owner-lane implementation consumes them.
- [ ] Make advance operate on scoped mutable state references and return before facade callbacks. The facade awaits each external effect with no outstanding mutable state borrow, then resumes the engine. Extend this same mechanism to the remaining loop suspension points during 9.7; do not introduce a second independent loop.
- [ ] Add a facade integration fixture whose legacy Hook reads the actual conversation and provider state across a bounded await. Mutate state through the engine before invocation and assert the Hook sees the mutation. Assert cancellation while waiting releases the prompt and a duplicate/wrong-kind effect reply cannot advance state.
- [ ] Add runtime unit tests using only contracts/provider/store/policy dependencies. Runtime tests must not import tact via dev-dependencies. No unsafe pointers, copied authoritative context, mem::take of callback-visible state or locks held across callbacks.
- [ ] Freeze tool backend preparation/execution and service contracts against actual native/MCP result fields. Prepared handles must support concurrent execution without one &mut backend borrow serializing all calls. Preserve generation checks and complete results.

**Gate:** The vertical slice and public API fixture compile and pass before full extraction. This is a required feasibility proof, not a claim that the current design has already compiled.

```sh
cargo test -p tact --test runtime_boundary
cargo test -p tact --test public_api_compat
cargo test -p tact-runtime --lib
```

### Task 9.2: Give persistence its own owner

**Files:** Create `crates/tact-session/Cargo.toml`, `src/lib.rs`, `src/sqlite.rs`, and `src/{session,background,subagent,task,team,worktree}/` under that crate. Move implementations from `crates/tact/src/store/`; modify its facade exports and manifests.

**Consumes:** Existing SessionStore and related store contracts, provider/protocol values. **Produces:** The same stores and database initialization under tact_session; old tact::store paths remain usable.

- [ ] Move all shared database initialization/migrations and related store implementations together. Move the small required lock helper locally; do not depend on tact::utils.
- [ ] Preserve filenames, schemas, migration numbering, connection setup, session locks, epochs, transaction boundaries, message row IDs and raw provider state. Plugin installation registry remains in plugins, not the session database.
- [ ] Move store tests with the implementations. Test opening an existing fixture database, session lock contention, compact replacement atomicity and injected write failure; reuse Tasks 1/7 fixtures.
- [ ] Wire the facade to the new DynSessionStore and related store exports. Run old API and host session tests against the moved implementation; no duplicate SQLite pipeline.

```sh
cargo test -p tact-session --lib
cargo test -p tact --test public_api_compat
cargo test -p tact-ui --test headless_session_integration
cargo test -p tact-ui --test recovery_compaction
```

### Task 9.3: Isolate permission, redaction and sandbox policy

**Files:** Create `crates/tact-permission/{Cargo.toml,src/lib.rs,src/policy.rs,src/settings.rs,src/sensitive.rs,src/redact.rs}`; move existing sandbox subtree beneath `src/sandbox/`. Modify `crates/tact/src/{permission,security,sandbox}` facade exports and manifests.

**Consumes:** Shared policy metadata from contracts, resolved configuration values. **Produces:** Existing PermissionManager, scanner, redactor and sandbox behavior independently of native tools and Agent.

- [ ] Move current policy precedence intact, including session grants, deny counters and server policy. Root config parsing stays in composition and supplies the existing typed settings.
- [ ] Move target-path analysis with policy; leave filesystem mutation with tools. Preserve platform-specific sandbox configuration and subprocess wrapping. Host prompting remains an injected service, not a permission-crate UI dependency.
- [ ] Move policy/redaction/sandbox tests and add contracts proving Read/Plan/Auto/settings/session precedence, unchanged-input Hook Allow handling at the runtime boundary, and ephemeral grant lifetime.
- [ ] Re-export legacy security and sandbox paths. Preserve Task 3's separately recorded correction without folding additional behavior changes into this move.

```sh
cargo test -p tact-permission --lib
cargo test -p tact --test public_api_compat
cargo test -p tact-ui --test permission_integration
```

### Task 9.4: Separate plugin packages from live MCP services

**Files:** Create `crates/tact-plugins/{Cargo.toml,src/lib.rs}` and `src/{package,marketplace,install,trust,hooks,skills}/`; move current plugin/skill implementations and package-specific Hook translation. Modify `crates/tact/src/{plugin,skill,hook}`, configuration/paths adapters and `crates/tact-ui/src/session_bootstrap.rs`.

**Consumes:** Shared Hook contracts and explicit package roots. **Produces:** Existing package operations plus resolved contributions consumed by composition; no live MCP router.

- [ ] Keep package/marketplace manifests and installed registry formats unchanged. Preserve source precedence, missing-file failures, trust hashes and reload cleanup.
- [ ] Return source-labelled MCP/Hook/Skill contributions using shared data; bootstrap merges and passes them to the relevant owner. Package parsers must not construct MCP clients.
- [ ] Move external Hook process/JSON translation into plugins; keep &Agent Rust callback adapters in tact. Feed both through the same runtime Hook ordering and result handling.
- [ ] Move fixture tests with their code. Exercise install/update/uninstall/reload, inline/file declarations, contributed skills and untrusted Hooks using temporary local sources.

```sh
cargo test -p tact-plugins --lib
cargo test -p tact-ui --test plugin_cli_tests
cargo test -p tact --test public_api_compat
```

### Task 9.5: Extract MCP with configuration supplied by composition

**Files:** Create `crates/tact-mcp/{Cargo.toml,src/lib.rs,src/config.rs,src/client.rs,src/router.rs}` and move current `mcp/{remote,resource,prompt,edit}` modules. Modify `crates/tact/src/mcp/` exports, bootstrap and MCP CLI wiring.

**Consumes:** Resolved contributions from composition, contracts and permission metadata. **Produces:** MCP lifecycle/router/transport APIs independent of installed plugin storage.

- [ ] Remove current imports of PluginStore/PluginRoot/PluginHome from live router logic. Root config and installed package discovery occur in composition; MCP retains transport-specific parsing/validation.
- [ ] Move real connection management, OAuth, dynamic tool-list refresh, resources/prompts and shutdown. Preserve mock service support without depending on tact test helpers.
- [ ] Preserve per-server resource keys, qualified names, generation invalidation, timeout/error translation and output limits. Same-name native and hosted tools remain distinct.
- [ ] Move unit tests and reuse Task 5 concurrency tests; assert same-server calls serialize, different servers overlap and reload invalidates stale preparation at the specified boundary.

```sh
cargo test -p tact-mcp --lib
cargo test -p tact-ui --test mcp_tools
cargo test -p tact --test public_api_compat
```

### Task 9.6: Move native tools behind narrow services

**Files:** Create `crates/tact-tools/{Cargo.toml,src/lib.rs,src/registry.rs}` and move existing native tool implementations from `crates/tact/src/tool/`; add composition-side service adapters under `crates/tact/src/services/`. Modify legacy ToolContext adapters, subagent construction and proc-macro integration fixtures.

**Consumes:** Contracts, permission/sandbox facilities and injected service handles. **Produces:** All native tool implementations without a tact/runtime/MCP/plugins dependency.

- [ ] Move direct filesystem/process tools first with current behavior. Preserve metadata, schema, output spilling, image content and progress events.
- [ ] Replace concrete managers in new internal execution context with narrow skill/task/team/background/worktree/subagent/compact/approval service handles. Preserve existing ToolContext and Tool::call signatures through facade adapters for external users.
- [ ] Move subagent tool request/result behavior, but keep Agent construction in the composition-side subagent service. Likewise, compact tool calls a service rather than importing the coordinator implementation.
- [ ] Keep proc-macro-generated public paths working. Do not make tact-tools depend on a macro expansion that imports tact; adapt the macro path selection or retain the legacy macro only at the facade and use internal implementations directly.
- [ ] Add a native tool fixture with fake services that builds without Agent. Verify service forwarding, cancellation, permissions snapshot and result redaction. Move existing tool tests to their owner or keep only facade compatibility tests in tact.

```sh
cargo test -p tact-tools --lib
cargo test -p tact --test public_api_compat
cargo test -p tact-ui --test tool_integration
```

### Task 9.7: Complete the runtime and reduce Agent to its facade

**Files:** Populate `crates/tact-runtime/src/{model_turn,tool_batch,hook_flow,approval,context,compact,recovery,events}.rs`; modify runtime/run.rs, `crates/tact/src/{agent,runtime,compact,recovery.rs}`, composition adapters and host driver wiring. Extend runtime_boundary.rs and host architecture fixtures.

**Consumes:** Passing Tasks 1–8 and 9.1–9.6, existing LlmProvider, moved stores/policy, independent backend contracts. **Produces:** Production run and tool-batch algorithms in tact-runtime; Agent methods delegate.

- [ ] Extend the proven resumable engine to all awaited Hook points, Stop continuation, compaction, provider recovery, tool waves and approval cancellation. Use typed effects only where required to release facade borrows; ordinary internal operations remain normal functions/futures.
- [ ] Move actual context/compaction/recovery algorithms and batch orchestration from the module stage. Keep one state owner and one pending-effect authority. Driver retains existing terminal publication ownership; no duplicate TaskComplete/TaskCancelled.
- [ ] Wire concrete implementations in tact composition. Remove old duplicated algorithms after each production path delegates. Agent wrappers may dispatch effects and expose compatibility fields but cannot decide policy, schedule waves or implement a parallel loop.
- [ ] Run runtime tests with fake backends and services; run the public facade fixtures against those same algorithms. Cover real legacy callbacks across await, partial-wave cancel, Stop continuation, pending approval shutdown, late children, compact baseline and uncertain side-effect recovery.
- [ ] Audit Cargo dependencies including dev/target-specific edges. Enforce the spec §8 graph with a metadata-based dependency check; runtime and leaf crate tests cannot depend on tact to construct fixtures.
- [ ] Demonstrate collaboration boundaries with three scoped fixtures: adding a native tool, handling an MCP refresh and accepting/rejecting a package field. Their implementations must not edit agent/mod.rs or runtime sequencing.

```sh
cargo test -p tact-runtime --lib
cargo test -p tact --test runtime_boundary
cargo test -p tact --test public_api_compat
cargo test -p tact-ui --test runtime_architecture_contract
cargo test -p tact-ui --test driver_integration
cargo test -p tact-ui --test recovery_compaction
cargo test -p tact_llm --lib openai::responses::
cargo metadata --format-version 1
cargo check --workspace
```

**Task 9 exit:** All seven owner crates contain their production responsibilities; no forbidden facade back-edge, duplicate state authority or duplicate execution algorithm remains. Failure of the compatibility/borrow proof means the milestone is incomplete and needs a concrete design correction; it does not turn physical extraction into optional work.

## Task 10: Documentation, coverage audit and handoff

**Files:** Modify this plan/spec, `ARCHITECTURE.md`, `docs/{state_machines,tool_rendering}.md`, affected `book/{01_chapter_store,05_chapter_compact,06_chapter_recovery,08_chapter_mcp,09_chapter_hook,10_chapter_permission,18_chapter_agent_loop,26_chapter_issue}_zh.md`. Plugin documentation follows its actual existing home found with rg; do not invent a chapter.

- [ ] At push time update the actual module/crate graph, full protocol inventory, Hook order and compatibility matrix. Preserve book Chinese-only requirement.
- [ ] Document Task 3 security behavior with symptom, decision, observable effect and code/test pointers. Add a persistence safety entry only if Task 7 changed that behavior.
- [ ] Record each task's actual test commands/counts and outcome; no box checked on zero matching tests. Do not repeat unchanged broad suites absent a concrete risk.
- [ ] Check spec-to-plan coverage using the matrix below; require all seven package milestones; only new dynamic plugin ABI work remains out of scope.
- [ ] Run git diff --check for tracked changes; when documents are untracked, use git diff --no-index --check /dev/null with each file individually. Confirm working tree contains only intended edits plus preserved pre-existing files.

## Coverage matrix

| Requirement | Owner |
|---|---|
| Existing plugin package/marketplace/trust/resources | 1, 8 |
| All host commands/updates/plugin worker routes | 1, 4, 8 |
| Ownership, module interfaces, legacy borrowing and facade cycles | 2, 7, 9.1, 9.7 |
| Hook policy baseline and explicit final-input correction | 1, 3, 6 |
| Native/MCP scheduling, refresh and cancellation | 2, 5, 6 |
| Hosted tools and raw replay | 5 |
| Ordered publication, lag/disconnect, no duplicate completion | 4, 7 |
| Store atomicity, crash window and no automatic side-effect replay | 7 |
| Public fields, constructors, legacy callbacks and proc macro | 1, 8, 9.1, 9.6, 9.7 |
| Exact test selection and bounded waits | Every task; execution rules |
| Required owner crates, acyclic graph and collaboration fixtures | 9.1–9.7 |
| Push-time documentation | 10 |

## Milestone coverage

| Target owner | Required milestone | Independent gate |
|---|---|---|
| tact-contracts | 9.1 | Value compatibility and real consumer compilation |
| tact-session | 9.2 | Store tests and host resume fixtures |
| tact-permission | 9.3 | Policy/sandbox tests and host approval fixtures |
| tact-plugins | 9.4 | Package/Hook fixtures without a live MCP client |
| tact-mcp | 9.5 | Transport/refresh/concurrency without plugin storage |
| tact-tools | 9.6 | Native/service-backed tools without Agent |
| tact-runtime | 9.1, 9.7 | Fake-service runtime suite and real legacy facade slice |
| Composition and existing hosts | 9.7, 10 | Public API, protocol, interactive/headless regression gates |

## Execution notes

No implementation or Cargo verification has been performed while drafting this revision. Record baseline commit, test inventory and results when Task 1 starts. No user approval to commit or push is implied by this plan.
