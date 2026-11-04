# Tact Desktop visual baseline

The 2026-10-31 review found that the desktop shell passed every automated gate
while still looking nothing like the approved prototype. The gates measured
routes, constants, and "does not panic"; none of them compared the rendered
surface with the design. This baseline exists so that cannot happen silently
again.

## What "done" means now

A desktop surface is not done because its route exists or its test is green. It
is done when:

1. the off-screen capture is deterministic (no developer session data),
2. the prototype and the implementation are reviewed side by side, and
3. the layout assertions that guard the reviewed geometry pass.

The prototype in `docs/design/tact-desktop/previews` is the design oracle. The
implementation is expected to match its structure and information density, not
merely its feature list.

## Status: UI restoration complete

Every surface the prototype defines is implemented and measured against a
1280x840 @2x render of the prototype: title bar, rail (project card, new run,
search, sections, trailing times, quota card, settings), transcript (run
header, user/assistant/tool/diff/notice/hook/error/gate rows, plan card),
composer (single footer row, running status, stop + send), inspector (Run map,
goals, subagents, context, background), and the overlays (command palette,
session manager, extensions popover/manager, hooks sheet, gate review, subagent
drawer, settings).

Automated guards: `chrome_matches_prototype_metrics`,
`overlays_match_prototype_sizes`, `extensions_overlays_match_prototype_sizes`,
`composer_shows_stop_and_send_while_running`, the row-kind tests, and the
Light/Dark anchor checks in `scripts/check-ui.sh`.

Deferred to business wiring, not to visual work: live account identity (avatar
initials, user avatar), the run summary line under the headline, model and
permission values, extension counts, and hooks/session data. The capture
harness seeds the design's copy from `26a` onward so the transcript compares
like-for-like.

## 1. Deterministic captures

`crates/desktop/src/capture.rs` now starts the controller against a disposable
project under the harness's temp data directory and seeds four fixed session
rows. It no longer opens the checkout's `.tact` store, so:

- the rail always shows `Sessions · 4` with stable titles and counts,
- no developer session, prompt, or title appears in a screenshot,
- two runs on different machines produce the same shell.

The seeded transcript, inspector, background, and gate captures come from
`crates/desktop_core/src/fixtures.rs`, as before.

```bash
cargo run -p tact-desktop --example ui_capture --features ui-capture -- target/ui-capture
```

## 2. Prototype-vs-implementation pairs

`scripts/ui-parity.py` composes one image per reviewed surface with the
prototype on the left and the capture on the right, plus a contact sheet.

```bash
python3 scripts/ui-parity.py target/ui-capture target/ui-parity
```

It requires Pillow. The pairs are:

| Name | Prototype | Capture |
|---|---|---|
| `shell-light` | `light.png` | `01-shell-light.png` |
| `shell-dark` | `dark.png` | `02-shell-dark.png` |
| `command-center` | `command-palette.png` | `03-command-center.png` |
| `session-manager` | `session-manager.png` | `06-session-manager.png` |
| `extensions` | `extensions-popover.png` | `07-extensions-mcp.png` |
| `permission-gate` | `permission-gate.png` | `33-gate.png` |
| `sticky-tasks` | `sticky-tasks.png` | `26b-transcript-bottom.png` |
| `composer` | `composer-attachments-queue.png` | `26b-transcript-bottom.png` |

## 2b. Pixel workflow (design parity)

The prototype is rendered by headless Chrome at the same logical size as the
capture (1280x840 @2x), so both images share coordinates:

```bash
node scripts/proto-shot.mjs target/ui-proto/prototype-light.png light
python3 scripts/ui-check.py target/ui-capture target/ui-proto/prototype-light.png
```

`scripts/ui-check.py` fails when a structural anchor drifts more than 3 device
pixels from the design: rail edge, titlebar height, composer left/right.
`scripts/ui-diff.py` writes the prototype | implementation | diff overlay and
per-region mean differences for human review. `scripts/check-ui.sh` runs the
whole loop (capture, prototype render, anchors, overlay).

Current anchors at 1280x840: rail edge 498px (249 logical), titlebar edge
114px (57 + 1px border = 58), composer 630..2429px (315..1214.5 logical,
900px wide, centered). Latest 2026-11-01 second-pass region means (0-255):
Light titlebar 10.09, rail 12.87, transcript 14.66, composer 6.31, inspector
5.23; Dark 9.83, 13.47, 17.96, 4.76, 6.86. The transcript figure compares the
empty shell with the prototype's seeded thread, so it measures content, not
chrome.

## 3. Review protocol

For every desktop change that touches a shell surface:

1. regenerate the captures;
2. regenerate the pairs;
3. open the affected pair and list the differences that are *not* intentional;
4. fix them, or record them as an explicit deferral in this file.

A pair that was generated but never opened is not a review.

## 4. Automated guards

These tests fail when the reviewed geometry regresses:

- `chrome_matches_prototype_metrics` — titlebar 58px, rail 250px, composer and
  sticky 900px centered.
- `overlays_match_prototype_sizes` — command palette 700px, session manager
  1040x720.
- `scripts/ui-check.py` runs for both Light and Dark in `check-ui.sh`.

- `session_list_fills_the_rail_instead_of_an_empty_spacer` — the session list
  must reach the extensions section; an empty flex spacer must not steal the
  rail's free height.
- `back_to_bottom_floats_above_the_sticky_strip` — the recovery pill must sit
  above the sticky strip *and* left of the open inspector.

The remaining P0/P1 gaps are tracked in the remediation plan, not hidden in a
green test run.

## 4b. Alignment pass (2026-11-01)

A review of the running shell found it still diverged from the prototype even
though every anchor passed. The differences were structural and typographic,
not missing surfaces:

- **Message column.** Plan, thinking, tool, diff, gate, notice, hook and error
  rows now start at the message column (avatar 28px + 12px gap), so a plan
  card lines up with the answer above it instead of running to the column
  edge. `row_starts_at_message_column` guards the rule.
- **Assistant prose.** An assistant answer is plain Markdown on the canvas,
  with the selection toolbar floating on demand (`SelectionActions::frameless`,
  P12). It had been wrapped in a bordered panel.
- **Plan steps.** The state mark leads (check, ringed dot, empty circle), the
  label follows, and the ordinal sits at the right edge, matching the design's
  `22px / 1fr / auto` grid. The kicker agrees with the count (`PLAN / 1 STEP`,
  `plan_step_label`).
- **Tool groups.** One card with a 37px header and hairline-separated rows at
  10px type, no per-row status pill (`ToolCall::grouped`,
  `ToolGroup::compact`, P14).
- **Rail scale.** Project card 11/10px, session rows 12/10px with a monospace
  time and 5px padding (P11), 10px monospace section and extension labels, the
  list capped at 32% of the window height (190–330px), and the quota card
  pinned to the bottom.
- **Composer.** The send control is the design's paper plane in a rounded
  square and the stop control a 7px-corner square (P13); the host serves
  `icons/send.svg` because the pinned asset bundle has no plane.
- **Row chrome.** User and assistant marks are the design's 28px rounded
  squares (dark stamp / bordered panel), labels are 11px ink, the run eyebrow
  is uppercase, and the lede is 14px.
- **Plan and tool cards.** A plan whose steps all map to visible tool rows does
  not draw a plan card above the execution group. The tool rows are the single
  surface for those calls' status, output and disclosure; a plan with a
  conceptual step keeps its card.
- **Hook context.** A plugin-injected context is one compact row: source,
  heading and a two-line plain-text preview. The row opens the full Markdown in
  the reader instead of expanding every injected line in the transcript.
- **Overlay scrolling.** Every modal/drawer owns the full pointer and wheel
  surface. Wheel input scrolls the reader or dialog body and never leaks to the
  transcript behind the scrim.
- **Overlay selection.** Reader prose uses the same selection surface as chat:
  a plain click clears selection, double-click selects a word, triple-click
  selects a line, and drag selection remains available.
- **Permission card.** The inline card carries the signal inside its border
  (not in the transcript gutter), 29px decision buttons with `↵` / `esc`
  hints, and a 19px prompt. Review scrolls to the card and focuses Allow once
  without answering it.
- **Resolved gates.** A settled gate keeps the group's compact row anatomy:
  status glyph, `approval` / `ask_user`, prompt and outcome.
- **Command Center.** The extra title row is gone; the order is search, list,
  caption, then labelled hints (`↑↓ move`, `↵ run`, `Esc close`), matching the
  prototype.

## 4c. Second alignment pass (2026-11-01, prototype audit)

The review compared every capture with its prototype preview and closed the
structural differences that remained after 4b:

- **Session Manager.** Flat hairline session rows with a status dot,
  `branch · session xxxx` subtitle, relative time, message count and
  icon actions; a project pane with icon tiles, counts and `Add project`; the
  search field and segmented Active/Archived/All filters share one row; the
  header ends in `×`.
- **Command Center.** Search, a top-biased 700px panel, 28px icon tiles, plain
  mono shortcut hints, and the `RECENT COMMANDS · N OF M` caption rendered as
  a labelled list group, so it sits between search and rows as the prototype
  draws it.
- **Extensions.** Rail-anchored popover at the rail edge with no scrim, tab
  title, underline tabs, `N enabled` footer and Manage; the manager keeps a
  centered dialog with half-width search, `Done` footer and `EXTENSIONS
  MANAGER` kicker; the Hooks sheet's source card hugs its rows.
- **Composer.** Flat textarea, plain `@` / `/` icons, plain model label with a
  chevron, permission label with an orange dot and chevron, and a solid blue
  plane submit control.
- **Transcript.** Hero title uses `clamp(24px, 2.25vw, 32px)` (CJK 1.12 line
  height, 840px measure); question cards use the design's kicker, title,
  description, horizontal option cards and pending/selected footer; a settled
  gate uses the group row anatomy.
- **Back to bottom.** Separate `New activity` chip and `Back to bottom`
  control with a chevron.
- **Rail and inspector.** Active rail rows carry a fill, section labels align
  with row text, the session list is capped at 20% of the viewport (190px
  floor) so the DURABLE block, Extensions, quota and Settings stay visible at
  840px,
  the inspector is a white panel and its current-step card uses a left accent
  bar rather than a full outline.
- **Colors.** Dark rail uses the prototype's `#1c1c1e`; light canvas, rail and
  inspector follow `--canvas`, `--rail` and `--panel`.
- **Execution grouping.** Every turn in one run appends to the run's single
  `ToolGroup`; the group settles when the run finishes. Thinking rows follow
  the TUI region rule: consecutive deltas share one card, while a tool,
  streamed answer, notice, hook, error or completion closes it; a repeated
  `Started` keeps the open card. The title expands the trace in place through
  the `Thinking` component.
- **Back to bottom.** The `New activity` chip appears only while the run is
  active and unseen transcript rows arrived; a finished run or a plain
  scroll-up shows only `Back to bottom`.
- **Session switch.** A new epoch fades the transcript in over 180ms (reduced
  motion settles directly), so the one-frame row replacement does not flash.
- **Modal isolation.** While an overlay is open, focus moves into it and the
  page-level shortcuts (submit, cancel, panel toggles, inspector tabs, queue
  navigation, new session) are ignored until it closes.

### Deliberate deviations from the prototype

These remain visible in a pixel comparison but are product decisions, not
unfinished work:

- The permission card and the sticky warning banner can both be visible while
  the card is on screen; the PRD requires the banner to persist until the
  decision.
- Windows taskbar badges are an overlay icon, not a number: the platform has
  no numeric badge API, so the gate banner and notification carry the count.

Message labels carry the shell's local arrival time (`You 10:42`); persisted
history has no per-message time, so seeded rows leave it out. The decorative
gutter is drawn as well: the thinking row's `∴`, the tool group's vertical rail
and the pending gate's alert mark. Protocol-level message timestamps remain out
of scope because `AgentUpdate` does not carry them.

## 4d. P2 surfaces (2026-11-01)

The polish surfaces have no prototype screens of their own; they reuse the
reader and inspector grammar the prototype already defines. Captures `41`–`44`
record them:

- **ANSI tool output** — SGR colours and attributes on the 11.5px Lilex scale,
  with a dark terminal panel in both appearances.
- **Web-search results** — `SearchResults` with the query header, title, and
  source domain; selecting a hit opens it.
- **Multi-file review** — `DiffTable` with File / + / − columns, a Decision
  column, and a 260px body.
- **Mermaid** — layered node boxes with ↓/→ separators under a `MERMAID`
  kicker; the source fence no longer appears in the answer.
- **Retained evidence** — `ContextCard` rows under the Context inspector's
  token metrics (capture `31`).

## 5. Known gaps at baseline (2026-10-31)

| Surface | Gap | Status |
|---|---|---|
| Session rail | Rows show title + count only; no relative time or preview | Fixed: two-line rows (`SidebarNavItem::detail`) |
| Session rail | Titles truncate without ellipsis; one UUID fallback observed | Fixed: `truncate()` on the row label |
| Composer | Permission picker is a separate full-width row; `@`/`/` are a separate button row; no single composer bar | Fixed: one footer row inside `PromptBar` |
| Sticky strip | One-line text bar; no task card, Goals/Agents counts, or background entry | Fixed: task card with header, counts, progress, rows |
| Title bar | No branch/worktree or account avatar (PRD §title bar) | Fixed: branch pill from `.git/HEAD` (avatar is tracked separately) |
| Inspector | Fixed 318px; values were clipped under the scrollbar (now padded, still cramped) | Fixed: scrollbar padding + truncation |
| Command Center | Flat list; no groups, icons, recent section, or per-row shortcut hints | Fixed: group subtitle, group icon, `/command` hint, count caption |
| Session Manager | No project grouping/chips, archive badge, or keyboard hints | Fixed: project chips include All projects; archived rows carry a badge |
| Extensions | Empty tabs render nothing; `Refresh MCP` is a bare link | Fixed: MCP shows `No MCP servers yet` above Refresh |
| Transcript | Hook context rendered as bare markdown; tool groups and diff were outside the capture window | Fixed: hook context card; captures now cover tool cards (`26c`) and the transcript tail |
| Inspector | Run tab was a flat key/value list with no current step or signal rows | Fixed: Run map header, gauge, Current step, Thread signals, Latest activity |
| Inspector | Latest activity was a text line and tool cards stored a second lifecycle state | Fixed: status chip reads the tool row, opens its reader, and group headers derive status from child tools |
| Gate | Centered review modal with full-width buttons instead of the inline card | Fixed: inline amber card with the signal inside it, one-click Allow once / Allow for session / Deny with keyboard hints; Review scrolls to it and focuses Allow once |
| Account avatar | Title bar has no account avatar | Fixed: local account initials in the title-bar stamp; plan/quota still come from `AccountView` |
| Command Center icons | Rows have no leading icon | Fixed: group icons via the `CommandSearchItem::icon` patch |
| Session Manager | Layout was a single narrow column | Fixed: 1040x720 dialog with a 232px project pane, header and list |
| Overlays | Palette was 640px; manager 760px | Fixed: palette 700px, manager 1040x720 |
| Extensions | One 720px dialog for both popover and manager | Fixed: popover 520px, manager 780x620 with underline tabs |
| Session rail | Project subtitle had no branch | Fixed: `4 sessions · feat/desktop` |
| Composer | No running status or stop affordance; send was a disabled outline | Fixed: `Running · Esc interrupts`, red stop button beside a solid blue send button |
| Title bar | Workspace pill showed only the folder name | Fixed: `owner/repo` from the origin remote, branch and short session id (`rust-infra/tact \| feat/desktop \| session 8f2a`) |
| Subagent transcript | Centered 820px modal | Fixed: right drawer 520px, full height |
| Gate review | Plain 560px modal with full-width rows | Fixed: 640px amber card with signal dot, kicker, esc hint |
| Rail rows | 55px pitch | Fixed: 53px pitch (`SidebarNav` row padding 7) |
| Run gauge | 4px ring | Fixed: 5px ring |
| Hooks | Only an Extensions tab | Fixed: dedicated 780x620 `Command hooks` sheet with trust rows and footer |
| Rail quota | Card was absent because the capture had no account data | Fixed: `Kimi K2.5 · 78%` card with green bar and reset line (real `AccountView::UsageQuota`) |
| Rail session rows | Right side showed an active check, no time | Fixed: `SidebarNav` trailing slot shows `19:04` / `Oct 28`; active dot marks selection |
| Transcript plan | No plan row kind existed | Fixed: `RowBody::Plan` with titled step list, `PLAN / N STEPS` kicker and EXECUTING chip |
| Run title | Included the leading URL from the first prompt | Fixed: `strip_leading_url` drops a leading `http(s)://` token, so the headline matches the design |
| Run summary | No second line under the headline | Fixed: the prompt's second line renders as the muted summary (`split_run_text`) |
| Avatars | Hard-coded `Y` / `RG` | Fixed: local account initials from `USER`/`USERNAME` |
| Dark theme | Not compared against the design | Fixed: dark prototype render and anchor check |
| Session Manager keyboard hints | No footer shortcut hints | Fixed: `↑↓ move · ↵ open · esc close` footer |
| i18n | System notices such as `Context compacted: ...` stay English in the Chinese shell | Deferred: the text is produced by the runtime, not the desktop shell |
