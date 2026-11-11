# TUI rendering

The terminal UI uses two crates:

- `crates/tui`: application state, key and mouse handlers, host integration,
  layout, and app-specific prepare phases.
- `crates/agent_tui_kit`: reusable render functions, state models, widgets,
  themes, and localization.

The split lets another host reuse the drawing code without depending on the
full Tact product.

## Frame flow

`run_tui()` in `crates/tui/src/lib.rs` owns the event loop:

1. Drain `AgentUpdate`, account, plugin, and voice events.
2. Flush queued prompts when the agent becomes idle.
3. Repaint only when state is dirty, a timed state needs cleanup, or an
   animation is active.
4. Split the terminal into status bar, main area, input, and two-row bottom
   bar.
5. Run app-specific prepare phases, then call the pure kit renderers.
6. Apply popup mouse surfaces and render commands.

The current layout is:

```text
status bar       1 row
main area        remaining rows
input            wrapped input rows + border + queued-message rows
bottom bar       2 rows
```

The main area contains one of these surfaces:

- the Log;
- the task history panel;
- the help panel;
- the subagent transcript view.

When a sticky Goal, task, subagent, or background strip is visible, the Log
occupies the upper part of the main area and the strip closes the lower part.
The scroll-back pill is drawn on the bottom border of the panel that closes the
Log box.

## RenderCtx

Pure renderers accept `agent_tui_kit::render::ctx::RenderCtx`. The app builds
one context per frame from disjoint reads of `App`.

```text
App
  -> RenderCtx
  -> pure render functions
  -> Vec<RenderCommand> and popup mouse surfaces
  -> App applies the commands after the frame
```

**Narrow-width drop order** (each row independently; `fit_row_spans` drops from the end of the group list, and push order *is* survival priority):
Row 1 drops **uptime → path** (permission mode, branch and account are never dropped). It carries no task clock: the live one is the log's task-stats row (2026-10-05).
Row 2 drops **timing → turns → cache → ctx** (model, `out` and `think` are never dropped, so `ctx` survives longest among the droppable segments).

**Helpers:** Pure formatting functions — `format_quota_value`, `format_model_name`, `format_max_out_tokens`, `format_think_segment`, `format_balance_entry`, `format_quota_window`, `format_cache_pct`, `format_context_meter`, `format_mm_ss`, `format_turn_user`, `format_turn_llm`, `format_turn_timing`, `context_usage_pct`, `group_total_width`, `fit_row_spans`, `build_account_spans` — are unit-tested in `agent_tui_kit::render::bar::render_tests`, and the App-level integration/width-budget tests live in `crates/tui/src/render/bar.rs::render_tests`.

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
- A scroll step moves `visual_top` by a number of *visual lines* and clamps
  it to the content, whatever the row under it is. A row is a storage unit,
  not a scroll unit: `j`/`k` and a wheel notch both move one line
  (`LOG_LINE_STEP`) whether the row is a single line or a paragraph that
  wrapped into twelve. `g` and `G` jump to the ends.
- `widgets/state/app/scroll.rs` owns these positions (`scroll_log_up/down`).
  The coarser steps belong to the subagent transcript only, which is one
  bounded card rather than a column read line by line: three lines per notch
  (`SUBAGENT_WHEEL_STEP`) and half a screen per key (`key_cell_step`).
- `log_scroll.follow` is the whole follow-the-tail policy: when set, appended
  rows re-pin the viewport to the bottom and clear `unseen`; when clear, the
  reader keeps their place and `unseen` lights the "back to bottom" pill. Only
  a step that **moves** the viewport clears it (`scroll_log_up`), so `k` in a
  log shorter than one screen, or already at the top, leaves both flags alone:
  there is nothing to return to. Reaching the end re-arms it
  (`scroll_log_down` -> `scroll_log_to_bottom`).
- Only renders `TextCell`s that fall inside the current viewport.

### 6.4 Card Overlays

Completed code cards remain overlays; thinking and tool cards are direct log cells:

| Card Type | File | Description |
|---|---|---|
| Thinking card | `cells/thinking.rs` | Direct live card with a header row carrying the title, elapsed time, line count and open button, then one row per reasoning line (1→3 tail); a finished card is that header and nothing else |
| Code card | `cells/code.rs` | Completed code block card with syntax highlighting; plain language label in title (no side emoji icons) |

### 6.5 Tool Blocks (`cells/tool.rs`)

Tool invocations are rendered as dedicated log rows (not plain `Info` text). Each block uses a **3-tier layout**:

1. **Title row** — step number + tool name + argument summary (e.g. `2. bash (git status)`; truncated at 120 chars)
2. **Meta row** — phase spinner / success or fail prefix, permission label, duration (live while running via `started_at`)
3. **Detail card** (optional) — command output, file preview, or error text; its `[Open]` button opens `DiffPopup` with full args when truncated (double-click remains a compatibility path)

Key types:

| Type | File | Role |
|---|---|---|
| `ToolWidget` | `widgets/tool_widget.rs` | Builds `ToolRenderOutput` from tool name, phase, args, detail |
| `ToolCell` | `render/cells/tool.rs` | Ratatui `Renderable` for one tool block |
| `ActiveToolBlock` | `widgets/state/tool_state.rs` | In-flight tool; supports **concurrent** running tools (`tools.active: Vec<_>`) |
| `ToolBlock` | `widgets/state/tool_state.rs` | Completed tool placeholder rows in the log |

`StepAdded` only updates the internal `app.plan.steps` store (`description` = `tool (arg_summary)`); there is no dedicated plan panel, and it does not insert a separate log line. The log block appears on `StepStarted`. Untruncated args are available in the detail popup via `StepResult.arg_full`.

One blank line separates tool blocks from preceding normal content (`ensure_gap_before_tools`). Tool blocks and thinking cards stack with each other without a gap — they are one region of work, not two blocks. Thinking completion changes its existing card in place and does not add a separator.

### 6.5.1 Block Spacing

Ordinary content and the card region are separated by exactly one blank row. Inside the card region — tool blocks and thinking cards — rows stack with no gap at all. The rules live in `widgets/state/app/visibility.rs`: `ensure_gap_before_tools` / `ensure_gap_after_tools`, and `ensure_gap_before_thinking` / `ensure_gap_after_thinking` for cards.

The four transitions and who owns them:

| previous → next | gap | rule |
|---|---|---|
| user / assistant prose → tool block | one row | `ensure_gap_before_tools` |
| user / assistant prose → thinking card | one row | `ensure_gap_before_thinking` |
| tool block → thinking card | none | both `ensure_gap_*` return early |
| thinking card → tool block | none | `ensure_gap_before_tools` returns early |
| thinking card → thinking card | none | `ensure_gap_before_thinking` returns early |
| tool block → assistant reply | one row | `ensure_gap_after_tools` |
| thinking card → assistant reply | one row | `ensure_gap_after_thinking` |
| thinking card → system message | one row | `ensure_gap_before_system_message` |

The system-message row is the one a reader would expect to be free and is not:
`add_system_message` appends to the log directly and runs none of the streaming
path's checks, so nothing on the reply side would have inserted that gap.

No gap inside the region is deliberate on two counts: a blank row is what ends a folded tool run (`tool_group::runs` joins blocks that start where the previous one ends), and a call belongs next to the reasoning that explains it rather than being pushed away from it.

The thinking card is why this is written down at all. It used to carry its own leading blank row and a border, so the spacing was implicit in the card's chrome. Once the card became content-only (`cells/thinking.rs`), the boundary between prose and the card region disappeared.

Two consequences worth knowing before editing this code:

- A card's placeholder rows have an empty `raw`, exactly like the blank row that separates blocks, so `raw.is_empty()` cannot tell a gap from a card. `phys_idx_is_separator` excludes rows owned by a card (`phys_idx_owned_by_card`) before answering, and the same predicate is what makes "the previous row is already a card" answerable.
- The gap belongs to the card that opened it (`ActiveThinkingBlock::preceded_by_gap`). A card that finishes with no content removes its own rows *and* that gap, so an empty thinking turn leaves the log byte-for-byte as it found it.

Indent: tool blocks use `LOG_TOOL_BLOCK_INDENT` (8 columns) in `render/util.rs`.

Mouse hit-testing uses `find_tool_at_logical()` so concurrent tool rows map correctly.

Full design (data flow, state model, extension guide): [`docs/tool_rendering.md`](./tool_rendering.md).

### 6.6 Folded Tool Runs

Adjacent finished tool calls fold into one summary row. `tool_group::runs`
derives the runs from the log itself: two blocks belong together when the second
starts exactly where the first ends, so a gap row or a thinking card breaks the
run and membership can never go stale.

A folded run is **one logical row**, not a run of zero-height rows. Its members
are left out of `visible_indices` and `phys_to_logical_cache` entirely. Giving
them zero height instead would repeat the same visual start for two rows, so
the row sitting under the viewport top would be ambiguous for the logical
offset mirror.

Folding changes how many logical rows exist without changing how many physical
rows there are, so `log.items.len()` cannot invalidate the layout caches. Both
cache keys carry `ToolState::fold_rev`, bumped on every toggle.

Three inputs decide whether a run is folded, in order: a run with a live member
never folds; a reader's explicit choice wins over the default; otherwise a
settled run folds. Fold state keys on the run's first member's `tool_id`, never
on `phys_idx`, which every row insert rewrites.

Failures fold like any other finished member. The summary counts them
(`4 tools · 1 failed · …`) and paints the row in the error color, so a failure
stays visible while costing one row instead of the six to seven a collapsed
`✗ Failed` detail card occupies.

The summary reports one wall-clock total for the run, computed as the **union**
of the members' spans rather than the sum of their durations: tools in one turn
run concurrently by wave, so four parallel one-second calls took one second.

#### Folding and unfolding the same run

Both affordances have to work in both directions, and each is where the run
*is* rather than where it was.

`z` takes the **first run with a row in the viewport**, not the run under the
viewport's top row. In a real log that top row is prose or a blank separator, so
a rule keyed on it found no run and the key silently did nothing; it also broke
the second press, because expanding a run moves its summary row off the top.
Scanning the visible rows makes one key do one run wherever the reader is.

A click on the folded summary flips it. While the run is expanded that summary
row does not exist, so the **first member's title row** takes the control: it is
the row the summary occupied, it draws `⏷` in the indent gutter in place of the
summary's `⏵` (same column, so the run does not shift when it opens), and its hit
rect covers the glyph plus that whole title row. The glyph is painted into the
gutter rather than prefixed to the title, because a prefix would push the head
one column right of the members under it and the run would read as a ladder.

Exactly one fold control exists per run at a time — the summary when folded, the
head when expanded — so a click never depends on which hitbox the list happens
to hold first.

#### Keeping the reader's place across a fold

Folding removes rows above whatever the reader is looking at, so the viewport is
moved up by the same amount (`compensate_for_removed_rows`). Two details make it
correct, and both were wrong first.

The run's bounds are **physical rows**, while the viewport is a **visual line**.
The two indices agree only when nothing above the run wraps, so the bounds are
converted through `phys_to_logical_cache` and the visual prefix sum before
anything is compared. Comparing them directly scrolled the reader by the run's own
height whenever the prose above it wrapped.

The number of lines that disappear is the run's visual span less one, because the
fold leaves its summary row where the run began. The compensation is still
skipped when the run was only partly above the viewport: the reader can watch
that collapse, and moving the content under their eyes while they watch is the
stranger of the two behaviours.

### 6.7 Scrollbar

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

Render code cannot mutate `App`. A renderer that needs to queue a message,
repaint, or record a click area returns a typed command or hit area.

### Card Cells
- `thinking.rs`: borderless direct card with a header row carrying the title, elapsed time, line count and open button, then one row per reasoning line; a finished card is that header alone
- `diff.rs`: green `+` prefix, shows file path and line numbers
- `code.rs`: dark blue-gray background, shows language tag and code preview

### Thinking Card Rows

A thinking card draws a header row, then one row per reasoning line. A **live**
card grows to four rows while its tail streams. A **finished** card is one row:
the status row carrying the title, the elapsed time, the line count and the open
button. Its summary is no longer drawn — the reasoning is behind the `[Open]`
button, so a summary row was a preview of something the reader can open in full,
and it cost a row on every settled thought.

The two states do not share a footer template either. A live card reports a
fraction (`⏱ 1.2s | ↕ 2/12 lines | [Open]`) because its tail rows are really on
screen; a finished card reports the total (`⏱ 3.4s | 12 lines | [Open]`), the
same shape a collapsed tool card uses. `↕ 0/12` would state a row count the card
does not have, and `12/12` would be a tautology.

`COMPLETED_THINKING_ROWS` (`render/cells/thinking.rs`) is the finished card's row
count. Every place that counts a settled card's rows — the log placeholder rows,
`find_thinking_at_logical`, `phys_idx_in_thinking_card` — uses that constant, not
`thinking_visual_rows(1)`: the latter is the *live* formula and answers 2.

The header leads because it names the state the block is in, and a live card's
reasoning grows downward from it. With the label on the last row it drifted down
the screen with every new line, and a status that moves is not a status.

The card has no border and no blank rows of its own. It used to spend five rows
on a one-line summary, and four of those five were two borders and two blank rows.

Dropping that chrome moved the boundary between prose and the card region onto
real log rows: a gap row is inserted when a card opens after prose, and removed
with the card if the card ends up empty. A card that follows a tool block gets
no gap — the two stack — see [§6.5.1 Block Spacing](#651-block-spacing).

A measurement over the 60 session transcripts in `.tact/transcripts` (2956
thinking cards, 9976 tool calls) put thinking at **24 percent** of the log's rows
and tool blocks at **32 percent**, under the shapes that were about to change: 5
rows for a finished card, 2 for a settled tool call. Collapsing the card to one
row removes 11824 rows, **19 percent of the whole log**. Those are counts from one
machine's history, not properties of this code, and the row totals are a
reconstruction from the transcripts rather than a render; re-measure before
quoting them. Within the same corpus, **78.6 percent of tool calls sit in an
adjacent run** of two or more, which is what folding can reach.

The header keeps a row to itself rather than joining the reasoning: one row would
have to carry the title and the readout at once, and the reasoning would have
nowhere to go.

The button does survive on one row at ordinary widths, so the extra row does not
buy a working click target. What it buys is summary width: a one-row card spends
a third of the panel on chrome before the summary starts.

Reasoning text starts at `LOG_THINKING_INDENT + 1`, the same column assistant
prose lands in, so a reasoning line and the reply under it share a vertical. The
border used to provide that column; `CONTENT_PAD` now does.

## Prepare phases

Some work needs mutable state or host data before drawing:

- `crates/tui/src/render/log.rs` rebuilds logical-to-physical row maps, wrapped
  visual caches, viewport clipping, and the live stats row reserve.
- `crates/tui/src/render/input.rs` clamps and scrolls the caret into view.
- popup modules may load a diff, prepare Markdown, or cache rendered selection
  text.

The prepare phase and the pure renderer consume the same frame geometry. This
keeps scroll, mouse hit testing, and painted rows aligned.

## Module map

```text
crates/tui/src/
  lib.rs                    event loop and frame composition
  handlers/                 keyboard and mouse behavior by mode
  render/
    layout.rs               main-area selection and sticky split
    log.rs                  mutable log prepare phase + pure kit call
    input.rs                input prepare phase
    bar.rs                  status and bottom-bar app wrappers
    task_panel.rs           sticky strip placement
    popups/                 app-specific popup entry points
  widgets/state/app/        App state and update handlers

crates/agent_tui_kit/src/
  render/
    bar.rs                  status and bottom bars
    input.rs                input rendering and wrapping
    log.rs                  pure Log renderer
    log_column.rs           viewport-clipped Renderable compositor
    sticky_host.rs          sticky strip rendering
    stats_line.rs           live and frozen task stats rows
    cells/                  text, Markdown, code, thinking, and tool cells
    popups/                 reusable popup renderers
  state/                    reusable UI state models
  widgets/                  shared widgets and tool-card builder
  components/               host-facing component registry
  theme.rs                  built-in themes
  i18n.rs                   English and Chinese messages
```

## Log rendering

The Log has four stages:

```text
physical messages
  -> visible logical rows
  -> wrapped visual lines
  -> viewport slice
  -> cells and overlays
```

`LogScroll` stores the authoritative visual top. Logical offsets are derived
for hit testing and popup selection. A bottom-pinned viewport follows new rows;
a user scroll position remains fixed until the user returns to the tail.

Completed thinking rows, tool cards, code blocks, and Mermaid diagrams use
placeholder row ranges so the Log height and mouse mapping stay stable while
content changes.

## Popups

Popups fall into two groups:

- `crates/tui/src/render/popups/`: entry points that need full `App` state.
- `crates/agent_tui_kit/src/render/popups/`: reusable renderers that need only
  `RenderCtx`.

Each active popup records its mouse surface. Click outside, `Esc`, scrolling,
copy, and selection behavior are handled by the app layer. Tool, thinking, code,
Mermaid, system-prompt, and subagent popups use shared chrome and scrolling
helpers where possible.

## Caching and repaint policy

| Cache or flag | Owner | Invalidated by |
|---|---|---|
| Logical and visual row maps | `LogScroll` | Message list or width change |
| Styled Markdown and code | Block state | Block creation or language/theme change |
| Prepared diff rows | Tool popup state | Popup content change |
| `App::dirty` | App | Agent, input, timer, or host event |
| Animation tick | App frame loop | Each active animation frame |

The event loop polls at 10 ms when dirty, 150 ms while animating, 200 ms while
a timed state is expiring, and 1000 ms when idle.

## Theme and localization

`agent_tui_kit::theme::ThemeName` defines the built-in themes. The current list
includes Dark, Light, Solarized Dark, Solarized Light, Gruvbox Dark, Nord,
Retro, Kawaii, Japanese, Brutal, Ink, Ink Light, Claude, and Claude Dark.

`agent_tui_kit::i18n::Messages` supplies English and Chinese strings. The app
builds a fresh `Messages` value in each frame, so stored render outputs must
not freeze localized text or colors.

## Tests

Buffer-level tests live beside the renderers in both crates. App-level tests use
`crates/tui/src/render/test_harness.rs` and the fixtures in
`crates/tui/src/test_fixtures.rs`.

```bash
cargo test -p agent_tui_kit --lib
cargo test -p tui --lib
```

## Related documents

- [`tool_rendering.md`](tool_rendering.md)
- [`state_machines.md`](state_machines.md)
- [`book/23_chapter_tui_zh.html`](../book/23_chapter_tui_zh.html)
