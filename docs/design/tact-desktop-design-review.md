# Tact Desktop Client — Design Review

Date: 2026-09-19
Last updated: 2026-09-21
Spec: `docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md`
Subject: `docs/design/tact-desktop-prototype.html` and its reference renderings

This is the Phase 2 review. It runs the GPUI Kit Design Guides checklists against
the prototype, records what was fixed, and parks what cannot be settled before
`tact-gui` exists.

## Method

- Checklists: the "Design review checklist" and "Accessibility checklist" from
  the GPUI Kit Design Guides.
- Evidence: headless Chromium renders at 1440×900, 1100×800, and 860×700;
  computed-style measurements taken from the live page; a WCAG contrast script
  run over the resolved token pairs.
- Renders are produced with `--force-prefers-reduced-motion` and
  `--hide-scrollbars`. The first makes captures deterministic and exercises the
  reduced-motion path; the second avoids a known headless race where a late
  scrollbar changes layout between capture runs.

## Design review checklist

| # | Question | Verdict | Evidence |
|---|---|---|---|
| 1 | Is the task clear? | Pass | The window title names the session, the transcript carries the work, and the composer is the single primary action. A new reader can identify purpose, primary action, and next step without explanation. |
| 2 | Does every action keep its promise? | Pass | Send becomes Stop while running and returns to Send on cancel. Permission choices write their result into the card and mark it resolved. Work-pane tabs switch the pane they name. |
| 3 | Is hierarchy decisive and restrained? | Pass | One accent hue, one primary button. Session state uses a dot plus a label; only the active session row and the current plan step take the accent surface. |
| 4 | Could the interface do less, better? | Pass, with scope note | Four work panes, one composer, one status bar. Browser and Terminal were dropped from v1 rather than added as panes. Deferred items are listed in the spec's approval points. |
| 5 | Is the structure exact? | Pass after fix | Alignment spines are consistent. The diff pane previously rendered its code column and marker column with inherited box styling from unrelated classes (see Finding 1); both are now correct. |
| 6 | Does it follow the component system? | Partial — expected | The prototype is HTML, so it cannot exercise gpui-kit component geometry. It does mirror the intended mapping (Tabs, Sidebar, Resizable, MessageScroller, DataTable, Command, Dialog, Sheet). Real verification lands in Phase 4–6. |
| 7 | Does it remain usable in every state and constraint? | Pass after fixes | Verified at 1440×900, 1100×800, and 860×700; empty/running/blocked/failed states are present; reduced motion no longer strands the palette (Finding 2); focus is visible (Finding 3) and restored (Finding 4). |
| 8 | Has it been tested in a real window? | Partial — expected | Interaction is verified in a real browser, not in a real GPUI window. This is the known and intended limit of a prototype. |

## Accessibility checklist

| Item | Verdict | Evidence |
|---|---|---|
| Every action reachable and operable by keyboard | Pass | Top tabs, work-pane tabs, theme toggle, sidebar toggle, detail cycle, and composer are native `button`/`input` elements. ⌘K, ⌘B, ⌘\, Ctrl+O, and Escape are bound. |
| Focus order follows visual and task order | Pass | DOM order matches reading order: title bar, sidebar, transcript, composer, work pane. |
| Focus visible and restored after overlays | Pass after fixes | Added a `:focus-visible` ring (2 px accent, 2 px offset). Opening the palette stores the trigger and closing restores focus to it. Tab now cycles inside the palette. |
| Controls have names; icon-only controls have tooltips | Partial — parked | The palette and work-pane close button carry `aria-label`s. The prototype's title-bar icons do not yet have tooltips; the spec already requires them, and they are tracked as a Phase 4 item. |
| Text and meaningful boundaries have sufficient contrast | Pass after fix | 40 of 40 measured text/background pairs now meet 4.5:1. See the contrast table. |
| Status is not communicated by color alone | Pass | Session state pairs a dot with text (`Running`, `Review`, `Archived`, `Clean`). Diff rows carry `+`/`−` markers, not only tint. Task states are labelled (`In progress`, `Done`, `Blocked`). |
| Disabled and read-only states distinguishable | Partial — planned | Send is disabled when the composer is empty and while running. Disabled styling exists but was not exercised across every control; the component layer must own it. |
| Labels, errors, descriptions near their controls | Pass | Permission choices sit inside the approval card; the permission selector sits in the composer. |
| Usable with longer translations and larger text | Partial — parked | Layout is `rem`-free and px-based in the prototype. The gpui-kit `zoom`/base-font system is the right place to prove this, so it is parked to Phase 4. |
| Pointer targets comfortably sized in a dense layout | Partial — parked | Session rows are ≥44 px. Icon-only title-bar buttons are 28×28. Either raise them to 32 px or keep the 28 px visual with a larger hit area; decide in Phase 5. |

## Contrast evidence

Measured with the WCAG 2.1 relative-luminance formula against the resolved
tokens. "Tint" means the semantic colour composited over `surface` at the
token's alpha.

| Pair | Light | Dark |
|---|---|---|
| `ink` on `surface` | 18.14 | 15.10 |
| `ink2` on `surface` | 6.48 | 7.15 |
| `ink3` on `surface` | 5.02 | 4.59 |
| `ink3` on `canvas` | 4.51 | 5.02 |
| `ink3` on `surface2` | 4.68 | 4.86 |
| `accentInk` on `accentTint` | 4.60 | 5.21 |
| `green` on `greenTint` | 4.65 | 4.99 |
| `red` on `redTint` | 4.64 | 4.61 |
| `blue` on `blueTint` | 4.62 | 4.61 |

Before this pass, eight pairs failed: `ink3` on `canvas` (4.32) and on
`surface2` (4.48) in light, `ink3` on `surface` (4.14) and `surface2` (4.38) in
dark, plus light-theme `green` (3.62), `blue` (2.88), `orange` (4.00), and
`accentInk` on its tint (4.10), and dark-theme `red`/`blue` on their tints
(4.21 / 4.37).

Corrected tokens:

| Token | Light before → after | Dark before → after |
|---|---|---|
| `ink3` | `#747168` → `#716e65` | `#858278` → `#8c8a80` |
| `accentInk` | `#b8563a` → `#ab5036` | `#e58d70` → `#e58e71` |
| `green` | `#788c5d` → `#60704a` | `#8fa875` → `#90a876` |
| `red` | `#c64b4f` → `#b54448` | `#e06c72` → `#e3787e` |
| `blue` | `#6a9bcc` → `#4b6e91` | `#6a9bcc` → `#6d9dcd` |
| semantic tint alpha | 0.12–0.14 → 0.10 | 0.14–0.16 → 0.12 |

Both the prototype and `docs/design/tact-desktop-theme.json` carry the corrected
values, so the renderings and the loadable theme agree.

## Findings fixed in this pass

1. **Diff pane class collisions.** The diff code column reused `.code` (the
   transcript code-block wrapper) and the diff marker column reused `.mark` (the
   app-logo swatch), so both inherited unrelated border, background, and radius
   styling and the diff rendered as stacked dark pills. Renamed to `.dcode` and
   `.dmark`.
2. **Reduced motion hid the command palette.** With `prefers-reduced-motion`,
   `animation-duration` was clamped to 1 ms while `@keyframes pop` starts at
   `opacity: 0`, leaving the palette computed at `opacity: 0` — an invisible
   command palette for anyone who asks for reduced motion. Animated regions now
   set `animation: none` under reduced motion; measured `opacity: 1`,
   `transform: none` after the fix.
3. **No keyboard focus styling.** The prototype had no `:focus-visible` rule, so
   it could not demonstrate the focus behaviour the spec requires. Added an
   accent ring system with a tighter offset inside the palette and work pane.
4. **Palette focus handling.** Focus was not returned to the trigger, Tab could
   escape the overlay, and the palette had no dialog semantics. Added trigger
   restoration, a Tab trap, `role="dialog"`, `aria-modal`, and labels.
5. **Contrast.** Eight pairs were below 4.5:1. Tokens were re-solved until all
   measured pairs pass.

## Deviations from the design guides

Each is deliberate and has a reason.

- **Raw hex and `rgba()` inside the prototype.** The guides allow raw colour in
  theme definitions, and that is exactly where these live: the `:root` and
  `[data-theme=dark]` blocks are the prototype's theme layer. Application code
  in `tact-gui` must resolve these through `cx.theme()` instead.
- **A serif for assistant prose.** The guides call for the platform UI font for
  interface text and monospace for code. Assistant answers are document content
  rather than interface chrome, so Lora carries them; every control, label, and
  metadata value uses the platform UI font.
- **Window traffic lights.** The three title-bar dots are macOS window chrome,
  not product tokens. They are drawn as decoration in the mock and must come
  from the platform in `tact-gui`.
- **Density varies by region.** The sidebar list uses 44 px rows while tables
  and toolbars use compact 28–32 px controls. The guides ask for density to be
  set per local context, not per control; each region is internally consistent.

## Parked items

These were parked during the prototype review. The Phase 4–7 follow-up below
records which items the production shell has since resolved.

| Item | Why parked | Where it lands |
|---|---|---|
| Icon-only targets at 28 px | Needs a decision between visual size and hit area | Phase 5 composer/toolbar work |
| Tooltips on title-bar icons | Spec already requires them; not needed to validate layout | Phase 4 shell |
| Zoom and larger-text behaviour | Must be proven with gpui-kit's base-font and `rem` system, not in HTML | Phase 4 shell |
| Disabled/read-only coverage | Belongs to the component layer | Phase 5–7 |
| Virtualized transcript and lists | Static prototype cannot exercise it | Phase 5 transcript |
| Diff horizontal scrolling in static renders | `--hide-scrollbars` is needed for deterministic headless captures; live behaviour was verified interactively | Note for future render scripts |
| `--orange` is a dead token in the prototype | Unused; `base.yellow` in the theme JSON remains the amber palette entry | Cleanup during token import |
| Spacing scale ownership | gpui-kit does not persist a custom `SpacingTokens` scale, so the app must own that snapshot | Phase 4 shell |
| Work pane's five ghost buttons | `Open in editor`, `Refresh plan`, `Comment`, `New task` and `Add file` are drawn as buttons (`crates/tact-gui/src/pane.rs`) but originally carried no handler, no palette row and no chord. `Open in editor` and `Comment` now route through real file/review actions; the remaining controls answer with their own reason instead of swallowing a press | Resolved in Phase 4–7: live actions and explicit v1 limits are called out in the follow-up below |

## Verification performed

- Rendered 1440×900 in light and dark, plus the Diff pane and the command
  palette in both themes; six reference images, all distinct.
- Rendered 1100×800 and 860×700 to confirm the work-pane drawer and sidebar
  overlay.
- Parsed the prototype script with `node --check` after every edit.
- Measured computed `opacity`/`transform`/`animation` on the palette before and
  after the reduced-motion fix.
- Ran a WCAG contrast sweep over 40 resolved text/background pairs in both
  themes; zero remain below 4.5:1.
- Confirmed repeat renders are byte-identical under the documented flag set.

## Not verified by the prototype

- No GPUI window was built or run during Phase 2. Component geometry, keyboard
  traversal inside real widgets, and theme-registry loading were verified later
  in Phase 4–7.
- No localization or long-string layout test was performed in the HTML
  prototype; the production shell uses rem-based dimensions and theme tokens.

## Phase 4–7 implementation follow-up

The production shell now verifies the items that Phase 2 could only park:

- Title-bar icon controls have tooltips and accessible names.
- The shell uses rem-based dimensions and theme tokens; the 960×640 minimum
  window and the <1280 px work-pane drawer / <960 px sidebar overlay are covered
  by headless UI tests.
- `MessageScroller` and the work-pane lists virtualize long transcripts and
  diffs.
- An answered approval card is a transcript row, not a card beside the list.
  The prototype leaves `.approval.done` in place after it is answered, and the
  shell keeps that card — actions swapped for the decision — but files it into
  the row order. A card that lives outside the list can only ever be the last
  thing on screen: the turn it unblocked keeps appending rows *above* it, so the
  newest content pushed upward under a stale approval. Only a *pending* request
  still sits past the rows, which is where the answer has to be given;
  `answer()` in `crates/tact-gui/src/shell.rs` appends the answered one through
  the ordinary row path. The terminal client reaches the same place from the
  other side — its select popup closes on the answer and the decision lands on
  the tool's meta row (`book/10_chapter_permission.md` §6).
- Disabled/read-only behavior is delegated to the gpui-kit components rather
  than restyled locally.
- The command palette, settings dialog, Normal → Thinking → Verbose transcript
  cycle, light/dark controls, and window-level keyboard contract are exercised
  by `crates/tact-gui/tests/shell.rs`.
- The sidebar lists workspace-local sessions by stored or derived title with
  project/branch/age metadata and a status badge; the short id is the fallback.
  Selecting a row or pressing `Ctrl+Tab` resumes that session through the shared
  `tact-session` runtime.
- The work pane's action buttons answer a press instead of swallowing it.
  `Open in editor` now acts on the selected Files row, `Comment` drafts one
  batch review from every recorded diff, Diff cards expose `Stage`, and Files
  rows open a bounded preview with `Reveal` and `Mention` actions. `Refresh
  plan`, `New task`, and `Add file` remain explicitly unavailable because they
  have no v1 protocol/store behavior to wire; they keep the prototype's place
  and answer with distinct limits. `the_pane_actions_each_answer_a_press` in
  `crates/tact-gui/src/shell.rs` presses the shared buttons and pins the
  distinct outcomes, so the broad walk's liveness-only gap stays closed.
- Plan step rows now act like the prototype they came from: clicking a row
  expands its input/result block, terminal failures carry the danger state and
  expose Retry, and Open transcript expands and scrolls to the tool card that
  owns the step. Retry goes through `SubmitTask` with the recorded tool and
  arguments instead of invoking a tool behind the driver's back. The focused
  tests are `failed_plan_step_expands_to_retry_and_transcript_controls`,
  `opening_a_plan_step_transcript_expands_the_tool_card`, and
  `retrying_a_failed_plan_step_submits_the_recorded_tool_and_args`.
- Tasks rows now expose their named actions without adding columns the prototype
  does not draw. Filter and Sort are local `TasksPane` display preferences; the
  status badge advances Pending → InProgress → Completed → Pending and sends
  `TaskUpdate` through the driver so the task store remains authoritative; an
  owner cell with a session id resumes that session through the sidebar's
  `resume_session` path. `task_filter_and_sort_keep_the_expected_rows` and
  `task_filter_and_sort_buttons_change_their_labels` cover filtering and
  sorting; `updating_a_task_sends_the_status_transition` and
  `opening_a_task_session_selects_its_session_row` cover the protocol and
  resume paths.
- Subagent rows now expose their named actions. Running children carry Cancel,
  which sends `CancelSubagent { child_id }` through `SessionHandle`; every row
  carries Inspect transcript, which loads `tact_session::history::history` on
  the background executor and renders the stored child transcript beneath the
  list. `cancelling_a_subagent_sends_its_child_id_to_the_driver` and
  `inspecting_a_subagent_loads_its_stored_transcript` pin the command and the
  loaded state.
- Every control the shell renders is pressed by a test, including the title
  bar's three preset tabs, which
  `the_workspace_tabs_pair_each_preset_with_its_pane` presses in turn and reads
  back through the pane each one opens (`Chat` -> plan, `Agent` -> tasks,
  `Code` -> diff). The exceptions are controls with no handler to reach: the
  sidebar avatar and the background-work row (both plain display rows, as in
  the prototype), the status bar's read-only chips, which are asserted by value
  rather than pressed, and the native file dialog the composer's attachment
  path opens.
- The broad walk proves presence and survival, not effect, and should be read
  that way. `every_entry_point_answers_a_click`'s `click!` macro asserts only
  that the id rendered and that the press did not panic, so a control that
  renders and does nothing still passes it -- which is exactly what the work
  pane's five ghost buttons did until the follow-up below gave each one a
  handler. The effect assertions inside that same test cover session rows, the
  new-session control, worktree rows, the session-menu rows, popover
  open/close, palette open/escape and the settings switches; every other press
  in it is liveness only. Adding a control to the walk does not, by itself,
  give it a contract.
- Tertiary ink has a role, and the controls that a component draws were the
  ones still missing it. `theme::ink3` exists because the shipped theme maps
  `muted.foreground` to `--ink2`, and the earlier pass moved ~30 call sites
  onto it -- but only the ones the shell paints itself. The two controls a
  component paints kept their variant colour: the attachment chip's delete
  affordance is `.chip button{color:var(--ink3)}` yet a ghost `Button` renders
  `secondary_foreground` (`--ink`), two tiers too strong, and the file tree's
  expand chevron is `.tree .row2{color:var(--ink2)}` yet the same ghost variant
  out-shouted the row's own label. Both now pass `.text_color(..)` at the call
  site, which is the idiom `prototype_icon_button` already used. The lesson is
  the general one: a component's variant chooses a colour, so any control the
  prototype types below `--ink` has to be told.
- Residual: the composer's placeholder still paints one tier too strong. The
  prototype asks `.prompt::placeholder{color:var(--ink3)}`, but the `Input`
  component reads `InputEditorStyle.muted_foreground` -- which is `--ink2` --
  and exposes no per-instance placeholder colour. Repointing `muted.foreground`
  is not available, because the theme-role test pins it to `--ink2` and the
  surfaces the prototype really types as `--ink2` would move with it. Closing
  this means drawing the placeholder ourselves rather than letting the
  component do it, which is not worth the complexity at a one-tier difference
  without a decision to that effect.
- Residual: the two `.text_color(..)` overrides above pin the resting state
  only. `Ghost` re-applies `secondary_foreground` from its `hover` and `active`
  style closures, which the component registers before the caller's
  `refine_style` and which therefore win while the pointer is over the control,
  so a hovered chip `x` and a hovered tree chevron step back up to `--ink`. The
  prototype raises neither colour on hover (`.chip button:hover` changes only
  the fill; `.tree .row2:hover` goes to `--accentInk`). Closing that needs a
  variant that takes a colour, which is upstream work.
- The command surface is inventoried rather than sampled.
  `crates/tact-gui/src/commands.rs` binds 17 actions under the `TactApp` key
  context: fourteen carry a chord, and three (`CompactSession`, `SessionStats`,
  `McpServers`) are reachable only from the palette. Every one of the seventeen
  is reached by at least one test and no registered action is dead. A palette
  row dispatches the row's own GPUI action, so the palette-only three land on the
  same table the chords do (`run_palette_command`).
- The controls that need a running turn are covered by an injected session
  rather than a live agent. `session::SessionHandle::new` lets a test own the
  command channel, so `the_stop_control_cancels_the_running_turn` presses the
  composer's primary control with a turn in flight and reads
  `UserCommand::Cancel` off the receiver, while
  `the_send_control_hands_the_draft_to_the_session` proves the idle press hands
  `SubmitTask` to the session instead of only painting a row — an offline shell
  appends that row either way, which is why the transcript assertion alone could
  not tell a queued command from a painted one.
  `the_palette_only_commands_map_onto_their_protocol_commands` pins
  `CompactSession -> UserCommand::Compact`, `SessionStats -> QueryStats`, and
  `McpServers -> McpList` the same way.
  `escape` is pinned to the same fixture: `escape_stops_at_the_layer_that_owns_it`
  proves an open palette and an open dialog each consume the press (the receiver
  stays empty) and that bare `escape` is then the turn's own stop, so the
  silences are not a dead binding.
- `NewSession` and both session cycles do reach a real `tact-session` runtime,
  and that is now covered rather than read. The connected branch is where the
  store is touched, so it cannot run against an offline shell: the two contracts
  live in `crates/tact-gui/src/shell.rs`'s own `mod tests`, where the private
  `offline` flag is reachable.
  `the_new_session_control_starts_a_runtime_and_writes_its_row` presses the
  composer's `session-new` control and then reads the row back out of the
  workspace's database through `session::recent`, which is what makes the press
  more than a painted sidebar row;
  `cycling_sessions_resumes_the_runtime_the_row_names` starts two sessions the
  same way and asserts `Primary+Tab` adopts the *other* row's id instead of only
  re-highlighting it. The fixture needs no network, no API key and no provider:
  `SessionRuntime::start` finishes its local half -- store open, row insert --
  before it spawns the agent thread, so
  `tact_session::test_support::install_test_config` installs a deliberately
  keyless provider and the agent's failure arrives as `AgentUpdate::Error` on
  the stream instead of derailing the press. Both
  contracts were mutation-checked: making `new_session` return before
  `session::start` fails them with `two presses leave two stored sessions`
  (`left: 0, right: 2`), so they catch the regression they describe.
- A resumed session redraws the transcript the store still holds, and the
  redraw is now a contract rather than a reading order. Switching away and back
  used to hand the window a blank page: `adopt` resets the `Conversation`, and
  nothing rebuilt it, so the conversation vanished until the next turn even
  though `messages.content` had held the canonical blocks all along. The store
  was never reachable from this crate -- `tact-gui` depends on `tact-session`
  and `tact_protocol`, never on `tact` or `tact_llm` -- so the seam is a new
  `tact_session::history::history(workdir, session_id)` that flattens the stored
  rows into presentation-neutral `HistoryBlock`s, which
  `Conversation::load_history` maps onto the shell's own rows.
  `tact_session::test_support::seed_session_history` puts a real transcript into
  a workspace's database so
  `switching_sessions_redraws_the_stored_transcript` can drive the press
  through `resume_session` into the store and read the rows back; dropping the
  `replay_history` call fails it with `the stored turns plus the resume notice:
  [System { .. }]` (`left: 1, right: 5`), which is exactly the blank page the
  report described. Fidelity is bounded on purpose: durations, progress lines,
  request cards and the producer's `arg_summary` are presentation state that was
  never persisted, so a redrawn card has no duration, falls back to the generic
  `ToolVisualKind`, derives its detail line from well-known input keys, and
  stays `Running` when no result was stored -- which is what the live transcript
  showed for a turn that ended mid-tool. The one visible consequence for the
  prototype's `.msgMeta` is that a redrawn row prints no clock: `load_session`
  does not return each row's `created_at`, so the row carries `NO_TIMESTAMP`
  rather than claiming the moment the session was reopened.
- The transcript orders itself by arrival, and a tool or thinking card is a
  boundary in that order. The prototype's static markup shows `think -> tool ->
  content` but cannot show what happens when prose arrives *after* a card:
  `Conversation` used one `open_assistant` index for the whole turn, so later
  prose was appended to the row that opened before the tool and the answer
  appeared above the card it followed. The shell now seals the open assistant
  row when a card takes its slot (`seal_open_assistant`), which is the rule the
  terminal client already had -- it flushes pending prose before allocating a
  tool placeholder, appends later prose at the tail, and lets the card mutate
  the row it reserved at first appearance. This is deliberately narrower than
  "any row seals the stream": a prompt queued mid-turn still does not seal, a
  rule `a_queued_user_row_does_not_seal_the_open_stream` already pinned. Both
  directions are contract-tested (`prose_after_a_tool_opens_a_new_row_below_the_card`,
  `prose_after_a_thought_opens_a_new_row_below_it`) and mutation-checked:
  removing the seal merges the two halves into one `beforeafter` row above the
  tool card.
- The shell draws the prototype's own control boxes instead of the component
  defaults: `.mini` is 25 px, `.toolbtn` is 26 px, `.btn` is 28 px, `.send` is
  28x28, and `.ring` is 24x24. Each is pinned by
  `the_composer_controls_use_the_prototype_boxes`,
  `the_title_bar_controls_use_the_prototype_boxes`, or
  `the_work_pane_chrome_uses_the_prototype_boxes`. The ring needs
  `.with_size(px(24.)).px(px(0.))` rather than `.compact()`: a component button
  whose content is a child keeps `size * 0.2` padding and measured 34x32, which
  grew `.bar` past the prototype's height.
- The status bar is pinned by value, not by existence:
  `the_status_bar_renders_its_segmented_chips` reads every chip's accessible
  label (`status-project`, `status-branch`, `status-permission`, `status-diff`,
  `status-turns`, `status-context`, `status-balance`, `status-running`) and
  compares it against the preview's seeded session — `Ask permission`,
  `+676 −152`, `42% context`, `$18.42`, `1 running`. The turn counter is
  asserted absent, because it stays hidden until the agent reports `TurnStats`.
- Two prototype affordances have no v1 counterpart. One is the chevron
  `<button class="icon">` in `.sideFoot`: the shell's sidebar footer shows the
  workspace and its live activity where the prototype shows `rg@local` and
  `Balance - $18.42`, leaving the account to the status bar's trailing chips, so
  there is no menu behind that chevron to open. The `.avatar` beside it is a
  plain label in both. The other is the footer's `checks passing` chip: the
  protocol reports no check status, so the bar drops that segment rather than
  render a chip that can never change.
- The 2026-09-19 GUI smoke run under Hyprland confirms the app launches in the
  live workspace, loads the Tact light theme without asset/theme-registry
  errors, renders the session surface, and reports a successful resume.
- The prototype's `.scrim` is implemented with its own press-ownership
  contract. GPUI has no `z-index`, so the prototype's stack — `.scrim{z-index:25}`,
  `.work{z-index:30}`, `.sidebar{z-index:40}` — becomes render order plus claims:
  the scrim mounts only while the pane is a drawer, spans the workspace, swallows
  the press it covers (`stop_propagation` on mouse-down) and closes the drawer
  from its click, while the drawer and the floating sidebar claim their own
  presses on the way out. Without those claims a press aimed at a floating
  sidebar row fell through to the scrim and closed the work pane behind the
  user's back. `the_work_pane_drawer_swallows_presses_behind_it`,
  `the_sidebar_overlay_keeps_its_presses_above_the_scrim` and
  `the_floating_sidebar_stays_above_the_work_pane_where_they_overlap` pin the
  split; the last one is why `TactApp::render` paints the floating sidebar after
  the drawer, matching `.sidebar{z-index:40}` over `.work{z-index:30}`.
- The floating sidebar marks its open row through the same `current` the column
  gets (`open_session_id()`). Handing it only `self.session` left an offline
  shell (`--preview`) with no active row below 960 px, so pressing a session
  there changed nothing on screen.
- The transcript's fenced-code card takes a per-block element id from the
  anchor the markdown renderer already stamps on a custom block — its byte
  range in the message (`node.source_range().start`) — so the `Copy` chip is
  `code-block-copy-<anchor>`, unique among a message's blocks and stable across
  re-renders without an index the renderer does not have. It also carries
  `aria_label("Copy code")` for assistive technology, which the bare `div` had
  none of. `the_code_block_copy_chip_writes_its_fence_to_the_clipboard` presses
  it and reads the fence body back out of the clipboard.
- Escape in the prototype closes the command palette when it is open and
  otherwise the work drawer (`docs/design/tact-desktop-prototype.html:159`).
  The drawer half is implemented: the shell's own `escape` path dismisses the
  pane while it is the drawer, and falls through to `StopTask` otherwise. The
  palette half needed no code — a focused dialog carries the `Dialog` key
  context, which outranks `TactApp`, so Escape is its `Cancel` before the shell
  is consulted. The one deliberate deviation left is the column: at a
  width where the pane has its own column, v1 leaves `escape` on `StopTask`
  rather than tearing a persistent column out of the layout, which is what the
  prototype's `work(false)` does at every width.
- The prototype's `pane(name)` ends with `if(innerWidth<=1120) work(true)`, so
  selecting a pane on a narrow window also shows it. v1 measures that at its own
  drawer threshold: `select_work_pane` opens the pane whenever the width leaves
  it no column, which is what makes a preset press below 1280 px bring the
  drawer back instead of changing a pane that is not on screen. Above the
  threshold a preset press still leaves a closed pane closed, which is the
  `innerWidth` guard's meaning.
- The prototype caps the drawer at `min(420px,88vw)` and the floating sidebar
  at `min(292px,86vw)`, and gives both `transform:translateX(±105%)` with
  `transition:transform 180ms var(--ease)`. v1 keeps those two floats at the
  base 420 px / 260 px, and now slides them: `work_pane_drawer` and
  `sidebar_overlay` are gated on the width-only form and driven by a `Presence`
  sampled against `overlay_slide()` (`180ms`, `cubic-bezier(.23,1,.32,1)`), so
  both surfaces move in *and* out and unmount only once the exit finishes. This
  GPUI exposes no paint-level transform, so the offset rides on the inset each
  overlay is anchored by (`overlay_inset`) -- the same geometry `translateX` of
  a full width plus 5% produces. The scrim stays out of the slide on purpose:
  the prototype declares it as a `display` toggle inside its own narrow block,
  so it appears and disappears on the frame the pane does and the drawer slides
  out un-dimmed. Reduced motion needs no per-surface guard, because
  `gpui-base`'s motion layer resolves to the end state when
  `App::reduce_motion()` is set -- which is also how the contract suite stays
  deterministic (`settle_motion`); the tween itself is pinned by
  `the_work_pane_drawer_slides_in_and_out_over_the_prototype_duration`.
  Keeping the drawer at 420 px matches the prototype's own `<=1120px` block,
  which resets `.work` to `min(420px,88vw)` rather than leaving it at the
  shrunken column.
  The two `vw` caps cannot
  bind anyway: they only fall below 420 px / 292 px past 477 px / 340 px of
  viewport, and the window's minimum is 960x640
  (`crates/tact-gui/src/main.rs`), so the whole `<=880px` block of the
  prototype — floating sidebar, hidden `.tl`/`.tr`, 28 px transcript gutters,
  the collapsed `.cmd` — is unreachable in the shipped app and exercised only
  by tests.
- The `<=1320px` column shrink is implemented: `Columns::for_width` resolves
  244 px / 374 px / 680 px with 36 px thread gutters below the breakpoint and
  260 / 420 / 720 with 48 px above it, and every column-drawn surface reads that
  one value. The work pane keeps 420 px in its drawer form, which is the same
  query's narrow block resetting `.work`.
- The one breakpoint that is still not the prototype's is the command field:
  the label and chord collapse below 1320 px (`crates/tact-gui/src/shell.rs`)
  where the prototype collapses them only at `<=880px`, so an 1100 px window
  shows a narrower `.cmd` than the prototype does. That is deliberate: the
  `<=880px` block is unreachable at the 960x640 minimum window, and the
  collapse is what keeps the title bar's controls inside the viewport at every
  width the tests sweep.
- The remaining prototype motion is implemented in the same vocabulary as the
  overlays. `.panel.active` fades in over `180ms` and adds a relative `top`
  inset for its 3px lift; user and assistant rows use `Presence` for the
  `320ms` rise and the same inset substitutes for `translateY(5px)`; thinking
  and tool summaries rotate one `ChevronRight` icon by `pi/2` over `160ms`
  through `transition()` rather than swapping glyphs. The row rise follows the
  virtual list's rendered window: a row that is still off-screen starts when it
  first renders, so the implementation keeps the prototype's timing but not its
  literal off-screen `both` semantics. Reduced motion resolves all three
  samples to their end state. The running composer action now uses the
  prototype's square stop glyph, and `Open in editor` uses its book glyph.
- `.prompt` is capped. The prototype sets `min-height:48px; max-height:150px`
  on a `rows="2"` textarea (`docs/design/tact-desktop-prototype.html:19,118`),
  which keeps a long draft in the box instead of letting the composer eat the
  transcript. The GPUI textarea sizes an auto-growing box in whole window
  line-height rows rather than continuous pixels, so v1 expresses the same
  envelope as a row count: `.auto_grow(1, 5)` measures 52 px at rest and stops
  at 132 px when the draft fills the box, inside the prototype's 48/150
  envelope where the previous `.auto_grow(1, 8)` started at 64 px and reached
  192 px with no ceiling at all. `the_prompt_grows_between_the_prototype_minimum_and_maximum`
  pins both ends.
- Assistant prose now renders in bundled Lora. The prototype carries document
  text in `--serif` (`:root` and `.body p`, 14.5px/1.65), the review calls
  that deliberate, and the approved spec names Lora in its decision summary,
  transcript section, typography table, and approval points
  (`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md:17-19,267,380,681`).
  `tact-gui` bundles Lora Roman/Italic variable faces under `assets/fonts`,
  registers them with `App::text_system().add_fonts`, and applies the family to
  assistant Markdown and expanded reasoning while leaving chrome on Inter and
  code on JetBrains Mono. This avoids GPUI's silent sans fallback on hosts
  without a system Lora; the OFL licence ships beside the fonts.
- The session chip is the spec's dropdown, and its four rows now act. The
  prototype draws it as a real `<button class="session">` with a chevron
  (`docs/design/tact-desktop-prototype.html:37`) and the spec asks for rename,
  duplicate, archive and reveal-in-filesystem
  (`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md:230-231`),
  so the static label is a `Popover` (`SessionChip` trigger,
  `session-menu-panel`) built from the primitives the composer menus already
  use, keeping the prototype's box exactly. Rename writes the new
  `sessions.title`; archive writes a reversible `sessions.archived_at` flag and
  renders the prototype's `Archived` badge; duplicate copies the conversation
  into a `(copy)` row and opens it without provider state or recorded usage;
  reveal opens the workspace directory through the first available platform
  launcher. The broad click walk presses the chip, all four rows and the
  dismissal, with effect assertions for each.
- The card step, the accent ink and the tool output come from the prototype, not
  from the nearest component token. `.card`, `.tool`, `.code`, `.thinking` and
  `.approval` are all `--r10`, so `card`, `card_with_id` and the transcript's
  error row draw `rems(0.625)` instead of `radius_2xl()` (15 px on the 6 px
  theme base); one theme radius cannot express the prototype's 6/8/10/14 family,
  which is why the step is explicit. `.out` keeps its 150 px cap but scrolls
  (`overflow_y_scrollbar()`) instead of clipping: the scroll wrapper re-ids the
  element it wraps and must be the last step of the chain, so the `tool-output-*`
  test anchor sits on an inner node and the wrapper carries a per-row id to keep
  the cards from sharing one scroll position. Accent-on-accent text uses
  `accent_foreground` (`.badge.run`, `.row.active strong`, `.gutter`,
  `.approval .warn`, the status bar's `.accent`) rather than `primary`, and the
  new `accent_tint()` reads `--accentTint` per theme -- `.10` light, `.12` dark
  -- where it used to be hardcoded at `.12`. The sidebar search field is drawn
  with `appearance(false).bordered(false)` so it does not paint a second box
  inside the shell's own 30 px well.
- Post-v1 layout persistence is parked: the v1 shell keeps fixed sidebar and
  work-pane widths, selected pane, and transcript detail instead of restoring
  them from a settings store.

- A pixel sweep of the prototype's role tokens cleaned up ten places where the
  shell had drifted onto the nearest component default. Hover and selection in
  the sidebar are no longer the same fill: `.row:hover` takes `--hover`
  (`accent.background`) while only `.row.active` takes `--accentTint`, and the
  tint alpha itself moved into `tint_alpha()` / `accent_tint()` in
  `crates/tact-gui/src/theme.rs` so the light theme gets the prototype's `.10`
  instead of a hardcoded `.12` (`pane.rs`, `transcript.rs`, `shell.rs`). Accent
  ink follows the same rule, so the current plan step, `In progress` and
  `Running` chips use `--accentInk` (`accent_foreground`) over the tint, and
  diff rows draw 70 % of the tint alpha, which is what `color-mix(… 70 %)`
  resolves to. The title-bar preset switch left the stock `TabBar::segmented()`
  (32 px tabs on a `--line`-filled bar with `--page` inside, no per-tab box
  override) for a composed `.tabs` strip: 2 px padding, a 1 px `--line` frame,
  `--surface2` fill, and 26 px `.tab`s that take `--surface` plus a 1 px ring
  when active. Each tab keeps the integer element id that
  `within("workspace-tabs")` selects by, and
  `the_title_bar_controls_use_the_prototype_boxes` now pins the 32 px strip and
  its 26 px tab. The worktree and background groups finally use the same `.row`
  box as the session rows (`sidebar_meta_row` gained the 44 px minimum, the
  12 px/600 title, the 10.5 px metadata line, and the 17 px mono `SessionBadge`),
  with the 7 px dot inside its 3 px halo instead of a hairline border;
  `the_sidebar_lists_worktrees_and_background_work` asserts the 44 px floor for
  both groups. Hover feedback that the prototype defines was added to the tool
  card (`--surface2` over `--line2`), the thinking summary, plan steps and task
  rows (`--surface2`), and the file tree (`.row2:hover` / `.row2.active` =
  `--accentTint` over `--accentInk`); the last plan step and the last task row
  drop their `border-bottom`, the status bar's project chip uses `--ink2`
  instead of full ink, and the sidebar's first group sits 6 px below the fixed
  header (`4 px` scroller padding + `2 px` group margin) instead of 16 px.
- One deviation is parked rather than approximated. The command palette is
  still the stock `Command`, whose selected row paints `accent.background` with
  `accent_foreground` (`gpui-component-0.6.4/src/command/state.rs:661-675`),
  while `.pitem.selected` in the prototype is `--accentTint` with `--ink`
  (`docs/design/tact-desktop-prototype.html:22`). The component exposes no
  per-row style hook, and `accent.background` cannot be repointed at
  `--accentTint` because that token carries the hover fill for `.tab`, `.icon`,
  `.row`, `.cmd` and `.wtab` — far more surfaces than the palette. Closing it
  means replacing the palette list with a shell-owned one that keeps the
  component's keyboard and filtering behaviour. The sidebar's hover and
  selected fills are likewise not assertable through `ElementSnapshot`, which
  exposes geometry, text and accessibility state but no colours, so that split
  is held by its single call site plus the theme role test rather than by an
  integration assertion.

- The composer's Enter-to-send path is covered by
  `the_enter_key_sends_the_draft_like_the_button`. An earlier probe concluded
  the opposite — that `press("enter")` never reached the handler that emits
  `InputEvent::PressEnter` — and that conclusion was a measurement artifact, not
  a harness limit and not a product bug. The transcript is a virtualized
  scroller: a submit really does append its rows, but they are only built once
  the tail is in view, so reading `transcript-row-*` straight after the
  keystroke reports "missing" for rows that exist. The contract therefore
  scrolls to the end before asserting. Two further checks confirmed the wiring:
  dispatching `input::Enter` directly through `Window::dispatch_action` submits
  exactly like the button, and the composer's own node chain sees the
  `input::Enter` action on the keystroke path. `gpui-base` binds `enter` to its
  `Enter` action in the `Input` key context
  (`gpui-base-0.6.4/src/input/base/state.rs:151-158`); `cx.propagate()` in the
  submit branch also lets the platform text-input fallback insert the newline
  the keystroke carries, which is why a stray newline is not evidence of a
  missed binding.

- The second pass of the pixel audit moved five more surfaces onto the
  prototype's own values. The running tool's glyph takes `--accentInk` on its
  `--accentTint` chip (`.tool.run .toolIcon`, `HTML:17`) instead of `--accent`.
  `.tasks th`'s upper case is carried by literal `TASK`/`STATUS`/`OWNER` labels
  because GPUI has no `text-transform` (`HTML:20`). The approval paragraph takes
  `.approval p`'s 1.5 line height (`HTML:18`). `.tree .row2`'s idle rows start at
  `--ink2` and let the expanded/hover layers carry them to `--accentInk`
  (`HTML:20`). The open worktree is a plain `.row` again -- `HTML:13`'s 7 px
  corner, a neutral dot, and `1 active` as its only mark (`HTML:65`) -- instead
  of the `.row.active` fill it had been given; `aria-selected` keeps the state in
  the accessibility tree, since no snapshot can read the colour.
- The `:focus-within` rules the shell had left unbuilt are now drawn from the
  shell's own focus tracking. Both fields are created with
  `appearance(false).bordered(false)`, so the components paint no ring of their
  own; instead the search well and the composer card `track_focus` their field's
  handle and carry a focused style, which is how GPUI spells `:focus-within`
  (`gpui-pre-0.3.5/src/elements/div.rs:3413-3422`). Focus turns the border
  accent, raises the search well to `--surface` (`HTML:13`), and draws
  `0 0 0 2px var(--accentTint)`; the composer card also gains the prototype's
  resting `box-shadow`, which the focused style replaces (`HTML:19`). One
  deliberate difference: `track_focus` marks the container focusable
  (`gpui-pre-0.3.5/src/elements/div.rs:794-798`), so a press on the card's
  padding focuses the field inside it, where the browser's plain `div` would
  ignore the press. Rings are colours and shadows, which `ElementSnapshot`
  cannot read, so their evidence is the construction plus a green click/focus
  suite rather than an assertion.
- The prototype's third ink tier now has its own role. `--ink3` is `#716e65`
  in light and `#8c8a80` in dark (`docs/design/tact-desktop-prototype.html:8-9`),
  but the shipped theme maps `muted.foreground` to `--ink2`; using that token
  wherever the prototype asks for `--ink3` collapsed two type tiers.
  `theme::ink3(cx)` resolves the role without changing the theme schema, and
  `the_tertiary_ink_matches_the_prototype` parses both `:root` blocks and
  compares them with the constants. The migration has 33 `ink3` uses across
  `shell.rs`, `pane.rs`, and `transcript.rs`, while the genuine `--ink2` sites
  stay explicit: `.status strong` (the project chip) and `.tree .row2`'s idle
  text.
- The dialog overlay now uses the prototype's fixed scrim in both themes.
  `.overlay` is `rgba(20,20,19,.18)` regardless of mode
  (`docs/design/tact-desktop-prototype.html:22`), while the component default
  varied with the theme. `theme::activate` pins
  `Theme::global_mut(cx).overlay` after `Theme::change(...)`, because
  `Theme::change` runs `apply_config` and resets the colour table; moving the
  assignment before it silently loses the value.
  `activation_pins_the_dialog_overlay` exercises that order and asserts the
  resolved light-mode overlay. Accepted deviation: GPUI exposes no
  backdrop-filter hook here, so the prototype's 2 px blur is not painted.
- The composer's resting shadow is pinned to the prototype's ink, not the
  theme's foreground. `.composer` hard-codes `rgba(20,20,19,.04)` and
  `rgba(20,20,19,.035)` in both modes (`docs/design/tact-desktop-prototype.html:19`),
  but `foreground.opacity(...)` made the dark theme paint a near-white glow.
  The shell now builds both shadows from `Hsla::from(rgba(0x1414_13ff))`; the
  focus ring still replaces the resting pair.
- The worktree's idle marker is now the prototype's neutral dot, not an ink
  opacity. `.dot` is `--line2` with a `--surface2` halo
  (`docs/design/tact-desktop-prototype.html:13,65`), which the theme exposes as
  `input` + `muted`; `sidebar_meta_row` uses that pair and no longer takes a
  dead `muted` argument. The active worktree remains a plain row with the
  `1 active` badge and `aria-selected`, matching the prototype's `.row` rather
  than `.row.active`.
- The new `the_error_row_reports_its_text_as_an_alert` contract seeds
  `TranscriptRow::Error` through `TactApp::with_error` and reads the rendered
  row as `Role::Alert` with `aria_label("provider failed")`; this covers the
  error path without a provider.
- The new `the_command_palette_keyboard_contract_moves_and_runs_rows` contract
  drives the palette with GPUI's lowercase key names (`down`, `up`, `enter`,
  `escape`, `tab`). It asserts the initial selection, upward and downward
  movement, Escape dismissal of an empty query, Enter running `Toggle sidebar`
  and closing the palette, and Tab leaving the selected row and palette mount
  unchanged.
- The new `the_composer_option_rows_keep_the_choice_they_set` contract covers
  the model, budget, effort, and permission popovers. Each chosen row reports
  `checked`, its sibling reports off, the choice survives close/reopen, and the
  offline shell appends exactly one notice per command.
- The new `the_composer_add_rows_do_what_they_name` contract covers Skills,
  Connectors, and Plugins. Skills leaves `/` in the composer, while Connectors
  and Plugins each append one notice; the native Attach-file picker is
  deliberately left to the platform and is not asserted here. GPUI text fields
  do not expose their value through the accessibility snapshot, so the new
  `TactApp::composer_draft(cx)` seam reads the draft directly for that check.
- The new `every_work_pane_states_its_own_empty_case` contract renders a shell
  with `TactApp::with_workspace(None)` and visits all five work panes. It reads
  back each `work-pane-empty-*` label rather than accepting a bare card:
  `No plan yet.`, `No file changes yet.`, `No tasks in this session.`,
  `No subagent runs yet.`, and `No workspace directory.`.
- Residual gap: the composer placeholder is styled inside the
  `gpui-component` input as `muted_foreground` (`input.rs:499`) and the
  `.placeholder(...)` call site has no colour hook, so it cannot yet take the
  prototype's `--ink3`.
- Residual gap: the attachment chip's `×` is a ghost `Button`, which paints
  `secondary_foreground` (`--ink`) and only reaches `accent_foreground` on
  hover (`gpui-component-0.6.4/src/button/button.rs:964,1141`); the prototype's
  `.chip button` asks for `--ink3`.
- Residual gap: a file-tree directory expand chevron has the same ghost
  `Button` limitation. Its row text is `--ink2`, but the icon is `--ink` until
  the pointer arrives, so the marker and label do not share the prototype's
  idle tier.
- Accepted deviation: the dialog overlay's 2 px backdrop blur has no GPUI hook,
  so the pinned colour is complete but the blur remains unimplemented.
- Three raised surfaces were still painting flat. The send square takes the
  prototype's `inset 0 1px 0 rgba(255,255,255,.2)`
  (`docs/design/tact-desktop-prototype.html:19`) *before* its idle/running
  branch, because `.send.run` swaps only the fill and the border and therefore
  keeps the hairline on the stop square as well as the accent square.
  `.btn.primary` asks for the same inset
  (`docs/design/tact-desktop-prototype.html:18`), which now rides on the
  approval card's "Allow always" and a question's "Confirm" through
  `<Button as gpui_kit::Styled>::shadow(...)`: the component's inherent
  `Button::shadow(bool)` occupies the name the styled setter wants, and the
  component refines the caller's instance style last
  (`gpui-component-0.6.4/src/button/button.rs:576,690`), so the caller's shadow
  list survives the variant's fill and border. The selected workspace tab adds
  `.tab.active`'s `0 1px 2px rgba(20,20,19,.06)` lift
  (`docs/design/tact-desktop-prototype.html:12`) in the prototype's own
  `0x141413` ink rather than a foreground role, which would paint a near-white
  glow in dark mode; the `0 0 0 1px var(--line)` half of that declaration is
  the tab's border, which was already modelled.
- The theme-role test now covers 21 reachable mappings instead of five, each
  asserted against both `:root` and `:root[data-theme=dark]`, so the ink ladder,
  the line roles, the accent hover and pressed steps, and the status hues cannot
  drift unnoticed. `TINT_ALPHA_LIGHT` / `TINT_ALPHA_DARK` replaced the bare tint
  literals, and `the_tint_ladder_matches_the_prototype` parses `--accentTint`,
  `--redTint`, `--greenTint` and `--blueTint` out of both prototype blocks and
  compares their alpha -- which is what every badge, diff wash, selection fill
  and composer ring is built from.
- `base.yellow` is deliberately left off that ladder. The prototype declares
  `--orange` (`#9F5D2F` in light, `#D9A441` in dark) but reads `var(--orange)`
  nowhere, and no shell surface reads `yellow`, so the shipped light value is
  left alone rather than remapped by guesswork. The parked-items table above
  records the same conclusion from the token side.
- The ghost button's vanishing outline is fixed rather than accepted. The
  prototype's `.btn.ghost` draws `1px solid var(--line)` and lightens that
  border to `--line2` on hover (`docs/design/tact-desktop-prototype.html:18`),
  but the component's `Ghost` variant returns a `transparent` border in every
  state (`gpui-component-0.6.4/src/button/button.rs:1032`), including inside its
  own hover style (`:1137`), and that hover style is refined at *paint* time
  over the element's style (`gpui-pre-0.3.5/src/elements/div.rs:3454-3469`),
  after the call site's instance style. An explicit `.border_color(...)` at the
  call site therefore restored the resting outline and lost it again the moment
  the pointer arrived. A button's own `hover` style cannot be extended from the
  call site -- `hover` asserts it is unset and replaces it
  (`gpui-pre-0.3.5/src/elements/div.rs:848-855`) -- so `prototype_button` now
  builds the `.btn.ghost` form from `ButtonVariant::Custom`, whose border colour
  is state-independent (`button.rs:1033-1039`) and which takes an explicit hover
  fill. The resting fill still comes from the call site's `.bg(--surface)`,
  because `Custom`'s own normal background is a 20 % blend of its colour
  (`button.rs:925,943`). One part stays unmatched: the hover border holds at
  `--line` instead of stepping to `--line2`. This one helper is the only
  producer of the `.btn.ghost` form, so all six of its call sites -- the
  approval card's `Deny` and `Allow once`, `Comment`, `New task`, `Add file`,
  and `Open in editor` -- move together.
- Interaction-state parity was audited selector by selector: 41 prototype
  rules and state pairs (`:hover`, `:focus`, `:focus-within`, `:focus-visible`,
  plus the reduced-motion block) were each matched against the shell, alongside
  its four transition/animation declarations. Every hover rule has a
  counterpart, and the four durations and easings match -- `panel` 180 ms,
  `chev` 160 ms, `msg` 320 ms, the two overlay slides 180 ms, all on
  `cubic-bezier(.23,1,.32,1)` -- and the
  prototype declares no `:active`, `:checked`, `[disabled]` or `[aria-selected]`
  rule at all, so there is no press-state or form-state contract to miss. The
  last real gap on that axis was the drawn controls' own focus: in the prototype
  `.row`, `.tab`, `.icon`, `.cmd`, `.new`, `.mini`, `.wtab`, `.toolbtn`,
  `.session` and `.send` are `<button>` elements and therefore tab stops that
  take the line 27 `button:focus-visible` ring, while the shell draws them as
  plain `div`s. They are now wired for it -- see the keyboard entry below.
- The drawn controls are tab stops that paint the prototype's keyboard ring.
  The wiring is small because GPUI already supplies both halves: `tab_index(0)`
  marks the element focusable and a tab stop, and an element that says so
  without handing in a handle gets one from its own element state, which GPUI
  keeps for as many frames as the element with that id is drawn
  (`elements/div.rs:2246-2264`) -- stable per id, which is what a purely local
  `cx.focus_handle()` would not be. `Div::focus_visible` (`div.rs:1292`) then
  applies the ring only while the element is focused *and*
  `window.last_input_was_keyboard()` (`div.rs:3425-3430`), which is
  `:focus-visible` rather than `:focus`. The ring itself is
  `focus_visible_ring` in `crates/tact-gui/src/shell.rs`: an *inset* 2 px
  spread `BoxShadow` in `--accent`, since GPUI has no `outline`. Inset rather
  than drop is what the live capture forced: GPUI paints a drop shadow as a
  *filled* rounded rect behind the element, so on the controls whose own
  background is transparent or translucent -- the session rows, which only
  carry a background while current, and the title-bar `.tab` chips -- that fill
  showed straight through and the "ring" was a solid orange slab over the whole
  row. An inset shadow paints after the background and before the children, so
  it stays a 2 px ring whatever the background is. It is applied by
  `session_row`, `sidebar_meta_row` (the worktree and background rows),
  `chrome_icon_button`, `tool_button`, `mini_chip`, `send_button`, the sidebar's
  `.new` row and the title bar's `.tab` strip.
  The activation half comes free too: GPUI synthesises a click for a focused
  element that carries `on_click` on the *key up* of an unmodified Enter or
  Space (`div.rs:3013-3060`), which is how a native `<button>` behaves.
  Measured on the preview shell, 34 tab stops are reachable by repeated Tab
  against 6 before the change, focus holds its handle across frames, and
  `tab_reaches_the_drawn_controls_and_enter_runs_them` walks Tab and Enter to
  the title bar's sidebar toggle, which is what closes the sidebar. Dropping the
  single `tab_index(0)` from `chrome_icon_button` fails that contract.
  Two residuals: the prototype's `outline-offset: 2px` has no GPUI counterpart,
  so the ring hugs the box -- and now hugs it from the inside; and a refined
  style swaps the shadow list rather than appending to it, so a focused active
  title tab wears the ring instead of its `0 1px 2px` lift, and a focused send
  square wears it instead of its top hairline.
  Hugging from the inside is also the safe half: the component's own ring
  (`gpui-component-0.6.4/src/styled.rs:205-273`) paints its band *outside* the
  border box, which is what the prototype's `outline-offset` asks for, but a
  clipping ancestor cuts that band off -- and the rows this shell draws by hand
  live in clipped boxes (the sidebar list scrolls, `shell.rs:2807`; the
  transcript column is `overflow_hidden`, `shell.rs:3810`). The components keep
  the outside band: `Button` turns on `focus_ring_style` while focused
  (`button.rs:835-836`, with `tab_index(0)`/`tab_stop(true)` by default at
  `:272-273` and `:774-775`), at `FOCUS_RING_OPACITY` rather than the
  prototype's solid accent. So a keyboard user sees two ring geometries in one
  window: the component's faded outside band on the component controls, the
  shell's solid inset 2 px ring on everything hand-drawn. That split is the
  price of mixing the two; unifying it would mean forking every component
  control or clipping the shell's rings at every list edge.
  The row-level path now has an executable contract:
  `tab_and_enter_re_root_the_window_at_the_row_they_reach` in
  `crates/tact-gui/tests/shell.rs` blind-walks Tab + Enter and asserts the
  window re-roots onto the worktree row the keyboard reached
  (`ElementSnapshot::selected()`), checking the effect rather than focus itself.
  Two harness rules are non-obvious: `gpui-kit`'s `press` sends only key down,
  so Tab/Enter/Escape need a hand-dispatched `PlatformInput::KeyUp` or the walk
  sticks; and Enter can open a popover/dialog that traps Tab, so every step
  sends Escape. The walk also re-opens the sidebar after Enter lands on the
  title-bar sidebar toggle.

- A click costs one frame, and it still does after every later alignment pass.
  Re-measured on this host against the current tree (preview shell, 1536x842,
  `/proc/<pid>/stat` utime+stime deltas): idle 0 %, a no-op key 4 ms, `Ctrl+B`
  9-10 ms and `Ctrl+O` 14 ms per press -- the same order as the ~11-12 ms the
  optimization commit recorded, so the work-pane, transcript and keyboard-ring
  passes added no per-click cost. The two animated actions are heavier in
  *total* but not per frame: opening and closing the palette costs 158 ms per
  pair and a theme toggle 250-430 ms, because the toast it pushes animates for
  roughly twenty frames at ~11 ms each. Skipping the component's `Theme::change`
  behind a temporary env gate moved the toggle only from ~250 ms to ~240 ms,
  which is what rules the theme swap itself out as the cost.
- Switching surfaces for a long time leaves nothing behind. Two rounds of ~215
  mixed key actions (sidebar, transcript detail, session cycling, palette
  open/close, and a theme toggle every sixth iteration) ran against the preview
  shell without a crash or an error in the log; resident memory went 183.5 MB
  (warm) -> 202.4 MB after the first round -- one-time cache growth -- and
  202.42 MB after the second, flat over five idle seconds. Descriptors stayed at
  80 throughout, and the two rounds cost 549 and 552 CPU ticks, so the work per
  round does not accumulate. The state is exactly reversible too: after the
  hammering, an even number of `Ctrl+B` presses reproduces the earlier frame
  byte for byte (`magick compare -metric RMSE` = 0) while a single press differs
  (RMSE 0.107), which is the machine-level version of
  `rapid_switching_between_surfaces_stays_responsive`.
- The responsive breakpoints hold in a real window, not only in a test
  viewport. The window was floated (`hl.dsp.window.float({window=..})`) and
  resized (`hl.dsp.window.resize({x=..,y=..,window=..})`), then one row of the
  capture was scanned for the `#302e2a` border; the capture rect is re-read from
  `hyprctl clients` after every compositor call, because a hardcoded rect keeps
  photographing the wallpaper once the window has moved. At 1440 px the
  sidebar's right border lands at 259 px and at 1300 px it lands at 243 px --
  the 1320 px switch from 260 to 244 px. At 1200 px the work pane's left border
  sits at 780 px, i.e. `1200 - 420`: the drawer's fixed width rather than the
  narrow column's 374, so the pane has left the grid below 1280 px. The one
  form this cannot reach is the sidebar overlay, and that is a property of the
  app rather than of the harness: the window minimum is 960 px
  (`crates/tact-gui/src/main.rs:14`, `:62`) while the overlay needs *less* than
  960 px (`crates/tact-gui/src/shell.rs:61`, `:1811`), so a floating resize
  stops one pixel short of it -- asking for 940x800 lands at 960x800.
  `the_sidebar_overlay_keeps_its_presses_above_the_scrim` still covers the
  overlay in a 900x700 test viewport, and the spec asks for both the 960x640
  minimum and the <960 px overlay (`spec:202`, `:216`), so the overlay is
  specified and exercised, but on a compositor that honours the size hint it
  appears only when something else forces the window narrower.
- The shell has been rendered and captured on a live Wayland session, in both
  themes, and compared against the prototype by eye and by sampled pixels. That
  pass produced the ghost-hover evidence below and is the only check in this
  document that sees actual antialiased output rather than buffer state. The
  recipe, including the three traps that cost earlier attempts:
  `cargo build -p tact-gui`, then
  `setsid nohup target/debug/tact-gui --preview &` with `WAYLAND_DISPLAY` and
  `XDG_RUNTIME_DIR` exported; focus it with
  `hyprctl dispatch 'hl.dsp.focus({window="class:dev.tact.Tact"})'` and move the
  pointer with `hl.dsp.cursor.move({x=..,y=..})` -- this `hyprctl` is the Lua
  dispatch build, so the older `dispatch focuswindow class:..` spelling (and any
  bare `movecursor x y`) is a syntax error. `grim -g "<x>,<y> <w>x<h>"` takes
  *logical* geometry and returns the physical bitmap, so a 1536x842 window on
  this 1.25-scale display lands in /tmp as 1920x1052. Toggle themes with
  `wtype -M ctrl -M shift -k l -m shift -m ctrl`, then wait out the "Light
  theme" toast before a hover capture -- it covers the footer's controls.
- The keyboard ring was checked on screen the same way the ghost outline was,
  by parking focus with `wtype -k Tab` and sampling consecutive `grim`
  captures. With the ring drawn as a drop shadow, a focused session row put
  ~16 000 accent pixels in the row's band against ~320 idle -- the slab, not a
  ring. With the same walk after the switch to an inset shadow the focused row
  measures ~700 and the same band returns to its ~320 baseline the moment Tab
  moves on, and the focused title-bar tab shows the same 2 px ring.
- The ghost button's hover state was verified by a mutation test rather than by
  reading. With the fixed `.custom(...)` variant a horizontal scanline through
  `Open in editor` reads the border pixel as `#E4E2D9` in *both* the idle and
  the hovered capture while the fill moves from `#FBF9F5` to `#EDE9E3`; with the
  old `.ghost()` variant restored the same pixel is the hover fill `#EDE9E3` --
  the outline the prototype draws is gone. The two captures differ only in that
  pixel and the fill, so the check discriminates the regression instead of
  merely passing on the fixed tree.

The remaining visual risk is platform-specific font fallback and OS-level
window chrome; this host falls back from DejaVu Sans Mono to Noto Sans Mono.
