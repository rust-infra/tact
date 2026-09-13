# Tool Rendering Design

This document describes how tool invocations are displayed in the TUI log panel: data flow from the agent runtime, state ownership, visual layout, concurrent tools, and extension points.

For the broader rendering pipeline see [`tui_rendering.md`](./tui_rendering.md). For state transitions see [`state_machines.md`](./state_machines.md) §7.5.

---

## 1. Goals

| Goal | Approach |
|---|---|
| One cohesive block per tool call | Title + meta + optional detail card in a single `ToolCell` |
| Concurrent tool calls | `ToolState.active: Vec<ActiveToolBlock>` instead of a single slot |
| Live elapsed while running | `started_at: Instant` + 10ms dirty tick when `active` is non-empty |
| Scroll / clip / hit-test correctness | Placeholder rows in `messages[]` + `LogColumnRenderer` |
| Readable args without log clutter | Truncated summary in plan + log title; full args in popup / `StepResult.arg_full` |
| Visual separation | One blank line before tool blocks after normal content |
| Permission choice visible once | `StepResult.permission_label` on meta row; select popup uses `log_confirm = false` |

---

## 2. End-to-end data flow

```mermaid
sequenceDiagram
    participant LLM
    participant Agent as tact::Agent
    participant TUI as tui::App
    participant Log as render/log.rs

    LLM-->>Agent: ToolUse blocks
    Agent->>TUI: StepAdded(PlanStep { description: "bash (git status)" })
    Agent->>TUI: StepStarted(idx, tool_id, tool_name, arg_summary)
    Note over TUI: ensure_gap_before_tools(); ToolWidget.build() → placeholder rows → active.push()

    Agent->>Agent: permission + hooks + execute()
    Agent->>TUI: ToolProgress { tool_id, chunks } *
    Note over TUI: update matching ActiveToolBlock.live_output in place
    Agent->>TUI: StepFinished(idx, tool_id, StepResult)
    Note over TUI: finalize_tool_block() → resize placeholders → blocks.push()

    loop Each frame while active non-empty
        TUI->>Log: ToolCell with running_elapsed_ms(started_at)
    end
```

### Runtime (`crates/tact/src/agent/tool_dispatch.rs`)

| Step | What happens |
|---|---|
| `execute_tool_call()` | Increments step index; emits `StepAdded` then `StepStarted` |
| `StepAdded.description` | `tool (arg_summary)` — e.g. `bash (git status --short)`; also stored in `PlanStep.args` |
| `StepStarted` | Carries `tool_id`, `tool_name`, truncated `arg_summary` from `tool_arg_summary()` |
| `ToolProgress` | Informational ordered chunks for the matching active `tool_id`; does not finalize the tool or affect thinking/loading gates |
| Tool execution | Builds `StepResult` with `arg_full`, `message`, `detail`, `duration_us`, `permission_label` |
| `StepFinished` | TUI finalizes the matching `ActiveToolBlock` by `tool_id` |

Helper functions:

- `tool_arg_full(name, input)` — untruncated path / command / JSON string
- `tool_arg_summary(name, input)` — same extraction, truncated to 120 chars for display
- `tool_detail_content(name, input, output)` — full stdout for bash/read; written content for `write_file`

TUI note: `StepAdded` only updates the internal `app.plan.steps` store — there is no dedicated plan panel, and it does not insert a separate log line. The log block appears on `StepStarted`.

### Legacy `Info` lines

`Agent::execute()` still emits `AgentUpdate::Info("Executing {}({})")` after a tool returns. These appear as plain system log rows (`LogItemKind::SystemPlain`). The **canonical** tool UI is the structured tool block from `StepStarted` / `StepFinished`, not these Info lines.

---

## 3. TUI state model

File: `crates/tui/src/widgets/state/tool_state.rs`

```rust
pub(crate) struct ToolState {
    pub active: Vec<ActiveToolBlock>,   // in-flight
    pub blocks: Vec<ToolBlock>,         // completed
    pub popup: Option<DiffPopup>,      // full-content modal
}

pub(crate) struct ActiveToolBlock {
    pub phys_idx: usize,                // first row in messages[]
    pub tool_id: String,                // LLM tool_use id
    pub output: ToolRenderOutput,       // pre-built layout
    pub live_output: ToolOutputBuffer,  // three-line tail + bounded popup detail
    pub started_at: Instant,
}

pub(crate) struct ToolBlock {
    pub phys_idx: usize,
    pub output: ToolRenderOutput,
}
```

### Lifecycle

| Event | Handler | Effect |
|---|---|---|
| `StepStarted` | `agent.rs` | `cancel_active_tool(tool_id)` if restart; `push_tool_placeholder_rows`; `active.push` |
| `ToolProgress` | `agent.rs` | Update matching `live_output`; visible output grows the card from one to three rows, then keeps a three-line tail; ignore unknown/late IDs |
| `StepFinished` | `agent.rs` | `ToolWidget::from_step_result().build()` → `finalize_tool_block` |
| `StepFailed` | `agent.rs` | Rebuild output as `ToolPhase::Failed` or fallback system message |
| `PlanGenerated` | `agent.rs` | **Legacy handler only** — agent does not emit; would call `cancel_all_active_tools()` |
| Double-click tool row | `lib.rs` / `popups.rs` | Open `DiffPopup` from `detail_full` or file path |

`finalize_tool_block()` either resizes existing placeholder rows (normal path) or inserts new ones (no matching active entry).

---

## 4. Two-stage render pipeline

Tool display splits **layout** (borrowed i18n/theme) from **render** (owned, storable):

```text
  ToolWidget (builder)          ToolRenderOutput (owned)         ToolCell (Renderable)
  ───────────────────          ────────────────────────         ─────────────────────
  borrows Theme, Messages  →   title_line, meta fields,    →   ratatui draw + height()
                                 layout, detail_preview           skip_lines clipping
```

| Type | File | Role |
|---|---|---|
| `ToolWidget` | `widgets/tool_widget.rs` | Fluent builder; computes layout, preview lines, card title |
| `ToolRenderOutput` | `widgets/tool_widget.rs` | Serializable snapshot for state + log renderer |
| `ToolCell` | `render/cells/tool.rs` | Implements `Renderable`; draws title, meta, detail card |

Why two stages: `ToolWidget` needs `&Theme` and `&Messages`. `ToolCell` must live across frames inside `LogColumnRenderer` without lifetime ties.

---

## 5. Visual layout (3 tiers)

```text
  ← LOG_TOOL_BLOCK_INDENT (8 cols)
  │
  ├─ Row 1  Title     "2. bash (git status)"         (bold; truncated at 120 chars)
  ├─ Row 2  Meta      "⠋ Running · 1.2s"  or  "✓ Success · 21ms · 4 lines · double-click"
  └─ Card   (optional: drawn for a running/failed command, a finished file
             tool, or a subagent — never for a finished command)
            ╭─ <card title> ──────────────────────╮
            │  preview rows (1 by default; 3 live) │
            ╰─ Double-click for full content ──────╯
```

Title format (`ToolWidget::title_text`):

- Command tools (`bash`, etc.): `{step}. {tool} ({arg_summary})`
- Other tools: `{step}. {label}  {arg_summary}` (double space before arg)

Constants (`render/util.rs`):

| Constant | Value | Used for |
|---|---|---|
| `LOG_TOOL_INDENT` | 4 | `LogItemKind::SystemTool` rows and tool placeholders |
| `LOG_TOOL_BLOCK_INDENT` | 8 | Full tool block (`ToolCell`) |

### Meta row (`build_meta_text`)

Built from phase, permission label, byte size (file tools), duration, and truncated error:

| Phase | Prefix | Color |
|---|---|---|
| `Running` | Braille spinner + "Running" | `theme.warning` |
| `Success` | `✓ Success` | `theme.success` |
| `Failed` | `✗ Failed` + error snippet | `theme.error` |

Running duration uses `running_elapsed_us(started_at)` until `StepFinished` supplies `duration_us`.

### Detail card rules (`ToolWidget::should_show_detail`)

Shown only when **phase is Success** and tool kind is:

| Kind | Tools | Card content |
|---|---|---|
| `FileWrite` | `write_file` | Written content; green `+` gutter |
| `FileRead` | `read_file` | Read file body |
| `Command` | `bash`, `shell`, `run_command` | **No card** — collapsed, see below |
| `Generic` | others | No card (title + meta only) |

Completed preview for the kinds that keep a card: default 1 line inside the card; overflow row when total > preview. For command tools, the cached detail is the full command followed by its output, so the popup's counter and content come from one source.

Running `bash` cards add no detail until the first visible output. They then
grow from one to three rows, titled `Live output`. The line count lives in the
card's bottom bar (`preview/total lines`, shown only when the output overflows
the preview), where the total is the streamed output line count (not the
`$ <command>` prefix). Later progress
updates a stable three-row tail without changing card height. Popup/`detail_full`
still prepend `$ <command>`. stdout uses
normal text styling and stderr spans use the theme warning color. ANSI CSI/OSC
is removed and carriage return replaces the current logical line.

### Collapsed command output (`ToolWidget::collapses_detail`)

A finished command draws **no card at all**. When the visual kind is `Command`, the phase is `Success`, and the card is not the live one, `ToolLayout.detail_collapsed` is set, `preview_lines` is 0, and the block is exactly its two header rows (`tool_visual_rows(false, 0, 0, false)` == `TOOL_HEADER_ROWS`). The collapse rule is keyed on the visual kind, not on tool names, so `background_run` and `worktree_run` follow it too (it applies only when there is detail to hide); file tools and subagents are untouched.

The full text is still kept in `ToolRenderOutput.detail_full`, and `detail_total_lines` carries its line count, so the popup path is unchanged — only the inline card is gone.

**Click target.** With no card to hit, the affordance is the header text itself: `ToolRenderOutput::header_text_cols(row)` returns the column range the drawn text occupies (`LOG_TOOL_BLOCK_INDENT .. indent + display width`), and a click opens the popup only inside that range — the empty rest of the row stays inert, so the trigger is what the user can see rather than an invisible full-width band. Row 0 is measured from `title_raw`; row 1 from `meta_text`.

| State | Block rows | Output visible inline |
|---|---|---|
| Running (live card) | header + `Live output` card, 1→3 rows | yes |
| Success | 2 (title + meta) | no — popup only |
| Failed | header + `Error` card, up to 5 preview rows | yes |

Because a card-less block would otherwise hide the fact that output exists, the meta row appends `… · {n} lines · double-click` (`collapsed_output_hint()`, from `tool_collapsed_output_hint` / `..._one`). `n` is `detail_total_lines` — the same number the popup reports, prefix line included.

`ToolRenderOutput.meta_text` holds the **finished** block's exact meta row (the widget and the cell assemble it through `build_meta_text` + `meta_suffixes`, so the measured text and the drawn text cannot drift; a test asserts they are equal). It is `None` while a tool runs, where the cell re-derives a ticking elapsed time — and a running block has no card-less hit area to measure.

---

## 6. Log panel integration

File: `render/log.rs`

1. Each logical log row maps to a physical index in `messages[]`.
2. Before building a `TextCell`, the loop checks whether `phys_idx` falls inside an active or completed tool block range.
3. If yes, it emits one `ToolCell` spanning `output.visual_rows()` visual lines and skips placeholder rows.
4. Viewport clipping uses `Renderable::render_partial` with `skip_lines` when the block is partially scrolled off-screen.

Placeholder strategy (`visibility.rs::push_tool_placeholder_rows`):

- Call `ensure_gap_before_tools()` first — inserts one blank line when the previous visible row is normal content.
- Reserve N blank `SysTool` rows up front so scroll height and mouse mapping stay stable.
- On finish, `resize_tool_placeholder_rows` grows or shrinks the range if final layout differs from running layout.
- On live output, resize from one to three preview rows as needed. Preserve numeric scroll offsets; a bottom-pinned viewport remains pinned.

Thinking is a separate direct card pipeline. Completion changes its existing placeholder range into a one-line summary and does not insert a trailing blank line.

---

## 7. Concurrent tools

Multiple `ToolUse` blocks in one assistant turn each get:

- Distinct `tool_id` (from the LLM)
- Separate `ActiveToolBlock` in `tools.active`
- Independent placeholder row ranges

Mouse hit-testing (`find_tool_at_logical`) iterates **all** active blocks first, then completed `blocks`, returning `(active_index, phys_idx, logical_start, row_count)`.

Main loop dirty rule (`lib.rs`):

```rust
if app.dirty || matches!(app.status, Status::Done) || !app.tools.active.is_empty() {
    // redraw — keeps running elapsed updating
}
```

---

## 8. Diff / detail popup

File: `widgets/state/tool_state.rs` + `render/popups/diff_popup.rs`

`DiffPopup` supports:

| Field | Use |
|---|---|
| `file_path` | Read from disk (write_file / read_file); uses full path from `arg_full` |
| `inline_content` | Bash output or other in-memory text (avoids treating command output as a path) |
| `use_diff_gutter` | Green `+` prefix for file writes |
| `title` | Modal header — for bash, `bash (<full command>)` even when the log title was truncated |

For `bash` / `run_command` / `shell`, the cached detail contains `$ <full command>` followed by the captured output. The card's preview/total counter, popup, and copy operation therefore share one content source; the popup is the primary place to read untruncated arguments.

An active `bash` card also opens this popup using its buffered output so far.
The live detail buffer is capped at 50,000 characters and marks omitted text;
the terminal `StepResult.detail` becomes authoritative after completion.

Centered modal styling (no drop shadow); scroll with `j`/`k`. Permission `RequestSelect` popups set `log_confirm = false` so approval text is not duplicated in the log.

A collapsed finished command (`ToolLayout.detail_collapsed`) draws no card, so its click target is the *text* of the header row under the pointer — the empty rest of that row is inert. Every other tool opens only from a click inside the detail card. See §5 "Collapsed command output".

Tool detail popups support left-button text selection over the visible body. Hit testing stores UTF-8-safe byte offsets into the original cached content, so line numbers, green diff gutters, borders, titles, and scrollbars are never selected or copied. Display cells map to complete extended grapheme clusters using Ratatui-compatible widths; forward and backward drags therefore include the whole visible grapheme under both endpoints, including combining and emoji sequences. Dragging above or below the body clamps to the first or last visible source boundary without changing popup scroll; scrolling otherwise preserves the current selection. Automatic drag-edge scrolling is intentionally out of scope.

While a tool detail popup is active, `y` copies its non-empty selection and falls back to the full original content for an empty or absent selection. This mouse-selection behavior is limited to tool detail popups; thinking and code popups are unchanged.

---

## 9. Wire types

File: `crates/protocol/src/lib.rs`

```rust
pub struct StepResult {
    pub tool: String,
    pub arg_summary: String,          // truncated display string (≤120 chars)
    pub arg_full: Option<String>,     // untruncated path / command / JSON for popups
    pub status: StepStatus,
    pub message: String,              // short summary (≤200 chars in runtime)
    pub detail: Option<String>,       // full content for card + popup
    pub duration_us: Option<u64>,
    pub permission_label: Option<String>,  // e.g. "Allow once", "Always allow this tool"
}

// AgentUpdate variants (abbreviated)
StepStarted(usize, String /* tool_id */, String /* tool_name */, String /* arg_summary */),
ToolProgress { tool_id: String, chunks: Vec<ToolOutputChunk> },
StepFinished(usize, String /* tool_id */, StepResult),
StepFailed(usize, String /* tool_id */, String),
```

Per-tool order is `StepStarted -> ToolProgress* -> StepFinished | StepFailed`.
For `bash`, two concurrent pipe readers merge chunks in aggregator-observed
order. The first batch may be immediate; regular events are at least 50 ms
apart and carry at most 4 KiB, followed by a final flush. The final capture is
independent of UI rate limiting and is bounded to 50,000 characters. This only
shows bytes emitted to the pipes: Tact does not use a PTY, inject `stdbuf`, or
rewrite commands to bypass application or pipeline buffering.

---

## 10. Adding a new tool display kind

1. **Runtime detail** — extend `tool_detail_content()` if the tool should show a card body.
2. **Arg summary** — extend `tool_arg_summary()` for a human-readable one-liner.
3. **Display kind** — add a arm in `display_kind()` / `tool_display_name()` in `tool_widget.rs`.
4. **Card rules** — update `should_show_detail()` and `detail_card_title()` if the card should appear.
5. **Gutter** — set `use_diff_gutter` in `build()` for diff-style lines.

A new kind that maps onto `ToolVisualKind::Command` inherits the collapse rule (`collapses_detail()`) as soon as it succeeds, so give it a detail only if an inline card is really wanted, and remember the popup must be able to show what the collapsed block hides.

No changes to `ToolCell` are needed unless the visual structure itself changes (e.g. a fourth header row).

---

## 11. File index

| File | Responsibility |
|---|---|
| `crates/tact/src/agent/tool_dispatch.rs` | `execute_tool_call`, `StepResult` assembly, `tool_*_summary/detail` |
| `crates/protocol/src/lib.rs` | `AgentUpdate`, `StepResult` types |
| `crates/tui/src/widgets/state/app/agent.rs` | `handle_agent_update` for tool events |
| `crates/tui/src/widgets/state/app/visibility.rs` | Placeholders, finalize, cancel, phys index shifting |
| `crates/tui/src/widgets/state/tool_state.rs` | `ToolState`, `DiffPopup` |
| `crates/tui/src/widgets/tool_widget.rs` | Layout builder, `ToolRenderOutput` |
| `crates/tui/src/render/cells/tool.rs` | `ToolCell` drawing |
| `crates/tui/src/render/log.rs` | Tool block detection in log loop |
| `crates/tui/src/render/popups/diff_popup.rs` | Full-content modal |
| `crates/tui/src/widgets/state/log_messages.rs` | `SysTool` classification for plain system lines |
| `crates/tui/src/render/util.rs` | Indent constants |

---

## 12. Related tests

Integration-style unit tests live in `render/cells/tool.rs` (`make_output`, height / partial render cases). Run:

```bash
cargo test -p tui tool_cell
```

Collapse-specific coverage: `collapsed_command_meta_row_reports_hidden_output` / `collapsed_command_meta_row_uses_the_singular_for_one_line` / `open_card_meta_row_has_no_collapsed_hint` / `widget_meta_text_matches_the_rendered_meta_row` (agent_tui_kit cells), `failed_command_keeps_its_error_card` + `collapse_applies_only_to_finished_commands` + `from_step_result_maps_permission_and_duration` + `finished_block_stores_its_meta_text_for_hit_testing` + `running_block_stores_no_meta_text` (widget layer), `completed_command_renders_header_rows_only` (log render, incl. the buffer-level indent check), `double_click_collapsed_command_header_opens_diff_popup` / `collapsed_command_ignores_clicks_past_the_text` / `double_click_tool_header_does_not_open_diff_popup` (mouse hit test).
