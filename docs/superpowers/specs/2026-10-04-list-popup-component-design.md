# List popups — one component instead of four renderers

Status: **implemented in the working tree** as of 2026-10-04, branch
`feat/codex-hook-and-mcp-parity`. Written alongside the code.

## Problem

Four list-style popups (a focused row, a scroll window, an empty state) were
built four ways. `SelectPopupWidget` was already the component — three callers
just never used it.

| Renderer | chrome | window | mouse hit area |
|---|---|---|---|
| `crates/tui/src/render/popups/command_palette.rs` | `render_list_popup_chrome` | **none** | **none** |
| `crates/tui/src/render/popups/file_picker.rs` | `render_list_popup_chrome` | **none** | **none** |
| `crates/tui/src/render/popups/slash_command.rs` | hand-built `Block` | inline, own formula | yes |
| `crates/agent_tui_kit/.../select_popup_widget.rs` | hand-built `Block` | `scroll_offset` | yes |

Consequences, not just duplication:

- **The window is the bug.** `command_palette` and `file_picker` size the frame
  `count + 6` / `count + 5` and hand every row to `List`, which clips from row
  0. Once the filtered list outgrows the frame, the focused row moves below the
  fold and the list reads as frozen. `select` and `slash_command` both carry a
  comment warning about exactly this failure, and both solved it separately.
- The pin rule `visible.saturating_sub(3).min(visible.saturating_sub(1))` is
  written twice, once per solver.
- Empty state (`fg(theme.muted)`), the focused-row style
  (`bg(theme.highlight).fg(theme.fg)`) and its light-theme rationale are
  written three times.
- `truncate_chars` exists verbatim in `command_palette.rs` and
  `slash_command.rs`; the kit holds a third, differently-behaved copy
  (`util::truncate_chars_with_ellipsis`, which reserves 3 for the ellipsis).
- `handle_mouse_event`'s scroll arms test `select_popup_area` and
  `slash_popup_area` by hand, so palette and file picker never got the routing.

## Design

### One component, `agent_tui_kit::widgets::list_popup`

`ListPopup` owns everything the four renderers repeated, and nothing else:

- **geometry** — `centered_list_popup_area` + `popup_inner`, caller-supplied
  width and desired height;
- **chrome** — `Clear`, border, top title, optional bottom footer;
- **window** — `visible` / `offset` from `window_offset`, the single pin rule;
- **selection** — the focused row's band and text color;
- **empty state** — `theme.muted` hint in the first content row.

Callers keep exactly one responsibility: **build the rows**.

```rust
pub struct ListRow<'a> {
    pub spans: Vec<Span<'a>>,
    /// Group headers (palette categories, slash sections) draw but never focus.
    pub selectable: bool,
}
```

- Rows are built *as if unselected*. Focus-dependent text (the `▶` / two-space
  prefix) is still the caller's job, because only the caller knows the column
  layout — but the *styling* of the focused row is the component's.
- In `SelectionStyle::Highlight` the component patches every span of the
  focused row to `theme.fg` and paints `theme.highlight` across the row's full
  inner width. Callers no longer branch on focus to pick a color.
- `SelectionStyle::CallerStyled` leaves both alone: the slash-command list
  draws its focused row in `theme.accent` + BOLD and must keep doing so
  (`popup_scene_tests::slash_popup_rows_take_their_colors_from_the_theme`
  asserts the accent). Unifying that look was not worth breaking the test and
  the visual.

### Windowing is the same rule everywhere

```rust
pub fn window_offset(row_count: usize, selected_row: usize, visible: usize) -> usize {
    if row_count <= visible || visible == 0 { return 0; }
    let max_offset = row_count - visible;
    let pin = visible.saturating_sub(3).min(visible.saturating_sub(1));
    selected_row.saturating_sub(pin).min(max_offset)
}
```

`select_popup_layout` and `slash_command` both feed this. `command_palette` and
`file_picker` inherit it for free — that is the behavior fix.

Note the row index, not the item index: group headers occupy window rows, so
both header-bearing popups map item → row before calling in (`slash_command`
already did; `command_palette` now does too).

### Sizing policy stays at the call site

The four popups genuinely disagree about width and height — palette 60 % of the
terminal clamped to `[60, 120]`, file picker a flat 50 columns, slash a
percentage of height, select a content-and-footer measurement. The component
takes `width` and `height` and caps them to the area; it does not invent a
fifth policy. Height is the *desired outer* height, so the palette keeps its
`count + 6` frame and its short-terminal cap.

### Two called-out visual changes

1. **The focused row's highlight now spans the full inner width**, not just the
   glyph columns. This is `AGENTS.md`'s "no shadow / residue" rule 2: a span's
   background is a patch, not a band, so a highlight that stops after the last
   glyph reads as a broken row. Only the focused row is affected.
2. **`slash_command`'s popup interior is now `theme.bottom_bar_bg`**, like the
   other three, instead of whatever `Clear` left behind (terminal default). The
   module comment already claimed "the popup background is `theme.bg`".

### Deliberately *not* changed

- The slash-command popup keeps `BorderType::Rounded` and its accent border
  even on the themes whose `block_border_type()` is `Plain` (Brutal, Ink,
  InkLight). It is the one popup with its own visual identity, and changing it
  would show up on three themes that nothing in this refactor is about.
- `SelectionStyle::CallerStyled` keeps the slash list's focused row in
  `theme.accent` rather than a highlight band.
- Sizing policy stays at the call site, so no popup's frame moves.

### Mouse routing becomes a table

`MouseState` gains `palette_popup_area` and `file_picker_popup_area`. All four
renderers are now called unconditionally and clear their own hit area when
inactive — the pattern `select` and `slash_command` already documented, which
the palette and the file picker could not follow while `lib.rs` gated the call
on the input mode.

`handle_mouse_event`'s two hand-written arms collapse into `list_popup_at` +
`scroll_list_popup`, so a fifth list popup costs a variant and a hit-area write
instead of a new `if` in two places.

### The palette filter gets one definition

`App::palette_filtered()` now returns the matching command indices, and
`App::step_palette_selection(delta)` clamps the cursor to them. The predicate
was written twice (renderer and Enter handler) and the cursor could grow past
the end of the list, since only the renderer clamped it.

## Filtering is caller state, not a component feature

`ListPopup` renders the rows it is handed. "Which rows" is a data question, so
the component deliberately owns no query — the palette filters because
`App::cmd_line` exists, the slash list because its candidates are derived from
the input text, and `SelectPopup` could not filter because it never had a query
field. That is why unifying the renderers did not, on its own, make `/model`
searchable.

So the filter lives where the options live:

- `SelectPopup::query` + `filtered_indices()` (case-insensitive substring, the
  palette's rule), and `push_query` / `pop_query` / `clear_query` which
  re-anchor the cursor on the first match.
- **`selected` stays an index into `options`, never into the visible subset.**
  `ThemePick` maps the confirmed index onto `ThemeName::all()` and
  `PermissionModePick` onto the three modes, so filtering must not renumber the
  options; `move_up` / `move_down` walk the visible subset but store the
  original index. `SelectPopupWidget` converts to a row position only for the
  window and the highlight band.
- **`filterable()` is `request_id.is_none()`** — the field's existing meaning.
  Local picks (`/model`, `/model-subagent`, `/theme`, `/permission`,
  `/view-system-prompt`, the effort / think-budget follow-ups) are the user's
  own list and get the filter; agent-originated prompts (`SelectKind::Agent`)
  stay arrow-only, because the agent is blocked waiting and a stray keystroke
  must not hide the choices.
- Keys on a filterable popup: printable characters filter, `Backspace` deletes,
  `Esc` clears the filter first and cancels only on the second press. `j`/`k`
  therefore stop being navigation there, and the footer says `↑↓` plus
  `a-z Filter` instead of `↑↓/j/k`.
- The filter line is a reserved header row — present even while empty, so the
  popup does not change height on the first keystroke. It reads as a text field
  rather than as one more row of the list: a `🔍` in `theme.accent`, the query in
  `theme.fg` (or a muted placeholder while empty), and a caret — a space with
  `theme.fg` behind it — at the insertion point. The caret is always drawn,
  because this line is the popup's only field and it always has the focus. A
  bare `>` was tried and read as a list row; a background band was rejected
  because `theme.input_box_bg` equals the popup's own `bottom_bar_bg` on six of
  the twelve themes, so it would be invisible on half of them.
- When the popup is too short for prompt + filter + one option row, the
  **prompt** is dropped: the filter line and one visible option are not
  negotiable.
- A filter matching nothing shows `select_no_match`, not `select_empty`, and
  `Enter` refuses to confirm a row that is not on screen.

## Who is on this path

The four *renderers* are the palette, the file picker, the slash list and
`SelectPopupWidget`. The last one is worth spelling out, because "select" reads
like one popup and is really every picker in the TUI:

| Flow | Reached through |
|---|---|
| `/model`, `/model-subagent` | `SelectKind::ModelPick(ModelTarget::{Main,Subagent})` |
| `/theme` | `SelectKind::ThemePick` |
| `/permission` | `SelectKind::PermissionModePick` |
| `/view-system-prompt` | `SelectKind::ViewSystemPrompt` |
| model effort / thinking budget follow-ups | `ModelProfileEffortPick`, `ThinkBudgetPick` |
| `ask_user` / permission prompts from the agent | `SelectKind::Agent` (request-id driven) |

So a change in `ListPopup` is a change to the model picker. The tests that read
like "select popup" (`select_popup_long_list_scrolls_selected_into_view`, …) are
in fact the `/model` list: their helper sets `SelectKind::ModelPick`. There is no
separate model-list renderer anywhere — `tact-ui` has none either.

Every row above except the last is a local pick (`set_local`) and is therefore
filterable; `SelectKind::Agent` uses `set` / `set_multi` and is not.

## Migration order

1. `list_popup.rs` + unit tests (window, band, empty, header).
2. `command_palette` → `file_picker` (both gain the window and the mouse area).
3. `slash_command` (own border styling and section headers; keeps
   `CallerStyled`).
4. `SelectPopupWidget` delegates its render to `ListPopup`; `select_popup_layout`
   keeps its signature and its tests, and now shares `window_offset`.

## Non-goals

- `history` (`PopupWidget`) is not a list popup: it has no focused row.
- The `task_dag` / `code` / `diff` / `thinking` / `subagent` popups scroll a
  document, not a selection.
- `centered_popup_area` (80 % of parent) is the document-popup geometry and is
  untouched.
