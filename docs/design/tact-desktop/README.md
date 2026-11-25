# Tact Desktop design and prototype

The prototype records the reviewed desktop layout. The implementation uses
Tact-owned `gpui-component` surfaces in `crates/desktop/src/ui/`; the
`data-gpui-ai` labels in the prototype are historical names from the reference
component review.

Open `prototype.html` directly in a browser. The prototype uses a native macOS
visual language, local font fallbacks, and no build step.

## Implementation contract

The macOS window chrome, transcript, inspectors, session navigation, slash
overlay, Markdown, Mermaid, ANSI and multi-file diff surfaces are Tact-owned.
They live under `crates/desktop/src/` and `crates/desktop_core/src/`, and depend
on `gpui-component` rather than `gpui-ai`.

Voice input runs through the configured voice worker. The microphone appears
only when that worker is available. The composer owns its model, permission,
reasoning, attachment, mention, queue and slash controls.

## App icon

The icon is a Tact-blue squircle with a white `T` and an open agent loop whose
node sits at the ring's end. The editable source is
`crates/desktop/assets/app-icon.svg`; generated assets are `app-icon.png`
(1024px, Linux/archive), `app-icon.icns` (macOS bundle) and `app-icon.ico`
(Windows executable resource).

```bash
rsvg-convert -w 1024 -h 1024 crates/desktop/assets/app-icon.svg -o /tmp/icon.png
# iconset: 16/32/64/128/256/512 plus @2x, then:
iconutil -c icns /tmp/Tact.iconset -o crates/desktop/assets/app-icon.icns
```

`crates/desktop/build.rs` embeds the `.ico` into the Windows executable; both
packaging workflows copy the `.icns` into `Contents/Resources/Tact.icns` and
set `CFBundleIconFile`, and ship the PNG in the archives.

## Views

- Default: `prototype.html`
- Dark theme: `prototype.html?theme=dark`
- Command palette: `prototype.html#palette`
- Permission-gate section: `prototype.html#permission-gate`
- Subagent drawer: `prototype.html#subagent`
- Extensions manager: `prototype.html#extensions`
- Hook review: `prototype.html#hooks`

## Interactions

- `Cmd/Ctrl+K` opens the command palette.
- The command palette contains every `tact-ui` entry: TUI slash commands and
  subcommands, CLI management commands, settings/flags, session actions, and
  composer inputs. Empty-query state shows eight recent commands; typing
  searches the full catalog.
- `Cmd/Ctrl+1..5` switches the inspector tabs.
- `Cmd/Ctrl+B` hides or shows the session sidebar.
- `Cmd/Ctrl+Alt+B` hides or shows the run inspector.
- The default state is focus-first: the inspector is collapsed, Sticky Tasks
  show only the current-task summary, and the queue is a compact count badge.
  Contextual navigation such as a Goal or Agent action reopens the inspector.
- On narrow windows, the sidebar and inspector become animated overlays with a
  dismissible scrim.
- Theme, tool cards, reasoning, diff review, permission decisions, Goal pause,
  subagent drawer, and prompt submission are interactive.
- Typing `/` opens the slash-command popup with keyboard filtering and
  autocomplete. `Enter` opens a subcommand or executes it; `Tab` completes
  without executing. Nested paths, argument value pickers and free-form command
  forms follow the `tact-ui` command tree.
- A `/` fragment also matches installed skills by name, so `/rust` offers
  `rust-skills` alongside the built-in commands. Selecting it inserts
  `/skill rust-skills `.
- Typing `@` opens the workspace mention popup with files and images.
- The composer model and permission controls open native-style selection menus.
- The quick model menu, full model picker and `@` mention list render fixed
  row windows (eight rows). Wheel pixels accumulate into row steps, and model
  lists longer than eight rows expose a search field inside the popup; mention
  results are already filtered by the typed `@` token.
- Composer attachments render as removable `AttachmentStrip` chips; the queue
  exposes send-now, remove and clear actions. Each queued row shows the files
  its prompt carries as chips — an image with a thumbnail, anything else with a
  file glyph — because a prompt that silently loses its picture on the way
  through the queue is a prompt the reader cannot see is wrong. Editing a
  queued prompt restores its attachments into the composer along with its text,
  so the edit is not a quiet way to drop them. The microphone appears only when
  the voice provider is configured and inserts the transcript on stop.
- Consecutive tools from one loop render as one compact ToolGroup; expand the
  group for the tool list, then expand one row for terminal output or diff.
  Consecutive Thinking/ToolGroup rows share one tight process rhythm (2px
  instead of the normal 12px row gap) when expanded. A run of two or more
  foldable process rows defaults to one summary line ("Ran 25 commands, read
  12 files, ran 1 skill"); clicking expands the original rows. A completed,
  collapsed one-tool group drops its card border/shadow and redundant `1`
  count. Pending/submitting gates are never folded, and running, expanded,
  multi-tool, approval and subagent groups keep the full card treatment.
- ToolGroup membership is by agent-loop iteration, not tool kind: commands,
  edits, `ask_user` single/multi-select, approval and subagent from the same loop
  stay in the same group. Resolved decision/approval rows collapse into the same
  compact row style as `edit_file`.
- Sticky Tasks sits between the transcript and composer, defaults to the full
  task list, can collapse to the current task, and links to Goals, Agents and
  Background. Background opens a task-status popover with command, elapsed time
  and output/stop actions. The card fills the composer's 900px column, so the
  two share both edges; the permission banner on top of it does the same.
- `ask_user` single-select uses a radio group with explicit Confirm/Cancel;
  multi-select uses toggle rows, a selected count and a resolved summary.
- After Confirm or Cancel, the question and option area auto-collapse, leaving
  only the resolved/cancelled status, title and answer summary.
- Sessions from every registered project are loaded and grouped by project in
  the rail, with a scrollable list and a Manage entry. Project switching stays
  in the title-bar workspace switcher; the rail and composer do not repeat the
  project directory. Sessions are listed newest activity first, and project
  groups follow the same rule — a group's position is where its most recent
  session would fall. `updated_at` moves on every appended message, so it is
  the activity order rather than a creation order; the rail sorts the rows it
  is given instead of trusting the snapshot's order, so a future source cannot
  hand it a different one.
- A session can be pinned from its rail context menu, and unpinned from the
  same entry, whose label flips with the row's state. Pinned sessions leave
  their project group for a **Pinned** block above every project, ordered by
  when they were pinned. They are not duplicated into the project group: the
  point of pinning is that the row stops moving with its project. The block's
  heading is Pinned rather than Sessions because the rail already carries a
  SESSIONS heading above the list. It has no project identity, so it carries no
  avatar, no remove-project action and no New Session action — each of those
  names a project, and a pinned set spans projects. The pin itself is a
  nullable `sessions.pinned_at` timestamp, so several pinned sessions keep a
  stable order and re-pinning does not reshuffle them under the pointer.
- The `+` beside the rail's session search field opens the New Session menu: one
  row per registered project (the active one marked), then a divider and
  **Open a project folder**, which raises the directory picker and starts a
  session in a workspace that is not registered yet. It supersedes the earlier
  rule that New Run was not a rail entry. Each project section header keeps its
  own New Session action, and the menu is the rail-level path that does not
  require finding the section first — a workspace that has never been opened
  has no section at all.
  The full Session Manager supports the same grouping, project filtering, fuzzy
  search, Active/Archived/All filters and archive/restore. Rail groups,
  manager groups, and the manager's project directory share one project
  identity and compact avatar/name/count header; a project's rows stay
  contiguous after global search/recency ordering. Both lists are two-level
  collapsible trees: the project node owns the disclosure arrow and count, and
  its session children indent under a shared left guide line. Collapse state is
  shared between the rail and the Session Manager.
- Anything clickable in the title bar (the workspace switcher, the sidebar
  toggle, the appearance toggle, Commands, the inspector toggle) claims its own
  press with `on_mouse_down` + `stop_propagation`. `TitleBar` turns *any*
  pointer movement while the button is held into `start_window_move`, and macOS
  then owns the gesture: the mouse-up never reaches gpui and the click never
  fires. A hand moves a pixel or two between press and release, so without the
  claim the whole bar is effectively dead.
- Project removal is available from both project-directory surfaces (the rail
  project node and the Session Manager project pane) and always opens a
  confirmation. Removal only unregisters the project from the desktop registry;
  files on disk stay intact. Removing a non-active project closes only that
  project's parked hosts, while removing the active one switches to a remaining
  registered project or returns to the empty state. Session archive/restore and
  the Active/Archived/All filters remain in the Session Manager. Rail session
  rows also expose Rename, Fork, Pin/Unpin and Archive/Restore from a
  right-click context menu. Fork copies the transcript into a new top-level
  session and opens it, so
  a reader can try another path without losing the branch already taken. The
  rename form is the same one the Session Manager opens, prefilled with the
  row's current title, so fixing a title no longer requires the full-screen
  overlay. The menu entries are *named* actions rather than one row-level
  label: the archive entry's own text flips to Restore, so a label could not
  identify the operation, and one entry could not carry two. A row locked by
  another process keeps the same four actions — the lock belongs to the live
  host, and Rename, Fork, Pin and Archive all write the store — while the row itself
  stays a disabled navigation target. Fork matters most there: the branch is a
  new row, so it is the one way into a session that is open elsewhere. A row
  carries no New session of its own; creating a session belongs to the project
  section header and the rail's `+`, where the project is what is being chosen. A finished final
  answer also carries its own Fork, revealed only while the pointer is over
  that row and reserving no space when hidden; it branches *at* that answer,
  keeping the turn it belongs to and dropping everything after. Commentary and
  running answers do not offer it — branching a half-written turn has no
  meaning — and only a row seeded from history can, because it is the only kind
  that knows which stored message it came from.
- The first visible idle sessions are prefetched (up to eight per project, capped
  at thirty-two total). Current, parked, locked, and empty sessions are skipped;
  running sessions keep their live state instead of being re-read from disk.
- Up to six idle sessions keep a warm parked host in an LRU pool. Returning to
  one adopts that host without a close/start cycle; overflow closes in the
  background. Running or queued sessions are always parked regardless of this
  limit. A cross-project switch adopts a warm target the same way; a cold
  target starts without closing the host being left.
- Goals, Subagents and Background are not duplicated in the rail; their canonical
  shortcuts live in the sticky strip.
- MCP, Skills and Plugins open one anchored Extensions popover with tab-specific
  lists and actions; outside click, the close button and `Esc` dismiss it. The
  popover uses a roomy 520px surface with its own fuzzy search and a scrollable
  list.
- In the Skills popover, a row click inserts `/skill <name> ` into the
  composer and closes the popover. `Enter` in the search field inserts the
  first match only when the field has a query; an empty query leaves the
  popover open. `Enter` and the row act on the same list the popover renders,
  so a skill hidden by the filter is a skill `Enter` cannot insert. Reopening
  the popover starts from an unfiltered list. The inline Enable/Disable control
  toggles catalog visibility without inserting, and long descriptions truncate
  so the control stays inside the popover. Skills sort alphabetically when the
  query is empty, and by score then name once there is one.
- Both surfaces keep one gesture set per surface. The popover is a launcher:
  its row click and `Enter` insert, and the inline switch is the only place a
  skill changes visibility. The manager is a manager: its Toggle action,
  `Enter` and `Space` only change visibility, and its search field filters
  without touching the composer.
- `Manage` opens the full Extensions Manager at the current tab. It supports
  fuzzy search, MCP/Skills/Plugins switching, enable/disable switches,
  authorization, update and reload actions, and returns focus to the originating
  rail entry. The popover's query carries into the manager on that one handoff;
  closing and reopening the popover starts unfiltered.
- Session, extension, command-palette, Slash-command and mention lists all use
  the same fuzzy matcher. Results are relevance-ranked, so abbreviated queries
  such as `tsd` find `test-driven-development` without paging through the list.
- Every built-in slash command and declared subcommand has an executable
  desktop interaction. `/hooks list` opens the Hook review sheet; Trust/Forget,
  `/skill list`, `/mcp prompts`, `/goal ...` and all other subcommands open
  their owning manager, value picker, form or result reader.
- The transcript header keeps only run state. Model and permission mode live in
  the composer; context usage lives in the Inspector. The run header starts
  24px below the Chat/Trajectory tabs so the title stays close to the page
  switch instead of reserving a large empty band.
- Pending permission is a persistent warning banner at the top of Sticky Tasks.
  Review expands and scrolls to the approval tool, then focuses Allow once
  without selecting it; Confirm remains disabled until the user explicitly
  chooses. Resolving the gate hides the banner.
- When the transcript actually scrolls away from the last row, a floating
  `New activity · Back to bottom` pill appears above Sticky Tasks; clicking it
  smooth-scrolls to the bottom and hides the pill. A disabled tail-follow flag
  alone never shows it while the reader is already at the bottom.
- Subagent transcript reuses the main-agent row grammar: parent task, Thinking,
  ToolCard and Assistant Markdown. It is read-only and omits composer, Goals,
  queue, sticky tasks and other main-agent-only controls.
- Expandable cards share one motion: the surface tint/ring responds, the body
  unfolds by measured height with opacity/blur/translate, inner rows stagger in,
  and collapse runs in reverse.
- Thinking and assistant answers render Markdown through `Thinking` and
  `StreamingText`: headings, lists, blockquotes, inline code, fenced code and
  tables are selectable.
- Transcript, composer, command search and popovers use Tact's reviewed desktop
  typography; macOS system type is reserved for the native shell.
- `Esc` retires exactly one surface per press, innermost first: a pending
  permission gate (denying it), then the topmost overlay, then an in-page panel
  (the composer's queue list, or the selected record in the Trajectory ledger),
  then a live run (interrupting it), then the side regions on a narrow window.
  Panels that are only mounted on one page do not consume the press while the
  other page is showing.
- Inspector tabs slide and crossfade, the tab indicator tracks the active pane,
  tool bodies expand by measured height, permission cards settle into their
  decision, and Light/Dark use a root crossfade.
- Search fields expose visible focus rings and accessible names; decorative
  icons are hidden from assistive technology, and reduced-transparency/high-
  contrast preferences get solid, stronger-border variants.

## Design Artifacts

- `previews/light.png` - default Light appearance.
- `previews/dark.png` - Dark appearance.
- `previews/permission-gate.png` - tool output, diff and permission gate.
- `previews/command-palette.png` - keyboard discovery for all tact-ui entries.
- `previews/panels-hidden.png` - both side regions collapsed.
- `previews/panel-collapse.gif` - animated sidebar and inspector collapse.
- `previews/session-menu.png` - hover session action menu with Archive.
- `previews/session-archived.png` - archived session moved into the recoverable
  Archived section.
- `previews/component-switch.gif` - animated Run -> Goals -> Agents inspector
  switching.
- `previews/tool-group.png` - one compact loop containing two tool calls.
- `previews/toolgroup-resolved.png` - the same loop after both decision tools
  collapse into summary rows.
- `previews/sticky-tasks.png` - expanded sticky task strip above the composer.
- `previews/sticky-background.png` - Background task popover from the sticky bar.
- `previews/sticky-permission.png` - persistent permission banner above tasks.
- `previews/back-to-bottom.png` - transcript tail recovery affordance.
- `previews/tool-selects.png` - single-select and multi-select tool decisions.
- `previews/tool-selects-collapsed.png` - auto-collapsed resolved decisions.
- `previews/card-expansion.gif` - animated tool detail and sticky task expansion
  with surface ring, measured-height unfold and row stagger.
- `previews/thinking-markdown.png` - Markdown rendered in a Thinking block.
- `previews/assistant-markdown.png` - Markdown, code and table rendered by the
  assistant answer.
- `previews/session-projects-rail.png` - project-grouped rail with recent sessions.
- `previews/session-manager.png` - large session manager grouped by project.
- `previews/subagent-consistent.png` - subagent transcript using the main-agent
  rendering grammar.
- `previews/extensions-popover.png` - MCP/Skills/Plugins extension popover.
- `previews/extensions-manager.png` - searchable full extensions manager.
- `previews/extensions-fuzzy-search.png` - abbreviated extension search.
- `previews/session-fuzzy-search.png` - abbreviated session search.
- `previews/slash-hooks.png` - `/hooks` nested subcommand popup.
- `previews/hooks-list.png` - `/hooks list` review and trust surface.
- `previews/composer-attachments-queue.png` - attachment strip and queue actions.
- `previews/selection-actions.png` - Ask / Explain / Rewrite selection toolbar.
- `previews/slash-command.png` - first-level slash-command popup.
- `previews/slash-subcommands.png` - `/mcp` subcommand popup.
- `previews/slash-command.gif` - animated filtering, keyboard navigation and
  subcommand completion.
- `previews/composer-mentions.png` - filtered `@` file mention popup.
- `previews/composer-model-menu.png` - composer model picker.

The current desktop architecture is in
[`ARCHITECTURE.md`](../../../ARCHITECTURE.md). The slash-command parity matrix
is in [`slash-command-audit.md`](slash-command-audit.md). The reviewed layout
baseline and capture procedure are in [`visual-baseline.md`](visual-baseline.md)
and [`visual-verification.md`](visual-verification.md).
