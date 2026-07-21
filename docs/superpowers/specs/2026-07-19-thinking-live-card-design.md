# Thinking Live Card Design

> Date: 2026-07-19
> Status: approved
> Scope: replace thinking's log-row plus overlay rendering with a direct live card

## Goal

Render streaming thinking in the log as a single card, following the same
structural model as a running bash result. The card body grows from one to two
to three logical lines as content arrives, then keeps a fixed three-line tail.
When thinking ends, the same card collapses to a one-line summary while its full
content remains available in the detail popup.

## Current problem

Thinking currently materializes every streamed line into the shared log message
vectors, then completes by hiding all but three source rows and clearing that
area with a separate overlay card. It maintains raw messages, a preview cache,
a Markdown cache, physical source ranges, visibility filtering, and overlay
viewport calculation for one piece of content. Insertions elsewhere in the log
also require physical-index repair.

The running bash path already demonstrates the preferred ownership direction:
one active model owns its bounded live output, and one `ToolCell` renders it
directly in the normal log-cell pipeline.

## Decision summary

| Concern | Decision |
|---|---|
| Rendering owner | New direct `ThinkingCell` in the normal log-cell pipeline |
| Active body height | 1, then 2, then 3 visible logical lines; fixed at 3 thereafter |
| Active content | Latest three logical lines, including a non-empty unterminated tail |
| Completed body height | One summary line: latest non-empty logical line, truncated to card width |
| Full content | Kept in completed thinking state for popup and clipboard copy |
| Popup during streaming | Opens the same detail popup with content buffered so far |
| Completion | Active card changes in place to completed, one-line summary form |
| Rendering removed | No thinking overlay, no per-thinking source rows in shared log messages |
| Scope | Thinking only; bash/tool cells keep their current implementation |

## 1. Data model and lifecycle

Replace the current range-based thinking model with explicit active and
completed card records.

An active record stores one stable placeholder physical index, the accumulating
plain text, a line-oriented tail, and `started_at`. It is created by
`ThinkingChunk::Started`, or lazily by the first `Delta` for compatibility with
older producers. The placeholder is one shared-log row, just as a running tool
has a stable position; the card's visual height is computed by the renderer and
does not add or remove per-line log messages.

On every delta, append to the active plain-text buffer. Split complete lines
into the tail model and retain a non-empty unterminated fragment as the current
last display line. The visible body is the latest `min(3, logical_line_count)`
lines. Empty chunks do not grow the card. The active card therefore changes
height only while moving from one to two to three lines and stays fixed after
that.

`ThinkingChunk::Finished`, or an existing content-producing update that closes
thinking as a compatibility fallback, flushes the remaining fragment and
finalizes the active record. Whitespace-only thinking is removed with no log
residue. Otherwise finalization preserves the stable placeholder index, stores
the complete plain text, Markdown-rendered popup lines, latest non-empty
summary line, and elapsed duration in a completed record. It does not insert a
trailing separator or rewrite unrelated log rows.

Any shared-message insertion/removal still adjusts the single thinking
placeholder index, but there are no title/end ranges or per-thinking content
rows to repair.

## 2. Rendering and interaction

`ThinkingCell` implements the existing `Renderable` contract and is selected
by the log's Phase 3 cell builder in the same manner as `ToolCell`.

The card contains a title/status row and a bordered body. While active, the
title uses the thinking spinner and elapsed duration; the body shows the tail
described above. The visual height is derived from one to three body rows, so
new data grows the surrounding log only until the third row. Once at capacity,
new thinking data updates the tail in place.

On completion, the card renders a completed title/status and exactly one
summary body row. The summary is the latest non-empty logical line, truncated
at a UTF-8 character boundary to fit the card width. A completed card does not
retain the current three-row tail in the log.

The existing double-click and `V` pathways open a detail popup for both active
and completed thinking. The active popup reads the buffer accumulated so far;
the completed popup renders the cached Markdown lines. `y` continues to copy
the complete thinking content from the active or completed record. Popup
scrolling, outside-click dismissal, and overlay-key routing remain unchanged.

Thinking cards no longer participate in log character selection because their
body is a direct renderable cell, matching tool cards. A single click selects
the card for a later double-click; it does not create a log text selection.

## 3. Integration boundaries

Remove the thinking-specific visibility filtering and overlay rendering path:

- `is_message_visible` no longer hides interior thinking message rows because
  those rows no longer exist.
- `render_thinking_cards` and its overlay viewport logic are removed.
- `render/layout.rs` continues to render only the thinking detail popup when
  one is open.
- The generic physical-index shift helpers update the active/completed
  placeholder index, not title/end ranges.

The tool-cell path is a structural reference, not a shared generic abstraction.
Thinking has different data semantics (plain reasoning, Markdown popup, no
stderr/diff gutter), so this change introduces a focused `ThinkingCell` rather
than broadening `ToolCell` or creating a speculative common live-card type.

## 4. Tests and documentation

Tests cover:

- `Started`, missing-`Started` compatibility, line buffering, unterminated
  tails, and whitespace-only finalization;
- active card body heights of one, two, and three rows, with a stable height
  after additional deltas;
- active tail replacement and completed one-line summary rendering;
- finalization in place without unrelated message insertion;
- active and completed popup content/copy behavior;
- double-click, `V`, and click-selection behavior for direct thinking cells;
- physical placeholder-index adjustment when shared rows are inserted or
  removed;
- removal of old visibility/overlay behavior.

Update the English and Chinese TUI chapter together, plus rendering
documentation, to describe the direct-card model and the 1-to-3 active body
height followed by one-line completion summary.

## Out of scope

- Changing bash/tool live-output layout or extracting a generic live-card API.
- Card-internal scrolling while thinking is active.
- Thinking text selection in the collapsed card.
- Persisting an unbounded reasoning transcript outside the existing task/log
  persistence model.
