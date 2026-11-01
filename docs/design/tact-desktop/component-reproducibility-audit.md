# gpui-ai Reproducibility Audit

Reference: `gpui-ai` v0.8.0, inspected from
`https://github.com/labcoder/gpui-ai` and the live component catalog at
`https://labcoder.github.io/gpui-ai/components`.

## Intent

The desktop PRD and prototype must be implementable without silently replacing
claimed `gpui-ai` components, and without hiding product requirements that the
upstream APIs do not support.

## Verified Findings

### F1. PromptBar cannot render the nested slash command tree by itself

Evidence:

- `prompt_bar.rs` exposes flat `PromptCommand { id, label, description }`.
- `prompt_bar/suggestions.rs` only tracks one whitespace-delimited token,
  filters by substring, and Enter inserts the selected suggestion.
- It has no nested subcommands, value candidates, or separate Tab-complete
  behavior.

Impact: the prototype's `/hooks trust --source`, `/goal pause <id>`,
`/mcp auth <server>` and other nested flows cannot be reproduced by
`PromptBar::set_commands` alone.

Resolution:

- `PromptBar` remains the real composer for text, models, mentions and
  attachments.
- Tact owns `TactSlashCommandOverlay` for nested paths, value completion,
  Enter/Tab semantics and command-result routing.
- The prototype marks the slash surface as
  `PromptBar + TactSlashCommandOverlay`.

### F2. ThreadList cannot reproduce the project-scoped session manager

Evidence:

- `ThreadList` exposes sections, active ID, archived toggle and its own native
  search input.
- Its row filter is case-insensitive substring matching.
- It has no project pane, Active/Archived/All filter, external fuzzy query, or
  project-scoped management model.

Impact: the full Session Manager cannot be a direct `ThreadList`, and the rail
cannot claim a bare `ThreadList` while also owning a separate search/Manage
shell.

Resolution:

- The full project/fuzzy Session Manager is Tact-owned.
- The rail is marked `TactSessionRail`; the manager is
  `TactSessionManager`.
- `ThreadList` is not claimed as the renderer for those surfaces.

### F3. CommandSearch is flat

Evidence:

- `CommandSearchItem` supports title, subtitle, keywords, shortcut and disabled
  state.
- It indexes one section (`index.section == 0`); there is no section/group
  header API.

Impact: the prototype's grouped command palette could not be a direct
`CommandSearch`.

Resolution:

- The palette is now a flat relevance-ranked list.
- Group names travel in the item subtitle, which maps directly to
  `CommandSearchItem::subtitle` and remains searchable through keywords.

### F4. Session title and archive metadata are absent from the current schema

Evidence:

- `SessionSummary` contains ID, root directory, timestamps and message count.
- `sessions` has no `title` or `archived_at` column.

Impact: the PRD/prototype show named and archived sessions, but the current
backend cannot persist either.

Resolution:

- The PRD now requires a session metadata migration before archive/title parity
  ships.
- The prototype keeps the interaction because it is an approved product
  requirement, not a direct backend capability.

### F5. Queue, attachments and voice were claimed but not represented

Evidence:

- The component map named `MessageQueue`, `AttachmentStrip` and
  `VoiceControls`.
- The prototype only had a static queue line, a toast-only attachment button,
  and no composer voice control.

Resolution:

- Queue rows now expose send-now, remove and clear actions.
- Attachments render as removable chips.
- A voice control toggles recording state and inserts the transcript on stop.

## Components That Reproduce Directly

- `StreamingText`
- `Thinking`
- `ToolCall` and `ToolGroup`
- `ApprovalCard`
- `PlanCard`
- `CodeBlock` and `CodeDiff`
- `QuestionFlow`, `ChoiceGroup` and `Toggle`
- `ContextMeter`
- `MessageQueue`
- `AttachmentStrip`
- `Suggestions`
- `SelectionActions`
- `VoiceControls` (desktop-finish scope)

## Components That Require Tact Composition

- Mixed virtualized transcript: custom.
- Project-scoped fuzzy Session Manager: custom.
- Nested slash command tree and result routing: custom overlay around
  `PromptBar`.
- Session rows, extension rows and stats summaries: `RecordsTable`,
  `FilterTable`, `SidebarNav` and `InsightCard` inside Tact-owned shells.
- Goal panel and subagent transcript: custom.
- Extensions Manager and command result sheets: `gpui-component::Dialog` plus
  Tact-owned rows/readers.
- Mermaid, Task DAG and ANSI output: Tact adapters.

## Verification Evidence

- Fetched the live catalog and repository:
  `curl -L https://labcoder.github.io/gpui-ai/components`,
  `git clone --depth 1 https://github.com/labcoder/gpui-ai`.
- Confirmed flat `PromptCommand` and one-token substring filtering in
  `crates/gpui-ai/src/prompt_bar.rs` and
  `crates/gpui-ai/src/prompt_bar/suggestions.rs`.
- Confirmed `ThreadList` substring filtering and no project/filter API in
  `crates/gpui-ai/src/thread_list.rs`.
- Confirmed flat `CommandSearchItem` indexing in
  `crates/gpui-ai/src/command_search.rs`.
- Ran the prototype in Chromium at 1440x900 and 390x844; exercised the
  command-palette entries and every declared slash-command path (including
  parent groups), queue
  send/remove/clear, attachment add/remove, voice toggle, follow-up chips and
  selection actions with no page errors.
- Re-ran `node --check docs/design/tact-desktop/prototype.js` and
  `python3 scripts/book-check.py book`.

## Vendored patches (Tact)

The pinned `gpui-ai` fork carries the Tact patches listed below. They are part
of the shipping surface: re-apply them when the dependency is upgraded.

### P1. `PromptBar::set_leading_controls`

The component's footer rendered the model, attach and submit controls only.
Tact needs the mention, slash and permission controls in that same row so the
composer is one bar instead of three stacked rows. `set_leading_controls`
installs a per-frame builder whose elements are rendered at the start of the
footer cluster. The builder reads live application state, so the permission
chip updates without rebuilding the bar.

### P2. `SidebarNavItem::detail`

The pinned row rendered one label and an optional badge. The rail needs a
second line ("2m ago · 46 messages") to match the prototype and to keep the
timestamp and message count readable at once. `detail` adds that line; rows
with a detail line render through a Tact-side two-line layout while rows
without one keep the pinned `SidebarMenuItem` presentation.

### P3. `CommandSearchItem::icon`

The pinned item supported title, subtitle, keywords and shortcut, but no
leading icon. The prototype's palette rows lead with a group icon. `icon`
takes the bundled icon path; the row renders it at 16px before the title and
keeps the pinned Command option/activation path unchanged.

### P4. `SidebarNav::set_chrome_visible`

The pinned navigation always drew its own new-task button and filter input.
The design's rail owns a larger project card, a blue New run button, a search
field with a `/` hint, and date-grouped sections, so the host needs the row
tree without the vendor header. `set_chrome_visible(false)` hides only the
header controls; rows, keyboard navigation, filtering and events are unchanged.

### P5. `PromptBar` footer visibility and trailing controls

The prototype's composer footer is one row: paperclip, `@`, `/`, model,
permission, then status and submit. The pinned bar always rendered attach and
enhance text controls and had no slot after the model cluster.
`set_attach_visible(false)` and `set_enhance_visible(false)` remove the
built-in text controls, and `set_trailing_controls` adds the permission chip
after the model. The attach action is re-created as an icon control by the
host, so the event path is unchanged.

### P6. `PromptBar` footer status and glyph submit

The footer had no right-edge status and the glyph submit was disabled whenever
the draft was empty, so it could not carry the design's solid send surface.
`set_footer_status` adds the short status text before the submit control and
`set_progress_visible(false)` hides the separate progress line. While running
the bar now renders a red stop control **beside** the send control (matching
the design), and the glyph submit stays enabled so an empty draft still shows
the blue send button. Stop emits `CancelRequested`; send always submits (the
shell queues a prompt while a run is active).

### P7. `SidebarNav` row padding

The pinned rows were 55px tall (two lines plus 8px vertical padding). The
design's row pitch is 53px. Padding drops to 7px; the two-line layout and the
1px separator still fit.

### P8. `SidebarNav` trailing slot

The rail's session rows need a right-aligned time on the title line while the
detail line keeps `2m ago · 46 messages`. `SidebarNavItem::trailing` adds that
label and the detail renderer lays it out opposite the label; the active check
it previously drew is gone (the active dot already marks the current row).

### P9. `glide.rs` — no animation frame for a merely-hovered list

`Glide`'s capture hook requested an animation frame on every painted frame
while a row was hovered, which pinned the whole window to the display refresh
rate for as long as the pointer rested on a list. The hook now requests
nothing: `transition` already asks for frames while its clock runs, and the
remaining captures ask when the layer or a painted row actually moves.

### P10. `todo_list.rs` — screen-reader label

The completion label read `"{list_name}: {done}/{total} {completion_label}"`,
which a screen reader announces as a single run-on token. It is now
`"{list_name}, {done} of {total} {completion_label}"`, so the counts are
spoken as words.

### P11. `SidebarNav::with_compact_rows`

The rail is chrome, not component content: the design draws session rows at
12px over 10px with a 10px monospace trailing time and 10px monospace section
labels, while the pinned two-line row renders 14px over 12px. `with_compact_rows(true)`
switches only type and padding (5px vertical) for the rail; selection, the
keyboard model, filtering and events are unchanged. The default stays the
component's own rhythm for every other host.

### P12. `SelectionActions::frameless`

The pinned surface always drew a bordered, padded, minimum-height panel around
selectable Markdown. The design draws an assistant answer as plain prose on the
transcript canvas, with only the floating selection toolbar as a surface.
`frameless()` drops the frame, background and inner padding while keeping
selection, actions and the toolbar. The default keeps the framed panel.

### P13. `PromptSubmit::Plane`

The glyph submit was a round button with an arrow. The design's send control is
a rounded square with Lucide's paper plane. `PromptSubmit::Plane` adds that
shape; the host that opts in serves `icons/send.svg`, which the pinned
`gpui-kit-assets` bundle does not carry. The stop control beside it now uses
the design's 7px corner instead of a circle.

### P14. `ToolCall::grouped` and `ToolGroup::compact`

The design makes the tool group the card: one border, a 37px header and the
calls as hairline-separated rows at 10px type with no status pill. The pinned
`ToolCall` always drew its own card with a `StatusBadge`. `ToolCall::grouped()`
removes the row's frame and badge and uses the dense header metrics;
`ToolGroup::compact()` uses the same type scale, a left-aligned header and a
hairline above the rows. The desktop wraps the group in the card. Defaults are
unchanged for other hosts.

### P15. The inline permission card is Tact-owned

The prototype's `ApprovalCard` gate is a 38px signal column, a mono kicker, the
prompt, an optional command block, and three actions with `↵` / `esc` hints.
The pinned `ApprovalCard` renders a title/description/footer with exactly
three fixed actions and no signal or command slot. The runtime can also offer
four permission choices (allow once, deny, always for this tool, always for
this pattern), which the three-button component cannot express. The desktop
therefore keeps `ApprovalCard`'s semantics (one click decides) but owns the
visual shell in `gate_element`, the same way Tact owns the nested slash overlay
around `PromptBar`. `QuestionFlow` and `Toggle` remain the mounted components
for `ask_user` single- and multi-select, including their Confirm rules.

### P16. `ChoiceGroup::cards` and the `QuestionFlow` card header

The prototype's `ask_user` cards put options side by side as bordered cards
under a mono kicker, title and description. The pinned `ChoiceGroup` drew a
vertical radio list, and `QuestionFlow` drew its title and question prompt in
two places. `ChoiceGroup::cards(true)` wraps each option in a bordered card in
a wrapping row (selection, keyboard model and events unchanged);
`QuestionFlow::kicker`, `card_options` and `no_selection_label` add the
decision-card header and replace the "1 of 1" counter with the pending/selected
state. Defaults keep the previous flow layout for other hosts.

### P17. `CommandSearch` row grammar and caption

`CommandSearch::set_caption` wraps the items in a labelled `CommandGroup`, so
the "RECENT COMMANDS · N OF M" heading renders between the search field and
the rows without forking the upstream `Command`.

The prototype draws a 28px rounded icon tile and a plain monospace shortcut at
the row's trailing edge; the pinned component drew a bare glyph and a boxed
keycap. `row_content` now wraps the icon in the tile and drops the keycap
border/background. Search, filtering, selection and accesskit labels are
unchanged.

### P18. `SelectionActions::text_style`

The transcript's answer is prose on the canvas, so Tact needs the design's
Markdown scale (h1 21px, h2 18px, h3 16px, 9px paragraph gap, dark code
highlighting) without forking the Markdown renderer. `text_style` forwards a
`TextViewStyle` to the pinned `TextView`; the default keeps the component's
own typography.

### P19. Active rail row fill

`SidebarNav` marked the chosen expanded row with a dot only. The design fills
the whole row; the render now applies `sidebar_accent` to an active expanded
row as well as the collapsed icon row. Selection, hover and keyboard behavior
are unchanged.

### P20. `Thinking` toggle alignment

The Thinking toggle was a full-width button whose content centred, so a
thought rendered in the middle of the transcript column. `.justify_start()` on
the toggle keeps the disclosure chevron and title at the leading edge, matching
the design's thought row. The trace, disclosure animation and reader event are
unchanged.

### P2 surfaces (no further vendor patches)

The P2 components ship unmodified; the shell only chooses when to mount them.

- **ANSI output.** `ui/ansi.rs` parses SGR (16/256/truecolour, bold, dim,
  italic, underline), drops non-SGR CSI/OSC sequences, and renders monospace
  spans. The tool reader uses it whenever the output carries an escape.
- **Mermaid.** `ui/mermaid.rs` parses `graph`/`flowchart` blocks
  (`TD`/`TB`/`LR`/`RL`, `A[Label]`, `A(Label)`, `A{Label}`, `A((Label))`, edge
  labels), lays nodes out by longest-path layer, and removes the source fence
  from the answer so the diagram is the answer's only representation.
- **SearchResults.** `ui/web_search.rs` reads JSON (`title`/`url`/`link`),
  Markdown links, or bare URLs; the tool reader mounts `SearchResults` for
  search tools and opens the selected URL.
- **ArtifactPanel.** A write/edit tool's code body opens in `ArtifactPanel`
  with Preview/Source switching; no vendor change was needed.
- **DiffTable.** A patch with more than one file opens a per-file table with
  added/removed counts and accept/reject decisions. The reader owns the
  decision epoch and rebuilds rows when a decision lands.
- **ContextCard.** The Context inspector lists the newest hook contexts and
  tool outputs still in the run; selecting a card opens its reader.
- **Dock badge.** macOS sets the Dock tile label from the pending-gate count
  through `NSDockTile`; zero clears it. Windows has no GPUI `HWND` hook, so
  the count stays in the gate banner and notification there.

### Vendor test note

These host patches deliberately express the prototype's metrics as raw pixels
(the 37px grouped tool header, the 29px gate controls, the 28px command icon
tile). `gpui-ai`'s own `pixel_discipline` integration audit is not part of the
desktop gate and would ask for an exception for each such metric; the vendor
library's 307 unit tests pass. The desktop workspace gate (`scripts/check-rust.sh`)
is the release gate for this repository.
