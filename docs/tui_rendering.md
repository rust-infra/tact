# TUI Rendering Documentation

This document describes the rendering architecture, module division, rendering flow, and performance optimization strategies of the TUI rendering layer.

Rendering is split across two crates: the **shell** keeps its `&App`-shaped entry points in `crates/tui/src/render/`, while the **pure drawing** lives in `crates/agent_tui_kit/src/render/` as `&RenderCtx`-shaped functions with no `App` dependency.

---

## 1. Architecture Overview

The TUI is drawn with [ratatui](https://docs.rs/ratatui) and follows a **layered rendering** design:

- The main loop in `lib.rs` initializes the terminal, handles events, and schedules rendering.
- The shell's `render/` directory owns the `&App`-shaped entry points; the kit's `render/` directory owns the pure drawing functions. `crates/tui/src/render/mod.rs` wires them together (re-exporting the kit's `render_md` / `renderable` / `util`, and `cells::{separator, thinking}`).
- Rendering is frame-based: each `Frame` converts the `App` state into a terminal screen.

```
crates/tui/src/render/            # shell layer: `&App` entry points + App-level tests
├── mod.rs              # module tree + re-exports of kit render modules
├── layout.rs           # main area layout (log / history / help)
├── bar.rs              # `&App` wrappers for the kit status / bottom bars
├── input.rs            # `&App` wrapper for the kit input box
├── log.rs              # `&App` log panel wrapper
├── log_style.rs        # log line styling
├── slash_style.rs      # /skill-name vs args highlighting
├── task_panel.rs       # task panel
├── test_harness.rs     # `make_app` / `render_app_text` (test-support)
├── cells/              # App-level cell integration tests
└── popups/             # popups that need full `App` state
    ├── command_palette.rs
    ├── code_popup.rs
    ├── diff_popup.rs
    ├── file_picker.rs   # @-file attachment directory browser
    ├── help.rs
    ├── history.rs
    ├── mermaid_popup.rs
    ├── select.rs
    ├── slash_command.rs # Insert `/` menu (Commands then Skills)
    ├── subagent_popup.rs
    ├── system_prompt_popup.rs
    ├── task_dag_popup.rs
    └── thinking_popup.rs

crates/agent_tui_kit/src/render/  # pure drawing: `&RenderCtx`, no `App`
├── mod.rs
├── bar.rs              # top status bar + 2-row bottom bar
├── ctx.rs              # `RenderCtx` — the state each render fn reads
├── input.rs            # input box + soft wrapping (`wrap_line`)
├── log.rs              # log panel
├── log_column.rs       # log column renderer
├── mermaid_sequence.rs # Mermaid sequence-diagram cards
├── pulldown.rs         # pulldown-cmark glue
├── render_md.rs        # Markdown rendering
├── renderable.rs       # Renderable trait
├── selectable_text.rs  # text selection model
├── slash_style.rs      # /skill-name vs args highlighting
├── sticky_host.rs      # sticky host rows for subagent/task domains
├── task_panel.rs       # task panel
├── util.rs             # text wrapping utilities
├── cells/              # card rendering cells
│   ├── text.rs
│   ├── thinking.rs
│   ├── tool.rs         # tool invocation blocks (title + meta + detail card)
│   ├── separator.rs
│   ├── markdown.rs
│   └── code.rs
└── popups/             # popups that render from the ctx alone
    ├── code_popup.rs
    ├── diff_popup.rs
    ├── history.rs
    ├── mermaid_popup.rs
    ├── select.rs
    ├── subagent_popup.rs
    ├── system_prompt_popup.rs
    └── thinking_popup.rs
```

---

## 2. Main Loop and Rendering Entry

The main loop is in `run_tui()` in `crates/tui/src/lib.rs`:

1. **Consume Agent updates**: drain `agent_rx` before drawing to keep state consistent.
2. **Dirty check**: redraw when `app.dirty` is `true`, `Status::Done`, or any tool is still running (`!app.tools.active.is_empty()` — keeps live elapsed time updating).
3. **Compute layout**: split the area based on terminal size and input box height (bottom bar is always 2 rows).
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
| default | 100% log panel (single-column; no side panel or divider) |

It also updates `app.mouse.log_area` from the layout result for later mouse hit testing.

---

## 4. Status Bars (`bar.rs`)

### Top Status Bar (`render_status_bar`)

- Always starts with the **input mode** (`emoji label` from `Messages`): `◆ NORMAL`, `◇ INSERT`, `⚡ PALETTE`, `▣ SELECT`, `📎 FILES`.
- Then the **focused panel** label, rendered by *every* status arm as `{mode} {focus} │ …` (`FocusedPanel` currently has only the `Log` variant, so it always reads `Log`).
- Then the task status, according to `Status` (four variants — see `docs/state_machines.md`):
  - `Idle`: `{mode} {focus} │ ⌨H Hist │ 🎨 {theme} │ 🌐 {language} │ ? Help │ ✕ Quit` (`status_idle_tmpl`, filled with exactly four placeholders in that order).
  - `Planning`: `{mode} {focus} │ {spinner} {status_planning}` (accent color).
  - `Executing { current_step, total }`: `{mode} {focus} │ {spinner} {status_executing_tmpl}` where the template is `Executing step {}` / `正在执行步骤 {}` — **no denominator**: the plan's step total is not what a running task is judged by. The progress step is derived from completed + active tools (`completed + 1` while tools are running, else `completed`), not from `current_step`. With parallel tools it gains ` │ running {n}` / ` │ 并行中 {n}` (warning color).
  - `Done`: `{mode} {focus} │ ✅ {status_done_tmpl}` (success background, bold, 2s highlight).
- Overrides: a temporary `flash_msg` replaces the whole line with `⚠ {msg}` (3s).
- The bar renders **no clock or gauge of its own**: the `[████░░] n%` step gauge was dropped on 2026-09-14 (it restated the step count as glyphs) and the live task elapsed moved to bottom-bar row 1, next to the uptime.

### Bottom Bar (`render_bottom_bar`)

Always **2 rows**, built as `Vec<DropGroup>` (each group a `Vec<Span>`) with a per-segment color hierarchy:

**Row 1 — run context** (separator ` │ `), in display order:

1. permission mode — `plan` / `default` / `auto` (`bottom_permission_*`), **never dropped** (security-critical);
2. working directory;
3. process uptime — `⊙ Up 00:58` / `⊙ 运行 00:58`;
4. live task elapsed — `⏱ Elapsed 00:12` / `⏱ 耗时 00:12`, only while a task is in flight;
5. Git branch — `⎇ name` (`unknown` when unset);
6. optional account — balance (`¤ CNY 9.60`) or quota windows.

**Row 2 — usage** (separator: two spaces), in display order: model name (`-` when unset), `out {max_tokens}`, `think {effort|budget}`, `ctx {pct}% {used}/{window}`, cache `▣ {pct}%`, turn counters `⟳ {user}` (+ `⇅ {llm}` once the task has made its first LLM call), frozen turn timing `⏱ mm:ss` + `avg mm:ss`.

Target layout (wide terminal):
```text
default │ ~/Projects/tact │ ⊙ Up 00:58 │ ⏱ Elapsed 00:12 │ ⎇ feat/web │ ¤ CNY 9.60
deepseek-v4-flash  out 128K  think high  ctx 4% 45K/1M  ▣ 30%  ⟳ 12  ⇅ 3  ⏱ 02:05 avg 01:45
```

**Icons (language-invariant Unicode):**

| Meaning | Glyph | | Meaning | Glyph |
|---------|-------|---|---|---|
| Task elapsed / turn timing | `⏱` | | Balance / quota | `¤` |
| Process uptime | `⊙` | | Session user turns | `⟳` |
| Git branch | `⎇` | | Task LLM turns | `⇅` |
| Cache hit % | `▣` | | | |

**Separators:** ` │ ` on row 1, two spaces on row 2.

**Color roles (all from `Theme`):**

| Role | Content | Theme source |
|------|---------|-------------|
| Dim | icons, separators | `theme.muted_fg()` |
| Primary | model, `out`, `think`, ctx meter | `theme.fg` |
| Secondary | path, uptime, task elapsed, cache %, turns, timing, permission `default` | `theme.bottom_bar_fg` |
| Accent | branch (`⎇ name`) | `theme.accent` |
| Success / Error | balance & quota availability; permission `auto` | `theme.success` / `theme.error` |
| Warning | permission `plan` | `theme.warning` |

**Narrow-width drop order** (each row independently; `fit_row_spans` drops from the end of the group list, and push order *is* survival priority):
Row 1 drops **task elapsed → uptime → path** (permission mode, branch and account are never dropped).
Row 2 drops **timing → turns → cache → ctx** (model, `out` and `think` are never dropped, so `ctx` survives longest among the droppable segments).

**Helpers:** Pure formatting functions — `format_task_elapsed`, `format_quota_value`, `format_model_name`, `format_max_out_tokens`, `format_think_segment`, `format_balance_entry`, `format_quota_window`, `format_cache_pct`, `format_context_meter`, `format_mm_ss`, `format_turn_user`, `format_turn_llm`, `format_turn_timing`, `context_usage_pct`, `group_total_width`, `fit_row_spans`, `build_account_spans` — are unit-tested in `agent_tui_kit::render::bar::render_tests`, and the App-level integration/width-budget tests live in `crates/tui/src/render/bar.rs::render_tests`.

---

## 5. Input Area (`input.rs`)

### Command Line (`render_command_line`)

Used for `Palette` mode:

- Displays `cmd_line` content.
- Cursor is positioned at the end of the text.

### Main Input Box (`render_input_box`)

- Supports multi-line input up to 3 display rows; long lines soft-wrap at character boundaries (CJK double-width aware), and box height / line stats count wrapped rows, not just explicit `\n` splits.
- Renders a rounded-border input box in `Insert` mode.
- **No approval banner**: an agent permission prompt arrives as `AgentUpdate::RequestSelect`, which fills the select popup (`popups/select.rs`) and switches to `InputMode::Select`; the prompt is rendered by the popup overlay, not by the input box.
- Cursor is computed by character width (supports CJK full-width characters) and mapped through the soft-wrapped rows (`caret_in_wrapped`) so it lands on the visible text.

---

## 6. Log Panel (`log.rs`)

The log panel is the most complex rendering component. Its core flow is:

### 6.1 Visibility Index (`visible_indices`)

- Some physical message rows are placeholders for direct cards (thinking, tool, and code output).
- Maintains `visible_indices`: logical row → physical row.
- Maintains `phys_to_logical_cache`: physical row → logical row.

### 6.2 Visual Cache (`visual_cache`)

- Wraps each row to the panel width automatically.
- Caches `visual_cache` and `visual_start_cache`.
- Rebuilt when `messages.len()` or width changes.

### 6.3 Viewport Clipping

- The viewport's first visible *visual* line is `log_scroll.visual_top`
  (authoritative; `usize::MAX` = pin-to-bottom sentinel). `log_scroll.offset`
  is a derived logical mirror for hit-testing and popups.
- Cells taller than the viewport are stepped through visually
  (`visual_step_up/down` in `widgets/state/app/scroll.rs`: half a viewport
  per `j`/`k`, 3 lines per wheel tick inside the cell; row-boundary jumps
  otherwise).
- Only renders `TextCell`s that fall inside the current viewport.

### 6.4 Card Overlays

Completed code cards remain overlays; thinking and tool cards are direct log cells:

| Card Type | File | Description |
|---|---|---|
| Thinking card | `cells/thinking.rs` | Direct live card with one blank row before and after: 1→3 line tail, then one-line completion summary; title and footer report the full line count |
| Code card | `cells/code.rs` | Completed code block card with syntax highlighting; plain language label in title (no side emoji icons) |

### 6.5 Tool Blocks (`cells/tool.rs`)

Tool invocations are rendered as dedicated log rows (not plain `Info` text). Each block uses a **3-tier layout**:

1. **Title row** — step number + tool name + argument summary (e.g. `2. bash (git status)`; truncated at 120 chars)
2. **Meta row** — phase spinner / success or fail prefix, permission label, duration (live while running via `started_at`)
3. **Detail card** (optional) — command output, file preview, or error text; double-click opens `DiffPopup` with full args when truncated

Key types:

| Type | File | Role |
|---|---|---|
| `ToolWidget` | `widgets/tool_widget.rs` | Builds `ToolRenderOutput` from tool name, phase, args, detail |
| `ToolCell` | `render/cells/tool.rs` | Ratatui `Renderable` for one tool block |
| `ActiveToolBlock` | `widgets/state/tool_state.rs` | In-flight tool; supports **concurrent** running tools (`tools.active: Vec<_>`) |
| `ToolBlock` | `widgets/state/tool_state.rs` | Completed tool placeholder rows in the log |

`StepAdded` only updates the internal `app.plan.steps` store (`description` = `tool (arg_summary)`); there is no dedicated plan panel, and it does not insert a separate log line. The log block appears on `StepStarted`. Untruncated args are available in the detail popup via `StepResult.arg_full`.

One blank line separates tool blocks from preceding normal content (`ensure_gap_before_tools`). Thinking completion changes its existing card in place and does not add a separator.

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
- Mouse / word selection (inverted color via `selection_range`)
- Left gutter indent columns

### Card Cells

- `thinking.rs`: direct card separated from adjacent log content by one blank row on each side, with a 1→3 line streaming tail and one-line completed summary; its title and footer use the full line count
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
| File picker | `file_picker.rs` | Triggered by `@`, directory-browsable file selector with query filtering |
| Select popup | `select.rs` | Agent asks the user to choose |
| Slash commands | `slash_command.rs` | Insert-mode `/`: Commands then Skills; **Tab** fills `/name `, **Enter** invokes skills / runs built-ins |
| Skill slash style | `slash_style.rs` | Accent+bold skill token, theme.fg args — used by `input.rs` and user lines in `log.rs` |
| Help panel | `help.rs` | Triggered by `Ctrl+?`, shortcut reference |
| History panel | `history.rs` | Triggered by `Ctrl+H`, retry historical tasks |
| Thinking detail | `thinking_popup.rs` | View full thinking content; adjacent ordered-list items are separated by blank rows |
| File detail | `diff_popup.rs` | Full tool output, file diff, or inline bash/command text |
| Code detail | `code_popup.rs` | View full code block |

Popups usually:

- Occupy 80% × 80% of the screen
- Render `Clear` first to erase the background
- No drop shadow (avoids dark bands on some terminals)
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

- 12 built-in themes: `Dark`, `Light`, `SolarizedDark`, `SolarizedLight`, `GruvboxDark`, `Nord`, `Retro`, `Kawaii`, `Japanese`, `Brutal`, `Ink`, `InkLight`
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

- `Status`: Idle / Planning / Executing / Done
- `InputMode`: Normal / Insert / Palette / Select / FilePicker
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
