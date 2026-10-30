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
- Ran the prototype in Chromium at 1440x900 and 390x844; exercised all 81
  command-palette entries, all 46 audited slash-command paths, queue
  send/remove/clear, attachment add/remove, voice toggle, follow-up chips and
  selection actions with no page errors.
- Re-ran `node --check docs/design/tact-desktop/prototype.js` and
  `python3 scripts/book-check.py book`.
