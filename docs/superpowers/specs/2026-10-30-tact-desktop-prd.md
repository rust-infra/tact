# Tact Desktop PRD

- Status: prototype-ready
- Date: 2026-10-30
- Scope: native desktop frontend built on `tact_host`, with `gpui-ai` as the
  AI-surface component layer
- Prototype: `docs/design/tact-desktop/prototype.html`
- Architecture: `docs/superpowers/specs/2026-10-30-tact-desktop-design.md`
- Plan: `docs/superpowers/plans/2026-10-30-tact-desktop.md`

## 1. Product Statement

Tact Desktop is a native, keyboard-first control room for a coding agent that
runs long tasks, parallel subagents, durable Goals, and background processes.
It is not a chat website wrapped in a window. The product's primary object is
the **run**: a mixed transcript of messages, reasoning, tools, diffs, gates,
goals, and child agents that can be inspected without losing the thread.

The desktop client reuses the existing agent runtime through `tact_host`.
`gpui-ai` supplies AI-native leaf components; Tact owns the application shell,
the heterogeneous transcript, domain state, persistence, and platform
integration.

## 2. Success Criteria

- A user can start, resume, cancel, compact, inspect, and steer a session
  without leaving the desktop app.
- Every permission request is visible, attributable to the main agent or a
  subagent, and answerable from the keyboard.
- Tool output, thinking, diffs, task state, Goals, and subagent state retain
  stable identity while streaming and reordering.
- A long run remains legible: active work stays anchored, while a user can
  scroll back and inspect completed work without being pulled to the bottom.
- When scrolled away from the live tail, a floating Back to bottom pill with a
  New activity cue appears above Sticky Tasks and restores tail-following on
  activation.
- The same `AgentUpdate` and `UserCommand` protocol drives the TUI, headless
  mode, and desktop mode; the desktop layer does not fork agent semantics.
- Core workflows remain usable with keyboard only, reduced motion, another
  theme, and a constrained window.

## 3. Non-Goals

- Reimplementing the agent loop, tool router, MCP client, memory, hooks,
  plugins, sessions, or Goal runner in the desktop crate.
- Making `gpui-ai::Chat` the Tact transcript. Its message model cannot
  interleave arbitrary tool, reasoning, diff, and system rows.
- Preserving terminal keybindings literally. Desktop shortcuts should express
  the same intent with native conventions.
- Shipping a remote or web-hosted frontend in the first release. The initial
  transport is the existing in-process `tact_host` channel.
- Multiple simultaneous project windows. V1 is one controller (one active
  session) per window, and project switching is a serialized close-then-open.
- Achieving exact pixel parity with the TUI. Feature parity and information
  continuity are the requirement; the desktop has its own layout language.

## 4. Product Principles

- **Run over conversation.** The transcript is an execution record, not a
  message inbox.
- **Control without noise.** Parallel work is summarized in one persistent
  inspector; detail opens on demand.
- **State is owned once.** GPUI components render snapshots and emit intent.
  The desktop reducer owns durable UI state, while `tact_host` owns runtime
  state.
- **Interruptibility is a feature.** Cancel, steer, pause, deny, and queue are
  always reachable while work is running.
- **The terminal soul remains.** Dense metadata, monospace evidence, explicit
  status, and fast keyboard paths survive the move to a graphical shell.

## 5. Target User and Jobs

- A developer delegates a multi-file change and wants to inspect each tool,
  diff, and verification step without reading a wall of terminal output.
- A developer runs several subagents and needs to know which one is blocked,
  which is spending context, and which needs approval.
- A developer pursues a durable Goal across turns and needs evidence,
  criteria, steering, and pause/resume controls in one place.
- A developer resumes yesterday's session and needs the transcript, pending
  state, branch, worktree, and token usage to be immediately understandable.

## 6. Information Architecture

The desktop shell has four persistent regions:

1. **Title bar** - workspace, repository, branch/worktree, session status,
   command palette, theme, and window controls.
2. **Session rail** - project switcher, new session, recent sessions,
   Extensions and Settings. Durable Goals/Subagents/Background shortcuts live in
   the sticky strip and Inspector, not duplicated in the rail.
3. **Run transcript** - mixed rows in execution order, with a sticky run
   summary and a composer/queue at the bottom.
4. **Inspector** - Run, Goals, Subagents, and Context views. It is the
   persistent overview that prevents parallel work from becoming invisible.

Transient surfaces:

- Command palette (`Cmd/Ctrl+K`)
- Permission and multi-select gates
- Session switcher
- Model/effort/budget chooser
- Goal editor and steering sheet
- Subagent transcript drawer
- Diff/code/thinking/system-prompt readers
- Plugin, MCP, Hook, and Skill management sheets
- Settings and theme/language panels

## 7. Chosen Layout Direction

The prototype uses a **three-pane control room**:

- translucent macOS session rail for navigation and durable objects;
- light system workspace for reading and review;
- compact material-backed inspector for active system state.

Two alternatives were evaluated:

- **Chat-first inbox** - simple to build but hides tool work, parallel agents,
  and Goals behind message cards. Rejected because it cannot express Tact's
  primary run object.
- **Infinite canvas/node graph** - strong for parallel-agent topology but poor
  for linear reasoning, long diffs, transcript reading, and keyboard
  navigation. Rejected as the primary shell; a graph may appear inside the
  Subagents inspector later.

## 8. Core Flows

### 8.1 Start or Resume

1. User chooses a workspace and either starts a new session or selects a
   persisted session.
2. The shell loads session metadata and seeds the transcript from the store.
3. Runtime bootstrap completes; startup notices appear as system rows.
4. The composer receives focus only after the session is ready.

### 8.2 Submit and Watch

1. User submits a prompt with model, permission mode, and attachments.
2. The user row appears immediately; the assistant row streams beneath it.
3. Thinking, tool calls, live output, diffs, tasks, and gates append as
   heterogeneous transcript rows with stable IDs.
4. The inspector updates the run map and usage data without stealing focus.
5. Completion settles the run; queued prompts may begin automatically.

### 8.3 Permission Gate

1. A `RequestSelect` or `RequestMultiSelect` event creates a pending gate.
2. The gate identifies the requesting agent and the tool action.
3. The user can approve once, approve for the session when supported, or deny.
4. The response is sent through `UserCommand::UiResponse`.
5. The originating tool row reflects the decision without losing its place.

### 8.4 Goal Supervision

1. Goals appear in the session rail and Goals inspector.
2. The inspector shows status, phase, budget, criteria, evidence, and latest
   run.
3. User can steer, pause, resume, cancel, accept, archive, or unarchive.
4. Goal activations and permission requests are visibly attributed.

### 8.5 Subagent Supervision

1. Spawned agents appear as a compact strip and in the Subagents inspector.
2. Each row shows status, model, elapsed time, context, and last activity.
3. Selecting a row opens an immersive transcript drawer.
4. A blocked child raises a permission gate that names the child.
5. The drawer reuses the main transcript row grammar (parent task, Thinking,
   ToolCard, Assistant Markdown) but is read-only and omits main-agent-only
   controls such as composer, queue, Goals and sticky tasks.

### 8.6 Session Recovery

1. Locked or orphaned sessions are shown as unavailable with an explanation.
2. A recovered session restores messages, tasks, Goals, background jobs, and
   pending UI requests.
3. Stale requests are reconciled from the `UiResponder` snapshot rather than
   relying on a single event.

## 9. Functional Requirements

### P0: Desktop Kernel and Core Run

- New/resumed session bootstrap through `tact_host`.
- Tokio-to-GPUI event bridge with bounded batching and backpressure.
- Controlled desktop reducer for `AgentUpdate`.
- Mixed transcript rows: user, assistant Markdown, thinking, tool, diff,
  system notice, hook context, hook status, and gate.
- Consecutive tool calls from one agent-loop iteration collapse into one
  `ToolGroup`: the group header shows tool count, loop number, combined summary
  and duration; each tool row expands independently to reveal output or diff.
- ToolGroup membership is by loop iteration and includes `ask_user` questions,
  approval gates and subagent work. Resolved questions and approvals collapse to
  the same compact row anatomy as command/edit tools: status glyph, tool name,
  summary, result and disclosure chevron.
- Streaming Markdown and high-frequency tool output without full-list rebuild.
- Thinking and assistant answers render Markdown, including headings, ordered
  and unordered lists, blockquotes, inline code, fenced code, tables and links.
  Code fences keep language labels and copy affordances; content remains
  selectable while streaming.
- Composer with multiline input, model, permission mode, attachments, submit,
  cancel, and queued messages.
- Inline permission and multi-select gates with keyboard operation.
- Single-select tool requests use `QuestionFlow` + `ChoiceGroup` semantics:
  radio selection, Confirm disabled until one option is chosen, Cancel, and a
  resolved answer line.
- Multi-select tool requests use `QuestionFlow` + `Toggle` semantics: independent
  checkbox state, selected count, Confirm disabled at zero, child-agent
  attribution, Cancel, and a resolved answer list.
- After Confirm or Cancel, both request types collapse their description and
  option area, leaving a compact resolved/cancelled summary in the transcript.
- Session teardown that sends `UserCommand::Cancel`, closes the command channel,
  and releases the session lock.

### P0: Run Inspector

- Active run status and elapsed time.
- Current phase and latest tool.
- Turn count, token usage, context meter, cache rate, and session budget.
- Sticky Goal/task/subagent summary while scrolling.
- Sticky Tasks is a persistent strip between transcript and composer. It shows
  the plan counter and the active task at rest (the focus-first default in
  section 12); expanding reveals every task row. Goals and Agents remain
  one-click inspector shortcuts; Background opens the current background-task
  summary and output/stop actions.
- Background task list with open-output action.

### P1: Tact Feature Parity

- Goal create/list/show/steer/pause/resume/cancel/accept/archive/unarchive.
- Subagent overview, cancel, and immersive transcript.
- Slash commands and command palette for all built-in commands.
- Model, effort, thinking budget, permission, theme, and language controls.
- Session history, input history, and resume.
- Session management is project-scoped. The rail shows the active project and a
  bounded, scrollable recent-session list; a dedicated Session Manager provides
  a project pane, relevance-ranked fuzzy search, Active/Archived/All filters,
  archive/restore and long list scrolling without compressing Extensions
  navigation.
- The transcript header is status-only. Model, permission mode and context usage
  stay in their owning controls (composer and Inspector) rather than duplicating
  them above the transcript.
- Pending permission is surfaced as a persistent sticky warning banner, not a
  transcript-header chip. Review expands the approval tool, scrolls it into view
  and focuses the primary allow action; the banner clears after the decision.
- MCP list/auth/prompts, Hook list/trust/forget, Plugin and Skill management.
- The rail MCP, Skills and Plugins entries open a shared Extensions popover with
  MCP / Skills / Plugins tabs, live status and row actions. The roomy popover
  keeps its list scrollable and searchable; `Manage` carries the query into a
  full dialog with fuzzy search, enable/disable, authorization, update and reload
  controls.
- Searchable list surfaces use one fuzzy matcher with relevance ranking:
  abbreviated queries can match words or acronym-like subsequences, and the
  strongest results remain visible without pagination.
- Stats, balance, background, system prompt, help, and history readers.
- Code, diff, thinking, system prompt, and subagent readers.
- Copy/select behavior for transcript and readers.
- Assistant answers expose stable follow-up prompt chips (`Suggestions`);
  selected Markdown exposes Ask / Explain / Rewrite actions
  (`SelectionActions`).
- macOS, Windows, and Linux packaging from one release workflow, with a macOS
  app bundle and native archives for Windows and Linux.

### P2: Desktop Polish

- Mermaid diagrams and the Task DAG graph view (the `/tasks-dag` command entry
  and dependency-list fallback are P1; the graph renderer is P2).
- ANSI-to-GPUI text rendering for terminal output.
- Web-search result reader using `SearchResults`.
- Artifact preview/source panel using `ArtifactPanel`.
- Multi-file review using `DiffTable`.
- Retained-evidence cards using `ContextCard`.
- Native menus and global shortcuts.
- OS notifications and dock/taskbar badges.
- Voice recording and transcription controls.
- Theme migration for existing Tact themes.

### Entry Parity Contract

The desktop is the complete product surface. No user-facing `tact-ui` entry may
remain reachable only by typing a CLI command, except where the action itself is
the CLI process (for example a headless run, which is still exposed as a desktop
entry and reports its output in a result surface).

- All 23 built-in TUI slash commands and their declared subcommands must have a
  desktop entry.
- `headless`, `upgrade`, `plugin`, `mcp`, `hooks`, and `--list-sessions` must
  have desktop entries.
- Top-level flags become settings or per-session controls: config path,
  provider, model, API key, base URL, max tokens, thinking budget, permission
  mode, notifications, context window, theme, project snapshot size,
  micro-compaction, tokio diagnostics, and skill-body injection.
- Composer entries include `@file`, image attachments, voice input, queued
  messages, stop, compact, and session actions.
- The Command Center is the canonical discovery surface. Every entry has a
  stable ID, group, title, description, shortcut/CLI alias, availability state,
  and an executable action.
- Tests must compare the Command Center catalog with `SlashCommand::iter()` and
  the `CliCommand`/setting enums so a new entry cannot ship without a desktop
  route.

## 10. `gpui-ai` Component Mapping

| Tact surface | `gpui-ai` use | Decision |
|---|---|---|
| Assistant streamed Markdown | `StreamingText` | Direct |
| Follow-up prompt chips | `Suggestions` | Direct |
| Selected Markdown actions | `SelectionActions` | Direct |
| Thinking | `Thinking` | Direct |
| Tool invocation | `ToolCall`, `ToolChip`, `ToolGroup` | Direct with Tact adapter; group consecutive calls by loop |
| Code and patch | `CodeBlock`, `CodeDiff` | Direct |
| Approval / plan approval | `ApprovalCard`, `PlanCard` | Direct with domain copy |
| Clarification | `QuestionFlow`, `ChoiceGroup` | Compose |
| Composer | `PromptBar` | Direct; Tact supplies commands/mentions |
| Pending prompts | `MessageQueue` | Direct |
| Attachments | `AttachmentStrip` | Direct |
| Slash command tree | `PromptBar` + Tact-owned slash overlay | Compose; PromptBar commands are flat and cannot represent nested values |
| Session list | Tact-owned project-scoped manager | Build locally; `ThreadList` lacks project/fuzzy/filter controls |
| Command palette | `CommandSearch` | Direct with a flat item list; group metadata travels in subtitle/keywords |
| Context usage | `ContextMeter` | Direct |
| Task and plugin tables | `TaskRow`, `TodoList`, `FilterTable` | Compose |
| Stats / balance summaries | `InsightCard` | Compose |
| Voice controls | `VoiceControls` | Direct in desktop-finish scope |
| Goal panel | none | Build locally |
| Subagent transcript | none | Build locally |
| Mixed transcript | `Chat` is insufficient | Build a virtualized row list |
| Mermaid / Task DAG | none | Build or defer |
| ANSI output | none | Build or strip in v1 |

### Component Contract

- AI-specific surfaces are real `gpui-ai` component instances, not visual
  recreations: `PlanCard`, `Thinking`, `ToolGroup`, `ApprovalCard`,
  `StreamingText`, `PromptBar`, `CommandSearch`, `ContextMeter`, `MessageQueue`,
  `AttachmentStrip`, `VoiceControls` and their `gpui-component` popovers.
- `PromptBar` owns the native editor, model picker, mentions and attachments.
  Tact composes a slash-command overlay around it because upstream
  `PromptCommand` is flat and has no nested/value/Tab-complete model.
- `CommandSearch` renders a flat item list. Tact supplies stable IDs, subtitles
  and keywords; group headers are not claimed.
- The full project-scoped Session Manager is Tact-owned. `ThreadList` is not
  claimed for it because upstream search is substring-only and has no project
  or Active/Archived/All filters.
- The prototype names these surfaces with `data-gpui-ai` attributes. The desktop
  implementation must preserve that mapping.
- Tact owns only the native desktop shell, the mixed virtualized transcript
  container, Goal and Subagent panels, Mermaid/ANSI adapters, and persistence.
- Call `gpui_ai::init(cx)` once and keep every window rooted in
  `gpui_component::Root`.
- Content typography follows gpui-ai: IBM Plex Sans for component text and Lilex
  for code/evidence. SF Pro remains the native shell typeface.
- Markdown rendering goes through gpui-component's `TextView::markdown` inside
  `Thinking` and `StreamingText`; do not maintain a second Markdown parser in the
  desktop shell.
- If a required component behavior is missing, add a thin Tact adapter or send
  the capability upstream. Do not fork the component's visual grammar.
- Pin `gpui-ai` to `=0.8.0` for the prototype handoff; re-audit the component
  contracts before upgrading.
- The complete 37-component disposition, including unused components that
  reveal possible Tact feature gaps, is maintained in
  `docs/design/tact-desktop/component-coverage-matrix.md`.

## 11. Desktop State Model

```text
DesktopState
  session: SessionView
  transcript: Vec<TranscriptRow>
  composer: ComposerState
  inspector: InspectorState
  pending_gates: VecDeque<PendingGate>
  goals: Vec<GoalView>
  subagents: Vec<SubagentView>
  background: Vec<BackgroundTaskView>
  commands: CommandCatalog
  overlays: OverlayState

TranscriptRow
  id: RowId
  sequence: u64
  kind: User | Assistant | Thinking | Tool | Diff | Gate | Notice | Hook
  state: Pending | Running | Complete | Failed
  collapsed: bool
  anchor_key: RowId

Intent
  Submit | Cancel | Queue | RespondGate | SelectCommand | OpenRow
  SetPermission | SetModel | SetEffort | GoalCommand | SubagentCommand
```

The reducer is pure where possible. Runtime effects are emitted as
`UserCommand`, plugin requests, session-store reads, or host service calls.

## 12. Interaction Model

- `Cmd/Ctrl+K`: command palette.
- `Cmd/Ctrl+B`: hide or show the session sidebar.
- `Cmd/Ctrl+Alt+B`: hide or show the run inspector.
- `Cmd/Ctrl+Enter`: submit prompt.
- `Esc`: close topmost overlay; if none, cancel the active CTE request or clear
  selection.
- `Cmd/Ctrl+.`: stop the current run after confirmation if a destructive tool
  is active.
- `Cmd/Ctrl+1..4`: switch inspector tabs.
- `Option/Alt+Up/Down`: move through queued prompts.
- `Space`: toggle selected multi-select options.
- `Enter`: activate focused row, gate action, or command.
- `Tab` and `Shift+Tab`: traverse controls; focus is always visible.
- An empty command palette opens on recent commands; typing searches the full
  catalog.
- The initial desktop state is focus-first: inspector collapsed, Sticky Tasks
  reduced to the current-task summary, and queued prompts reduced to a count
  badge. Contextual Goal/Agent/Context navigation reopens the inspector.
- Side panels animate their grid tracks on desktop and become dismissible
  overlays with a scrim below the full three-column breakpoint.
- Typing `/` opens the slash-command popup. Arrow keys move, `Tab` completes,
  `Enter` executes or enters a subcommand, and `Esc` closes. The popup searches
  names, descriptions and groups, then exposes subcommands for `skill`,
  `plugin`, `mcp`, `hooks` and `goal`.
- Slash navigation mirrors the `tact-ui` command tree to arbitrary depth.
  Argument-taking verbs offer live value candidates (skills, plugins, MCP
  servers, hook sources and Goal IDs); free-form arguments open desktop forms.
  Leaf commands execute instead of leaving inert text in the composer.
- Typing `@` opens the workspace mention popup. Files and images keep stable
  paths, filter as the user types, and insert a path reference without inlining
  file contents.
- The composer's model and permission controls open anchored selection menus;
  choosing a value updates the controlled composer state and reports the
  change through the same intent path as `/model` and `/permission`.

Gates and transient surfaces must restore focus to their trigger when closed.

## 13. Visual Direction

The desktop uses a native platform visual language:

- a native window frame with traffic lights, material-backed title bar, and
  translucent sidebar;
- system blue for selection and primary actions;
- orange for permission and attention;
- green for running and verified states;
- red for destructive actions and failed terminal output;
- SF Pro for UI and display text, SF Mono for evidence;
- thin separators, compact metadata, soft depth, and restrained entrance motion.
- Long CJK prompt titles use a smaller display scale, tighter tracking, natural
  wrapping and a 1.12 line height instead of the Latin hero-title scale.

Only **Light** and **Dark** themes ship in the prototype. They share the same
token contract so a future native settings panel can switch appearance without
changing component geometry.

### Motion Policy

Motion communicates a state change, not decoration:

- Inspector tabs crossfade and slide in the direction of navigation while a
  shared underline moves to the selected tab.
- Session changes blur and settle the transcript rather than replacing it with
  an unannounced snapshot.
- Tool and reasoning bodies expand to their measured height and collapse only
  after the exit transition.
- Card expansion has a visible process: the owning surface gains an accent ring,
  the body unfolds from zero to its measured height with opacity and a small
  vertical/blur settle, and grouped rows arrive with a short stagger. Collapse
  reverses the sequence before the body is hidden.
- A tool group collapses as one unit while preserving the individual tool rows
  inside it; only the selected tool's terminal output or diff expands.
- Permission cards visibly leave the waiting state and settle into approved or
  denied.
- Command palette, theme change, and Subagent drawer use one restrained
  entrance and exit.
- Every scripted transition checks `prefers-reduced-motion` and falls back to a
  direct state change.

## 14. Error and Edge States

- Runtime disconnected: persistent banner with reconnect/restart action.
- Session locked: readable reason and open-another-session action.
- Provider failure: failed assistant row with retry.
- Tool failure: failed tool row open by default; output preserved.
- Pending gate lost: reconcile from responder snapshot on a timer.
- Queue blocked by cancellation: queued prompts become editable rather than
  auto-submitting.
- Subagent failure: parent card and inspector row both settle; transcript
  remains openable.
- Goal lease loss: paused row with recovery explanation.

## 15. Accessibility and Internationalization

- Use AccessKit roles and labels for transcript rows, gates, tabs, and
  inspector regions.
- Do not encode state by color alone; pair tone with text and glyph.
- Respect system reduced motion and Tact's motion preference.
- Keep layout usable at 125% and 150% text size and at a constrained window.
- Every overlay has a keyboard dismissal path and restores focus.
- Strings are supplied by a desktop message catalog; no component-owned
  English label leaks into Chinese UI.
- Chinese labels require shorter alternatives for the rail and inspector.

## 16. Delivery Phases

1. **Spike** - prove GPUI + Tokio bridge, streaming, one mixed row list, and one
   permission gate.
2. **Core MVP** - session lifecycle, transcript, composer, tools, diff, gates,
   run inspector.
3. **Parity** - Goals, subagents, slash commands, sessions, MCP/Hooks/Plugins,
   stats, readers.
4. **Desktop finish** - themes, i18n, ANSI, Mermaid, voice, packaging,
   accessibility matrix.

## 17. Required Session Metadata Migration

The current `SessionSummary` and `sessions` schema have no title or archive
fields. Before session naming/archive parity ships:

- add `sessions.title TEXT NULL`;
- add `sessions.archived_at TIMESTAMP NULL`;
- expose both through `SessionSummary`;
- migrate list/query operations to filter by project, title, archive state and
  updated time;
- derive a fallback title from the first user prompt when `title` is null.

Until this migration lands, the prototype's naming and archive controls are
approved product requirements, not claims about current persistence.

## 18. Risks

- `gpui-ai` is pre-1.0 and must be pinned with `gpui-pre` and `gpui-component`.
- The heterogeneous transcript is the largest custom UI component and should
  be proven before feature breadth.
- High-frequency tool output can starve rendering if not batched.
- Hard-coded component strings and missing Mermaid/ANSI support may require
  local wrappers or upstream patches.
- Cross-platform packaging and accessibility must be tested on the real
  targets, not inferred from the browser gallery.
- Session title/archive requires the migration in section 17 before it can be
  called implementation-ready.

## 19. Acceptance Snapshot

The desktop is ready for implementation planning when:

- the three-pane prototype is reviewable in both themes and a narrow window;
- the mixed-row, gate, Goal, and Subagent flows are visually specified;
- the `tact_host` ownership boundary is accepted;
- the component mapping is accepted, including the custom transcript;
- the Command Center catalog covers every TUI slash command, CLI subcommand and
  user-facing setting entry;
- the P0 scope is small enough to prove the architecture before parity work.

## 20. Entry Coverage Matrix

| Source | Entry | Desktop destination |
|---|---|---|
| TUI | `/theme` | Appearance control or Command Center |
| TUI | `/model` | Main-model sheet |
| TUI | `/model-subagent` | Subagent-model sheet |
| TUI | `/permission` | Permission picker |
| TUI | `/view-system-prompt` | Prompt reader |
| TUI | `/save` | Save transcript command |
| TUI | `/compact` | Compact command |
| TUI | `/cancel` | Stop control |
| TUI | `/subagent_cancel` | Subagent actions |
| TUI | `/quit` | Window close / Command Center |
| TUI | `/help` | Keyboard help |
| TUI | `/history` | Task-history reader |
| TUI | `/skill` | Skills manager; `list`, `reload`, `enable`, `disable` |
| TUI | `/plugin` | Plugins manager; includes marketplace operations |
| TUI | `/mcp` | MCP manager; includes CLI get/add/remove/login/logout, `auth`, prompts and prompt execution |
| TUI | `/hooks` | Hook review and trust |
| TUI | `/hook-output` | Hook-output display toggle |
| TUI | `/balance` | Account balance detail |
| TUI | `/lang` | Language setting |
| TUI | `/stats` | Session-stats reader |
| TUI | `/tasks-dag` | Task graph reader |
| TUI | `/background` | Background-task inspector |
| TUI | `/goal` | Goals manager; includes every declared Goal verb |
| CLI | `headless` | Run-headless-task entry |
| CLI | `upgrade` | Check-for-updates and install-update entries |
| CLI | `plugin` | Same Plugins manager as `/plugin` |
| CLI | `mcp` | Same MCP manager as `/mcp` |
| CLI | `hooks` | Same Hook manager as `/hooks` |
| CLI | `--list-sessions` | Session list |
| CLI | `--session`, `--resume-last` | Session picker and Resume Last |
| CLI flags | provider, model, API key, base URL, max tokens | Provider and credentials |
| CLI flags | permission mode, thinking budget | Agent settings |
| CLI flags | context window, snapshot size, micro-compaction | Context settings |
| CLI flags | notifications | Notifications setting |
| CLI flags | theme | Appearance setting |
| CLI flags | config path | Configuration file reader |
| CLI flags | tokio-console | Diagnostics |
| CLI flags | skill-body auto-inject | Skills settings |
| Input | `@file`, image mention, attachment picker | Composer |
| Input | voice input | Composer voice control |
| Input | queued messages | Composer queue |
| Input | permission and `ask_user` | Gate and question flow |

Language has no CLI flag; it is the TUI `/lang` command and a desktop setting.
`--session` and `--resume-last` are navigation entries, not settings. The
desktop surfaces every entry above, but startup-global settings (provider,
credentials, token/context limits, config path, diagnostics) apply on the next
launch and are labeled as such; session settings (model, effort, thinking
budget, permission mode) apply immediately through `UserCommand`.
