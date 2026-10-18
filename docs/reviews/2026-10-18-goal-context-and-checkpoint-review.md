# Code Review — Goal checkpoint protocol, injected-context identity, idle reporting

Date: 2026-10-18. Scope: the fix pass that followed a live Run
(`ef770062`, 36/500 turns, 63 tool calls, outcome `no_progress`) whose user
report was “既然完成了，也没看到 checkpoint 啊，而且状态也不同步？”. Reviewed by
reading the final source (not the diffs alone) and re-running the affected
suites.

**Headline:** no CRITICAL findings. Three real defects (one HIGH, two MEDIUM)
were found in the live run's evidence and fixed; the “state out of sync” report
turned out to be a wording defect, not a state-machine defect. Two LOW items
are recorded below with the reasoning for leaving them open. One earlier claim
of mine was wrong and is corrected here.

---

## HIGH

### H1. The checkpoint reminder existed only on the turn-cap path
- `agent/mod.rs`: `goal_final_turn = active_goal_id.is_some() && max_turns == Some(turns_taken)`.
  The `<goal-budget-reminder>` therefore fires on exactly one turn — the last
  permitted one of a turn-capped run.
- A run that ends `no_progress` (a text-only cycle), on a yield, or on a cancel
  never reaches that turn. Evidence: the run used 36 of 500 turns, and the
  workspace database had **zero** `checkpoint` events, ever.
- `book/28_chapter_goal_zh.md` §7 promised “最后一轮会提醒模型写 checkpoint”,
  which was true but covered almost none of the endings.
- **Fix:** every injected `<goal-context>` now carries a
  `<checkpoint-protocol>` section (one standing instruction: call
  `goal_checkpoint` before the activation ends, `goal_propose_completion` when
  the objective is met). The `<goal-budget-reminder>` stays as the last chance
  on the cap path — it is still the only enforcement that can act mid-run.
- Tests: `goal_context_is_labelled_bounded_and_includes_criteria` now asserts
  the section and that it names `goal_checkpoint`.

### H2. A run that wrote no checkpoint said nothing about it
- The `🎯 … run finished: NoProgress` line carried the outcome and nothing else.
  The next activation's `<latest-checkpoint>` then read `(none)`, and nothing
  connected the two facts — the user had to query the events table to learn that
  no checkpoint was ever written.
- **Fix:** before the finish transaction, the runner probes
  `has_event_of_kind_since(goal, "checkpoint", run.started_at)` (the same
  indexed, bounded probe `map_completion` already uses) and, when the run wrote
  none and the Goal is not terminal, records `note=no checkpoint written` on the
  durable `run_finished` event **and** appends `(no checkpoint written)` to the
  log line — one shared constant (`NO_CHECKPOINT_NOTE`), so the two surfaces
  cannot drift. No synthetic checkpoint is fabricated and no extra model call is
  made.
- The terminal case is deliberately excluded: a Goal that just succeeded owes no
  further checkpoint, and the note would read as a defect where nothing is owed.
- Tests: `run_once_reports_a_run_that_wrote_no_checkpoint`,
  `run_once_stays_quiet_when_the_run_checkpointed`,
  `run_once_completes_a_criterialess_goal_as_self_attested` (asserts the note is
  absent).

---

## MEDIUM

### M1. The injected `<goal-context>` was an ordinary user message
- `run_activation_cycles` pushed it as `Message::new_text(Role::User, injected)`
  with no kind marker. Consequences:
  - `compact::is_real_user_message` classified it as a **real user turn**, so
    every compaction rebuild carried a stale Goal snapshot forward as if the
    user had typed it.
  - `tui::…::load_history` rendered it as a user turn and incremented
    `turn_user`, so a reloaded session showed a fake turn and a wrong counter.
  - Four such user-role rows already exist across the workspace's sessions
    (a `LIKE '%<goal-context>%'` count over-reports — assistant turns and tool
    results quote the text too — so the check has to parse the JSON and match on
    the leading tag).
- **Fix:** the frame has a name and an identity now —
  `goal::GOAL_CONTEXT_OPEN_TAG` / `GOAL_CONTEXT_CLOSE_TAG` +
  `goal::is_goal_context_text`, and `MessageKind::GoalContext` with
  `Message::is_goal_context()`. That is the same two-signal pattern hook context
  already uses, and for the same reason: the kind is `#[serde(skip)]`, so a
  reloaded session only has the marker text.
  - `is_real_user_message` skips both signals.
  - `load_history` skips the cell entirely (see L1 for why it is not rendered).
- Tests: `goal_context_marker_recognizes_only_the_injected_frame`,
  `goal_context_is_not_a_real_user_message`,
  `run_once_injects_the_goal_context_as_a_goal_context_cell`,
  `load_history_skips_goal_context_cells`,
  `injected_context_kinds_do_not_collide`.
- **Accepted trade-off:** excluding the cell from `is_real_user_message` means a
  compaction that lands mid-activation drops the current context until the next
  cycle re-injects it (the summary instructions do not describe Goals). This is
  exactly what hook context does (“let it age out”), and the Goal context is
  rebuilt every cycle, so the cost is bounded. The alternative — teaching the
  summarizer about Goals — is a separate design change.

### M2. A truncated context was an unclosed frame
- `build_goal_context` appended `</goal-context>` and *then* applied the
  `MAX_GOAL_CONTEXT_CHARS` backstop. If the backstop ever fired, the emitted
  document kept the opening tag and lost the closing one, so a reader matching
  on the marker would treat everything after it as context.
- **Fix:** the bound is applied first and the closing tag appended after it, so
  a truncated document is still a closed frame.
- Covered by the existing bounded-context assertion (the frame is now
  `≤ MAX_GOAL_CONTEXT_CHARS` including its closing tag).

---

## The “state out of sync” report

The user's second complaint was not a state-machine bug. Verified in the store:

- The Goal stays `running` with no active run after `no_progress`. That is
  designed and documented (Ch 28 §7/§12): another activation is an explicit
  action, and startup recovery is what parks it (`running` + no active run →
  `paused`).
- The panel already says so (`goal_panel.rs`: `running (idle)` when
  `run_budget.is_none()`), and `emit_goals_changed` runs after `run_once`.
- The lease row (`epoch 3`, `expires_at` in the future, holder pid alive) is
  consistent: the run is finished, so the 15-second reconciler — which only
  looks at `state = 'running'` rows — correctly ignores it.

What was actually wrong is the **wording**: the log line read
`run finished: NoProgress` next to a panel row reading `running`, with nothing
bridging them. **Fix:** when the finished run leaves the Goal `running` with no
active run, the same line now ends with `— Goal idle; /goal resume to continue`.
That is a presentation change; the state machine is untouched.

---

## LOW / deliberately left open

### L1. The injected context is skipped in the TUI, not shown as a notice
- Hook context is rendered as a labelled barred block; goal context is dropped
  from the log entirely. The reasoning: it is *derived* data (objective,
  criteria, linked tasks, latest checkpoint, recent events) that the Goal panel
  already presents, and it is re-injected every activation — one block per cycle
  would push the conversation off the screen while adding nothing the panel does
  not say. The decision is documented at `is_goal_context_cell`.
- If a future surface wants it, the hook-context renderer is the pattern to
  copy (strip the frame, label the block).

### L2. `GoalRun.tokens_used` is still never populated
- Raised as L1 in the 2026-10-15 review and again as L2 in the 2026-10-17
  review; unchanged here. `AgentLoopOutcome` carries turns and tool calls but no
  token usage, and `SessionStats` holds only cache-hit / cache-miss / reasoning
  counters (no plain prompt/completion totals), so a per-activation delta would
  require new accounting plumbing on the streaming path.
- It is surfaced nowhere: `run_summary` reports
  `outcome / turns_used / tool_calls_used / elapsed_ms / stop_reason / checkpoint`,
  and the panel's budget row shows turns and tool calls. Nothing reads the field
  except the default-value assertion in `goal::tests`.
- Left open on purpose: it is dead weight today, and inventing an accounting
  path to fill a field nobody displays is churn. If token budgets ever become an
  enforced limit, this becomes a real item — the field already exists, so the
  change stays additive.

### L3. `MockClient::new` cycles its script (`idx % len`)
- Not a product defect, but it cost a test iteration: a two-entry script meant
  to express “one tool call, then a text-only turn” repeats the first entry
  forever, so a test written against it never reaches the second turn. Use
  `MockClient::with_responder` when the script has to depend on the call index.
  Recorded in the Ch 26 entry for the next person.

---

## Verified sound

- **The context is rebuilt per cycle**, not once per run: `run_activation_cycles`
  re-reads the Goal, its linked tasks, and the event facts at the top of every
  iteration, so a checkpoint appended by one cycle is visible to the next.
  (An earlier claim of mine — that the context was built once and never
  refreshed — was wrong; the duplicate `<goal-context>` cells a reload shows are
  history replay of the persisted per-cycle injections, not a stale rebuild.)
- **Injection still goes through `agent_loop_detailed(Some(message))`**, so the
  context participates in the turn like before; only its kind changed. The
  Responses path's `replace_persisted_context_and_state` rewrites persisted
  context from `runtime.context`, which is why “do not persist it” was not a
  viable fix and filtering at the readers is the right layer.
- **`is_real_user_message` remains the single filter** used by
  `collect_user_messages`; no second copy of the predicate was introduced.
- **The finish path still persists before it notifies**: the checkpoint probe
  runs before `finish_run_and_transition`, and the log line is emitted after the
  transaction commits — the module's invariant 1 is intact.
- **No new unbounded reads.** The two additions are one indexed `LIMIT 1`
  existence probe (`has_event_of_kind_since`) and one
  `list_running_runs()` (a bounded list of active rows, already called on the
  same path by `recover_dead_runs` and the driver).
- **Hook-context behaviour is unchanged**: the goal-context branch sits beside
  it, filters on its own kind and marker, and cannot swallow a hook cell (a
  `<hook-context>` frame does not start with `<goal-context>`).
- **`MessageKind` still never reaches the wire**: the new variant rides the same
  `#[serde(skip)]` field, and `injected_context_kinds_do_not_collide` pins the
  three variants apart.

## Verification

- `cargo test -p tact --lib` → 1407 passed, 1 ignored
- `cargo test -p tact-ui --lib` → 126 passed
- `cargo test -p tact-ui --test driver_integration` → 10 passed
- `cargo test -p tui --lib` → 750 passed
- `cargo test -p tact_llm --lib` → 280 passed, 17 ignored
- `cargo test -p agent_tui_kit --lib` → 428 passed
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo fmt --all` applied

Docs synced in the same pass: `book/26_chapter_issue_zh.md` (newest-first
entry), `book/28_chapter_goal_zh.md` §5/§7/§9/§11/§12,
`book/05_chapter_compact_zh.md` (the two `is_real_user_message` exclusion
lists), `docs/compaction.md` step 4.
