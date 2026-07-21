# Tool Popup Text Selection Design

> Date: 2026-07-19
> Status: approved
> Scope: mouse text selection and context-sensitive copy in tool detail popups

## Goal

Make text in the tool detail popup directly selectable without weakening the
existing one-key full-content copy workflow. A non-empty mouse selection makes
`y` copy the selected original text; with no selection, `y` continues to copy
the complete original popup content.

## Current problem

The tool detail popup displays file content, diffs, command output, and buffered
live output, but it has no popup-specific text selection state. Mouse clicks
inside an overlay are currently consumed only for outside-click dismissal, and
`y` always copies the complete cached content. Users must therefore copy a large
tool result and trim it elsewhere when they need only a fragment.

Screen-cell extraction is not a reliable solution because the popup adds line
numbers and diff gutters, scrolls by source line, wraps some displayed text,
and may contain wide Unicode characters.

## Decision summary

| Choice | Decision |
|--------|----------|
| Selection input | Left-button click and drag inside the tool popup body |
| Stored coordinates | Byte offsets into the original cached content |
| Copy key | `y` copies a non-empty selection, otherwise full content |
| Display-only text | Borders, title, line numbers, diff gutter, and scrollbar are excluded |
| Unicode | Hit testing uses terminal display width and returns valid UTF-8 boundaries |
| Scroll | Selection survives scrolling for the lifetime of the popup |
| Scope | Tool detail popup only; thinking and code popups are unchanged |

## 1. Selection model

Add a small popup selection type with an anchor and active byte offset. The
normalized half-open range is used for both rendering and copying, so dragging
backwards behaves the same as dragging forwards. Equal offsets represent a
caret-like empty selection and do not override full-content copy.

Selection belongs to `DiffPopup`, not global log selection state. Opening a new
tool popup starts without a selection, and closing the popup drops the state
with the popup. The mouse state only records whether a tool-popup drag is in
progress.

All offsets refer to `cached_content`. They are clamped to the content length
and normalized to UTF-8 character boundaries before slicing. Copying retains
the exact original bytes between the normalized offsets, including original
newlines. It never includes text introduced only for presentation.

## 2. Display layout and coordinate mapping

Refactor tool-popup body construction into a pure layout result shared by
rendering and hit testing. Each visible display row carries:

- styled spans to render;
- the body column at which selectable source text begins;
- source byte boundaries for the text cells shown on that row;
- the source byte boundary represented by the row end.

Line numbers and `use_diff_gutter` prefixes occupy non-selectable columns.
Clicking or beginning a drag in those columns clamps to the source line start.
The right side of a displayed row clamps to that row's represented source end.
The popup border, title, footer, and scrollbar do not start a selection.

The mapping walks extended grapheme clusters and uses Ratatui's terminal cell
width calculation. Every occupied cell maps to the complete source byte range
of its grapheme, so wide characters, combining sequences, emoji variation
selectors, modifiers, and ZWJ sequences remain indivisible for hit testing and
selection styling. The mapping uses byte offsets only after calculating display
columns, avoiding invalid string slicing.

Existing display semantics remain unchanged: plain/file views keep their line
numbers and optional green gutter, unified diffs retain their native colors,
and scrolling remains source-line based. If a rendered source row wraps, every
wrapped display row maps back to the corresponding contiguous source range.
Text that is not currently displayed cannot be newly selected by pointing at a
screen cell, but an existing selection remains valid while the user scrolls.
Automatic edge scrolling during a drag is out of scope for this focused change.

## 3. Mouse interaction

Overlay mouse dispatch takes priority over log, plan, and divider interactions.
When a tool popup is open:

1. A left-button down inside a selectable body row resolves the screen cell to
   a source byte offset, sets both selection endpoints, and starts dragging.
2. A left-button drag updates the active endpoint when the pointer resolves to
   a body row. Dragging above or below the body clamps to the first or last
   visible selectable boundary without changing popup scroll.
3. Left-button up ends the drag.
4. A click outside the popup keeps the existing close-and-consume behavior.

Mouse wheel scrolling remains routed to the active overlay. Scrolling does not
clear the selection.

## 4. Rendering and copy behavior

The renderer applies a reversed selection style only to source cells whose byte
ranges intersect the normalized non-empty selection. Existing foreground and
diff colors remain available where the terminal supports combined modifiers.
Display-only prefixes are never highlighted.

`copy_diff_popup` first ensures content is available through the existing
cached/file/inline fallback. It then copies:

- `cached_content[selection]` for a valid non-empty selection;
- all content when no selection exists or the selection is empty.

The existing clipboard fallback chain is unchanged: native clipboard, OSC 52,
then the internal clipboard buffer. Existing copy feedback is reused and shows
a preview of the actual copied text.

## 5. Testing

Focused unit and render tests cover:

- forward and backward range normalization;
- display-column mapping with ASCII, wide Unicode, combining and emoji
  graphemes, empty lines, line numbers, diff gutter, and wrapped rows;
- mouse down, drag, and up routing while a tool popup is active;
- selections surviving popup scroll;
- `y` copying only a non-empty selection and retaining full-content fallback;
- selection highlighting without highlighting line numbers or gutters;
- closing and reopening a popup clearing the old selection.

The TUI documentation is updated in both English and Chinese to describe the
tool-popup-specific interaction and copy precedence.

## Out of scope

- Selection in thinking or code detail popups.
- Keyboard-driven selection expansion.
- Automatic scrolling when dragging beyond the popup body.
- Copy-on-mouse-release.
- Horizontal scrolling or changing the popup's existing content presentation.
