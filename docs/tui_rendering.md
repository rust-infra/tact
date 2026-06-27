# TUI Rendering Documentation

This document describes the rendering architecture, module division, rendering flow, and performance optimization strategies of the `crates/tui/src/render` module.

---

## 1. Architecture Overview

The TUI is drawn with [ratatui](https://docs.rs/ratatui) and follows a **layered rendering** design:

- The main loop in `lib.rs` initializes the terminal, handles events, and schedules rendering.
- The `render/` directory contains all drawing logic, split into submodules by feature.
- Rendering is frame-based: each `Frame` converts the `App` state into a terminal screen.

```
crates/tui/src/render/
├── mod.rs              # module re-exports
├── layout.rs           # main area layout
├── bar.rs              # top/bottom status bars
├── input.rs            # input box and command line
├── log.rs              # log panel
├── log_column.rs       # log column renderer
├── plan.rs             # execution plan panel
├── render_md.rs        # Markdown rendering
├── renderable.rs       # Renderable trait
├── util.rs             # text wrapping utilities
├── welcome.rs          # startup logo component
├── cells/              # card rendering cells
│   ├── text.rs
│   ├── thinking.rs
│   ├── tool.rs         # tool invocation blocks (title + meta + detail card)
│   ├── diff.rs
│   └── code.rs
└── popups/             # popups
    ├── command_palette.rs
    ├── select.rs
    ├── help.rs
    ├── history.rs
    ├── thinking_popup.rs
    ├── diff_popup.rs
    └── code_popup.rs
```

---

## 2. Main Loop and Rendering Entry

The main loop is in `run_tui()` in `crates/tui/src/lib.rs`:

1. **Consume Agent updates**: drain `agent_rx` before drawing to keep state consistent.
2. **Dirty check**: redraw when `app.dirty` is `true`, `Status::Done`, or any tool is still running (`!app.tools.active.is_empty()` — keeps live elapsed time updating).
3. **Compute layout**: split the area based on terminal size, input box height, and balance row count.
4. **Render by layer**: status bar → main area → input box → bottom bar → popups.
5. **Clean up state**: e.g., `Done` highlight reverts to `Idle` after 2s, `flash_msg` clears after 3s.

```rust
terminal.draw(|f| {
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),          // top status bar
            Constraint::Min(3),              // main area
            Constraint::Length(input_height),// input box
            Constraint::Length(bottom_height),// bottom bar
        ])
        .split(size);

    render_status_bar(f, chunks[0], &app);
    render_main_area(f, chunks[1], &mut app);
    render_input_box(f, chunks[2], &mut app);
    render_bottom_bar(f, chunks[3], &app);

    if app.input_mode == InputMode::Palette { render_command_palette(f, size, &app); }
    if app.input_mode == InputMode::Select  { render_select_popup(f, size, &app); }
})?;
```

---

## 3. Layout Module (`layout.rs`)

`render_main_area()` is responsible for the main content area:

| Display State | Layout Behavior |
|---|---|
| `show_history == true` | Full-screen history task panel |
| `show_help == true` | Full-screen help panel |
| `plan.visible == true` | Left 20% plan panel, right 80% log panel |
| default | 100% log panel |

It also updates `app.mouse.plan_area` and `app.mouse.log_area` from the layout result for later mouse hit testing.

---

## 4. Status Bars (`bar.rs`)

### Top Status Bar (`render_status_bar`)

- Shows current input mode (`Normal` / `Insert` / `Search` / `Palette` / `Select`).
- Shows current focus panel (`[Log]` / `[Plan]`).
- Shows task status according to `Status`:
  - `Idle`: theme, language, shortcut hints
  - `Planning`: planning in progress
  - `Executing`: step N of M
  - `WaitingForUser`: waiting for user approval
  - `Done`: task complete (green highlight for 2s)
- Special overrides:
  - `party_mode`: full-color party banner
  - `flash_msg`: temporary notification (3s)

### Bottom Bar (`render_bottom_bar`)

- Focus panel hint
- Working directory, Git branch
- Current model, max tokens, thinking budget
- Token statistics (prompt / completion / cache hit / reasoning)
- **Cost**: elapsed time for the current prompt (live while running; frozen after task complete/fail until the next prompt)
- **Up**: total TUI process uptime
- DeepSeek account balance (optional third row)

---

## 5. Input Area (`input.rs`)

### Command Line (`render_command_line`)

Used for `Search` (prefixed with `/`) and `Palette` (no prefix) modes:

- Displays `cmd_line` content.
- Cursor is positioned at the end of the text.

### Main Input Box (`render_input_box`)

- Supports multi-line input up to 3 lines.
- Renders a rounded-border input box in `Insert` mode.
- Renders an approval banner in `WaitingForUser` state.
- Cursor is computed by character width (supports CJK full-width characters).

---

## 6. Log Panel (`log.rs`)

The log panel is the most complex rendering component. Its core flow is:

### 6.1 Visibility Index (`visible_indices`)

- Some physical message rows may be hidden (e.g., detailed thinking content, code block placeholders).
- Maintains `visible_indices`: logical row → physical row.
- Maintains `phys_to_logical_cache`: physical row → logical row.

### 6.2 Visual Cache (`visual_cache`)

- Wraps each row to the panel width automatically.
- Caches `visual_cache` and `visual_start_cache`.
- Rebuilt when `messages.len()` or width changes.

### 6.3 Viewport Clipping

- Computes the visible logical row range based on `log_scroll.offset`.
- Only renders `TextCell`s that fall inside the current viewport.

### 6.4 Card Overlays

Three card types are overlaid on the log panel:

| Card Type | File | Description |
|---|---|---|
| Thinking card | `cells/thinking.rs` | Collapsed thinking block, up to 3 preview lines |
| Diff card | `cells/diff.rs` | File write preview with line numbers and `+` prefix |
| Code card | `cells/code.rs` | Completed code block card with syntax highlighting |

### 6.5 Tool Blocks (`cells/tool.rs`)

Tool invocations are rendered as dedicated log rows (not plain `Info` text). Each block uses a **3-tier layout**:

1. **Title row** — step number + display name + argument summary (e.g. `2. Bash: git status`)
2. **Meta row** — phase spinner / success or fail prefix, permission label, duration (live while running via `started_at`)
3. **Detail card** (optional) — command output, file preview, or error text; double-click opens `DiffPopup`

Key types:

| Type | File | Role |
|---|---|---|
| `ToolWidget` | `widgets/tool_widget.rs` | Builds `ToolRenderOutput` from tool name, phase, args, detail |
| `ToolCell` | `render/cells/tool.rs` | Ratatui `Renderable` for one tool block |
| `ActiveToolBlock` | `widgets/state/tool_state.rs` | In-flight tool; supports **concurrent** running tools (`tools.active: Vec<_>`) |
| `ToolBlock` | `widgets/state/tool_state.rs` | Completed tool placeholder rows in the log |

`StepAdded` plan rows show **tool name only** (no JSON args); full arguments appear in the tool card below.

Indent: tool blocks use `LOG_TOOL_BLOCK_INDENT` (8 columns) in `render/util.rs`.

Mouse hit-testing uses `find_tool_at_logical()` so concurrent tool rows map correctly.

Full design (data flow, state model, extension guide): [`docs/tool_rendering.md`](./tool_rendering.md).

### 6.5 Scrollbar

- Position is computed from the **total visual line count**.
- Custom symbols: `▲` / `▼` / `│` / `█`

---

## 7. Rendering Cells (`cells/`)

### `Renderable` trait (`renderable.rs`)

All renderable units implement this trait:

```rust
pub(crate) trait Renderable {
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize);
    fn height(&self, width: u16) -> u16;
}
```

### `TextCell` (`cells/text.rs`)

The basic log rendering unit, supporting:

- Pre-wrapping cache
- Search highlight (yellow background)
- Mouse selection (inverted color)
- Word-level double-click selection
- Thinking block collapsed indicator prefix

### Card Cells

- `thinking.rs`: purple-tinted border, shows up to 3 recent thinking lines
- `diff.rs`: green `+` prefix, shows file path and line numbers
- `code.rs`: dark blue-gray background, shows language tag and code preview

---

## 8. Markdown Rendering (`render_md.rs`)

Uses `tui-markdown` to convert Markdown into a list of `Line`s:

- Custom `TuiStyleSheet`: headings, code, links, blockquotes
- Code block post-processing: unified dark blue-gray background
- Table formatting: aligned columns, bold header
- Horizontal rule detection

> Note: Hyperlink OSC 8 sequences are not handled because ratatui strips escape sequences.

---

## 9. Popups (`popups/`)

| Popup | File | Description |
|---|---|---|
| Command palette | `command_palette.rs` | Triggered by `:`, fuzzy-filtered commands |
| Select popup | `select.rs` | Agent asks the user to choose |
| Help panel | `help.rs` | Triggered by `Ctrl+?`, shortcut reference |
| History panel | `history.rs` | Triggered by `Ctrl+H`, retry historical tasks |
| Thinking detail | `thinking_popup.rs` | View full thinking content |
| File detail | `diff_popup.rs` | Full tool output, file diff, or inline bash/command text |
| Code detail | `code_popup.rs` | View full code block |

Popups usually:

- Occupy 80% × 80% of the screen
- Render `Clear` first to erase the background
- Show hints like `[y] Copy`, `[Esc] Close`, `[j/k] Scroll`
- Record their area in `app.mouse.*_popup_area` for click-outside-to-close

---

## 10. Performance Optimization

### 10.1 Dirty Rendering

- `terminal.draw()` is called only when `app.dirty == true` or `Status::Done`.
- Polls at 1s intervals when idle to reduce CPU usage.

### 10.2 Caching Strategy

| Cache | Location | Invalidation Condition |
|---|---|---|
| `visible_indices` | `log_scroll` | `messages.len()` changes |
| `visual_cache` | `log_scroll` | `messages.len()` or width changes |
| `phys_to_logical_cache` | `log_scroll` | `messages.len()` changes |
| code block `styled` | `CodeBlock` | block creation |
| diff preview rows | `DiffBlock` | block creation |

### 10.3 Viewport Clipping

- `LogColumnRenderer` only renders cells that fall inside the current viewport.
- Each `TextCell` supports `render_partial` to skip invisible lines.

### 10.4 Adaptive Event Polling

| State | Poll Interval | Reason |
|---|---|---|
| `Done` or `flash_msg` | 200ms | Clean up timed-out states promptly |
| `dirty == true` | 10ms | Trigger redraw quickly |
| idle | 1000ms | Reduce CPU usage |

---

## 11. Theme and Internationalization

### Theme (`theme.rs`)

- 9 built-in themes: `Dark`, `Light`, `SolarizedDark`, `SolarizedLight`, `GruvboxDark`, `Nord`, `Retro`, `Kawaii`, `Japanese`
- Each theme defines background, foreground, accent, warning, border, etc.
- Cycle through themes with `Ctrl+T`

### Internationalization (`i18n.rs`)

- Supports `English` and `Chinese`
- All UI strings are centralized in the `Messages` struct
- Naming conventions:
  - `_tmpl`: templates with `{}` placeholders
  - `_pl`: plural forms
- Cycle languages with `Ctrl+L`

---

## 12. Related State Machines

Rendering is driven by the following state machines; see `docs/state_machines.md` for details:

- `Status`: Idle / Planning / Executing / WaitingForUser / Done
- `InputMode`: Normal / Insert / Search / Palette / Select
- `SelectPopup`: Inactive / Active / Confirmed / Cancelled
- `StreamState` / `ThinkingState`: streaming output parsing

---

## 13. Debugging and Extension

### Adding a New Panel

1. Create a new rendering module under `render/`.
2. Allocate area based on state in `layout.rs`.
3. Add required state to `App` in `state/mod.rs`.
4. If needed, record the area in `mouse_state.rs` for mouse hit testing.

### Adding a New Popup

1. Create a new module under `render/popups/`.
2. Re-export it in `popups/mod.rs`.
3. Call it from `layout.rs` or the main loop.
4. Add popup state and open/close/scroll methods to `App`.

### Performance Profiling

- Watch cache hit rates in `log.rs`.
- Use `app.dirty` to control redraw frequency.
- Avoid file I/O inside `render()` (e.g., `diff_popup.rs` uses `cached_content` lazy loading).
