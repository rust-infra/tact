# Durable Agent Goals — bounded Runner, evidence-based verification, and workspace-scoped persistence

Status: **implemented** (2026-10-15). All tasks in the implementation plan
[`docs/superpowers/plans/2026-10-14-agent-goal.md`](../plans/2026-10-14-agent-goal.md)
are checked off; the design chapter is
[`book/28_chapter_goal_zh.md`](../../../book/28_chapter_goal_zh.md).

## Problem

A single `agent_loop` call models one turn: the model responds, tools run, the loop
returns. There is no durable container for work that spans multiple turns, survives
restart, respects a budget, waits for the user without hanging when no UI is attached,
or requires evidence before claiming success. The existing `TaskManager` tracks
executable work items and their dependency edges but has no lifecycle, acceptance
criteria, or supervision layer above the agent loop.

Without this layer:

- **Restart loses progress.** A long-running objective that hit budget exhaustion
  or a transient error has no resumable state; the user must re-explain the goal.
- **No completion contract.** The model can declare success in free text; there is
  no structured check that acceptance criteria were met with traceable evidence.
- **No budget enforcement.** A single activation can run unbounded turns, tool calls,
  or wall-clock time.
- **No safe wait.** When the agent needs user input or a permission decision, a
  headless or disconnected session has no way to persist "waiting" and resume later.
- **No concurrency isolation.** Two processes pointed at the same workspace can
  mutate shared state without fencing.

## Design

### Layer separation: Goal ≠ Task ≠ Run ≠ Turn

| Concept | Role | Lifetime |
|---------|------|----------|
| **Goal** | The user's desired outcome and acceptance criteria | Cross-session, cross-run; until succeeded/failed/cancelled |
| **Task** | Executable work item with status and dependencies | Managed by existing `TaskManager`; optionally linked to a Goal via nullable `goal_id` |
| **Run** | One lease-bounded activation that advances a Goal | Explicit start, outcome, budget usage, checkpoint |
| **Turn** | One model interaction inside the agent loop | Not a Goal completion unit |

The runner is an orchestrator **around** `agent_loop`, not inside it. `agent_loop`
keeps its single-turn semantics (model call → tool dispatch → hooks → permissions →
compaction). The runner calls it with a budget, observes an explicit
`AgentLoopStop` outcome, maps that to a `GoalRunOutcome`, persists it, and decides
the next state transition.

### State machine

```
draft → queued → running → verifying → succeeded
                    │          │
                    ├─ waiting_user
                    ├─ waiting_permission
                    ├─ paused
                    ├─ failed
                    └─ cancelled
```

`phase` (planning / execution / verification) is a separate field — it is display
metadata, not a lifecycle stage. Each Run ends with an explicit outcome:

```
progress_made | needs_user | needs_permission | completion_proposed
budget_exhausted | no_progress | cancelled | transient_error | fatal_error
```

The runner maps outcomes to transitions. Every transition is persisted to SQLite
**before** any UI notification is emitted.

### Persistence model

Five new tables in the existing workspace database (`.tact/tact.db`):

- **`goals`** — aggregate, status, phase, version, session linkage, timestamps.
- **`goal_criteria`** — per-criterion description, required flag, verification kind,
  status, evidence (tagged with criteria version), updated_at.
- **`goal_runs`** — per-activation record: outcome, budget usage, checkpoint,
  stop reason, timestamps.
- **`goal_events`** — append-only audit log: creation, steering, transitions,
  run start/finish, verification results.
- **`goal_leases`** — workspace singleton lease with fencing epoch for write isolation.

The existing `tasks` table gains a nullable `goal_id` column via idempotent
`ALTER TABLE` migration. Legacy Tasks without `goal_id` behave unchanged.

### Lease fencing and crash recovery

- One active writer per workspace execution directory. The lease carries an
  instance-unique owner (session label + pid + random suffix), expiry/heartbeat,
  the **holder process identity** (`holder_pid` + start-time `holder_identity`),
  and a monotonically increasing **fencing epoch**. A live lease is never
  re-issued, even to the same label; only its token holder may heartbeat, finish,
  or release it. "Live" means the TTL has not passed **and** the holder process
  is still that process (2026-10-17): an unexpired lease whose holder is gone is
  reclaimable at once, while an unverifiable holder (legacy row, foreign
  platform identity, or a live process on another uid whose start-time identity
  the kernel will not disclose — `EPERM`, never `ESRCH`) keeps the time-based
  path. Existence and describability are separate questions: only `ESRCH` is
  evidence that a holder is gone.
- All state writes check the current epoch; a stale process whose lease expired
  cannot overwrite newer state.
- On restart, the runner finds leases that expired **or whose holder process is
  gone**, marks interrupted Runs as `interrupted`, records why, and lets the
  reconciliation pass pause a `running` Goal with no active Run — never reusing
  or overwriting old Run records.
- Recovery reads from SQLite (Goal/Run/Event state) and session transcript, never
  from in-memory futures, channels, or pending queues.
- Pending permission requests are **not** auto-approved on restart; they re-enter
  the current permission flow.

### Evidence-based verification

`GoalVerifier` is a pure decision function — no I/O, no clock, no model text
inspection. It reads `Goal` state and returns:

- **`Verified`** — every required criterion holds valid evidence at the current
  criteria version, and no human criterion is pending.
- **`NeedsUser`** — no required criterion is provably unmet, but at least one
  required human criterion awaits the user.
- **`Rejected`** — at least one required criterion is provably unmet, or the Goal
  has no usable acceptance criteria (prevents auto-success on empty criteria).

Evidence is tagged with the criteria version it was recorded against. When the
user modifies goals or criteria, the version bumps; stale evidence is invalid
without explicit re-verification.

Automatic verification is reserved in this implementation: no runner-side
checker exists yet. `GoalCriterion::required` defaults to Human, and production
create/update paths reject Automatic criteria rather than creating a Goal that
can never pass. Legacy Automatic criteria are closed by an explicit human
acceptance.

### Budget and no-progress protection

Each run carries: max turns (default 500, `[goal].max_turns`) and max
elapsed (default 600s, `[goal].max_elapsed_secs`). Tool calls are recorded for
the run summary (`GoalRun::tool_calls_used`) but deliberately not capped: every
tool call already happens inside a model call, so the turn budget bounds the
fan-out. These are centralized in `GoalSettings`/`GoalLimits` and
test-injectable.
Reaching any limit saves a checkpoint and transitions to `paused`.

No-progress detection relies on observable changes (Task status transitions,
evidence updates, checkpoint diffs), not model self-report. Consecutive cycles
without observable progress trigger a pause; a single transient error does not.

### Agent tool surface

Goal-aware tools are injected via `ToolContext`, not global state:

- `goal_read` — read active Goal, criteria, Task progress.
- `goal_checkpoint` — append progress to the event stream.
- `goal_request_input` — ask the user a question (persisted as `waiting_user`).
- `goal_propose_completion` — submit a completion proposal with **untrusted**
  criterion claims. Claims do not write evidence; human acceptance does.

No agent-callable method can directly set `succeeded`, mutate user-confirmed
acceptance, or operate on arbitrary Goals. Subagents do not inherit the parent's
Goal binding and have no Goal tools; they cannot end or reopen the parent Goal.

### Protocol and TUI

`AgentUpdate::GoalsChanged` carries a full `Vec<GoalSnapshot>` replacement — no
incremental deltas. The snapshot includes status, phase, criteria, acceptance
counts, run budget, and wait reason.

Typed `UserCommand` variants: `CreateGoal`, `ListGoals`, `ShowGoal`, `SteerGoal`,
`PauseGoal`, `ResumeGoal`, `CancelGoal`, `AcceptGoal`. List/show queries read
directly from `SharedGoalManager` even during an active run. Steer/pause/cancel
also act on the shared store and cancel flag without awaiting the run's
`JoinHandle`: pause/cancel persist a request event that the runner honours at the
next safe point, and steer is injected on the next activation. Resume/accept
still need the Agent and stay serialized.

The Goals sticky panel in the TUI shows: title, status/phase, run budget, Task
progress, wait reason, acceptance summary, and per-criterion status. Actions
route back as typed `UserCommand`s.

### Headless invariant

Headless and interactive modes share the same `GoalRunner` and state machine.
A headless session that needs user input or a permission decision persists
`waiting_user` / `paused` and exits the current Run — it never blocks on a
nonexistent UI responder. A subsequent session or API call supplies the input
and resumes.

## Alternatives considered

- **Goal as a Task flag.** Rejected: Tasks are work items, not supervisory
  containers. A Goal needs its own lifecycle, criteria, lease, and verification
  — none of which fit the Task model.
- **Runner inside agent_loop.** Rejected: `agent_loop`'s contract is one bounded
  model interaction. Embedding cross-turn supervision inside it would break
  existing callers, compaction, and the single-turn recovery model.
- **In-memory state.** Rejected: restart recovery is a first-class requirement.
  State must survive process death.
- **Model text as success.** Rejected: the model declaring "done" is not proof.
  Evidence must be traceable to tool results, test outcomes, or user
  confirmation, validated against the current criteria version.

## Pointers

- Domain types: `crates/tact/src/goal/mod.rs` — `GoalStatus`, `GoalPhase`,
  `GoalRunOutcome`, `GoalCriterion`, `GoalManager`, `SharedGoalManager`.
- Verifier: `crates/tact/src/goal/verifier.rs` — `GoalVerifier::verify`.
- Store: `crates/tact/src/store/goal_store/{mod.rs,sqlite.rs}`.
- Runner: `crates/tact-ui/src/goal_runner.rs` — `GoalRunner::run_once`.
- Agent tools: `crates/tact/src/tool/goal.rs`.
- Protocol: `crates/protocol/src/agent.rs` — `GoalSnapshot`, `GoalsChanged`,
  `UserCommand` variants.
- TUI: `crates/agent_tui_kit/src/{state/goal_panel.rs,components/goal_panel.rs}`,
  `crates/tui/src/render/task_panel.rs`.
- Book: `book/28_chapter_goal_zh.md` (implementation-accurate design chapter).
- Plan: `docs/superpowers/plans/2026-10-14-agent-goal.md`.
