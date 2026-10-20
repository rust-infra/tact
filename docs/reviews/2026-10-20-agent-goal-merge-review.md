# Code Review — agent-goal, pre-merge pass

Date: 2026-10-20. Scope: the whole `wt/agent-goal` branch against `main`
(24 commits, ~27k lines across 135 files) immediately before merging. Reviewed
by reading the final source rather than the diffs alone, and by re-running the
workspace gates on both the committed tree and the uncommitted working tree.

**Headline:** the branch is mergeable. Five defects were found and fixed in this
pass, each with a test that fails when its fix is reverted. Two items raised
during review were withdrawn after checking the evidence; they are recorded
below so the reasoning is not lost.

---

## Fixed in this pass

### H1. The lease heartbeat retried forever, silently
`spawn_lease_heartbeat` ended only on `StaleLease`, and did so without saying
anything; every other store error fell into an `Err(_)` arm whose comment read
"Transient store error: try again on the next tick". A store that kept failing
therefore never stopped: the lease lapsed, another process took it over, and
this run's `finish_run_and_transition` was fenced out — its outcome dropped with
no trace. Both endings now report through the runner's UI channel, and the
failure path stops after `HEARTBEAT_MAX_CONSECUTIVE_FAILURES` (3, about the
original TTL) with a success clearing the counter.

### H2. The last permitted turn's completion proposal was thrown away
The turn cap is checked *before* the model call, so an activation that spent its
final turn proposing completion had that tool call skipped on the next
iteration and was stopped as `MaxTurns`. The proposal never reached the store,
and the run was recorded as `budget_exhausted` next to a Goal the verifier had
never seen. `agent_loop_detailed` now ends such an activation as `Completed`
once its tools have run, so the Runner maps the verdict it actually got. The cap
itself is unchanged.

### H3. A dropped activation stranded the workspace lease
`activate` released the lease only on its normal return path. The command loop
hands the `Agent` to a spawned task, so a cancelled or panicked task skipped
that release and left the lease held for `max_elapsed + 60s` (~11 minutes by
default) — answering `LeaseHeld` to every `/goal resume` in every session, while
`goal_runs` kept a `running` row nobody owned. `LeaseReleaseGuard` spawns a
fenced, best-effort release from `Drop`.

### M1. Event lookups could not use an index
`latest_event_of_kind` / `latest_event_with_status` / `has_event_of_kind_since`
all filtered on `json_extract(data, '$.kind')`. No index can serve an
expression, so `LIMIT 1` scanned the Goal's rows until the first match — and
these run several times per activation. `goal_events` now carries `kind` /
`status` / `source` projection columns written in the same INSERT as `data`,
with `(goal_id, kind, seq)` and `(goal_id, status, seq)` indexes. A legacy
database is backfilled once, keyed on `kind = ''`.

### M2. "During this run" was a millisecond comparison
Events written in the same millisecond as a run's start could not be ordered
against it, and `>=` counted a previous run's proposal as this one's — which
lets a Goal succeed on a stale verdict. `GoalRun` now carries `event_seq_anchor`,
stamped by the store inside the transaction that inserts the run row (so
concurrent appends fall cleanly before or after it), and the runner asks
`seq > anchor`. A `0` anchor means a pre-field record and falls back to the
timestamp window; the two are never mixed.

### L1. Recovery notices named runs another session owned
The 15-second reconcile pass is workspace-wide, but `ui_tx` reaches one session.
Naming each reclaimed run told this user about work they never started while
staying invisible to the session that was actually waiting. The timer now emits
one summary line and leaves the per-Goal truth to the shared `GoalsChanged`
snapshot, which every session receives.

### Also: `GoalEvent::source`
`book/28` §8 requires that a model tool can never forge a user decision; until
now that held only because no tool happened to write `user_accepted`. Every
event now records its author (`model` / `user` / `runner`, defaulting to
`runner` so a legacy row cannot read as a user decision), and the store's single
write path refuses a model-authored `user_accepted` / `user_answer`.

---

## Withdrawn after checking the evidence

### `GoalEventKind` has no `Other` fallback
Raised as Important on the assumption that rows with unmodelled kinds existed on
disk. The user's call was that old data does not matter, and it does not: the
event log is a new table on this branch. Closed by decision, not by a code
change.

### Removing `Goal.constraints`
Raised as Important on the assumption that it removed a shipped capability. It
did not: the field never existed on `main`, no entry point ever wrote a non-empty
value (`/goal create` does not pass it, and `CreateGoal` has no such field), and
every row in the local database held `[]`. Withdrawing an unused, unshipped
field needs no migration and no changelog entry. My original finding was
over-called; the correct severity was "no action".

---

## Verified sound

- **Lease protocol**: `process_identity` separates "exists" from "describable"
  (EPERM is liveness, not absence) and guards `pid > i32::MAX`; takeover happens
  only on expiry or a provably gone holder; `start_run_with_lease` re-checks
  `Running` inside its transaction, closing the load-then-start race.
- **Finish transaction**: `finish_run_in_tx` marks the run completed before
  applying transitions, and a lease-less status write is refused while a run is
  active — the "paused but run active" state is unreachable.
- **Budget accounting**: the cap is checked before `turns_taken` advances, and
  the outcome is a delta, so a multi-cycle run cannot spend double.
- **Injected context**: `<goal-context>` is tagged, neutralised, bounded, and
  closed even when truncated; `MessageKind::GoalContext` does not persist, does
  not count as a user turn, and is skipped by `load_history`.
- **Verifier**: a pure function whose `Verified` is exactly
  `validate_acceptance`'s `Ok`; a Goal with no required criteria is recorded as
  self-attested rather than verified.
- **Cancel precedence**: `requested_stop` is checked before and after each
  activation, cancel outranks pause, and a mid-stream cancel abandons pending
  tools before booking the response.
- **Slash completion**: `goal_verb_candidates` filters by the store's own
  transition table, so an offered row is one the driver will actually accept.

---

## Verification

- `cargo test --workspace` → 29 targets green: tact 1413, tact-ui 132, tui 750,
  agent_tui_kit 428, tact_llm 281
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo fmt --all -- --check` clean
- `scripts/check-rust.sh` green before the push
- Every fix above was mutation-checked: reverting it makes its test fail.

Docs synced in the same pass: `book/28` §4/§7/§8, `book/26` (three entries),
`book/01` (event projection columns, `event_seq_anchor`, lease holder columns),
`book/18` (the last-turn verdict exception), and the regenerated
`book/28_chapter_goal_zh.html` twin.
