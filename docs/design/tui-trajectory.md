# TUI trajectory: design

Status: implemented for the parts below.

Built: the record log (`agent_tui_kit::state::trajectory`), the renderer
(`agent_tui_kit::render::trajectory`), and the app wiring
(`crates/tui/src/widgets/state/app/trajectory.rs`) with `/trajectory` as its
entry. Tests: 44 in the kit, 18 in the TUI through the real Normal-mode
handler.

Not built, and still a decision: a key binding. The page is reachable through
`/trajectory` and no letter is claimed, which is the state this document
argues for.

## Purpose

The desktop has a Trajectory page. It renders the same transcript the chat
view renders, projected as a ledger: one numbered record per thing the run
did, grouped by turn, with the record's kind, status, duration, input and
output beside it. Plus a three-lane overview plot above the ledger.

The TUI has no equivalent. A reader who wants to know what a run did and in
what order has one option: scroll the log. The log is a conversation, which is
the right shape for reading prose and the wrong shape for auditing a run.

This document decides what a TUI trajectory view is, where it lives, and how
it is built. It records the measurements that constrain the layout, because
two of the three obvious designs do not fit a terminal.

## What the desktop page is, so the TUI can be compared honestly

Three parts, top to bottom.

1. **Summary bar.** Turns, records, tool calls, measured time. A figure is
   omitted when no producer reported a basis for it, so the bar never shows a
   zero it cannot back.
2. **Overview.** A 50px plot, three lanes (Input, Model, Tools), one block per
   record placed at `index * slot_width`. Turn boundaries are drawn as rules
   behind the blocks. Clicking a column selects its record.
3. **Ledger.** A virtualised list. Each row is `#N`, a kind chip, a one-line
   summary, and a right-aligned duration. A turn rule row precedes each turn's
   first record. Selecting a row opens a detail panel with five tabs (Summary,
   Payload, Result, Schema, Timing).

The record model is `TrajectoryRecord`
(`crates/desktop_core/src/trajectory.rs:123`): `index`, `turn`, `step`, `kind`,
`text`, `tool_id`, `tool_name`, `at`, `status`, `input`, `output`,
`duration_ms`, `usage`, `search`.

## The constraint that shapes everything: width

The desktop overview gives each record one equal-width column. In pixels with
a 691px track that is generous. In a terminal it is not.

Measured, at the widths the TUI actually runs (track = width − borders − the
lane label gutter). HEIGHT: how much room the page has
  terminal=24  main=18  content=16
  terminal=30  main=24  content=22
  terminal=40  main=34  content=32
  terminal=50  main=44  content=42

WIDTH: cells per record if one record gets one column
  (this is the desktop rule; 0 means the plot cannot show it)
  records= 12  w=80:  6  w=120:  9  w=160: 12
  records= 30  w=80:  2  w=120:  3  w=160:  5
  records= 60  w=80:  1  w=120:  1  w=160:  2
  records=120  w=80:  0  w=120:  0  w=160:  1
  records=300  w=80:  0  w=120:  0  w=160:  0

WIDTH: records per bucket with one cell per bucket
  records= 12  w=80:  0.2  w=120:  0.1  w=160:  0.1
  records= 60  w=80:  0.8  w=120:  0.5  w=160:  0.4
  records=120  w=80:  1.7  w=120:  1.1  w=160:  0.8
  records=300  w=80:  4.2  w=120:  2.7  w=160:  2.0 reproduces the table:

| records | width 80 (72 cells) | width 120 (112 cells) | width 160 (152 cells) |
|---|---|---|---|
| 12 | 6 cells each | 9 | 12 |
| 30 | 2 | 3 | 5 |
| 60 | 1 | 1 | 2 |
| 120 | **0** | **0** | 1 |
| 300 | **0** | **0** | **0** |

A session with 120 records is an ordinary session, not an extreme one. One
result from `cargo test` is one tool call; a debugging session reaches 300
without trying. So **one cell per record does not survive contact with a real
session**, and copying the desktop overview into the TUI would ship a plot that
is blank exactly when it would be most useful.

## The constraint that shapes everything else: height

`crates/tui/src/lib.rs:411` owns the frame split: status 1 row, main area
`Min(1)`, input `input_lines + 2 + pending`, bottom bar 2 rows.

Computed from those constraints:

| terminal | main area | content (after the log box borders) |
|---|---|---|
| 24 rows | 18 | 16 |
| 30 rows | 24 | 22 |
| 40 rows | 34 | 32 |
| 50 rows | 44 | 42 |

At the most common terminal size, 24 rows, the whole page has 16 content rows.
A summary bar, a 3-row overview and a bordered ledger leaves 12 rows for the
ledger, which is about 11 records plus a turn rule. That is a working page. It
is also small enough that every fixed row has to earn itself.

## Decisions

### D1. A page, not a popup

The view replaces the main area, the way `show_help`, `show_history` and the
subagent view do (`crates/tui/src/render/layout.rs:15-26`). It is not a
`centered_popup_area` box.

Measured both ways against the frame split. The popup geometry is 80% of the
parent with a 40x10 floor and three rows of chrome (two borders plus the footer
hint row, `crates/agent_tui_kit/src/render/popups/mod.rs:110` and `:244`):

| terminal | popup content | popup ledger rows | page content | page ledger rows |
|---|---|---|---|---|
| 24 rows | 11 | 7 | 16 | 12 |
| 30 rows | 16 | 12 | 22 | 18 |
| 40 rows | 24 | 20 | 32 | 28 |

A ledger row is one record, so the popup costs 5 of 12 records at the most
common terminal size and 8 of 28 at a large one. Two further costs are not
arithmetic: the popup calls `Clear`, so it hides the log the reader may want to
cross-reference, and it closes on an outside click, which is wrong for a
surface navigated with `j`/`k` while reading.

The subagent view is the precedent for a page with its own keymap and its own
scroll state, and it is the closest existing thing to what this is.

**One consequence to accept.** `render_main_area` resolves pages *before* the
sticky-strip split, so a page also hides the Goal, task, subagent and
background strips. The trajectory page inherits that. It is the right trade for
an audit surface (the strips are live status, the page is a still record), but
it should be a stated choice rather than a surprise.

### D2. The overview is bucketed, not per-record

The plot has one cell per **bucket**, and a bucket aggregates every record that
falls in it. The cell glyph encodes how many records the bucket holds
(`·` none, then `▁▂▄█` by density), and the colour is the dominant lane.

This is the one place the TUI deliberately differs from the desktop. It is a
measurement, not a taste: at 300 records and 80 columns there are 72 cells, so
bucketing gives about 4 records per cell and the plot still shows where the
work concentrated. Per-record gives zero cells per record and shows nothing.

The turn rules survive bucketing unchanged: a rule is drawn at the bucket
holding a turn's first record.

Below a floor (fewer cells than records divided by a density threshold) the
plot degrades to a single-line density strip rather than lying about
resolution. A session short enough for one cell per record renders as the
desktop's discrete blocks, because at that length the discrete form is more
informative.

### D3. The overview is collapsible, and collapses by default on short terminals

The plot costs 3 rows of 16. On a 24-row terminal the ledger is the thing
being read, so the plot starts collapsed there and expands on demand. The
decision is a pure function of the content height, so it is testable without a
terminal.

Collapsed it is the summary bar plus one density line: 1 row total.

### D4. The TUI gets its own record model; it does not reuse `TrajectoryRecord`

`TrajectoryRecord` is built from `TranscriptRow`
(`crates/desktop_core/src/transcript/row.rs`). The TUI has no transcript rows.
It consumes `AgentUpdate` directly and paints into `LogCoordinator`, whose
`LogItem` is `{ line, raw, kind, markdown_cell, popup_source, gutter }`
(`crates/agent_tui_kit/src/state/log.rs:140`) and carries no id, status,
timestamp, duration or tool id.

Two options were considered.

**Reuse by moving the transcript reducer into a shared crate.** The reducer is
5,367 lines (`crates/desktop_core/src/transcript/reducer.rs`). It has almost no
coupling outside its own module: two `crate::` references, one for
`LatestToolView` and one for `gates::PendingUiRequest`. It is extractable.

Rejected for now, and the reason is cost, not architecture: the TUI would have
to adopt the transcript as its log source, which means the log's painting,
scrolling, folding and 300+ render tests all move onto a different row model
in the same change. That is a rewrite wearing a refactor's clothes. Extraction
is the right end state and should be its own project, after this page proves
the model is worth sharing.

**Add a parallel record log at ingestion.** Chosen. `App::handle_agent_update`
(`crates/tui/src/widgets/state/app/agent.rs:49`) is the one choke point every
update passes through before dispatch. A `TrajectoryLog` beside the existing
components appends one record per update, capturing the fields the page needs
while they still exist.

This is also forced by evidence: several fields the ledger needs are **dropped
today** and cannot be recovered later.

- `run_id` is discarded everywhere in the TUI (the tool component matches it
  with `..`). It is the only per-activation grouping key.
- Streaming output is dropped on finalize (`finalize_push`); only the capped
  `detail_preview` and a conditional `detail_full` survive.
- Assistant text has no row object at all; it is painted from a markdown
  buffer.
- Thinking has no status field and no start/end wall clock.
- Hook durations exist only inside the painted string.

### D5. Turn numbering comes from the same choke point as the status bar

`StatusBarState.turn_user` is bumped at dispatch
(`crates/tui/src/handlers/skills.rs:113`). The record log reads the same
counter at the same moment rather than re-deriving turns from separators. A
sentinel row (`\x07tact-task-end`) exists but segmenting on it is string
sniffing, and it is not emitted for every turn shape.

### D6. Follow the desktop's honesty rule about durations

A duration is shown only when the producer reported one. Tool durations come
from `StepResult.duration_us` (measured by the kernel). Thinking durations come
from `ActiveThinkingBlock.started_at`. Anything else shows `—`, not a
stopwatch the UI invented.

## What the page looks like

At 24 rows, the whole frame:

```text
 1  status bar
 2  ┌ trajectory ─────────────────────────────────────────────┐
 3  │ 3 turns · 12 records · 4 calls · 8.7s                   │
 4  ├─────────────────────────────────────────────────────────┤
 5  │ INPUT █     ·     ·   █   ·   █     ·                   │
 6  │ MODEL   █   █   █   ·   █   █   █   █                   │
 7  │ TOOLS ·   █   █   ·   ·   █   █   █                     │
 8  ├─────────────────────────────────────────────────────────┤
 9  │ TURN 1                                                  │
10  │ # 1 USER      why does the retry test fail              │
11  │ # 2 THINKING  reading the assertion              1.2s   │
12  │ # 3 TOOL      read src/lib.rs                    0.3s   │
13  │ # 4 ASSISTANT the mock returns early                    │
14  │ TURN 2                                                  │
15  │ # 5 USER      fix it                                    │
16  │ ...                                                     │
17  └─────────────────────────────────────────────────────────┘
```

Rendered from the prototype at that exact geometry, not sketched by hand.

The ledger row is a fixed-column layout: `#N` (3), kind (9), summary
(remaining), duration (right-aligned, 6). At 80 columns the summary gets 55
cells. Narrower than 60 columns the kind is abbreviated to 4 characters and the
duration moves to the detail panel, because a truncated summary is the row's
whole purpose.

## Entry points

Three ways in, all resolved to one open function, `App::open_trajectory_page`,
so the surface cannot be entered in a half-initialised state.

1. **`/trajectory` slash command.** The primary entry, matching `/tasks-dag`.
   Registered as `BuiltinCommand::Trajectory` in `crates/catalog` so `--help`,
   the palette and the slash completion all list it from one source.
2. **A card affordance.** The log's task-end separator and the status bar's
   turn counter are the two places a reader already is when they want to audit.
   The separator gets an `[ abreast ]`-style action like the thinking card's
   `[Open]`; this is a later step, not the first PR.
3. **A key binding, left open.** Normal mode's free single-letter keys after
   the existing set (`/ c g G i j k q t V y Y z` are taken) are `a b d e f h l m
   n o p r s u v w x A B C D E F H I J K L M N O P Q R S T U W X Z`. Choosing
   one is a decision about someone else's muscle memory, and the wrong choice
   is worse than no binding. The design leaves it open and says so; the page is
   fully usable through `/trajectory` without it.

Opening does three things, all of which the subagent view already does and the
existing page flags do not:

- sets `show_trajectory`, and clears `show_help`, `show_history` and
  `show_subagent_view` so two pages cannot both claim the main area;
- switches to `InputMode::Normal`, because the page's keymap (`j`/`k`/`g`/`G`)
  is dispatched from the Normal handler and a session starts in Insert, where
  those letters would type into the input box instead;
- computes an initial selection (the last record) and leaves `follow` on, so
  opening during a live run lands on the newest work rather than the top.

Closing is one function, `App::close_trajectory_page`, which clears the flag,
restores `InputMode::Insert`, and clears every hit rect the page recorded. The
subagent view's close path does exactly this and the comment on it explains
why: leaving one field set let a later transcript update badge a view that was
already closed.

## Presentation

**It is the subagent view's form, not a popup.** Full main area, one header row
of chrome, the rest the content. Concretely, the same three-part shape the
subagent view uses (`crates/tui/src/render/popups/subagent_popup.rs:24`): a
fixed first row, then `Min(1)` for the body, with one column reserved for a
scrollbar when the terminal is wide enough.

The differences from the subagent view, all because this is a list of records
rather than a conversation:

| | subagent view | trajectory page |
|---|---|---|
| header | breadcrumb: parent › child | summary: turns · records · calls · time |
| body | wrapped prose rows, one scroll | fixed-column ledger rows, plus overview band |
| keys | `j k g G y Enter Esc` | `j k g G Enter Tab o / q Esc` |
| selection | none, it is a transcript | one selected record, coloured by its own status |
| live | follows the child's tail | follows until the reader scrolls up |
| state kept across close | scroll, expanded steps | scroll, selection, filter, tab, overview |

**No popup chrome.** The page does not use `render_popup_chrome`: no `Clear`
(it covers nothing), no centred geometry, no footer hint row (it would spend
one of 12 ledger rows). Hints live on the header row, right-aligned, the way
the subagent view puts its `normal · esc back` badge
(`crates/agent_tui_kit/src/i18n.rs:578`).

**The detail panel is a bottom split, not a side panel.** At 80 columns a side
panel would leave the ledger 40 cells and the summary would not fit. It takes
40% of the content height, so 6 rows at the smallest terminal, and scrolls
inside itself. `Tab` cycles Summary / Payload / Result / Timing. Schema is
dropped from the desktop's five tabs because at 6 rows a schema block is one
line of the answer and four of its wrapper.

**Selection is a record id, not an index.** Appending during a live run must
not move the reader's place, and an index would.

**The overview band is the page's own, not the desktop's plot.** See D2. It is
drawn into the ledger's block area as three labeled rows, so it adds no border
rows of its own.

## Keys

In the page:

| key | effect |
|---|---|
| `j` / `↓` | next record |
| `k` / `↑` | previous record; turns `follow` off |
| `g` / `G` | first / last record; `G` turns `follow` on |
| `Enter` | toggle the detail panel |
| `Tab` | next detail tab |
| `o` | toggle the overview band |
| `/` | filter; `Esc` restores the unfiltered ledger |
| `q` / `Esc` | back to the log |

Wheel scrolling over the ledger works through the existing
`SurfaceId` hit-rect mechanism: the page records its own area on open and
clears it on close.

Live: while the page is open, `App::handle_agent_update` appends to the record
log and the page repaints, the way `/tasks-dag` re-renders its lines
(`crates/tui/src/widgets/state/app/agent.rs:689`).

## Where the code goes

Kit side (`crates/agent_tui_kit`), because it is reusable and pure:

- `src/state/trajectory.rs`: `TrajectoryRecord`, `TrajectoryKind`,
  `TrajectoryLog` (append, clear, iterate, filter), the bucketing function and
  the collapse decision. No ratatui rendering, no `App`.
- `src/render/trajectory.rs`: `render_trajectory_page(area, buf, &TrajectoryView,
  &TrajectoryPageState, &RenderCtx)` plus the ledger row and band renderers.
- `src/render/trajectory/overview.rs`: the density ramp and turn rules.

App side (`crates/tui`):

- `src/widgets/state/app/trajectory.rs`: the `show_trajectory` flag, the page
  state (selection, scroll, filter, tab, overview collapsed), the open/close
  functions and the key handler.
- `src/render/popups/trajectory_page.rs`: the app wrapper, resolving i18n and
  theme into the kit call.
- `src/render/layout.rs`: one more early-return page branch, before the sticky
  split.
- `src/handlers/mod.rs`: the `/trajectory` slash command; `src/handlers/
  normal.rs`: keys while the page is up; `src/lib.rs`: `Esc` closes the page in
  the same branch that closes help, history and the subagent view.
- `crates/catalog`: `BuiltinCommand::Trajectory` so `--help` and the palette
  list it; `crates/agent_tui_kit/src/i18n.rs`: the strings, both locales.


## Verification

The page is only worth shipping if these hold. Each is a test, not an
intention.

1. **Bucketing is faithful.** For a synthetic 300-record session at 80
   columns, every record lands in a bucket, no bucket is empty while a later
   one is full, and the total density equals the record count.
2. **The layout fits.** At 80x18 and 120x34 main areas, the rendered buffer
   contains the summary, the ledger header, and at least 8 ledger rows. A
   buffer-level test like the ones beside the other renderers.
3. **Nothing is invented.** A record whose producer reported no duration
   renders `—`. A turn with no records renders no turn rule.
4. **The record log is append-only and lossless for the fields named.**
   Feeding a captured `AgentUpdate` sequence produces records whose tool name,
   arg summary, status and duration equal what the tool component holds.
5. **Selection survives an append.** Scrolling to record 40, then receiving a
   new update, leaves the selection on record 40 and does not jump.
6. **The collapse rule is a function of height.** 16 content rows collapses,
   22 does not, asserted directly with no terminal.
7. **One page at a time.** Opening the trajectory page while the subagent view is
   up leaves `show_subagent_view == false`, and opening either of the other two
   pages closes the trajectory. Closing returns `InputMode::Insert` and clears
   every hit rect the page set: the subagent view's close path exists for
   exactly this reason, and a missed field made a closed view badge on later
   updates.
8. **The page hides the sticky strips.** `render_main_area` resolves page flags
   before the sticky split, so a test asserts the strip is gone while the page is
   up. That is the chosen behaviour, and the test is what stops it drifting.
9. **A free key is not silently stolen.** If a binding is added later, the test
   set that proves the other Normal-mode letters still do what they did has to
   keep passing. Today no letter is bound, so this one is a note rather than a
   test.

## What this deliberately does not do

- No editing, retrying or cancelling from the page. It is an audit view.
- No per-record diff rendering beyond the summary line; the existing diff popup
  stays the place to read a patch.
- No session switch inside the page; it shows the current session, and the
  existing session switch owns changing that.
- No attempt to unify with the desktop page's record model yet. D4 explains why,
  and what would have to be true to change that.
