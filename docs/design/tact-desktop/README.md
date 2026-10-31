# Tact Desktop Prototype

Open `prototype.html` directly in a browser. The prototype uses a native macOS
visual language, local font fallbacks, and no build step.

## gpui-ai Contract

The macOS window chrome, mixed transcript container, Goal panel, Subagent panel,
project-scoped Session Manager, nested slash overlay, Mermaid/ANSI renderers and
session persistence are Tact-owned. AI-specific surfaces instantiate real
`gpui-ai` components: `PlanCard`, `Thinking`, `ToolGroup`, `ApprovalCard`,
`StreamingText`, `PromptBar`, `CommandSearch`, `ContextMeter`, `MessageQueue`,
`AttachmentStrip` and the `gpui-component` popovers they compose. Voice is a P2
capability (recording plus a transcription pipeline); the P1 composer does not
mount `VoiceControls` or a dead microphone. `PromptBar` is composed with a Tact-owned slash overlay because its
upstream commands are flat. HTML nodes carry `data-gpui-ai` attributes naming
the actual mapping.

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
- `Cmd/Ctrl+1..4` switches the inspector tabs.
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
- Typing `@` opens the workspace mention popup with files and images.
- The composer model and permission controls open native-style selection menus.
- Composer attachments render as removable `AttachmentStrip` chips; the queue
  exposes send-now, remove and clear actions. Voice input is P2 and is not part
  of the P1 desktop composer.
- Consecutive tools from one loop render as one compact ToolGroup; expand the
  group for the tool list, then expand one row for terminal output or diff.
- ToolGroup membership is by agent-loop iteration, not tool kind: commands,
  edits, `ask_user` single/multi-select, approval and subagent from the same loop
  stay in the same group. Resolved decision/approval rows collapse into the same
  compact row style as `edit_file`.
- Sticky Tasks sits between the transcript and composer, defaults to the full
  task list, can collapse to the current task, and links to Goals, Agents and
  Background. Background opens a task-status popover with command, elapsed time
  and output/stop actions.
- `ask_user` single-select uses a radio group with explicit Confirm/Cancel;
  multi-select uses toggle rows, a selected count and a resolved summary.
- After Confirm or Cancel, the question and option area auto-collapse, leaving
  only the resolved/cancelled status, title and answer summary.
- Sessions are scoped by project. The rail shows a project switcher, a scrollable
  recent list and a Manage entry; the full Session Manager supports project
  filtering, fuzzy search, Active/Archived/All filters and archive/restore.
- Goals, Subagents and Background are not duplicated in the rail; their canonical
  shortcuts live in the sticky strip.
- MCP, Skills and Plugins open one anchored Extensions popover with tab-specific
  lists and actions; outside click, the close button and `Esc` dismiss it. The
  popover uses a roomy 520px surface with its own fuzzy search and a scrollable
  list.
- `Manage` opens the full Extensions Manager at the current tab. It supports
  fuzzy search, MCP/Skills/Plugins switching, enable/disable switches,
  authorization, update and reload actions, and returns focus to the originating
  rail entry. The popover search query carries into the manager.
- Session, extension, command-palette, Slash-command and mention lists all use
  the same fuzzy matcher. Results are relevance-ranked, so abbreviated queries
  such as `tsd` find `test-driven-development` without paging through the list.
- Every built-in slash command and declared subcommand has an executable
  desktop interaction. `/hooks list` opens the Hook review sheet; Trust/Forget,
  `/skill list`, `/mcp prompts`, `/goal ...` and all other subcommands open
  their owning manager, value picker, form or result reader.
- The transcript header keeps only run state. Model and permission mode live in
  the composer; context usage lives in the Inspector.
- Pending permission is a persistent warning banner at the top of Sticky Tasks.
  Review expands and scrolls to the approval tool, then focuses Allow once
  without selecting it; Confirm remains disabled until the user explicitly
  chooses. Resolving the gate hides the banner.
- When the transcript leaves the tail, a floating `New activity · Back to bottom`
  pill appears above Sticky Tasks; clicking it smooth-scrolls to the bottom and
  hides the pill.
- Subagent transcript reuses the main-agent row grammar: parent task, Thinking,
  ToolCard and Assistant Markdown. It is read-only and omits composer, Goals,
  queue, sticky tasks and other main-agent-only controls.
- Expandable cards share one motion: the surface tint/ring responds, the body
  unfolds by measured height with opacity/blur/translate, inner rows stagger in,
  and collapse runs in reverse.
- Thinking and assistant answers render Markdown through `Thinking` and
  `StreamingText`: headings, lists, blockquotes, inline code, fenced code and
  tables are selectable.
- Transcript, composer, command search and popovers use the gpui-ai typography
  and component grammar; macOS system type is reserved for the native shell.
- `Esc` closes the topmost overlay.
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
- `previews/session-projects-rail.png` - project-scoped rail with recent sessions.
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

The PRD is at `docs/superpowers/specs/2026-10-30-tact-desktop-prd.md`.
The slash-command parity matrix is at
`docs/design/tact-desktop/slash-command-audit.md`.
The `gpui-ai` reproducibility audit is at
`docs/design/tact-desktop/component-reproducibility-audit.md`.
The 37-component disposition is at
`docs/design/tact-desktop/component-coverage-matrix.md`.
