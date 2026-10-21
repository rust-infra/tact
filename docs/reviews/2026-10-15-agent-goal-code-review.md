# Code Review — Durable Agent Goals (`wt/agent-goal`, HEAD `b0d38241`)

Date: 2026-10-15. Scope: `git diff main..HEAD` (~9.8k lines, 66 files), reviewed in five passes:
domain (`goal/`), store (`goal_store/`), runner (`goal_runner.rs` + agent-loop integration),
tools/verifier, protocol/TUI. High-severity claims below were re-verified against the source
by hand; line numbers refer to the branch HEAD.

**Headline: no CRITICAL findings.** No verifier bypass reachable from the model, no cross-Goal
write, no permission regression, happy-path persist-before-notify ordering holds. The three
HIGH items are real and user-reachable, and all three sit in the driver/runner layer.

---

## HIGH

### H1. `resume_goal` / `accept_goal` never clear `cancel_flag` — an Esc before resume destroys the goal
- `crates/tact-ui/src/driver.rs:654-689` (`resume_goal`), `:825` (`accept_goal`); flag set at
  `driver.rs:155` (`UserCommand::Cancel`), cleared only at `driver.rs:960` (`SubmitTask`).
- Scenario: user presses Esc during a normal turn → `cancel_flag = true` → `/goal resume <id>` →
  the activation's first loop iteration (`agent/mod.rs:1311`) returns `AgentLoopStop::Cancelled`
  before any model call → `goal_runner.rs:643` maps it to `GoalStatus::Cancelled`, a **terminal**
  state → `load_runnable_goal` refuses terminal goals, so the goal is permanently destroyed
  without doing any work.
- Fix: `agent.runtime.cancel_flag.store(false, Ordering::Relaxed)` at the top of both functions.

### H2. Per-activation budgets are only enforced *after* the activation returns
- `crates/tact-ui/src/goal_runner.rs:530-584`. `agent_loop_detailed` is called at `:530`; the
  runner never sets `agent.max_turns` (grep: `goal_runner.rs` only touches its own `limits`);
  the subagent path does use `with_max_turns` (`agent/mod.rs:656`). `permit_activation` is
  evaluated post-hoc at `:576-583`.
- Scenario: one activation consumes unbounded turns/tool-calls (the model keeps returning
  `ToolUse`); only the *next* activation is refused. Contradicts `DEFAULT_MAX_TURNS`'s own doc
  comment (`goal_runner.rs:74-79`, "a runaway loop is cut off long before a token budget would
  notice"). The wall-clock half of this is the documented limitation; turns/tool-calls is not.
- Fix: set `agent.max_turns = Some(self.limits.max_turns)` for the duration of the activation
  and restore after (the `turns` delta is computed against `turns_before`, so accounting stays
  correct).

### H3. Run-finish is three transactions, not one
- `crates/tact-ui/src/goal_runner.rs:589-599` sequences `finish_run` → `persist_status` →
  `record_run` as three independent store commits (`store/goal_store/sqlite.rs:331-374`, `:224-238`).
- Scenario: crash or any `Err` between `:589` and `:599` leaves the durable state as
  `run.state='completed'` + `goal.status='running'` with no `run_finished` event. If
  `persist_status` fails its version CAS, the run's outcome/usage/checkpoint are lost from
  history entirely. The code acknowledges the gap (`goal_runner.rs:403`,
  `test` at `:1587`) but reconciliation only runs at startup — it does not make the write atomic.
- Fix: one store method (`BEGIN IMMEDIATE`) doing run-finish + goal CAS + event insert.

---

## MEDIUM

### M1. The `EndTurn` arm does not re-check the cancel flag
- `crates/tact/src/agent/mod.rs:1632-1644`. Cancel is checked at the loop top (`:1311`) and after
  a `ToolUse` response (`:1647`) but not on the final-stop-reason arm.
- Scenario: user hits Esc while the activation's *last* model call streams → `Completed` →
  runner persists `ProgressMade`/`CompletionProposed` and may drive the goal to `Succeeded`
  (`goal_runner.rs:671-677`) — a durable success recorded against a goal the user just cancelled.
- Fix: check `cancel_requested()` in that arm and return `AgentLoopStop::Cancelled`.

### M2. `?` after `start_run` leaves the run row stuck `running`
- `crates/tact-ui/src/goal_runner.rs:524` (`list_events`), `:583` (`map_stop`), `:589`
  (`finish_run`). The activation-error path (`:535-564`) finishes the run before returning; these
  post-activation paths do not.
- Scenario: a transient `SQLITE_BUSY` on `list_events` → phantom `running` row → `list_running_runs()`
  non-empty → bogus budget snapshot; on next startup, recovery records a spurious
  "lease expired" `TransientError` for a run that actually failed on a store read.
- Fix: wrap everything after `start_run` so every error path finishes the run (as `:535-564` does).

### M3. Lease is never heartbeated during an activation; an activation can outlive its own TTL
- `heartbeat_lease` exists (`goal/mod.rs:583`) but is only called from tests; TTL is
  `max_elapsed + 60s slack` (`goal_runner.rs:495-497`). Combined with H2, an activation can run
  past 660s.
- Scenario: activation takes 700s → lease lapses mid-run → the next `claim` (or this session's
  own next run, `sqlite.rs:171-176`) interrupts the run row → this runner's `finish_run` fails
  `StaleLease` (`sqlite.rs:436-449`) → `?` at `goal_runner.rs:589` → outcome, transitions, and
  `run_finished` event all lost; user sees "lease is held" until the zombie TTL lapses.
- Fix: heartbeat from a periodic task during the activation (or re-claim + retry `finish_run`).
- Related: `release_lease` (`sqlite.rs:289-302`) validates expiry, so a lease that merely lapsed
  without takeover fails release on an otherwise successful activation. Release is already fenced
  by `owner+epoch`, so the expiry precondition is unnecessary.

### M4. The fence has three holes around status transitions
1. `update` accepts `lease: None` (`sqlite.rs:83-85`) and then performs **no epoch check and no
   active-run check**. `update_goal_status` (pause/cancel/accept) passes `None`
   (`driver.rs:628`). A second writer process on the same workdir pauses a goal mid-activation;
   the pause commits while the run row stays `running` — the exact "paused but run active"
   observation invariant 1 forbids, reachable without any crash.
2. `DEFAULT_OWNER` ("tact-goal-runner", `goal_runner.rs:335/350`) is used for real runs while
   recovery uses a session-scoped owner (`driver.rs:52-59`). `lease_tx` treats a same-owner
   re-claim of a live lease as a silent extend (`sqlite.rs:158-170`), so two TUI sessions never
   see `LeaseHeld` — only the `start_run` `ActiveRunExists` guard stops the second.
3. `Verifying` is missing `Paused` / `WaitingPermission` in the transition table
   (`goal/mod.rs:422`; every other non-terminal state allows them). Scenario: the AcceptGoal path
   persists `Verifying` then `Succeeded` in two updates (`driver.rs:772-822`,
   `goal_runner.rs:676`); a pause in that window errors out and the goal advances to `Succeeded`
   anyway.

### M5. `Succeeded` is gated only by `validate_acceptance`, which is vacuous for all-optional criteria
- `goal/mod.rs:330-332` → `validate_acceptance` (`:370-383`) errors only on empty criteria or on
  a *required* criterion lacking valid evidence. `Goal::new` accepts an all-optional criteria
  list (`:231-233`), and the verifier returns `Rejected` for exactly those (`verifier.rs:60-64`).
- In-tree callers gate on the verifier (`goal_runner.rs:671`, `driver.rs:770`), so the invariant
  holds by call-site discipline, not by the type. Any new/API caller using the public
  `GoalManager::update` can succeed a Goal with zero evidence.
- Fix: `transition_to(Succeeded)` additionally require ≥1 required criterion, or route success
  through `GoalVerifier`.

### M6. `record_evidence` can resurrect a `Rejected` criterion
- `goal/mod.rs:137-143` unconditionally sets `status = Passed` on evidence write.
- Scenario: user rejects a human criterion (status `Rejected`); a later `GoalUpdate` carrying
  `evidence: [{ id, value: "ok" }]` flips it to `Passed` with no new user decision — bypassing
  the legitimate criteria-redefinition path (`:284-291`). Not reachable from the model (agent
  tools never write evidence) but it is a public facade that mutates user-confirmed acceptance.
- Fix: refuse evidence for `Rejected` criteria.

### M7. Goal panel body is hardcoded English under a localized title
- `crates/agent_tui_kit/src/state/goal_panel.rs:60-89, 123-131` — `status_label`, `phase_label`,
  `criterion_marker`, ` (human)`, `↳ {reason}` are English literals while the tab title is
  localized (`i18n.rs:1067/1167`). Chinese-UI users get `▸ 目标 1/1` followed by
  `· waiting_user/verification · tasks 0/1 · accept 0/1`. (Task/subagent group headers already
  hardcode, but this is by far the largest untranslated surface.)

### M8. `list_for_goal` is an N+1 pattern on the activation path
- `store/task_store/sqlite.rs:222-231` issues two `edges()` queries per task; the sibling `list()`
  (`:314-352`) assembles all edges in one pass. `list_for_goal` runs on **every** activation
  (`goal_runner.rs:518-523`).

### M9. `list_events` is unbounded and events are never pruned
- `store/goal_store/sqlite.rs:252-260` — no LIMIT, no seq window, no DELETE in the file. Every
  activation re-reads and JSON-parses the full history into the prompt (`goal_runner.rs:524`),
  and `map_completion` rescans all of them per run (`:657-664`). The `(goal_id, seq)` index
  already supports range scans.

---

## LOW

- **L1.** `GoalRun.tokens_used` is never populated (`goal_runner.rs` on every path; field exists
  at `goal/mod.rs:457`), and the live `run_budget` snapshot is therefore all zeros — the panel
  shows `0/24, 0/60, 0ms` for the whole run, then vanishes (`driver.rs:223-232` builds it from
  `list_running_runs()`, whose usage is only written by `finish_run`).
- **L2.** Budget check preempts stop mapping (`goal_runner.rs:576-584`): a run cancelled *and*
  over budget is recorded `BudgetExhausted`/`Paused` instead of `Cancelled`; a `BlockedByHook`
  activation likewise.
- **L3.** Every activation error maps to `TransientError` (`goal_runner.rs:545`, `:555`) —
  including `StopReason::Refusal` (`agent/mod.rs:1602-1604`), which is fatal. `FatalError` is
  currently only reachable via `BlockedByHook`.
- **L4.** `GoalRunOutcome::NoProgress` is never produced (`goal_runner.rs:652-692`): an activation
  that did nothing is booked as `ProgressMade`.
- **L5.** The four goal tools are advertised in every model spec even with no active Goal
  (`tool/registry.rs:47-50`); the fail-closed error is correct but wastes model turns. They are
  correctly absent from the subagent toolset.
- **L6.** Criterion `id` uniqueness within a Goal is not enforced (`goal/mod.rs:154-164`);
  duplicate ids make completion claims ambiguous (fails closed, but still a data-integrity gap).
- **L7.** `GoalPanelState::visible` is `!snapshot.is_empty()` (`goal_panel.rs:44-50`) — terminal
  goals stay on the strip forever, and after restart the recovery `GoalsChanged` auto-expands
  every finished goal from prior sessions. Siblings use `has_open_items`/`has_running`.
- **L8.** Inert `selected` affordance on `GoalPanelState` (`goal_panel.rs:11, 53-57`) — nothing
  can change it (`on_key` is a no-op; clicks only toggle expansion). No stale-selection bug
  found; it reads as a half-wired feature.
- **L9.** `unwrap_or_default()` on row reads hides real failures (`sqlite.rs:210`, `:442-444`);
  `idx_goals_status` / `idx_goals_updated_at` (`:59-60`) are dead (no query filters on them).

## Doc drift found during the review (same class as the 2026-10-15 sync pass)

- `book/28_chapter_goal_zh.html` §9 promises an expanded view with per-criterion **evidence**,
  **recent events**, and **pause/resume/cancel/accept actions** (§242) — the shipped panel renders
  criteria status only; `evidence_count` (`driver.rs:462`) has no renderer; `on_key` is a no-op.
- §12 implies budgets are shown for active runs, but `elapsed_ms`/`wall_clock_limit_ms`
  (`protocol/agent.rs:269-270`) are never rendered — only turns/tools counts are.
- `/goal create` title is truncated at 80 chars while the objective keeps the full text
  (`tui/handlers/mod.rs:529-532`), contradicting §41's "文本同时作为标题和 objective".

## Verified sound (no action)

- Lease fencing is done **in SQL** (`WHERE owner = ? AND epoch = ?` inside `BEGIN IMMEDIATE`,
  busy-timeout serialized) — no TOCTOU; epoch monotonic; interrupted runs are immutable
  (`state='running'` guards on `finish_run`/`interrupt_running`).
- `update` CAS is sound (`WHERE id = ? AND version = ?` + `rows_affected` check); all SQL is
  parameter-bound; schema init and the `tasks.goal_id` migration are idempotent.
- Happy-path persist-before-notify ordering holds; lease release is a single site on every
  return path; `active_goal_id` binding restored unconditionally.
- `AgentLoopStop → GoalRunOutcome` mapping is exhaustive; completion detection uses durable
  `completion_proposed` events timestamped against `run.started_at` — observable state, not
  model self-report.
- Driver serializes all mutating goal commands behind the in-flight run's `JoinHandle`.
- TUI: `GoalsChanged` is full-replacement everywhere (no delta path); the new goal render unit
  has the required buffer-level blank-bg test (`tui/render/task_panel.rs:234-259`); no ratio math
  in budget rendering; slash parsing of all eight verbs validated; goal `UserCommand`s surface
  through `user_cmd_tx` directly (bridge `Err(())` is not silently dropped); mouse routing scoped
  to the strip; subagent view correctly absent from overlay checks.
- Tools: all five input structs carry `deny_unknown_fields`; `non_empty` trims; active-goal lookup
  fails closed; permission/resource declarations match the task-tool siblings; no
  `unwrap`/`expect`/truncating casts in production paths of the reviewed files.

## Suggested fix order

1. H1 (one-line, guards real user-visible data loss) → H2 → H3.
2. M1, M2, M3 (+ `release_lease` expiry), M4.3 (one-line transition-table fix).
3. M5, M6, M7, M8, M9.
4. LOW batch + trim Ch28 §9/§12 to delivered behavior.
