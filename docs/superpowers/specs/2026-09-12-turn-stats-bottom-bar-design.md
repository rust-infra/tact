# Turn Statistics on the TUI Bottom Bar

Status: **implemented** — superseded in part by the
[2026-09-12 row-2 compaction](#amendment-2026-09-12-row-2-compaction) below

> **Amendment (2026-09-12, after shipping):** this design added three segments to
> an already-dense row, which pushed row 2 to ~138 columns and made `fit_row_spans`
> drop segments on ordinary terminals. A follow-up compaction cut it to **97
> columns** without removing any distinct information: the `∑ₜₒₖ {total}` segment
> was deleted (it rendered the same `token_total` the ctx meter already shows as
> `used`), `max_out_token` → `out`, `▣ cache% 30%` → `▣ 30%`, `⟳ 12 turns` →
> `⟳ 12`, and `⏱ 02:05 · avg 01:45` → `⏱ 02:05 avg 01:45`. The survival order in
> §5 is therefore now `ctx > turns > cache > timing` (no `∑ₜₒₖ` slot). See
> `book/26_chapter_issue.md` (2026-09-12, "Bottom-bar row 2 compacted to a
> 97-column budget") and the `bottom_bar_fits_every_segment_in_100_columns`
> width-budget test. Everything else below still describes the shipped behavior.
Date: 2026-09-12
Scope: `crates/protocol`, `crates/tact`, `crates/agent_tui_kit`, `crates/tui`, docs

## Problem

The TUI shows token/cache statistics but nothing about **how many turns** a
session has taken or **how long turns take**. The only timing surface is the
frozen `⏱ mm:ss` on the task-end separator plus the task stats block
(`widgets/state/app/messages.rs::add_task_stats_block`), both of which are
historical log rows — never a live, always-visible counter.

Users asked for three numbers on the bottom bar:

1. how many turns this session has run,
2. how many LLM turns (agent-loop iterations) the current task has run,
3. turn wall-clock timing: the last completed turn and the session average.

## Goals

- Render all three on bottom-bar row 2, next to the existing `ctx` / `∑ₜₒₖ` /
  `cache%` segments.
- Keep the existing "bottom bar reflects the **main** agent only" rule intact.
- Zero new persistence: counts and timings derive from data already in memory
  (or already loaded from the store on resume).
- Every new segment is i18n'd (EN + ZH) and participates in the existing
  narrow-terminal drop logic.

## Non-goals

- No schema change to `token_usages` / `sessions` / `messages`.
- No `/stats` output change (the GFM session summary stays as-is).
- No subagent turn display: subagent loop counters do **not** reach the shared
  UI channel (same rule as subagent `TokenUsage` / `ToolMeta`).
- No historical turn-timing analytics (that would need new persistence).

## Design

### 1. State (`StatusBarState`, `agent_tui_kit/src/state/status_bar_state.rs`)

```rust
/// User turns dispatched in this session (seeded from history on resume).
pub turn_user: u32,
/// LLM turns (agent-loop iterations) completed in the current task.
pub turn_llm: u32,
/// Agent-loop turn cap (`Agent::max_turns`); `None` = unbounded.
pub turn_llm_cap: Option<u32>,
/// Wall-clock seconds of the most recently finished turn.
pub turn_last_secs: Option<u64>,
/// Completed turns and their summed wall-clock seconds (for the average).
pub turn_done: u32,
pub turn_total_secs: u64,
```

All `new()` defaults: `0` / `0` / `None` / `None` / `0` / `0`.

### 2. Protocol (`crates/protocol/src/agent.rs`)

New variant, emitted once per agent-loop iteration (same cadence as
`TokenUsage`, so no new flood risk):

```rust
/// Agent-loop turn counter for the current task, plus the cap when set.
TurnStats { turns_taken: u32, max_turns: Option<u32> },
```

Emitted from `Agent::agent_loop` (`crates/tact/src/agent/mod.rs`) immediately
after `self.turns_taken += 1` — i.e. before the cap check, so the count is
correct even on the iteration that trips a cap. `max_turns` rides along for
future use (see the cap-decision note in §5); it is not rendered today.

### 3. Per-crate plumbing

| Crate | Change |
|---|---|
| `tact` | emit `TurnStats` per loop iteration |
| `agent_tui_kit` | claim `TurnStats` in the status-bar component (`components/status_bar.rs`) → writes `turn_llm` / `turn_llm_cap` |
| `tui` | `dispatch_user_task` → `turn_user += 1`, `turn_llm = 0`, `turn_llm_cap = None`; `load_history` → seed `turn_user` from persisted user messages |

Counter ownership notes:

- `turn_llm` / `turn_llm_cap` are written by the kit component (protocol-driven),
  reset by the shell at task dispatch.
- `turn_user` is TUI-local (there is no protocol event for it): incremented in
  `handlers/skills.rs::dispatch_user_task`, which is the single choke point for
  user turns (direct submits, queued flushes, skill dispatch).
- Resume seeding: `load_history` already walks persisted messages and calls
  `add_user_message` for each user turn; it increments `turn_user` on the same
  path. No new store query.

### 4. Turn timing

`widgets/state/app/popups.rs::add_task_end_separator` is the **only** place that
actually freezes elapsed time (`task_start_time.take()`), and it runs before
`freeze_last_prompt_cost` on both `TaskComplete` and `TaskCancelled` — the
latter finds `task_start_time == None` and is already a no-op. So accumulation
goes there, inside the existing `Some(start)` branch, to avoid double counting:

```rust
self.status_bar_mut().turn_last_secs = Some(s);
self.status_bar_mut().turn_done += 1;
self.status_bar_mut().turn_total_secs += s;
```

Cancelled turns count too — their wall time is real. `add_task_end_separator`'s
`else` branch (no start time, e.g. a synthetic separator) must **not** accumulate.

The bottom bar shows the **frozen** last/average; live in-flight elapsed stays
where it is today (top status bar via `format_task_elapsed` + `task_start_time`).

### 5. Rendering (`agent_tui_kit/src/render/bar.rs`)

Two new drop groups on row 2:

- **Turn counters** (droppable) — `⟳ {n} {turns}` and `⇅ {llm}`, joined by
  `SEP_ROW2`, rendered as one group immediately after the `ctx` meter.
- **Turn timing** (droppable) — `⏱ {last}` plus `· {avg} {avg}` when at least one
  turn has completed; rendered as the last segment.

Exact rendered shapes:

| Terminal | Segment |
|---|---|
| EN | `⟳ 12 turns ⇅ 3 ⏱ 01:05 · avg 00:42` |
| ZH | `⟳ 12 轮 ⇅ 3 ⏱ 01:05 · 均 00:42` |

- `⟳` (U+27F3) = session user turns; `⇅` (U+21C5) = current-task LLM turns;
  `⏱` stays consistent with the existing `bottom_elapsed` label and the top
  status bar.
- The whole `⇅` segment is omitted while `turn_llm == 0` (no LLM call yet this
  task).
- The timing group is omitted while `turn_last_secs` is `None`; `avg` is omitted
  while `turn_done == 0`.
- `mm_ss` formatting reuses the existing `{:02}:{:02}` convention; sessions over
  an hour keep the same mm:ss (the turn metrics are per-turn, not uptime).

> **Decision (2026-09-12): `max_turns` is plumbed but not displayed.**
> `Agent::max_turns` is only ever set by `spawn_subagent`'s `max_turns` input
> (`crates/tact/src/tool/subagent.rs:542` → `with_max_turns`); the main agent
> defaults to `None` and there is no CLI flag or TUI wiring, so a main-agent
> bottom bar could never render a `/cap` suffix. The `TurnStats` event therefore
> still carries `max_turns` (kept in `StatusBarState.turn_llm_cap`) so the
> segment lights up if a cap is ever added for the main agent, but the bottom
> bar renders bare `⇅ {n}` and the docs make no `/cap` claim. Subagent caps
> remain visible where they already are: the subagent summary's
> `(max_turns reached)` marker and the `max_turns (N) reached` info line.

Push order on row 2 (survival priority is push order, since
`fit_row_spans` removes the last droppable first):

```
model, out, think, ctx, turns, ∑, cache, timing
```

Survival: `ctx > turns > ∑ > cache > timing`. This **preserves** every existing
relative priority (`ctx > ∑ > cache` still holds) and inserts the new counters
between `ctx` and `∑`; timing drops first. The existing comment/docs line about
drop order gets updated to match.

### 6. i18n (`agent_tui_kit/src/i18n.rs`)

New `Msgs` fields, EN + ZH:

```rust
pub bottom_turns: &'static str,      // "turns" / "轮"
pub bottom_llm_turns: &'static str,  // "turns" / "轮次"
pub bottom_avg: &'static str,        // "avg" / "均"
```

### 7. Tests

- `agent_tui_kit/src/render/bar.rs`: row-2 renders `⟳`/`⇅`/`⏱` with seeded
  state; omits `⇅` when `turn_llm == 0`; omits `avg` when `turn_done == 0`;
  renders bare `⇅ {n}` (no `/cap`) even when a cap is present; drop test
  asserting timing drops before `cache` at a narrow width.
- `tui/src/render/bar.rs`: App-level buffer test through `render_bottom_bar`.
- `tui/src/widgets/state/app/agent.rs`: `TurnStats` updates the status bar;
  two completed turns produce the correct `turn_done` / `turn_total_secs`.
- `tui/src/handlers/skills.rs` (or its test module): `dispatch_user_task`
  increments `turn_user` and resets `turn_llm`.
- `tui/src/widgets/state/app/messages.rs`: `load_history` seeds `turn_user`.
- `tact`: `agent_loop` emits `TurnStats` (counter + cap passthrough), including
  the capped run that trips the limit.

Async tests must use timeouts; no unbounded `recv().await`.

### 8. Docs to sync (per AGENTS.md trigger table)

| Trigger | File |
|---|---|
| TUI bottom-bar display | `docs/token_usage_schema.md` (bottom-bar section — add the turn segments and the new drop order) |
| TUI bottom bar | `book/23_chapter_tui.md` §6.6 + `book/23_chapter_tui_zh.md` §6.6 (keep structurally aligned) |
| `AgentUpdate` variant list | `book/23_chapter_tui.md` / `_zh.md` update-routing tables (`TokenUsage` / `ModelInfo` row) |
| Shipped user-visible change | `book/26_chapter_issue.md` + `book/26_chapter_issue_zh.md`, newest-first entry |

## Risks

- **Row-2 crowding.** Three more segments on an already dense row mean the
  timing group (and, on narrow terminals, the counters) will often be dropped.
  Mitigated by the compact glyph forms and by the explicit survival order.
- **Counter drift on resume.** `turn_user` is seeded by counting persisted user
  messages, so a session that was compacted or had user messages pruned may seed
  low. This is a display-only approximation — accepted, and documented.
- **Double counting.** Accumulating timing in `add_task_end_separator` is safe
  only while it remains the single freeze point; if a future change freezes
  elapsed elsewhere, timing can double count. Called out in a code comment.

## Resolved questions

- **Which turns?** All three: session user turns, current-task LLM turns, and
  turn wall-clock timing (last + session average). User confirmed.
- **Cap display?** Not displayed. The main agent never has a cap, so the bottom
  bar renders bare `⇅ {n}` while `StatusBarState.turn_llm_cap` stays plumbed for
  a future main-agent cap. User confirmed 2026-09-12.
- **Where?** Bottom-bar row 2, per the user's request.
- **Subagent turns on the bottom bar?** No — keeps the documented
  "main agent only" rule. The cap only renders when the main agent has one.
