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

TUI note: `StepAdded` updates the **plan panel only** — it no longer inserts a separate log line. The log block appears on `StepStarted`.

### Legacy `Info` lines

`Agent::execute()` still emits `AgentUpdate::Info("Executing {}({})")` after a tool returns. These appear as plain system log rows (`RawMessageType::SysTool`). The **canonical** tool UI is the structured tool block from `StepStarted` / `StepFinished`, not these Info lines.

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
    pub live_output: ToolOutputBuffer,  // five-line tail + bounded popup detail
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
| `ToolProgress` | `agent.rs` | Update matching `live_output`; first visible output expands once to a fixed five-row card; ignore unknown/late IDs |
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
  ├─ Row 2  Meta      "⠋ Running · 1.2s"  or  "✓ Success · Always allow · 21ms"
  └─ Card   (optional, Success + detail only)
            ╭─ Command output (24 lines) ─────────╮
            │ $ git status                         │
            │  M crates/tact/src/agent/mod.rs      │
            ╰─ double-click for full content ──────╯
```

Title format (`ToolWidget::title_text`):

- Command tools (`bash`, etc.): `{step}. {tool} ({arg_summary})`
- Other tools: `{step}. {label}  {arg_summary}` (double space before arg)

Constants (`render/util.rs`):

| Constant | Value | Used for |
|---|---|---|
| `LOG_TOOL_INDENT` | 4 | Plain `Executing …` system lines |
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
| `Command` | `bash`, `shell`, `run_command` | Command stdout/stderr |
| `Generic` | others | No card (title + meta only) |

Completed preview: default 1 line inside the card; overflow row when total > preview. Full text remains in `detail_full` for the popup.

Running `bash` cards add no detail until the first visible output. They then
expand once to a stable five-row tail titled `Live output (N lines)`. Further
progress mutates those rows without changing card height. stdout uses normal
text styling and stderr spans use the theme warning color. ANSI CSI/OSC is
removed and carriage return replaces the current logical line.

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
- On first live output, resize once for the five-row card. Preserve numeric scroll offsets; a bottom-pinned viewport remains pinned.

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

For `bash` / `run_command` / `shell`, popup content is prefixed with `$ <full command>` before the captured output. This is the primary place to read untruncated arguments.

An active `bash` card also opens this popup using its buffered output so far.
The live detail buffer is capped at 50,000 characters and marks omitted text;
the terminal `StepResult.detail` becomes authoritative after completion.

Centered modal styling (no drop shadow); scroll with `j`/`k`. Permission `RequestSelect` popups set `log_confirm = false` so approval text is not duplicated in the log.

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
