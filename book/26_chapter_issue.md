# Engineering Issue Log

> Language: [English](./26_chapter_issue.md) · [中文](./26_chapter_issue_zh.md)

This chapter is a **chronological log of optimizations and bug fixes** that changed user-visible or API-visible behavior. It is not a tutorial: each entry records the problem, the decision, and where the code / design docs live so future work does not rediscover the same trade-offs.

Related process docs: `AGENTS.md` (when to append here), `docs/superpowers/specs/` (design), `docs/superpowers/plans/` (implementation plans).

---

## 0. Purpose

| Goal | Detail |
|------|--------|
| Continuity | Capture *why* a change landed, not only *what* files moved |
| Cross-link | Point at design specs, PRs, and book chapters that teach the subsystem |
| Avoid churn | Prefer one entry per shipped behavior change; do not log pure refactors or test-only edits |

### Entry template

Newest entries first. Each entry should include:

1. **Date / ID** — `YYYY-MM-DD` and optional PR number  
2. **Type** — `optimization` · `bugfix` · `removal` · `docs`  
3. **Symptom / motivation** — what was wrong or expensive before  
4. **Decision** — the chosen contract (not discarded alternatives in full)  
5. **Behavior after** — observable rules agents and users rely on  
6. **Pointers** — code paths, specs, related book chapters  

---

## 1. 2026-07-27 — Ink themes + unified popup chrome

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 21, Ch 23; `docs/tui_rendering.md` |

**Symptom / motivation:** Default theme was `retro`; popup overlays had inconsistent border types, hardcoded colors, no shared chrome.

**Decision:** Added `ink`/`ink-light` themes with pixel-matched colors, `heading`/`version`/`muted` Theme fields, unified `render_popup_chrome` for all overlays. Default changed to `ink`.

**Behavior after:** Default theme is `ink`; all overlay popups share a consistent border, title bar (bold title, `[x]` hint), and footer layout; popup code is DRY.

**Pointers:** `crates/tui/src/theme.rs`, `crates/tui/src/render/popups/mod.rs`, `crates/tui/src/render/render_md.rs`, `crates/tact/src/config/resolve.rs`

---

## 1. 2026-07-26 — Subagent tool renamed `task` → `spawn_subagent`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 7, Ch 10, Ch 11, Ch 12, Ch 19 |

**Symptom / motivation:** The subagent spawn tool was named `task`, sharing a prefix with the four persistent-task tools (`task_create` / `task_get` / `task_list` / `task_update`) while meaning something entirely different. Models and readers conflated "the `task` tool finished" with "the task record is complete" — an observed failure left a checklist item Pending after its subagent had returned. Ch 1 / 11 / 12 / 19 each carried a disambiguation note as a workaround.

**Decision:** Rename the tool to `spawn_subagent` (verb + object, matching its description); wrapper type `TaskTool` → `SpawnSubagentTool`, handler `task()` → `spawn_subagent()`. The persistent-task tools keep the `task_*` prefix. `spawn_subagent` remains `CapabilityRisk::High` and remains a scheduling barrier.

**Behavior after:** The model-facing tool name is `spawn_subagent`; no tool named `task` exists. Restored sessions containing historical `task` tool_use blocks still load — `load_history` renders only `Text` blocks and the router resolves names only for live dispatch, so an absent name causes no error. The in-memory `always_allowed_tools` list is session-scoped, so nothing needs migrating.

**Pointers:** `crates/tact/src/tool/subagent.rs`, `crates/tact/src/tool/registry.rs`, `crates/tact/src/permission/mod.rs`

---

## 1. 2026-07-26 — `TasksChanged` no longer appends a Log card

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 19, Ch 23 |

**Symptom / motivation:** `on_tasks_changed` used to append a `📋 # Task.N · …` system message, duplicating the `task_*` tool row that already renders the same title. Commit `4116c23` commented the emission out as collateral damage rather than removing it, leaving `format_tasks_log_card` behind `#[allow(dead_code)]` and `tasks_changed_shows_panel_and_appends_log` red.

**Decision:** Keep the tool row as the only Log representation. Delete `format_tasks_log_card`, `focus_changed_task`, and `primary_action_for_change`; rewrite the test to assert the sticky updates while the Log length stays unchanged. `AgentUpdate::TasksChanged` keeps its `reason` field — producers and the protocol are unchanged.

**Behavior after:** A `task_create` / `task_update` call produces one Log row (the tool card) plus a sticky refresh, never two.

**Pointers:** `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/state/task_panel.rs`

---

## 1. 2026-07-26 — Sticky host separates tabs from body

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** `sticky_host_content_height` reserved `1 + body` rows and the renderer drew the body at `inner.y + 1`, so the tab row (`[Tasks] [Subagent] …`) sat flush against `── Pending ──` / the subagent log, with the Log box border immediately above. Everything read as one crowded block.

**Decision:** Reserve one extra row (`2 + body` for Tasks, `3 + header + lines` for Subagent) and draw a muted full-width `─` rule between the tab row and the body.

**Behavior after:** The expanded sticky shows tabs, a hairline, then content. Collapsed height is unchanged at one row.

**Pointers:** `crates/tui/src/render/task_panel.rs`

---

## 1. 2026-07-26 — Bash non-zero exit is Failed

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 7 |

**Symptom / motivation:** `bash` collected `ExitStatus` but ignored it, so `cargo test` failures and other non-zero exits still rendered as `Success · …` while stdout/stderr showed the error.

**Decision:** After the process exits cleanly (no timeout/cancel/pipe failure), `!status.success()` returns `Err` via `error_with_partial` (`exit code N` or `terminated by signal`), mapping to `StepStatus::Failed` with captured output retained for the model.

**Behavior after:** Non-zero shell exits show Failed in the TUI; zero exits unchanged.

**Pointers:** `crates/tact/src/tool/bash.rs`

---

## 1. 2026-07-25 — Subagent sticky tab (clean main Log)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 12, Ch 23; `docs/superpowers/specs/2026-07-25-subagent-sticky-pane-design.md` |

**Symptom / motivation:** Subagent shared the parent `ui_tx`, so Stream/Step/Thinking mixed into the main Log and child `TokenUsage` overwrote the bottom bar.

**Decision:** Tag subagent updates as `AgentUpdate::Subagent`; sticky host tabs Tasks | Subagent; main Log keeps only the parent `task` tool row; `RequestSelect*` passthrough; first-run auto-tab, later badge.

**Behavior after:** Nested work is visible under Subagent; main Log and ctx meter stay parent-scoped during `task`.

**Pointers:** `crates/tact/src/tool/subagent_ui.rs`, `crates/tui/src/widgets/state/subagent_pane.rs`, `crates/tui/src/render/task_panel.rs`

---

## 1. 2026-07-25 — Subagent sessions linked via `ref_id`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 1, Ch 12; `docs/superpowers/specs/2026-07-25-subagent-session-ref-design.md` |

**Symptom / motivation:** `task` subagents had no `session_id` / store — turns, token usage, and DeepSeek `user_id` isolation were missing; crashes mid-`task` lost all subagent history.

**Decision:** Each subagent gets a new session row with `sessions.ref_id` = parent id (`''` if parent has none). `list_sessions` returns only top-level (`ref_id = ''`). `delete_session` cascades children. No `SessionLock` on children.

**Behavior after:** Subagent messages / `token_usages` persist under the child id; `--list-sessions` stays parent-only; deleting a parent removes its children.

**Pointers:** `crates/tact/src/tool/subagent.rs`, `crates/tact/src/store/session_store/sqlite.rs`, `ToolContext.session_id` / `session_store`

---

## 1. 2026-07-25 — Ctx meter visible at low usage

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `docs/token_usage_schema.md` |

**Symptom / motivation:** With a 1M context window, ~1% usage (`13.7K/1M`) painted `▏` (1/8 block). Next to `·` that hairline read as an empty bar, so the numeric `1%` looked wrong.

**Decision:** Any positive fractional cell clamps to at least `▍` (3/8); never fall back to `·` for `frac > 0`.

**Behavior after:** Non-zero ctx usage always shows a clearly filled partial in `[…]` (e.g. 1% → `[▍·······]`).

**Pointers:** `crates/tui/src/render/bar.rs` (`partial_block_char` / `render_usage_bar`)

---

## 1. 2026-07-25 — Task tool titles, short Log cards, sticky tree, `/tasks-dag`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 11, 19, 23, 25; `docs/superpowers/specs/2026-07-25-task-tool-ui-redesign.md` |

**Symptom / motivation:** `task_*` tools dumped raw JSON; Log cards repeated full checklists; dependency graph was hard to see in-terminal.

**Decision:** Human tool titles (`# Task.N · …`); sticky defaults expanded as a `blocks` tree with `#id`; `/tasks-dag` opens a meraid Mermaid Unicode popup (nodes: status + id only). `TaskSnapshot` carries `blocks`/`blocked_by`. Log does **not** append task system cards (progress lives in sticky + tool rows).

**Behavior after:** Readable tool rows; sticky tree; slash DAG viewer; no task system spam in Log.

**Pointers:** `crates/tact/src/task/display.rs`, `crates/tui/src/widgets/state/task_panel.rs`, `crates/tui/src/widgets/state/task_dag.rs`

---

## 1. 2026-07-25 — Task checklist renders fully (no `… +N`)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 19, Ch 23 |

**Symptom / motivation:** Log detail cards and sticky expand capped the checklist at 6 rows (`… +N`), so an 8-task board looked incomplete even when all items were updated.

**Decision:** Drop `STICKY_BODY_CAP`; sticky height and Log cards list every task.

**Behavior after:** Full checklist in both sticky expand and each `TasksChanged` Log card.

**Pointers:** `crates/tui/src/widgets/state/task_panel.rs`

---

## 1. 2026-07-25 — Serialize persistent `task_*` tools in one turn

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 11, Ch 19 |

**Symptom / motivation:** Models often emit many `task_update` / `task_create` calls in one turn. When those ran in the same wave, TaskManager updates and `TasksChanged` UI events interleaved, producing a jammed Log and a single incomplete progress card.

**Decision:** Classify `task_create` / `task_update` / `task_get` / `task_list` as writers of a synthetic `__tact_tasks__` resource so they always land in separate waves (order preserved) while still overlapping unrelated `read_file` calls.

**Behavior after:** Within one assistant tool batch, task tools run one-at-a-time; each mutating call can emit its own `TasksChanged` in order.

**Pointers:** `crates/tact/src/agent/tool_schedule.rs`

---

## 1. 2026-07-24 — Persistent task progress sticky + Log card

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 19, Ch 23, Ch 25; `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md` |

**Symptom / motivation:** Persistent tasks (`task_create` / `task_update`) only appeared as ordinary tool JSON/text in the Log. There was no always-visible checklist and no structured timeline card for mutations.

**Decision:** Emit `AgentUpdate::TasksChanged` after successful mutating tools. TUI keeps a sticky strip under the Log via an **outer layout split** (Log internals unchanged), collapsed by default with click-to-expand, and appends a Log detail card on each change. Hide the sticky when no pending/in_progress items remain; do not show on resume until the first `TasksChanged` this session.

**Behavior after:**

- Sticky one-liner: `▸ Tasks done/total · focus` (click expands full checklist)
- Each `TasksChanged` adds a system Log checklist card
- `task_get` / `task_list` do not emit

**Pointers:** `crates/protocol/src/agent.rs`, `crates/tact/src/tool/task.rs`, `crates/tui/src/render/task_panel.rs`, `crates/tui/src/render/layout.rs`

---

## 1. 2026-07-24 — Remove redundant `[Log]` from bottom bar

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 23 |

**Symptom / motivation:** Bottom bar Row 1 always started with `[Log]` even
though the UI is permanently single-column log-only, so the focus label added
noise without information.

**Decision:** Drop the focus segment from `render_bottom_bar` Row 1. Top status
bar may still mention Log where useful; bottom bar starts with cwd / uptime.

**Behavior after:** Row 1 no longer shows `[Log]`; first segment is workspace
path (then uptime, branch, optional account).

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/bar.rs` |

---
## 2. 2026-07-24 — Slash popup Esc hint + priority over overlay

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** Opening `/` while the agent was busy felt "stuck":
no Esc-to-close hint on the title, and Esc could be swallowed by a thinking/diff
overlay instead of dismissing the slash list.

**Decision:** Append shared `popup_close_hint` (`[Esc] Close`) to the slash
popup title (including empty state). Route Insert+slash keys before
`handle_overlay_key` so Esc always closes the slash popup first.

**Behavior after:** Slash popup title shows Esc close; Esc dismisses slash
without clearing typed input; overlay Esc only after slash is closed.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/popups/slash_command.rs`, `crates/tui/src/lib.rs` |

---
## 3. 2026-07-24 — Idle bottom-bar `Up` ticks without CPU spin

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** Fully idle TUI never dirtied on poll timeout, so
`Up MM:SS` froze until the next key/mouse/agent event.

**Decision:** On idle poll (~1000 ms), dirty only when the displayed uptime
whole-second changes. Active statuses still dirty for spinners; poll intervals
unchanged. Done keeps force-repaint via `should_repaint`.

**Behavior after:** Idle `Up` advances about once per second; no faster idle
redraw loop.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/lib.rs` (`on_poll_timeout`) |

---
## 4. 2026-07-24 — Prompt elapsed moves to task-end separator

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23 |

**Symptom / motivation:** Bottom-bar `Elapsed` competed with cwd/branch/balance
and was easy to miss relative to the response it measured.

**Decision:** Freeze prompt duration into the task-end sentinel
(`\x07tact-task-end\x1f{secs}`) and render it centered on the accent rule
(`──── Elapsed 00:03 ────`). Remove elapsed from the bottom bar.

**Behavior after:** Each completed/cancelled task shows its duration on the
trailing separator; bottom row 1 no longer shows `Elapsed`.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/cells/separator.rs`, `widgets/state/app/popups.rs`, `render/bar.rs` |

---

## 5. 2026-07-24 — Bottom bar readability restore

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23, `docs/token_usage_schema.md` |

**Symptom / motivation:** After the icon-only polish, the bottom bar was hard to
decode (`8K/32K`, bare `∑` / `▣`, faint ` · ` separators). Thinking effort level
was not shown even though `model_reasoning_effort` was already available.

**Decision:** Short i18n labels beside icons; thinking shows effort+budget
(`high(32K)`); row 1 uses ` │ `, row 2 uses two spaces; cache as `缓存%` /
`cache%`; last-call total as `∑ₜₒₖ`; ctx meter fill uses mid-height `■` / `·`
inside `[]`.

**Behavior after:** Readable two-row bar without a legend; same underlying
token/cache numbers. Narrow drop order: cache → uptime → path → ∑ → ctx.

| Pointer | Path |
|---------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-readability-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-readability.md` |
| Code | `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs` |

---

## 6. 2026-07-24 — Slash popup: Tab completes, Enter runs skills

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 2, Ch 23 |

**Symptom / motivation:** After restoring Insert-mode `Tab` for the slash
popup, **Tab** and **Enter** still did the same thing for skills (both only
filled `/name `). Users could not tell complete vs run apart.

**Decision:** Slash popup **Tab** always autocompletes to `/name `. **Enter**
invokes skills and runs built-ins immediately. `/plugin` (needs a subcommand)
still only autocompletes. Command palette Enter on a skill still prefills
Insert (undo-friendly).

**Behavior after:** Pick a skill in `/` → Tab to edit args, or Enter to run
now.

**Pointers:** `crates/tui/src/handlers/insert.rs`, Ch 2 §7, Ch 23 slash skills.

---

## 7. 2026-07-24 — TUI left Execution Plan panel removed

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 23, Ch 25 |

**Symptom / motivation:** The left plan panel duplicated information already
visible in the log (tool blocks appear on `StepStarted`), while adding
`Tab` focus switching, an `e` visibility toggle, a draggable divider, and a
`panel_split_ratio` layout knob that most users never touched. The extra
panel-focus state also complicated mouse hit testing and keyboard handling.

**Decision:** Remove the panel UI entirely; keep `PlanStep` tracking as an
internal, panel-less store (`app.plan.steps` / `steps_set`) so step data
stays available for future consumers. The log is now permanently
single-column. `FocusedPanel` keeps only its `Log` variant. Delete `Tab`
focus switching, the `e` toggle, and divider drag/resize; `j`/`k`/`g`/`G`/`y`/`Y`/`V`
now always act on the log. `Insert`-mode `Tab` for slash-command
autocompletion (previously shadowed by the global `Tab` handler) now fires
correctly since nothing above it in `lib.rs` intercepts `Tab` first.

**Behavior after:** `render_main_area` always renders the log panel at full
width; there is no plan panel, divider, or panel-focus indicator in the top
or bottom bar. `StepAdded` still updates `app.plan.steps` for internal
bookkeeping but never draws a dedicated panel.

**Pointers:** `crates/tui/src/widgets/state/plan_panel.rs`,
`crates/tui/src/render/layout.rs`, `crates/tui/src/widgets/state/mod.rs`
(`FocusedPanel`), `crates/tui/src/handlers/normal.rs`,
`crates/tui/src/handlers/mouse.rs`, `book/23_chapter_tui*.md`.

---

## 8. 2026-07-24 — Project config file renamed `tact.toml` → `config.toml`

| Field | Value |
|-------|-------|
| **Type** | docs |
| **Related** | Ch 21 |

**Symptom / motivation:** Auto-discovery listed `./tact.toml` while user-global /
`.tact/` paths already used `config.toml`, which was easy to misplace.

**Decision:** Search `./config.toml` instead of `./tact.toml`. Rename
`tact.example.toml` → `config.example.toml`.

**Behavior after:** Discovery order is `./.tact/config.toml`, `./config.toml`,
`~/.tact/config.toml`. Explicit `--config` unchanged.

**Pointers:** `crates/tact/src/config/load.rs`, `book/21_chapter_config*.md`,
`config.example.toml`.

---

## 9. 2026-07-24 — Session Stats GFM cells padded for plain-text alignment

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**Symptom / motivation:** End-of-session `eprintln` of `SessionStats::summary()`
printed unpadded GFM (`| Elapsed | 1.2s |` next to longer metric names), so
pipe columns did not line up in the terminal after `tact-ui` exited.

**Decision:** Keep GFM pipe tables for tui-markdown. Pad header / body cells to
the per-column max width (right-align numeric columns from `:` separators).

**Behavior after:** CLI / headless / TUI exit summaries show aligned columns in
monospace; `/stats` popup still renders via tui-markdown box tables.

**Pointers:** `crates/tact/src/stats.rs`, `docs/token_usage_schema.md`
(Session Stats Display).

---

## 10. 2026-07-24 — Extra `skill_dirs` + project-local `.tact/skills`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-extra-skill-dirs-design.md` |

**Symptom / motivation:** Only fixed skill roots existed; teams could not point at
shared or vendor skill trees. The old `<workdir>/skills/` root also sat outside
`.tact/`.

**Decision:** Replace `<workdir>/skills/` with `<workdir>/.tact/skills/`. Add
optional `[agent].skill_dirs = [...]` (relative to workdir; `~` expands). Load
order: `.tact/skills` → `~/.tact/skills` → `~/.agents/skills` → `.claude/skills`
→ config extras → plugin cache. Missing dirs soft-skipped.

**Behavior after:** Config can append skill roots that override earlier
same-named standalone skills. Bare `<workdir>/skills/` is no longer scanned.

**Pointers:** `crates/tact/src/consts.rs`, `crates/tact/src/skill/mod.rs`,
`crates/tact/src/config/types.rs`, `config.example.toml`, Ch 2.

---

## 11. 2026-07-24 — `/skills` list via tui-markdown (no pipe table)

| Field | Value |
|-------|-------|
| **Type** | bugfix |

**Symptom / motivation:** `/skills` built a Skill/Description pipe table through
`format_table`. Long frontmatter descriptions made each row wider than the log
panel; visual wrap shattered `|` columns into unreadable fragments.

**Decision:** Keep the titled block + blank separators. Emit wrap-friendly
markdown (`**\`name\`**` then description paragraph) and render with
`render_markdown_tui` / tui-markdown. Do **not** use a GFM table here (unlike
Session Stats): catalog descriptions are too wide for fixed columns in the log.

**Behavior after:** `/skills` shows one skill name + description block per entry;
text wraps cleanly at any panel width. Namespace names (`plugin:skill`) unchanged.

**Pointers:** `crates/tui/src/handlers/mod.rs` (`show_skills_command`,
`skills_list_markdown`).

---

## 12. 2026-07-24 — Session Stats as GFM tables via tui-markdown

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**Symptom / motivation:** `/stats` fed comfy-table UTF8 box output through
`render_markdown_tui`. Soft breaks became spaces, so the whole table collapsed
into one wrapped line and looked unreadable in the popup.

**Decision:** Keep `SessionStats::summary() -> String`. Emit **GFM pipe tables**
(with right-aligned numeric columns). TUI keeps using `render_markdown_tui` /
[tui-markdown](https://github.com/joshka/tui-markdown) table rendering (Unicode
box borders). Drop the `comfy-table` dependency. CLI / headless print the same
markdown source.

**Behavior after:** Session Statistics popup shows aligned box tables; exit
summaries are GFM markdown. Counters and visibility rules unchanged.

**Pointers:** `crates/tact/src/stats.rs`,
`crates/tui/src/widgets/state/app/agent.rs`, `docs/token_usage_schema.md`.

---

## 13. 2026-07-24 — Session Stats rendered with comfy-table

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-session-stats-table.md` |
| **Superseded by** | §7 (GFM + tui-markdown) |

**Symptom / motivation:** End-of-session Tool calls rows used ad-hoc space
padding, so columns drifted as names and timings grew.

**Decision:** Keep `SessionStats::summary() -> String`. Render a head
Metric/Value table, an optional Tool calls table
(`Tool | Count(s/f) | Total | Avg`), then a trailing Metric/Value table for
tool aggregates / cache / reasoning. *(Originally used `comfy-table` UTF8
boxes; that path conflicted with TUI markdown — see §7.)*

**Behavior after:** Same counters and visibility rules; layout is aligned
tables instead of free-form lines.

**Pointers:** `crates/tact/src/stats.rs`, `docs/token_usage_schema.md`
(Session Stats Display).

---

## 14. 2026-07-24 — `/model` supplements config from `/v1/models`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-openai-models-api-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-openai-models-api.md` |

**Symptom / motivation:** `/model` required a hand-maintained `models = [...]`
list; providers already expose `GET /v1/models`.

**Decision:** Config remains primary; API appends missing ids; conflicts keep
config; fetch once per `(base_url, api_key)` on first `/model`; Anthropic skipped;
failures soft-fail to config-only / empty hint.

**Behavior after:** See Ch 21 `/model` section.

**Pointers:** `crates/tact_llm/src/models.rs`, `crates/tui/src/handlers/select.rs`,
Ch 21, Ch 22 (account-style queries).

---

## 15. 2026-07-24 — `read_file` pagination and `batch_read` removal

| Field | Value |
|-------|-------|
| **Type** | optimization + removal |
| **PR** | [#50](https://github.com/rust-infra/tact/pull/50) |
| **Spec** | `docs/superpowers/specs/2026-07-24-read-file-pagination-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-read-file-pagination.md` |

### 6.1 Symptom

`read_file` loaded the whole file with `read_to_string`, then silently discarded the tail with `chars().take(50000)`. That conflicted with line-based `offset` / `limit`, gave the model no recovery signal (hallucination risk — see [Ch 20](./20_chapter_hallucination.md)), and competed with dispatch-level `persist_large_output` (30k characters → `<persisted-output>`).

`batch_read` was a second multi-file API with its own 200k-character hard cap, duplicating schedule / recent-file special cases.

### 6.2 Decision

1. Delete `batch_read`. Parallel multi-file reads use concurrent `read_file` waves.  
2. Stream lines with Tokio `BufReader` (no whole-file buffer for the page).  
3. Bound pages with prefixed constants in `read_file.rs`:

```rust
const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;
```

Token estimate: existing `approx_token_count` (`ceil(UTF-8 bytes / 4)`).  
4. No per-line character limit (a single oversized line errors; never silent mid-line cut).  
5. Incomplete **implicit** / default pages return a leading marker:

```text
[PARTIAL view — lines {start}-{end}; continue with offset={next}]

{joined lines}
```

6. **Explicit** `offset` and/or `limit` that still exceed the token budget → **error** (do not silently return less than requested).  
7. `run_native_tool` **skips** `persist_large_output` when `name == "read_file"`.  
8. Tool `description` stays short — limits are enforced at runtime, not duplicated in the schema blurb.

### 6.3 Behavior after

| Case | Result |
|------|--------|
| Small file, no args | Full content, no PARTIAL |
| File longer than 2000 lines, no args | First 2000 lines + PARTIAL with `offset=2001` |
| Token budget hit on implicit read | Complete lines that fit + PARTIAL with next `offset` |
| Explicit range over token budget | `Err` asking to reduce `limit` / shrink the section |
| Single line alone over budget | `Err` (cannot recover via line offset) |
| Offset past EOF | Empty string |
| Large `read_file` vs bash / MCP | `read_file` never gets `<persisted-output>`; others still may |

### 6.4 Pointers

| Area | Path |
|------|------|
| Implementation | `crates/tact/src/tool/read_file.rs` |
| Persist exemption | `crates/tact/src/agent/tool_dispatch.rs` (`run_native_tool`) |
| Tool registration | `crates/tact/src/tool/registry.rs` (no `BatchReadTool`) |
| Approx tokens | `crates/tact/src/utils/truncate.rs` |
| Tool chapter | [Ch 7](./07_chapter_tool.md) |
| Compaction / spill | [Ch 5](./05_chapter_compact.md), `docs/compaction.md` |

---

## 16. 2026-07-24 — Bottom bar visual polish

| Field | Value |
|-------|-------|
| **Type** | optimization |

**Symptom / motivation:** The bottom bar mixed emoji, long bilingual labels (`Elapsed:`, `Balance:`, `cache hit:`), and mixed separators (`│` / `|`). Both rows used a single `Paragraph` style, giving flat color hierarchy that was hard to scan.

**Decision:** Replace emoji with narrow Unicode icons (`◷`, `⊙`, `⎇`, `¤`, `∑`, `▣`). Unify separators to ` · `. Compact model limits to `8k/32k` format and collapse verbose balance/quota strings. Render with ratatui `Line` / `Span` segments: dim icons & separators, bright primary values, accent branch, success/error balance.

**Behavior after:** Two-row bottom bar with consistent iconography and color hierarchy. Pure formatting helpers (`format_model_compact`, `format_balance_entry`, `format_quota_window`, `format_cache_pct`) are unit-testable without a terminal. Narrow-width drop order removes uptime → path on row 1, cache → tokens → meter on row 2.

| Area | Path |
|------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-polish-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-polish.md` |
| Implementation | `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs` |
| Docs | `docs/tui_rendering.md` (Bottom Bar section) |
| Rendering framework | [Ch 23](./23_chapter_tui.md) |

---

## Related Docs

- [Tool System](./07_chapter_tool.md)
- [Context Compaction](./05_chapter_compact.md)
- [Hallucination in Agent Loops](./20_chapter_hallucination.md)
- [AGENTS.md](../AGENTS.md) — documentation sync triggers including this chapter
