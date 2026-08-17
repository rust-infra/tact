# Pending Queue Cancel Button Placement Design

- **Date:** 2026-08-17
- **Status:** Approved for planning
- **Related:** `crates/tui/src/render/input.rs`; `crates/tui/src/handlers/mouse.rs`; `crates/tui/src/widgets/state/app/pending.rs`

## Goal

Make the pending-prompt queue's `[Cancel]` control easier to reach by placing it immediately after the pending-submit hint text instead of at the far right edge of the pending block.

## Scope

### In scope

- Render `[Cancel]` on the same hint row, directly after the hint text with one visual space.
- Keep the button's mouse hit area aligned with its rendered cells.
- Reserve enough width for the button before truncating the hint text.
- Hide the button and clear its hit area when the terminal is too narrow.
- Add or update rendering assertions that verify the visual order and hit area.

### Out of scope

- Changing queue semantics: clicking `[Cancel]` still clears only pending messages and does not interrupt the running task.
- Changing `/cancel`, Normal-mode `c`, Enter queueing, or Esc behavior.
- Moving the button to a separate row or to a queued-message row.
- Changing pending-message count, truncation policy for queued message rows, or input-box height.

## Current Context

`render_pending_block` currently computes a button rectangle at the right edge of the hint row, truncates the hint to the remaining width, inserts a flexible gap, and then draws `[Cancel]`. `handle_mouse_down` already checks `pending_cancel_btn_area` and calls `clear_pending_messages`, so the behavior contract is independent from the visual position.

## Design

1. Compute the available width for the hint while reserving the button label width plus one space.
2. Truncate the hint to that available width when the button is visible.
3. Set the button rectangle's `x` coordinate to the hint's rendered end plus one column, rather than aligning it to the right edge.
4. Render the hint, one background-painted space, and `[Cancel]` consecutively. The existing pending-block background rendering remains responsible for every cell.
5. Keep the existing narrow-terminal guard: when the button cannot fit, use an empty hit area, render the full-width hint, and omit `[Cancel]`.

The resulting wide-terminal layout is:

```text
Message will be submitted after the current task [Cancel]
```

## Interaction and Data Flow

- Rendering records the exact visible button rectangle in `app.pending_cancel_btn_area`.
- Mouse down checks that rectangle before other input-area handling.
- A click calls `app.clear_pending_messages()` only; the active task and its status remain unchanged.
- Clearing the queue invalidates the stored rectangle, preventing stale clicks.

## Testing

- Rendering test verifies `[Cancel]` appears on the hint row after the hint text, not before it or at the far-right edge.
- Rendering test verifies the recorded hit rectangle contains the visible button cells.
- Narrow-terminal test continues to verify the button is absent and the hit rectangle is empty.
- Existing mouse test continues to verify clicking the rendered button clears the queue without sending a task-cancel command or changing the active task.
- Run the focused TUI tests, then the repository formatting, Clippy, and package test gates.

## Documentation

If the user-visible behavior is shipped, update the TUI behavior documentation and the newest-first issue log entry in both English and Chinese according to `AGENTS.md`.
