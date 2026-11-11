# Tool rendering

The terminal UI renders each tool call as a title row, a status row, and an
optional detail card. Concurrent calls remain separate blocks. The render path
uses stable tool ids so late progress, language changes, and theme changes do
not rewrite the wrong card.

## Data flow

```text
AgentEvent::ToolQueued
  -> AgentUpdate
  -> App tool state
  -> queued ToolCell

AgentEvent::ToolStarted
  -> running ToolCell

AgentEvent::ToolProgress
  -> live output buffer

AgentEvent::ToolFinished
  -> final StepResult
  -> completed ToolCell
```

The kernel emits the events in `crates/agent_core/src/kernel.rs`.
`crates/tact/src/runtime/event_adapter.rs` converts them to the host protocol.
`crates/tui/src/widgets/state/app/agent.rs` applies them to the TUI.

`ToolQueued` means the call was accepted for the turn. The call may wait behind
an earlier conflict wave. `ToolStarted` means its wave has begun. A denied call
emits `ToolStarted` and `ToolFinished` without executing the tool.

## State

`crates/agent_tui_kit/src/state/tool_state.rs` owns:

| Type | Purpose |
|---|---|
| `ToolState` | Active blocks, completed blocks, detail popup, and subagent transcripts |
| `ActiveToolBlock` | Running call, live output buffer, start time, and optional subagent child id |
| `ToolBlock` | Completed call and its cached render output |
| `DiffPopup` | Full command output, file content, or diff viewer |
| `SubagentPopup` | Structured child transcript |

The app-level placeholder row management lives in
`crates/tui/src/widgets/state/app/visibility.rs`. Placeholder ranges keep the Log
height and mouse mapping stable while a card grows or collapses.

## Build and render phases

Tool rendering has two phases:

```text
ToolWidget::build
  -> ToolRenderOutput
  -> ToolCell::from_output
  -> LogColumnRenderer
```

`crates/agent_tui_kit/src/widgets/tool_widget.rs` builds layout facts and stores
no localized text or theme colors. `crates/agent_tui_kit/src/render/cells/tool.rs`
draws with the current frame's `Theme` and `Messages`. This lets a stored card
repaint correctly after `/lang`, a theme change, or a later frame.

## Visual layout

```text
  2. Read  src/main.rs
  ✓ Success · 12ms · 4 KB · [Open]
  ╭─ src/main.rs ───────────────────────╮
  │ preview                             │
  ╰─────────────────────────────────────╯
```

The title row contains the step number, display name, and argument summary. The
meta row contains one or more of:

- queued, running, success, failed, or cancelled state;
- background task id;
- permission label;
- error summary;
- output size;
- duration;
- subagent model, tokens, context, and cancel control;
- collapsed-output line count and Open button.

## Detail rules

| Tool kind | Finished presentation |
|---|---|
| Command | Two header rows, then an Open action for full output |
| File read | Two header rows, then an Open action for the body |
| File edit | Two header rows, then an Open action for the diff |
| File write | Two header rows, then an Open action for the written content |
| Subagent | Summary card plus the child transcript view |
| Task, sleep, memory, team, worktree, generic | One-line result stays inline; a multi-line result gets an Open action |
| Failed or cancelled | Error or cancellation card remains visible |
| Running command or subagent | Live output card, capped at the running preview |

The exact collapse decision comes from `ToolVisualKind` and tool metadata, not
from a second table of tool names. New native tools declare their presentation
through `ToolMetadata` in `crates/tact/src/tool/metadata.rs`.

## Live output

Running commands stream `ToolOutputChunk` values into `ToolOutputBuffer`.
`ToolCell` draws a bounded tail and the full popup reads from the same buffer.
The terminal result replaces the live buffer when the call finishes.

stdout and stderr keep their own styles. Carriage returns update the current
logical line. ANSI control sequences are removed before display. Tact does not
use a PTY or rewrite the command to defeat application buffering.

## Click targets

Every Open button records its exact rectangle for the frame. A click reaches
the tool only when it lands inside that rectangle.

Collapsed tools put the Open button at the end of the meta row. The hit test
uses the same localized label and display width as the cell, so a language or
theme change cannot move the drawn button away from its target.

Tool detail popups support:

- `j` and `k` scrolling;
- mouse-wheel scrolling;
- drag selection over rendered text;
- `y` copy, using the selection or the full content;
- `Esc` close.

## Subagents

The child transcript uses the same Markdown, thinking, and tool cells as the
main Log. `ToolState.transcripts` keeps the structured child conversation after
the parent card finishes.

`AgentUpdate::ToolTranscript` entries append to the child transcript.
`ToolTranscriptStream` updates the current child assistant text without
creating a second assistant card. A child tool's started and finished halves
pair by child tool id.

## Tests

Relevant suites:

```bash
cargo test -p agent_tui_kit --lib tool
cargo test -p tui --lib tool
```

Tests cover queued versus running state, concurrent calls, live output tails,
failed and cancelled cards, collapsed Open targets, language changes, theme
changes, subagent pairing, and mouse hit testing.

## Related documents

- [`tui_rendering.md`](tui_rendering.md)
- [`state_machines.md`](state_machines.md)
- [`parallel_tool_execution.md`](parallel_tool_execution.md)
- [`book/07_chapter_tool_zh.html`](../book/07_chapter_tool_zh.html)
