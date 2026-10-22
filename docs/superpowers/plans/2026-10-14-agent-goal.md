# Agent Goal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development` (recommended) or `executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement durable, supervised Agent Goals that can be created, run in bounded activations, paused/resumed, verified with evidence, and inspected through Tact's command/TUI surfaces.

**Architecture:** Add a SQLite-backed Goal domain with criteria, runs, events, legal transitions, and workspace-scoped lease fencing. Link existing persistent Tasks to a Goal. Keep `Agent::agent_loop` as the execution kernel; a driver-owned Goal runner schedules bounded runs, handles explicit outcomes, budgets, and verification. Expose user lifecycle control through protocol/driver commands and project Goal snapshots into the existing sticky host.

**Tech Stack:** Rust workspace, Tokio, SQLx SQLite, serde, `tact_protocol`, Ratatui / `agent_tui_kit`.

**Spec:** [`docs/superpowers/specs/2026-10-14-agent-goal-design.md`](../specs/2026-10-14-agent-goal-design.md) (approved design); `book/28_chapter_goal_zh.html` is the user-facing chapter and must stay synchronized with delivered behavior.

## Global Constraints

- Keep Goal distinct from Task, Run, Turn, and ephemeral PlanStep.
- A model completion message, a normal `agent_loop` return, or all Tasks becoming completed is not sufficient to mark a Goal successful.
- Goal lifecycle and Run outcomes are persisted before UI notification; SQLite is authoritative after restart.
- Goal execution does not bypass permission checks or silently approve future side effects.
- Every Runner activation has turn, wall-clock, and tool-call budgets; exhaustion pauses with a checkpoint.
- Use idempotent SQLite schema setup and preserve existing Task/session data.
- Keep headless and interactive behavior on the same Goal manager/runner path; never wait for a nonexistent UI responder.
- Preserve the current uncommitted book changes; do not discard or stage them as part of feature implementation.
- Do not run Cargo test/build/clippy invocations concurrently against the same workspace `target/`.
- New TUI render units must paint their full area and include buffer-level blank-background tests.
- Book documentation remains Chinese; code/docs/specs remain English.

## Review Focus

- Old databases and Tasks without a `goal_id` remain readable and behave unchanged — test in Task 2 and Task 3.
- Illegal transitions, duplicate requests, and stale lease epochs cannot overwrite newer Goal state — test in Task 1 and Task 2.
- Model self-report or completed Tasks alone cannot produce `succeeded`; stale criteria evidence is invalid after criteria version changes — test in Task 4.
- Cancellation, budget exhaustion, restart, and missing UI responder leave a durable resumable state without claiming success — test in Task 5.
- Concurrent Goals in one workspace and read-only Goal queries during an active run behave deterministically — test in Task 2 and Task 7.

---

## File Map

- `crates/tact/src/goal/mod.rs`: public Goal domain types, validation, transition rules, shared manager, rendering helpers.
- `crates/tact/src/store/goal_store/mod.rs` and `sqlite.rs`: storage trait, SQLite schema, transactions, lease fencing, event/criteria/run persistence.
- `crates/tact/src/store/mod.rs`, `crates/tact/src/lib.rs`: module exposure.
- `crates/tact/src/task/mod.rs`, `crates/tact/src/store/task_store/{mod.rs,sqlite.rs}`: optional Goal linkage and scoped queries.
- `crates/tact/src/tool/mod.rs`, `tool/goal.rs`, `tool/registry.rs`, `agent/tool_dispatch.rs`: Goal context and constrained agent-facing operations.
- `crates/tact/src/agent/mod.rs`: explicit goal execution outcome and bounded-loop integration without changing ordinary chat semantics.
- `crates/tact-ui/src/session_bootstrap.rs`, `crates/tact-ui/src/driver.rs`: shared manager wiring, Goal command dispatch, runner lifecycle and recovery.
- `crates/protocol/src/agent.rs`, `crates/protocol/src/lib.rs`: Goal snapshots, updates, and user commands.
- `crates/tui/src/widgets/state/app/agent.rs`, relevant app state and command parsing/completion files, `crates/agent_tui_kit/src/components/`: Goal sticky surface and interaction.
- `crates/tact/src/**`, `crates/tact-ui/src/**`, `crates/tui/src/**`: unit/integration/buffer tests colocated with existing patterns.
- `book/28_chapter_goal_zh.html`, `book/index_zh.html`: reconcile proposal into implementation-accurate documentation after behavior lands.

## Task 1: Goal Domain Types and Lifecycle Rules

**Files:**
- Create: `crates/tact/src/goal/mod.rs`
- Modify: `crates/tact/src/lib.rs`
- Test: colocated unit tests in `crates/tact/src/goal/mod.rs`

**Interfaces:**
- Produces `GoalStatus`, `GoalPhase`, `GoalRunOutcome`, `Goal`, `GoalCriterion`, `GoalRun`, `GoalEvent`, `GoalCreate`, `GoalUpdate`, and `GoalError`.
- Produces pure transition/validation methods; storage and UI are not dependencies of transition logic.

- [x] **Step 1: Write failing unit tests** for legal transitions (`draft→queued→running`, wait/pause/resume, verify/succeed), rejected transitions from terminal states, non-empty objective/title validation, and criteria version bump invalidating prior evidence.
- [x] **Step 2: Run the focused test** with `cargo test -p tact goal:: --lib`; confirm failures are due to missing Goal domain API/behavior.
- [x] **Step 3: Implement minimal domain types and pure transition rules** in `goal/mod.rs`; expose `pub mod goal`.
- [x] **Step 4: Re-run the focused tests** and verify every invalid transition returns a typed error rather than mutating state.

## Task 2: SQLite Goal Store, Events, and Lease Fencing

**Files:**
- Create: `crates/tact/src/store/goal_store/mod.rs`
- Create: `crates/tact/src/store/goal_store/sqlite.rs`
- Modify: `crates/tact/src/store/mod.rs`
- Test: colocated SQLite tests in `goal_store/sqlite.rs`

**Interfaces:**
- `GoalStore` async trait supports create/get/list/update, criteria read/write, event append/list, Run start/finish/recovery, lease claim/heartbeat/release, and fenced state mutation.
- `SqliteGoalStore::new(path: &Path) -> Result<Self>` shares `open_pool`.
- Store operations accept expected Goal version/lease epoch where a stale writer could otherwise overwrite newer state.

- [x] **Step 1: Write failing SQLite tests** for create/read/list; criteria and evidence persistence; event ordering; legal transition transactionality; idempotent schema initialization; unique active workspace lease; expired lease takeover with incremented fencing epoch; stale epoch write rejection; interrupted Run recovery.
- [x] **Step 2: Run focused tests** with `cargo test -p tact goal_store:: --lib`; verify expected missing store behavior.
- [x] **Step 3: Implement schema and transactions** for `goals`, `goal_criteria`, `goal_runs`, `goal_events`, and `goal_leases`; keep event append and state update atomic for lifecycle changes.
- [x] **Step 4: Re-run focused tests**, including opening the same test database twice to prove migrations are idempotent.

## Task 3: GoalManager API and Task Association

**Files:**
- Modify: `crates/tact/src/goal/mod.rs`
- Modify: `crates/tact/src/store/goal_store/mod.rs`
- Modify: `crates/tact/src/store/goal_store/sqlite.rs`
- Modify: `crates/tact/src/task/mod.rs`
- Modify: `crates/tact/src/store/task_store/mod.rs`
- Modify: `crates/tact/src/store/task_store/sqlite.rs`
- Test: colocated GoalManager and TaskStore tests

**Interfaces:**
- `SharedGoalManager` wraps `Arc<GoalManager>` like existing managers.
- `GoalManager` exposes create/get/list/update, criteria update, append event, run/lease lifecycle, and Goal-scoped task lookup/association.
- `TaskRecord.goal_id: Option<String>` (or the final identifier type selected consistently by the repository) is optional; legacy rows deserialize as unassociated.

- [x] **Step 1: Write failing tests** for creating/listing/resuming Goals, rejecting cross-workspace/unknown Task associations, preserving legacy Tasks with no Goal, and listing only non-deleted tasks for a Goal.
- [x] **Step 2: Run focused tests** with `cargo test -p tact goal:: --lib` and `cargo test -p tact task:: --lib` sequentially; confirm the new assertions fail first.
- [x] **Step 3: Implement manager facade and nullable Task linkage** with a safe idempotent column migration and Goal-scoped queries.
- [x] **Step 4: Re-run the two focused test filters sequentially** and verify existing task behavior remains unchanged.

## Task 4: Evidence-Based Verification and Goal Tools

**Files:**
- Create: `crates/tact/src/tool/goal.rs`
- Modify: `crates/tact/src/tool/mod.rs`
- Modify: `crates/tact/src/tool/registry.rs`
- Modify: `crates/tact/src/agent/tool_dispatch.rs`
- Modify: `crates/tact/src/goal/mod.rs`
- Test: Goal verifier/domain tests and tool tests

**Interfaces:**
- Add `SharedGoalManager` and `active_goal_id: Option<GoalId>` to the runtime tool context through explicit builder/wiring, not global state.
- Agent tools may read the active Goal, propose scoped Task changes, append checkpoint/progress, request user input, and submit a completion proposal with criterion evidence.
- Agent tools cannot directly set `succeeded`, mutate user-confirmed acceptance, or operate on arbitrary Goals.
- `GoalVerifier` accepts a Goal snapshot and evidence; returns `Verified`, `NeedsUser`, or `Rejected` and never treats free-form completion text as proof.

- [x] **Step 1: Write failing tests** that a completion proposal with missing required evidence is rejected; stale-version evidence is rejected; a valid automatic evidence set can pass; a human criterion yields `NeedsUser`; Goal tools are unavailable without active Goal context and reject cross-Goal ids.
- [x] **Step 2: Run focused tests** with `cargo test -p tact goal:: --lib` and relevant `tool` tests sequentially.
- [x] **Step 3: Implement verifier and narrow tool surface**, routing mutations through GoalManager and existing permission/resource policies.
- [x] **Step 4: Re-run focused tests** and verify no agent-callable method can directly mark success.

## Task 5: Bounded Goal Runner and Agent Outcomes

**Files:**
- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact-ui/src/session_bootstrap.rs`
- Modify: `crates/tact-ui/src/driver.rs`
- Modify: Goal manager/store as required for atomic run outcomes
- Test: agent loop tests and `crates/tact-ui` driver integration tests

**Interfaces:**
- Preserve the existing ordinary `agent_loop(...) -> Result<()>` API for existing callers; add a goal-capable entry point or internal detailed loop returning an explicit `AgentLoopOutcome`.
- `GoalRunner::run_once(goal_id, limits, cancellation) -> Result<GoalRunOutcome>` acquires lease, persists a Run, executes bounded Agent work, verifies completion proposals, persists outcome/checkpoint, and releases lease on every exit path.
- Initial limits are explicit and test-injectable: max turns, tool calls, and elapsed duration; config defaults are centralized rather than embedded in control flow.

- [x] **Step 1: Write failing agent/driver tests** for normal progress yield, completion proposal→verification, user/permission wait without hanging when UI is absent, cancellation, each budget limit, no-progress pause, transient error pause, lease release on every return/error path, and restart recovery from an interrupted Run.
- [x] **Step 2: Run focused tests** sequentially: `cargo test -p tact agent:: --lib`, then the relevant `cargo test -p tact-ui --lib driver::` filter.
- [x] **Step 3: Implement explicit loop outcome and a driver-owned bounded runner**; keep ordinary SubmitTask completion behavior unchanged.
- [x] **Step 4: Re-run focused tests** and inspect traces to confirm every outcome is persisted before notification.

## Task 6: User Goal Commands and Protocol Snapshots

**Files:**
- Modify: `crates/protocol/src/agent.rs`
- Modify: `crates/protocol/src/lib.rs`
- Modify: `crates/tact-ui/src/driver.rs`
- Modify: existing TUI slash/command parsing and completion files identified from `rg` before editing
- Test: protocol serialization tests, driver command tests, parser/completion tests

**Interfaces:**
- Add Goal snapshot/status/criterion summary and a full-snapshot `GoalsChanged` update.
- Add typed user commands for create/list/show/steer/pause/resume/cancel/accept; command payloads carry Goal IDs and user text explicitly.
- Driver answers list/show immediately from GoalManager even while an Agent run is active; mutations are serialized against the active runner and persisted before emitting updates.

- [x] **Step 1: Write failing tests** for protocol snapshots, each command dispatch, invalid ids, concurrent read during a run, pause/cancel while running, steering persistence, and command parser/completion behavior.
- [x] **Step 2: Run focused protocol and driver tests** sequentially; confirm the new variants/commands are missing.
- [x] **Step 3: Implement typed commands and driver handling**; ensure read-only queries do not wait for ownership of the in-flight Agent.
- [x] **Step 4: Re-run focused tests** and verify headless commands report waiting states rather than invoking UI responders.

## Task 7: TUI Goal Surface

**Files:**
- Modify: `crates/tui/src/widgets/state/app/agent.rs` and related app state
- Modify: `crates/agent_tui_kit/src/components/` and sticky host rendering/state as needed
- Modify: mouse/keyboard routing and slash UI integration as needed
- Test: state/update tests and buffer-level render tests

**Interfaces:**
- Goal panel consumes full Goal snapshots and exposes current selection/expanded state through the existing sticky host pattern.
- It shows Goal status/phase, run budget, associated Task progress, wait reason, and acceptance summary; user actions send typed `UserCommand`s.

- [x] **Step 1: Write failing state/render tests** for snapshot updates, waiting/terminal states, budget/criteria summary, action routing, narrow-terminal layout, and blank-cell theme background across the entire panel area.
- [x] **Step 2: Run focused TUI tests** with the owning crate's test filter; verify the missing panel behavior.
- [x] **Step 3: Implement Goal state projection and sticky panel** reusing existing component conventions; every render unit paints its whole area.
- [x] **Step 4: Re-run focused tests** and inspect a buffer snapshot for blank-cell background correctness.

## Task 8: Documentation and Full Verification

**Files:**
- Modify: `book/28_chapter_goal_zh.html`
- Modify: `book/index_zh.html` only if final chapter summary needs correction
- Modify: relevant `book/01_chapter_store_zh.html`, `book/18_chapter_agent_loop_zh.html`, `book/19_chapter_persistent_tasks_zh.html`, `book/23_chapter_tui_zh.html`, and `book/25_chapter_protocol_zh.html` where implementation changes their contracts

- [x] **Step 1: Re-read all touched docs** and update proposal markers, schema/API names, lifecycle, and actual current limitations to match shipped implementation.
- [x] **Step 2: Check relative Markdown links** in touched book chapters and run `git diff --check`.
- [x] **Step 3: Run crate verification serially**: targeted `tact` tests, targeted `tact-ui` tests, targeted `tui`/protocol tests, then the repository-prescribed full checks without parallel Cargo invocations.
- [x] **Step 4: Review final diff** for unplanned files, accidental edits to pre-existing user changes, and any behavior that violates the spec.

---

## Execution Notes

- Use strict TDD within each task: write one behavioral test, observe the expected failure, implement the minimum, then verify green before proceeding.
- Tasks 1–3 establish the persistent contract; Tasks 4–7 depend on those interfaces and should not be parallelized against shared files.
- Do not commit unless the user explicitly asks. Preserve the existing dirty `book/` files; if implementation is isolated in a worktree, carry the documentation changes intentionally rather than overwriting either side.
- Once execution begins, run one Cargo command at a time per workspace to avoid `target/` lock contention.
