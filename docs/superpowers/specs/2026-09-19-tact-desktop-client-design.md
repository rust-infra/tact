# Tact Desktop Client — Design Spec

Status: approved; v1 implemented through Phase 8 (Phase 3 deferred)
Date: 2026-09-19  
Scope: product and visual design for the `crates/tact-gui` desktop client. This
document stays the design source of truth; implementation evidence lives in the
delivery plan and the desktop design review.

## Decision summary

- **Recommended direction:** A — Continuous Workspace. A Claude-like transcript
  is the primary surface; a persistent left sidebar holds sessions; a right work
  pane is resizable and can be collapsed.
- **Visual language:** Anthropic warm neutrals and orange as the product accent,
  with Beautiful UI's geometry discipline: 6/8/10/14 radii, restrained borders,
  flat surfaces, and elevation only for overlays.
- **Primary font split:** Inter or the platform UI font for chrome and controls,
  Lora for assistant prose and editorial reading surfaces, JetBrains Mono for
  code, paths, diffs, and numeric data.
- **Top-level tabs:** `Chat`, `Agent`, and `Code` in one window. The selected tab
  changes the sidebar grouping and the default work pane; it does not create
  three unrelated applications.
- **v1 panes:** transcript, plan, diff, tasks, subagents, and files. Browser and
  terminal are intentionally deferred; their commands open the system browser
  or the preferred external terminal.
- **Architecture boundary:** `protocol` remains the shared contract. `tact-gui`
  consumes `AgentUpdate` and `UserCommand`; GPUI must not leak into the
  headless `tact`, `tact_llm`, or `protocol` crates.
- **Dock is not a v1 dependency.** Start with fixed shell regions and one
  resizable work pane. Introduce `Dock` only after the pane model and keyboard
  behavior are proven.

### Implementation status (2026-09-19)

- The production shell, Tact light/dark themes, transcript/composer, Plan, Diff,
  Tasks, Subagent, and Files panes, command palette, settings, and window-level
  keyboard contract are implemented in `crates/tact-gui`.
- The sidebar lists workspace-local sessions by stored or derived title with
  project/branch/age metadata and a status badge. Selecting a row resumes that
  id through the shared `tact-session` runtime, and `Ctrl+Tab` cycles sessions
  without a pointer. Session titles and the title-bar dropdown actions are
  implemented: rename persists in `sessions.title`, archive is a reversible
  `archived_at` flag, duplicate copies the conversation and opens the copy, and
  reveal opens the workspace in the platform file manager.
- The shared session driver and agent construction were extracted from
  `tact-ui` into the headless `crates/tact-session` crate; `tact-ui` re-exports
  the historical paths, so the TUI and GUI use one implementation.
- Phase 3 (Figma) remains deferred because the connector is unavailable. The
  clickable prototype and theme JSON remain the reviewed source of truth.
- Post-v1 layout persistence is explicitly parked: the v1 shell uses the fixed
  defaults below, and restoring sidebar/work-pane widths, selected pane, and
  transcript detail will be added with a layout settings store. Session search,
  grouping, pinning, and branch metadata remain post-v1; titles and the
  dropdown actions shipped after the initial v1 pass.

## 1. Product job

Tact's desktop client is a local-first workspace for long-running agent work.
The user should be able to:

- start or resume an agent session;
- steer work in natural language;
- see what the agent is doing without reading raw logs;
- approve or reject consequential actions;
- review diffs and tasks while the agent continues;
- understand token, model, permission, and background-task state;
- leave and return to work without losing the thread.

The primary task is **steer, observe, and verify**, not simply chat. The
transcript is the main object. Work panes expose the artifacts needed to trust
and verify the agent's result.

### Non-goals for v1

- Replacing an IDE, editor, browser, or terminal.
- Reproducing every Claude Desktop feature.
- A dashboard-first home screen or a grid of generic cards.
- Free-form Dock rearrangement before the core workflow is stable.
- Embedding a webview or a terminal emulator.

## 2. Inputs and constraints

### Reference inputs

- **Claude Desktop** supplies the interaction model: persistent sessions,
  continuous transcript, parallel work, diff review, pane switching, and a
  composer that keeps model and permission state close to the send action.
- **Beautiful UI** supplies the component geometry and token discipline:
  modern control radii, hairline borders, restrained elevation, compact
  toolbars, and clear hover/selection states.
- **Anthropic brand guidance** supplies the color and typographic direction:
  warm off-white, near-black ink, orange primary accent, blue and green as
  supporting accents.
- **GPUI Kit Design Guides** are normative for desktop behavior, tokens,
  keyboard access, focus, density, overlays, motion, copy, and accessibility.

### Source quality

The Beautiful UI values used here were normalized from the public CSS variables
already cached from the site, then checked by converting the source OKLCH values
to sRGB. This is a starter design system, not a claim that the whole public site
has been exhaustively reverse-engineered. The theme must be reviewed in a real
GPUI window before it is treated as final.

### Installed skill set

The following skills are installed and relevant to this work:

- `brand-guidelines`: Anthropic color and typography policy.
- `gpui-kit` and `gpui-kit-design-guides`: component/API choice, desktop
  behavior, token rules, interaction states, and review standards.
- `extract-design-system`: token extraction and starter-file workflow.
- `ui-ux-pro-max`: layout, accessibility, typography, color, and UX checks.
- `frontend-design`, `minimalist-ui`, `design-taste-frontend`: visual-direction
  critique and anti-template review.
- `prototype` and `web-artifacts-builder`: clickable high-fidelity prototype.
- `canvas-design` and `imagegen`: static presentation/reference renderings.
- `figma-use`, `figma-generate-design`, and
  `figma-create-design-system-rules`: eventual Figma source-of-truth workflow.

## 3. Design principles

1. **The transcript is the product.** Navigation and work panes support it; they
   must not compete with it for visual weight.
2. **The agent must be legible at a glance.** Every tool invocation has a stable
   row with state, object, result, and a path to detail.
3. **Trust is shown, not asserted.** Diff, plan, task, permission, and token
   state remain visible or one command away.
4. **Desktop before web.** Keyboard paths, resizable regions, persistent
   navigation, native menus, and focus ownership are requirements.
5. **Tokens before values.** Product colors, radii, spacing, and typography live
   in a theme or application token layer. Feature code reads semantic roles.
6. **Quiet surfaces, precise states.** Use hairlines and spacing before cards,
   shadows, or color. Reserve strong color for selection, state, and one primary
   action.
7. **Brand is a seasoning.** Anthropic warmth belongs in reading surfaces,
   typography, and the primary accent. It must not turn the application into a
   branded web page.

## 4. Layout directions

### Direction A — Continuous Workspace (recommended)

```text
┌──────────────────────── 44 px title bar ─────────────────────────┐
│ Chat | Agent | Code     session title          ⌘K  theme  settings │
├───────────────┬───────────────────────────┬──────────────────────┤
│ Sessions      │ Transcript                │ Work pane            │
│ 260 px        │ 680–760 px reading width  │ 320–720 px, resizable│
│               │                           │ Plan / Diff / Tasks  │
│ New session   │ messages, tool activity   │ Subagent / Files     │
│ search        │ thinking, diffs, plans    │                      │
├───────────────┴───────────────────────────┴──────────────────────┤
│ path · branch · permission · +12 −1 · CI · tokens · balance      │
└──────────────────────── 26 px status bar ────────────────────────┘
```

Why it wins:

- closest to the Claude desktop mental model;
- keeps the conversation readable at wide window sizes;
- lets verification panes stay open without turning the app into an IDE;
- affordable to implement with `Sidebar`, `Resizable`, `MessageScroller`,
  `Tabs`, and `StatusBar`.

Primary risk: the right pane can become a second product. Keep its initial tab
set small and make the transcript the default returning focus target.

### Direction B — Agent Console

```text
Sessions 240 | Transcript 520 | Activity / Workbench 520
```

The right side is a permanent multi-tab console, closer to an IDE. It is useful
for power users running several agents, diffs, and terminals at once. It costs
more window space, makes the transcript less dominant, and raises the
complexity of focus, resize, and session-to-pane ownership. Keep this as a
post-v1 layout preset, not the default.

### Direction C — Focus Canvas

The sidebar collapses to an icon rail and work panes become full-window tabs or
overlays. This produces the calmest reading experience and works well on
smaller laptops, but weakens multi-tasking and makes it easy to lose the
relationship between a conversation and its artifacts. Use it as a focus mode,
not the primary shell.

### Recommendation

Adopt **A** for v1. Preserve B's activity tabs as an optional future layout and
C's collapse behavior as a focus mode.

## 5. Window shell

### Dimensions and regions

| Region | Default | Minimum | Maximum | Surplus behavior |
|---|---:|---:|---:|---|
| Title bar | 44 px | 44 px | 44 px | fixed |
| Sidebar | 260 px | 220 px | 320 px | user-resizable; collapsible |
| Transcript | flexible | 480 px | unlimited | receive surplus, cap text measure |
| Work pane | 420 px | 320 px | 720 px | user-resizable; collapsible |
| Status bar | 26 px | 26 px | 26 px | fixed |

- Preferred window: 1440 × 900.
- Minimum supported window: 960 × 640.
- Transcript content measure: 680 px, maximum 760 px. Long prose never spans the
  full window.
- Target: persist sidebar width, work-pane width, selected pane, and last
  transcript detail level per user, then clamp restored values to the current
  window. The v1 implementation ships fixed defaults; this persistence is parked
  post-v1 rather than claimed as verified.

### Responsive behavior

| Window width | Shell behavior |
|---|---|
| ≥ 1280 px | sidebar + transcript + work pane |
| 960–1279 px | work pane becomes a right sheet/drawer; sidebar still visible |
| < 960 px | sidebar becomes an overlay; transcript keeps primary width |

At every width:

- the composer remains reachable without scrolling the whole window;
- the primary action is never hidden behind hover;
- only the transcript and data panes scroll;
- all commands remain reachable from the command palette or menu.

## 6. Surface structure

### 6.1 Title bar

- Three top-level tabs: `Chat`, `Agent`, `Code`.
- Session title with a dropdown for rename, duplicate, archive, and reveal in
  filesystem where applicable. Rename stores `sessions.title` (blank clears it);
  archive sets a reversible `sessions.archived_at` flag rather than deleting;
  duplicate copies only the conversation and opens the copy; reveal opens the
  workspace directory in the platform file manager.
- `⌘K` / `Ctrl+K` command palette trigger.
- Theme toggle.
- Settings.
- Platform window controls. Use the platform-native control placement and
  behavior; do not imitate macOS controls on Windows or Linux.

`Chat` favors conversation and quick questions. `Agent` exposes plans, tasks,
background runs, and subagents. `Code` opens the work pane on Diff/Files by
default and keeps repository state prominent. These are workspace presets over
the same session model, not separate data models.

### 6.2 Sidebar

Order from top to bottom:

1. `New session` primary command.
2. Search field.
3. Pinned/current sessions.
4. Grouped sessions by project or date.
5. `Worktrees` section when the current repository uses them.
6. `Background` tasks.
7. Footer: account, model balance, settings.

Session rows show title, project/branch hint, status dot, and token or diff
summary only when useful. Do not put a row of hover-only icons on every item;
use selection plus a context menu.

The shipped v1 row uses the stored or derived title as its label, with
`project · branch · age` metadata and a status badge; the short id is the
fallback when neither a name nor an opening message exists. That is enough to
resume and distinguish threads while search, grouping and pinning remain
post-v1.

### 6.3 Transcript

- User messages use a quiet filled bubble and align to the trailing edge.
- Assistant messages are unboxed prose with generous vertical rhythm.
- Assistant prose may use Lora; controls and metadata use the UI sans.
- Tool calls are compact inline rows with a status glyph, display name, object,
  duration, and expansion affordance.
- Thinking is collapsed by default and rendered as a muted editorial block.
- File edits show a compact `+12 −1` indicator that opens the diff pane.
- Errors are inline where they belong to a tool; fatal errors add a persistent
  notice near the composer.
- The transcript scrolls independently. When the user is at the tail, new
  content follows automatically. When the user scrolls away, show a
  `Jump to latest` affordance instead of stealing scroll position.

Transcript detail levels, cycled with `Ctrl+O`:

| Level | Shows |
|---|---|
| Normal | assistant text, user messages, tool summaries |
| Thinking | Normal + reasoning blocks |
| Verbose | Thinking + tool input, full output, file reads, intermediate steps |

### 6.4 Composer

The composer is a bottom-docked region within the transcript, not a floating
web chat bar.

- Multi-line input grows from one to eight lines, then scrolls internally.
- `+` menu: attachments, skills, connectors, plugins.
- `@` opens file/project mention search and inserts a removable chip.
- Attachment chips appear above the input and are removable by keyboard.
- Model, reasoning effort, thinking budget, and permission mode are visible as
  compact controls; advanced values open in a popover.
- Usage ring shows context-window consumption and opens a usage detail popover.
- When idle, the primary button is `Send`.
- When running, the primary button becomes `Stop`; a new `Enter` submits a
  queued instruction by default, displayed as a queued chip. `Stop` cancels the
  active turn through `UserCommand::Cancel`.
- Permission prompts never rely on the composer alone; they open a focused
  selection dialog and retain the pending request until answered or cancelled.

### 6.5 Work pane

Initial tabs:

| Tab | Contents | Primary actions |
|---|---|---|
| `Plan` | live plan steps and status | expand step, retry, open transcript row |
| `Diff` | file list + diff + review comments | stage decision, batch send review |
| `Tasks` | task table from `TasksChanged` | filter, sort, update, open session |
| `Subagent` | run list + selected transcript | cancel, inspect transcript |
| `Files` | project tree + preview | open, reveal, mention in composer |

The Plan row actions are live, not prototype chrome. Clicking a step expands
its input, result, and actions; a failed step shows the error state and offers
Retry; Open transcript expands the tool card that owns the step and scrolls to
it. Retry is submitted as a new agent instruction containing the recorded tool
and arguments, so the driver remains the only executor and can account for the
retry in the session history.

Subagent rows are live too. A running child exposes Cancel, which sends
`CancelSubagent { child_id }` through the driver's cooperative cancellation
path; Inspect transcript loads the child's stored session history on demand,
renders it under the run list, and toggles back to Hide transcript.

Deferred tabs: `Browser`, `Terminal`, `Chart`, and free-form `Dock`.

### 6.6 Status bar

Left to right:

- current workspace path or project name;
- git branch and worktree state;
- permission mode;
- live diff summary;
- CI/check summary when available;
- token usage;
- account balance;
- background-task count.

Status text is supporting information. It must not become a second navigation
bar. Clicking a status item opens the corresponding pane or popover and returns
focus on dismissal.

## 7. Visual language

### 7.1 Color

The product theme is Anthropic-first, not a literal copy of Beautiful UI. Keep
Beautiful UI's structural ratios and use its neutral roles as the fallback
system; use Anthropic's warm palette for the product identity.

| Role | Light | Dark | Use |
|---|---|---|---|
| page | `#FAF9F5` | `#141413` | window base |
| canvas | `#F3F1EA` | `#1B1A18` | sidebar, title bar, status bar |
| surface | `#FFFDF9` | `#23221F` | transcript cards, panes |
| inset | `#F7F5F0` | `#1E1D1B` | input wells, grouped rows |
| hover | `#F1EDE7` | `#2A2824` | hover and quiet selection |
| field | `#F5F3ED` | `#292723` | text fields |
| ink | `#141413` | `#FAF9F5` | primary text |
| ink-2 | `#5F5D57` | `#B0AEA5` | secondary text |
| ink-3 | `#747168` | `#858278` | metadata and disabled text |
| line | `#E8E6DC` | `#302E2A` | hairline boundaries |
| line-strong | `#D9D5C9` | `#403D37` | inputs and selected boundaries |
| accent | `#D97757` | `#D97757` | primary action, focus, selection |
| accent-ink | `#B8563A` | `#E58D70` | accent text on quiet surfaces |
| accent-tint | `#D977571F` | `#D9775729` | selected rows, highlight wash |
| blue | `#6A9BCC` | `#6A9BCC` | informational, links, hosted tools |
| green | `#788C5D` | `#8FA875` | success, clean checks |
| orange | `#D97757` | `#D97757` | warning or modified state |
| red | `#C64B4F` | `#E06C72` | destructive and failed state |

Rules:

- Do not use semantic colors as decoration.
- Never encode state by color alone; pair color with icon, text, or shape.
- Primary foreground on orange is near-black in both modes; verify contrast
  rather than assuming white text.
- Beautiful UI's blue system remains available as a secondary theme, but the
  default Tact theme uses Anthropic orange.

### 7.2 Typography

| Role | Family | Size/weight | Notes |
|---|---|---|---|
| UI chrome | Inter / platform UI | 12–14 px | menus, labels, controls, metadata |
| Section/display | Poppins only for marketing or a dedicated About surface | 18–24 px | do not use for dense controls |
| Assistant prose | Lora / Georgia | 15–16 px, 1.6 line height | editorial reading surface |
| Code and diff | JetBrains Mono / SFMono | 12–13 px | never use for ordinary prose |
| Numeric data | UI sans with tabular numerals | 12–14 px | align right where comparable |

CJK fallbacks must be explicit: use `PingFang SC`, `Noto Sans CJK SC`, or
`Microsoft YaHei` behind the UI sans. Do not size controls from English strings
alone; verify expanded Chinese and German labels.

### 7.3 Radius, spacing, elevation, motion

- Radii: chip 6, control 8, card/pane 10, window-level surface 14.
- Spacing base: 4 px; semantic steps 2, 4, 8, 12, 16, 24, 32.
- Borders: 1 px hairline; one boundary owner only.
- Elevation: base window and panes are flat. Popover, menu, dialog, sheet, and
  notification use the semantic popover/elevation treatment.
- Motion: 120–180 ms for state changes, 180–240 ms for panes/overlays.
  Use `cubic-bezier(.23, 1, .32, 1)` for entrances and
  `cubic-bezier(.16, 1, .3, 1)` for content reveal. Honor reduced motion.
- Streaming text may use `TextView::stream_fade`; do not add ambient animation
  to the transcript.
- Hover is feedback, never the only access to an action.

### 7.4 Icons and imagery

- Use one icon family throughout. Lucide or Phosphor are appropriate.
- Use icons as supplements to labels; icon-only controls require a tooltip and
  an accessible name.
- Do not use emoji as interface icons.
- Product imagery should be rare; the desktop app is primarily typographic and
  data-driven.

## 8. Component inventory and GPUI mapping

| Tact component | Purpose | GPUI Kit base | Custom work |
|---|---|---|---|
| App shell | window regions and resize | `TitleBar`, `Sidebar`, `Resizable`, `StatusBar` | shell state, breakpoints |
| Top tabs | Chat/Agent/Code | `Tabs`, `TabBar` | workspace preset routing |
| Session list | sessions/worktrees | `Sidebar`, `List`, `VirtualList` | grouping, status metadata |
| Command palette | global commands | `Command`, `CommandGroup` | Tact action registry |
| Transcript | streaming conversation | `MessageScroller`, `Message`, `TextView` | row model and tail behavior |
| User bubble | user message | `Bubble`, `MessageContent` | compact bubble policy |
| Assistant prose | markdown answer | `TextView::markdown().stream_fade()` | brand typography |
| Thinking block | reasoning lifecycle | `Collapsible`, custom row | stream lifecycle |
| Tool activity row | tool status/result | `Button`/disclosure + custom row | tool visual policy |
| Tool detail | output and arguments | `TextView`, `Textarea`, `Editor` | virtualization policy |
| Composer | prompt entry | `Textarea`, `Button`, `Popover` | growth, queue, mentions |
| Attachment chip | files/images | `Attachment` | remove and keyboard flow |
| Model controls | model/effort/budget | `Select`, `DropdownMenu`, `Popover` | session state |
| Permission picker | approval mode | `Select` / `Dialog` | request lifecycle |
| Usage ring | context usage | custom `canvas()` | ring plus popover |
| Plan pane | step list | `Tree`, `List` | live status |
| Diff pane | file list + diff | `Editor`, `List`, `Resizable` | semantic diff view |
| Tasks pane | task table | `DataTable` | snapshots from `TasksChanged` |
| Subagent pane | runs and transcript | `DataTable`/`List`, `TextView` | keep-live finalization |
| Files pane | project tree/preview | `Tree`, `Editor`, `TextView` | file picker integration |
| Settings | multi-section form | `Settings`, `Input`, `Select`, `Switch` | config mapping |
| Notifications | async status | `Notification` | no-decision events |
| Dialogs | approval/decision | `Dialog`, `AlertDialog` | request focus flow |
| Sheets/drawers | secondary work | `Sheet` | work pane at narrow widths |
| Tooltips | icon and shortcut help | `Tooltip` | copy and key notation |
| Menus | secondary commands | `DropdownMenu`, `ContextMenu` | action registry |
| Charts | usage/statistics only | `Chart`, `Plot` | defer unless data earns it |

### Components that must be custom

- `PromptComposer`: rich queue/mention/attachment behavior is not a stock
  textarea. Build on primitives; keep behavior out of render code.
- `UsageRing`: a small `canvas()` ring is cheaper and more precise than trying
  to compose it from progress bars.
- `ToolActivityRow`: use the protocol's `ToolPresentationInfo`; this is product
  policy, not a reusable base primitive.
- `DiffView`: GPUI Kit has editor and syntax highlighting, not a semantic diff
  viewer. Build a typed diff model and render it with theme highlight tokens.
- `SessionGroup`: grouping, archive, worktree, and status policy belong to the
  application.

## 9. Runtime event mapping

### AgentUpdate

| Event | UI destination |
|---|---|
| `StepAdded` | Plan pane + transcript placeholder row |
| `StepStarted` | tool row enters running state |
| `StepFinished` | tool row success, duration, result, diff indicator |
| `StepFailed` | tool row error with expandable recovery detail |
| `ToolProgress` | live expandable output in tool row/detail pane |
| `TaskComplete` | final transcript block, stop state, summary metadata |
| `TaskCancelled` | transcript system row, return composer to idle |
| `Error` | inline error or persistent notice depending on scope |
| `TokenUsage` | usage ring, status bar, usage popover |
| `TurnStats` | status bar/turn detail |
| `ModelInfo` | composer model chip and model popover |
| `Info` | compact system row |
| `MdInfo` | one-shot markdown system block |
| `SessionStats` | statistics sheet |
| `RequestSelect` | single-choice dialog; permission or `ask_user` |
| `RequestMultiSelect` | multi-select dialog |
| `StreamChunk` | append to assistant `TextView` |
| `ThinkingChunk` | thinking block lifecycle |
| `TasksChanged` | Tasks table snapshot |
| `ToolMeta` | tool row metadata update |
| `BackgroundTaskFinished` | finalize keep-live tool row |
| `SubagentFinished` | finalize subagent row and load transcript |
| `SubagentsChanged` | subagent run list/sticky status |

### UserCommand

| Command | UI origin |
|---|---|
| `SubmitTask` | composer Send |
| `Cancel` | composer Stop / Escape |
| `Compact` | command palette or session menu |
| `QueryBalance` | account/status surface |
| `QueryStats` | session statistics sheet |
| `QueryBackground` | background task pane |
| `SetPermissionMode` | permission control |
| `SetThinkingBudget` | model popover |
| `SetReasoningEffort` | model popover |
| `SetModel` | model picker |
| `SubagentFinishedNotification` | internal wake-up policy, no direct control |
| `CancelSubagent` | subagent row action |
| `McpAuth` | MCP settings action |
| `McpList` | MCP settings list |
| `UiResponse` | answers a pending selection dialog |

The protocol stays transport-agnostic. The desktop client must not add UI
handles, oneshot receivers, or GPUI entities to `AgentUpdate`.

## 10. Keyboard and interaction contract

Use `⌘` on macOS and `Ctrl` on Windows/Linux. The app must expose the platform
equivalent in menus and tooltips.

| Action | Shortcut | Notes |
|---|---|---|
| Command palette | `⌘K` / `Ctrl+K` | global |
| New session | `⌘N` / `Ctrl+N` | opens composer in new session |
| Send / queue | `Enter` | while running, queue by default |
| New line | `Shift+Enter` | never sends |
| Stop task | `Esc` | when no overlay is open |
| Toggle work pane | `⌘\` / `Ctrl+\` | returns focus to transcript on close |
| Focus composer | `⌘L` / `Ctrl+L` | browser convention is acceptable here |
| Transcript detail cycle | `Ctrl+O` | Normal → Thinking → Verbose |
| Toggle sidebar | `⌘B` / `Ctrl+B` | focus-safe collapse |
| Open diff | `⌘⇧D` / `Ctrl+Shift+D` | selects Diff work tab |
| Open tasks | `⌘⇧T` / `Ctrl+Shift+T` | selects Tasks work tab |
| Settings | `⌘,` / `Ctrl+,` | platform convention |
| Cycle sessions | `Ctrl+Tab` / `Ctrl+Shift+Tab` | only when focus is not in a text field |
| Dismiss overlay | `Esc` | topmost surface; restore focus to trigger |

Focus rules:

- A pane is not an island. Closing a pane returns focus to the command that
  opened it or the transcript row that owns the result.
- Focus rings remain visible at pane edges and inside overlays.
- A permission dialog owns focus until answered or dismissed.
- Keyboard shortcuts are discoverable through menus, tooltips, and the command
  palette; do not require memorization.

## 11. Required states

Every feature must design these states before implementation:

- Empty: explain the next action, do not show a blank pane.
- Loading: preserve layout, show progress in the object being loaded.
- Running: show phase, elapsed time, and the Stop path.
- Waiting for approval: block only the affected operation, keep context visible.
- Failed: name what failed and provide the next recovery action.
- Cancelled: distinguish user cancellation from failure.
- Offline/provider failure: show retry and preserve input.
- Read-only/permission denied: explain the boundary, do not present a disabled
  control with no reason.
- Long output: virtualize or progressively disclose; never freeze the window.
- Large diff: use file-level navigation and do not render the entire repository
  at once.

## 12. Accessibility and internationalization

- Every action is keyboard reachable; every focus change is visible.
- Icon-only controls have an accessible name and tooltip.
- Status uses text or shape in addition to color.
- Contrast meets 4.5:1 for normal text and 3:1 for large text and meaningful
  boundaries.
- Hit targets remain comfortable at the compact density.
- Reduced motion removes non-essential transitions while preserving state.
- User content, translations, and zoom must not clip controls or pane headers.
- Keep the layout stable for CJK and RTL expansion; do not concatenate
  translated sentence fragments.
- Use sentence case for English UI copy and natural Chinese copy, not literal
  translations of English word order.

## 13. Implementation boundary

- New crates: `crates/tact-session` (headless shared runtime) and
  `crates/tact-gui` (desktop presentation).
- `tact-gui` depends on `tact-session` and `gpui-kit`; `tact-session` is the
  headless bridge to `tact` and `protocol`.
- The session driver, account poller, user-message rendering, and agent builder
  moved from `tact-ui` into `tact-session`; `tact-ui` re-exports them at their
  historical module paths so no downstream behavior forks.
- `tact`, `tact_llm`, `protocol`, and `tact-session` remain headless and must
  not depend on GPUI.
- `tui` and `tact-ui` remain ratatui-specific and are not reused as GUI widgets.
- The prototype's `surface` token maps to gpui-kit `popover`, because the
  built-in `ThemeColor` API has no `surface` role; the prototype's `red`
  destructive role maps to `danger`.
- The GUI owns application state, pane layout, focus, and presentation policy.
- The protocol owns transport-neutral events and commands.
- Theme JSON is data, not a place for application-only behavior.
- Custom rendering such as `UsageRing` and `DiffView` must read semantic theme
  roles rather than hard-coded colors.

## 13.1 Prototype artifact and verification

The clickable prototype lives at `docs/design/tact-desktop-prototype.html`
(self-contained; no network, no build step). Open it directly in a browser.

Controls:

- Top-right the light/dark toggle switches `Tact Anthropic Light` /
  `Tact Anthropic Dark`; `?theme=light|dark` preselects a theme.
- Work pane tabs switch Plan, Diff, Tasks, and Files; `?pane=plan|diff|tasks|files`
  preselects a pane.
- `Cmd/Ctrl+K` opens the command palette; `?cmd=1` opens it on load.
- The production shell collapses the work pane to a drawer below 1280 px and
  turns the session sidebar into an overlay below 960 px; the prototype's
  1100 px and 860 px captures demonstrate those states.

Verified states and renderings:

| Check | Result |
|---|---|
| Transcript with streaming message, thinking block, tool row, code block | verified |
| Plan, Diff, Tasks, Files panes | verified in both themes |
| Permission/approval card and toast | verified |
| 1440 x 900 default shell | verified, light and dark |
| 1100 x 800 work-pane drawer | verified |
| 860 x 700 sidebar overlay | verified |
| `prefers-reduced-motion` | honored; animations removed |

Five defects were found and fixed during verification:

- The diff line's code cell reused `.code` (the transcript code-block wrapper)
  and the diff marker column reused `.mark` (the app-logo swatch), so both
  inherited unrelated box styling. The columns are now `.dcode` and `.dmark`,
  and long diff lines scroll horizontally instead of clipping.
- Under `prefers-reduced-motion` the command palette computed to `opacity: 0`
  and was invisible, because the duration was clamped to 1 ms while the pop
  keyframes start transparent. Animated regions now disable animation entirely
  under reduced motion.
- The prototype had no `:focus-visible` styling, so it could not demonstrate the
  keyboard focus the accessibility section requires. An accent focus ring is now
  defined, with a tighter offset inside overlays and the work pane.
- Palette focus was not restored to its trigger and Tab could escape the
  overlay. Focus restoration, a Tab trap, and dialog semantics were added.
- Eight contrast pairs fell below 4.5:1. Tokens were re-solved until all 40
  measured pairs pass in both themes; the corrected values are carried by both
  this spec's theme file and the prototype.

The full checklist run, with per-item verdicts, deviations, and parked items,
is in `docs/design/tact-desktop-design-review.md`.

Static reference renderings:

- `docs/design/tact-desktop-prototype.light.png`
- `docs/design/tact-desktop-prototype.dark.png`
- `docs/design/tact-desktop-prototype.diff.light.png`
- `docs/design/tact-desktop-prototype.diff.dark.png`
- `docs/design/tact-desktop-prototype.palette.light.png`
- `docs/design/tact-desktop-prototype.palette.dark.png`

Renders are captured with `--force-prefers-reduced-motion` (deterministic, and
it exercises the reduced-motion path) and `--hide-scrollbars` (avoids a headless
race where a late scrollbar changes layout between captures).

## 14. Delivery sequence

1. Completed: confirm this design direction and the three workspace tabs.
2. Completed: build the clickable 1440 × 900 prototype with light/dark modes and
   realistic transcript, tool rows, diff, tasks, and composer states.
3. Completed: review the prototype against the GPUI Kit design review checklist.
4. Completed: produce static reference renderings for documentation and review.
5. Deferred: translate the approved prototype into a Figma source of truth when
   the Figma connector is available.
6. Completed: bootstrap `crates/tact-gui` with the theme registry and shell.
7. Completed: implement the transcript, composer, session resume, and event
   mapping.
8. Completed: add Plan, Diff, Tasks, Subagent, and Files panes.
9. Completed: add settings, command palette, notifications, keyboard flows, and
   accessibility polish.
10. Post-v1: add Dock/layout presets and persisted layout state after the v1
    shell is stable.

## 15. Approval points

The defaults below were approved and are carried by the implementation:

- Direction A, Continuous Workspace.
- Top tabs `Chat`, `Agent`, `Code`.
- Anthropic orange as the primary accent.
- Lora for assistant prose; Inter/platform font for UI chrome.
- Browser and Terminal deferred to v1.1.
- Fixed shell with one work pane; Dock and persisted/resizable layout are
  post-v1.

## References

- [Beautiful UI](https://www.beautifului.dev/)
- [GPUI Kit](https://gpui-kit.com/)
- [GPUI Kit Design Guides](https://gpui-kit.com/docs/design-guides)
- [Claude Desktop documentation](https://code.claude.com/docs/en/desktop)
- [Anthropic brand colors and typography](https://www.anthropic.com/)
- `docs/design/tact-desktop-theme.json`
- `docs/design/tact-desktop-prototype.html`
- `docs/design/tact-desktop-design-review.md`
- `docs/superpowers/plans/2026-09-19-tact-desktop-client.md`
