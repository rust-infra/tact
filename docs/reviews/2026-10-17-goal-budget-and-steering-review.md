# Code Review — Goal budgets, steering and TUI handoff (`wt/agent-goal`)

Date: 2026-10-17. Scope: the two commits that changed Goal budgets and the TUI
handoff (`e2bd559f`, `e4214bdc`) plus the review follow-up. Reviewed by reading
the final source (not the diffs alone) and re-running the affected suites.

**Headline:** no CRITICAL findings. The review found two HIGH accounting/UX
bugs, one MEDIUM edge case, and three domain invariants that the earlier
2026-10-15 review had left open. All are fixed in this branch. The remaining
items are LOW or outside this repository (the external harness that injects
`<goal-context>` without binding `active_goal_id`).

> **Follow-up (2026-10-17, second round):** the lease/control conclusions below
> were superseded by the Ch 26 entry “Goal 执行控制与 lease 第二轮修复”. Pause,
> steer and cancel now act on the shared store while a run is active (without
> awaiting its `JoinHandle`), a live lease is never re-issued even to the same
> owner label, and acceptance is one SQLite transaction. This document remains
> the record of the first review round.

---

## HIGH

### H1. A multi-cycle Run could spend ~2× its cumulative turn budget
- `GoalRunner::run_activation_cycles` installed `turns_taken + max_turns` on
  every cycle, while `permit_activation` only checked the *cumulative*
  `run.turns_used` after the cycle returned.
- A first cycle using 490 of 500 turns still left `budget_left == true`, and the
  next cycle received a fresh 500-turn cap: the run could reach ~999 calls.
- The same accounting counted the probe that trips the cap as a spent turn, so
  `max_turns = 0` reported `turns_used = 1`.
- **Fix:** each cycle receives `max_turns - run.turns_used` (`remaining_turns`),
  and `agent_loop_detailed` checks the cap **before** incrementing
  `turns_taken`. The over-limit probe is no longer a reported model call.
- Regression tests: `run_once_never_overspends_the_cumulative_turn_budget`,
  `agent_loop_detailed_reports_max_turns`, `agent_loop_max_turns_zero_stops_immediately`.

### H2. Empty/stale Goal snapshot made `/goal steer` mis-target
- With no snapshot, the TUI treated the first word of `/goal steer <text>` as a
  Goal id; with a lagging snapshot (a fresh `/goal create` is still `draft`
  while its activation runs) the implicit fallback needed a second source of
  truth.
- `/goal accept` also had an empty completion list on a fresh TUI because the
  driver never published the durable inventory until some later command.
- **Fix:** the driver emits `GoalsChanged` once at command-loop start (after
  recovery). `UserCommand::SteerGoal.goal_id` is now `Option`; the TUI forwards
  the raw instruction unless it can confirm an id, and the driver resolves
  against the store: explicit id/unique prefix → unique `running` Goal → unique
  non-terminal Goal. Ambiguous explicit prefixes and an id with no instruction
  are errors, never guesses.
- Tests: `steer_target_resolution_is_unambiguous`,
  `steer_pause_and_cancel_goal_are_durable_before_snapshot`, the TUI
  `goal_steer_*` tests.

## MEDIUM

### M1. `/goal steer <id>` with no instruction replayed the id as the text
- The no-id resolution treated a known id followed by nothing as instruction
  text and steered the implicit target with it.
- **Fix:** `Ok(Some(_))` with an empty remainder is now
  `goal steering text must not be empty`; covered by the driver steering test.

### M2. An all-optional acceptance list could reach `Succeeded`
- `Goal::validate_acceptance` only rejected an *empty* list; an all-optional
  list passed vacuously. `GoalVerifier` already refused it, so the invariant
  depended on call-site discipline.
- **Fix:** `NoRequiredAcceptanceCriteria`; the verifier test that pinned the
  divergence now asserts the closed invariant.

### M3. Rejected criteria could be resurrected by new evidence
- `Goal::update` unconditionally applied evidence and set `Passed`.
- **Fix:** evidence for a `Rejected` criterion is refused
  (`CriterionRejected`); redefining the criteria is the only way back.

### M4. Duplicate criterion ids were accepted
- Claims and evidence address a criterion by id; duplicates made that ambiguous.
- **Fix:** `DuplicateCriterionId` on create and on criteria updates.

### M5. Acceptance raced the runner and failed after being advertised
- `/goal accept` required `status == waiting_user`. A message sent in that
  window can wake the runner; the activation ends `no_progress`, and the Goal is
  left `running` with no active run. The UI had just offered acceptance, but the
  command now answered “is Running; acceptance requires waiting_user”.
- **Fix:** `accept_goal` also accepts `paused` Goals (user pause or recovery)
  and a `running` Goal when no activation is in flight (the store's active-run
  list is the authority); an in-flight run still refuses, naming it. The TUI
  `accept` completion offers `waiting_user`, `paused`, and
  `running + run_budget == None` Goals for the same reason. Evidence and the
  verifier are unchanged, so the human decision still cannot bypass a criterion.
- Successful acceptance now emits `🎯 Goal <id> accepted — succeeded`, so the
  last visible log line is not the stale “run /goal accept” instruction.
- Tests: `accept_goal_accepts_a_stranded_running_goal_with_no_active_run`,
  `accept_goal_accepts_a_recovered_paused_goal`,
  `accept_goal_refuses_while_an_activation_is_in_flight`,
  `accept_offers_a_running_goal_with_no_active_run`,
  `accept_offers_a_paused_goal`.

## LOW / deliberately left

- **L1. Startup snapshot auto-expands the Goal panel** whenever a Goal exists
  (`GoalPanelState::apply_snapshot` expands on first visibility). This is the
  pre-existing 2026-10-15 L7 behavior, now deterministic instead of accidental.
  A dedicated "silent snapshot" flag would be protocol churn for a cosmetic
  issue.
- **L2. `GoalRun.tokens_used` is still never populated.** `AgentLoopOutcome`
  carries turns and tool calls but not token usage; plumbing it would touch the
  streaming/accounting path. The run summary remains correct for the budgets
  that are enforced.
- **L3. Goal panel labels are still English literals** under a localized title
  (2026-10-15 M7). Unchanged.
- **L4. External goal-context injection without binding.** In a session driven
  by the external harness, `goal_propose_completion` returns “no active Goal is
  bound”. The repository binds `agent.tool_context.active_goal_id` in
  `GoalRunner::run_with_lease`; an injector that writes `<goal-context>` itself
  must bind the same field. Do not weaken the tools: failing closed is correct.

## Verified sound

- Lease heartbeat: aborted and awaited on every exit; a takeover stops it.
- `finish_run` / `release_lease` fence on owner+epoch; the run row still must
  be `running` (a takeover interrupts it, so the finish fails).
- Config resolution: defaults have one source (`GoalSettings`), `0` is rejected,
  every `ResolvedConfig` construction site is updated, `[goal]` is
  `deny_unknown_fields`.
- `TaskIdle` is emitted only by the driver's Goal paths and clears
  `Planning`/`Executing`; queued input then flushes through the existing
  `flush_pending_when_idle`.
- Final-turn reminder lives only in that request's system prompt; it cannot
  leak into the conversation or fire for subagents (no active Goal).
- No new unbounded reads: the runner uses `count_events`, `list_recent_events`
  and targeted lookups.

## Verification

- `cargo test -p tact --lib` → 1409 passed, 1 ignored
- `cargo test -p tact-ui --lib` → 108 passed
- `cargo test -p tui --lib` → 742 passed
- `cargo test -p tact_protocol --lib` → 27 passed
- `cargo test -p agent_tui_kit --lib` → 424 passed
- `cargo clippy --all-targets` (tact, tact-ui, tui, tact_protocol,
  agent_tui_kit) and `cargo fmt --all --check` clean.
