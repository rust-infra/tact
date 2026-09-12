# Plan: Turn Statistics on the TUI Bottom Bar

Spec: [`../specs/2026-09-12-turn-stats-bottom-bar-design.md`](../specs/2026-09-12-turn-stats-bottom-bar-design.md)
Date: 2026-09-12
Status: **implemented**

## Steps

1. **Protocol event** — add `AgentUpdate::TurnStats { turns_taken, max_turns }`
   (`crates/protocol/src/agent.rs`).
2. **Agent emit** — emit it from `Agent::agent_loop` right after
   `self.turns_taken += 1` (`crates/tact/src/agent/mod.rs`), so the count is
   correct on the iteration that trips a cap.
3. **State** — add `turn_user`, `turn_llm`, `turn_llm_cap`, `turn_last_secs`,
   `turn_done`, `turn_total_secs` to `StatusBarState` + `new()`
   (`crates/agent_tui_kit/src/state/status_bar_state.rs`).
4. **Component claim** — claim `TurnStats` in `StatusBarComponent::on_update`
   (`crates/agent_tui_kit/src/components/status_bar.rs`).
5. **Shell wiring** (`crates/tui`):
   - `coordinator_prepass`: register `TurnStats` as per-call metadata in **both**
     the thinking gate and the loading gate (`widgets/state/app/agent.rs`) —
     required, see step 8.
   - `handle_agent_update`: add it to the "dispatch-only, no tail work" arm.
   - `handlers/skills.rs::dispatch_user_task`: `turn_user += 1`, reset
     `turn_llm` / `turn_llm_cap`.
   - `widgets/state/app/popups.rs::add_task_end_separator`: accumulate
     `turn_last_secs` / `turn_done` / `turn_total_secs` inside the existing
     `Some(start)` branch only.
   - `widgets/state/app/messages.rs::load_history`: seed `turn_user` on both the
     `Text` and `Blocks` user-message paths (skipping blank text, matching the
     display skip).
6. **i18n** — add `bottom_turns`, `bottom_llm_turns`, `bottom_avg` (EN + ZH)
   (`crates/agent_tui_kit/src/i18n.rs`).
7. **Render** — add `ICON_TURNS` / `ICON_LLM_TURNS` / `ICON_ELAPSED` and
   `format_mm_ss` / `format_turn_user` / `format_turn_llm` / `format_turn_timing`;
   push the two new droppable groups on row 2 in survival order
   `ctx > turns > ∑ > cache > timing`
   (`crates/agent_tui_kit/src/render/bar.rs`).
8. **Metadata-gate regression fix** — the first cut of step 5 did not register
   `TurnStats` in `coordinator_prepass`, so the per-iteration event dropped the
   loading spinner. Caught by the new test
   `turn_stats_is_metadata_and_keeps_the_loading_placeholder` (written to fail
   first), then fixed.
9. **Tests** — see below.
10. **Docs** — `docs/token_usage_schema.md`, `book/23_chapter_tui.md` §6.6 +
    `_zh.md`, `book/26_chapter_issue.md` + `_zh.md`.
11. **Verify** — `cargo test` per crate, `cargo clippy --all-targets`,
    `cargo fmt --check`, all with `no_proxy` set for loopback tests.

## Tests added

| Crate | Test |
|---|---|
| `tact` | `agent_loop_emits_turn_stats_each_iteration` |
| `tact` | `agent_loop_turn_stats_carries_the_cap_and_counts_the_capped_turn` |
| `agent_tui_kit` | `turn_stats_updates_loop_counter_and_cap` (incl. stale-cap clearing) |
| `agent_tui_kit` | `format_turn_counters_labeled`, `format_turn_timing_last_and_average`, `format_turn_timing_omits_average_before_any_turn_completes`, `format_turn_timing_hour_scale_stays_mm_ss`, `turn_bar_icons_are_narrow` |
| `tui` | `bottom_bar_shows_turn_counters_and_timing_on_row_2` |
| `tui` | `bottom_bar_omits_llm_segment_before_first_turn` |
| `tui` | `bottom_bar_omits_turn_average_before_any_turn_completes` |
| `tui` | `bottom_bar_drops_turn_timing_before_cache` (width sweep, monotonic invariant) |
| `tui` | `turn_stats_update_reaches_status_bar` |
| `tui` | `turn_stats_is_metadata_and_keeps_the_loading_placeholder` |
| `tui` | `completed_turns_accumulate_timing_and_average` |
| `tui` | `synthetic_separator_does_not_accumulate_turn_timing` |
| `tui` | `submit_user_task_counts_session_turn_and_resets_llm_counter` |
| `tui` | `queued_messages_each_count_as_a_session_turn` |
| `tui` | `load_history_seeds_session_turn_counter` |
| `tui` | `load_history_skips_blank_user_text_when_seeding_turns` |

## Deviations from the spec

- **Cap is not rendered.** Confirmed during implementation that only
  `spawn_subagent` sets `max_turns`; the spec's `⇅ {n}/{cap}` would never appear
  in the main-agent bar. Rendered as bare `⇅ {n}` and documented as such; the
  field stays plumbed.
- **`TurnStats` added to `coordinator_prepass`.** Not in the original spec text,
  but required: without it the metadata event ran the content gates and dropped
  the loading spinner at the start of every task.

## Original drop-order claim

The spec's first draft claimed the drop order was "unchanged". It is **not**
identical — the new segments necessarily interleave. The preserved invariant is
the *relative* order of the pre-existing segments (`ctx > ∑ > cache`), with
turns inserted between `ctx` and `∑` and timing dropping first. `book/23` and
`docs/token_usage_schema.md` state the new order explicitly.
