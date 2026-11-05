# gpui-ai Component Coverage Matrix

> Historical: `gpui-ai` was removed on 2026-11-05; this matrix now records the
> component mapping that the Tact-owned `ui::*` implementations replaced.

Reference: `gpui-ai` v0.8.0 component catalog and exported `prelude`.

Status values:

- **Direct**: the prototype/PRD uses the component for the same job.
- **Compose**: the component is used inside a Tact-owned shell or adapter.
- **Deferred**: relevant to Tact, but outside the current desktop release.
- **N/A**: not a Tact product function.

| Component | Status | Tact surface / decision |
|---|---|---|
| `LoadingState` | Compose | Compaction/background progress; Tact owns the surrounding result sheet. |
| `StatusBadge` | Direct | Tool, goal, task and subagent states. |
| `Thinking` | Direct | Reasoning traces with Markdown. |
| `Orbs` | Direct | Subagent/background activity signal. |
| `ContextMeter` | Direct | Inspector context tab. |
| `ToolChip` | Direct | Latest-tool lifecycle chip in the Run inspector. |
| `ToolCall` | Direct | Tool input/output card; permission gates remain separate. |
| `TaskRow` | Direct | Sticky task rows. |
| `SearchResults` | Direct | Web-search tool reader: query, title and source domain; selecting a hit opens it. |
| `TodoList` | Direct | Task checklist in the Run inspector. |
| `ImageGeneration` | N/A | Tact is a coding agent; image generation is not a product tool. |
| `StreamingText` | Direct | Assistant Markdown stream. |
| `CodeBlock` | Direct | Terminal output and code evidence. |
| `CodeDiff` | Direct | Single-file patch review. |
| `SelectionActions` | Compose | Selectable Markdown with a floating Ask / Explain / Rewrite toolbar; click, drag, and keyboard selections are supported. |
| `Chat` | N/A | Its message model cannot interleave tools, reasoning, diffs and gates. |
| `Suggestions` | Direct | Follow-up prompt chips after an answer. |
| `AttachmentStrip` | Direct | Composer attachments. |
| `ArtifactPanel` | Direct | A write/edit tool body opens with Preview / Source switching. |
| `FineTuneCard` | N/A | Design-property tuning is not a Tact agent workflow. |
| `PromptBar` | Compose | Native editor/model/mentions/attachments; Tact adds the nested slash overlay. |
| `VoiceControls` | Direct | Composer microphone when the voice worker is configured; inserts the transcript on stop. |
| `MessageQueue` | Direct | Queued prompts with send/remove/clear. |
| `CommandSearch` | Direct | Flat command palette with group metadata in subtitle/keywords. |
| `SidebarNav` | Compose | Registered-project sessions grouped by project, with the app-owned filter and lock badges. |
| `ThreadList` | Deferred | Insufficient for project-grouped fuzzy/filter manager; not claimed for that surface. |
| `RecordsTable` | N/A | Historical only; the Tact-owned `FilterTable`/`DiffTable` render their own rows. |
| `DiffTable` | Direct | A patch touching several files opens a scrollable per-file table with localized accept/reject decisions. |
| `FilterTable` | Compose | Extensions Manager rows and filters; single click selects, double click or Open activates. |
| `ComparisonTable` | N/A | No bounded feature-matrix workflow in Tact. |
| `ApprovalCard` | Compose | Permission gates keep its one-click semantics; the visual shell is Tact-owned (P15) because the runtime offers four permission choices. |
| `PlanCard` | Compose | Goals criteria use the plan-step grammar; the transcript plan card is Tact-owned so it can carry row identity. |
| `RecommendationCard` | Deferred | No current recommendation decision workflow. |
| `ContextCard` | Direct | Retained hook/tool evidence in the Context inspector. |
| `InsightCard` | Compose | Stats and balance summaries. |
| `ChoiceGroup` | N/A | Historical only; the Tact-owned `QuestionFlow` renders its own option rows. |
| `QuestionFlow` | Direct | Single-question Ask gate card; multi-select uses `Toggle`. |

## Tact-owned surfaces

The desktop now uses `gpui-component` directly; every surface below is
Tact-owned:

- `LoadingState`, `ToolChip`, `Suggestions`, `ContextCard`, `ContextMeter`,
  `Toggle`, `AttachmentStrip` and `Orbs` live in
  `crates/desktop/src/ui/leaf.rs`.
- `CommandSearch` lives in `crates/desktop/src/ui/command_search.rs`.
- `CodeBlock`, `CodeDiff`, `SearchResults`, `ArtifactPanel`, `InsightCard`
  and `MessageQueue` live in
  `crates/desktop/src/ui/reader_components.rs`.
- `StreamedContent`, `StreamingText`, `Thinking`, `ToolCall` and `ToolGroup`
  live in `crates/desktop/src/ui/transcript_components.rs`.
- `PromptBar` and its model/mention/attachment surface live in
  `crates/desktop/src/ui/prompt_bar.rs`.
- `SidebarNav` and its sections/items live in
  `crates/desktop/src/ui/sidebar_nav.rs`.
- `SelectionActions`, `PlanCard`, `QuestionFlow`, `FilterTable` and `DiffTable`
  live in `selection_actions.rs`, `plan_card.rs`, `question_flow.rs`,
  `filter_table.rs` and `diff_table.rs`.

No `gpui-ai` dependency or vendored fork remains; `gpui_component::init(cx)`
is the only UI initialization.

## Unused Components That Reveal Product Gaps

The following are not silently ignored:

- `RecommendationCard`: no decision/recommendation workflow exists.

`SearchResults`, `ArtifactPanel`, `DiffTable` and `ContextCard` were P2
candidates and are now mounted by the readers and the Context inspector
(captures `41`–`44` and `31`).
