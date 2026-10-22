# Chain Review — Goal Runner execution path (`wt/agent-goal`)

Date: 2026-10-15. Scope: does the Goal chain actually run end to end? Triggered by hand-driving
the feature: a freshly created Goal reported a finished run within milliseconds. The review
followed the execution path only — `driver.rs` → `goal_runner.rs` → `agent_loop_detailed` — plus
the state machine and the documented flow (§3/§5/§7 of `book/28_chapter_goal_zh.html`). Every claim
below was checked against the source and against the persisted rows in `.tact/tact.db`.

**Headline: the chain was broken at its first step.** A Goal activation could not make a single
model call in any session that had already taken 24 turns, and nothing drove a second cycle even
when one was possible. Three smaller drifts (a terminal state with no writer, an outcome with no
writer, and a documented-but-absent cycling edge) sat behind it.

## Evidence: two Goals, identical run rows

| Goal | status | run outcome | turns | tool calls | tokens | elapsed | stop reason |
|---|---|---|---|---|---|---|---|
| `0443a985-cd85-4f92-b19a-c3c037374fed` | `paused` | `budget_exhausted` | 1 | 0 | 0 | 1 ms | `max_turns` |
| `03a4caef` | `paused` | `budget_exhausted` | 1 | 0 | 0 | 13 ms | `max_turns` |

Events read `Running: run ended as BudgetExhausted` → `paused`. Zero tokens and 1–13 ms mean no
model call happened at all: the turn cap is checked before the request is built
(`crates/tact/src/agent/mod.rs:1295-1306`), so 13 ms is prompt assembly and SQLite.

## HIGH — the per-activation turn budget is compared against a process-lifetime counter

- `crates/tact-ui/src/goal_runner.rs:537` set `agent.max_turns = Some(self.limits.max_turns)` (24).
- The loop's check is `self.turns_taken += 1; if self.turns_taken > max { MaxTurns }`
  (`crates/tact/src/agent/mod.rs:1295-1306`), and `turns_taken` is **never reset**: it is
  initialised once at construction (`mod.rs:541`) and `agent_loop_detailed` only reports a delta
  against it (`mod.rs:1207-1219`). The field's own contract is cumulative
  (`mod.rs:444-452`).
- Therefore any session that had already taken 24 turns stopped the activation on its first
  iteration: `MaxTurns` → `BudgetExhausted` → `Paused`, with no model call. 24 turns is a few
  minutes of ordinary tool use, so in practice **every** Goal was affected.
- Introduced by `922635fa` ("cap one activation", H2 of the earlier feature review). Before it the
  activation was unbounded, which is why the feature worked when first demoed.
- Subagents never hit it: they get a fresh `Agent` (`crates/tact/src/tool/subagent.rs`), so their
  counter starts at 0. Every runner test started at `turns_taken == 0` too — the suite could not
  see the difference.

**Fix:** express the activation budget on the counter's own scale
(`Some(agent.turns_taken.saturating_add(self.limits.max_turns))`), plus a regression test that
warms the counter to 500 first.

## HIGH — no cycling: one activation per command, and `progress_made` left the Goal stranded

`GoalRunner::run_once` performed exactly one activation and `resume_goal` called it once
(`driver.rs:675-687`). §5's flowchart has an edge back from "outcome" to "load" when there is
progress and budget; the code had no such edge. A `progress_made` run therefore ended with the
Goal `Running` and no active run — nothing scheduled the next activation, and only a restart's
recovery (`Running` + no active run → `Paused`, `goal_runner.rs:413-431`) would move it.

**Fix:** the cycle lives inside one run: several bounded activations share a single lease, a
single `GoalRun` record and a single finish transaction. Another cycle is only spent when the
previous one left *observable* progress behind — an executed tool call or a new Goal event — so a
cycle that only produced text ends the run instead of buying another model call that would
re-read the same context. The run's cumulative usage is what the budget bounds, which also bounds
the cycle count (every cycle costs at least one model call).

## MEDIUM — `BlockedByHook` left the Goal `Running` behind a finished run

`map_stop` mapped it to `(FatalError, Vec::new())` (`goal_runner.rs:667`), so the run was recorded
as finished while the Goal stayed `Running` — a state the UI cannot distinguish from "work in
flight". `Running → Failed` is a legal transition. **Fix:** transition to `Failed`.

## MEDIUM — documented states and outcomes with no writer

- `GoalStatus::WaitingPermission` and `GoalRunOutcome::NeedsPermission` are in the state machine,
  the protocol and §3, but nothing produces them. The reason is architectural, not an oversight:
  the permission gate decides by itself when no UI is attached (`ask_user` denies high-risk and
  allows the rest, `crates/tact/src/permission/mod.rs:409-413`), and an interactive prompt is
  answered inside the activation, so an activation is never parked on a permission question.
  **Fix:** keep the vocabulary, but say so in the code and in §12.
- `GoalRunOutcome::NoProgress` had no writer either, and the run that should have produced it was
  mislabelled `progress_made`. **Fix:** a cycle with no observable progress is recorded as
  `no_progress` (the Goal stays `Running`).

## Verified as covered

Draft → Queued → Running activation; lease claim/release and fencing; run persisted before the
first model call; atomic run finish + transitions + event; completion proposal → verifier →
`verifying` → `succeeded`/`waiting_user`; `GoalYield` → `waiting_user`; cancel → `cancelled`;
recovery (expired lease → `interrupted`, `Running` without a run → `paused`);
`/goal create|list|show|steer|pause|resume|cancel|accept`.

## Left alone deliberately

`/goal create` with no explicit criteria adds a single required **human** criterion, so an
interactive Goal can only succeed through `/goal accept`. That is the documented product decision
(§2, §12), not drift: an automatic default would let a Goal succeed with no verification at all.
