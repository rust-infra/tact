# gpui-ai Component Coverage Matrix

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
| `ToolCall` | Direct | Tool input/output/approval card. |
| `TaskRow` | Direct | Sticky task rows. |
| `SearchResults` | Direct | Web-search tool reader: query, title and source domain; selecting a hit opens it. |
| `TodoList` | Direct | Task checklist in the Run inspector. |
| `ImageGeneration` | N/A | Tact is a coding agent; image generation is not a product tool. |
| `StreamingText` | Direct | Assistant Markdown stream. |
| `CodeBlock` | Direct | Terminal output and code evidence. |
| `CodeDiff` | Direct | Single-file patch review. |
| `SelectionActions` | Compose | Ask / Explain / Rewrite toolbar for selected Markdown. |
| `Chat` | N/A | Its message model cannot interleave tools, reasoning, diffs and gates. |
| `Suggestions` | Direct | Follow-up prompt chips after an answer. |
| `AttachmentStrip` | Direct | Composer attachments. |
| `ArtifactPanel` | Direct | A write/edit tool body opens with Preview / Source switching. |
| `FineTuneCard` | N/A | Design-property tuning is not a Tact agent workflow. |
| `PromptBar` | Compose | Native editor/model/mentions/attachments; Tact adds the nested slash overlay. |
| `VoiceControls` | Direct | Composer microphone when the voice worker is configured; inserts the transcript on stop. |
| `MessageQueue` | Direct | Queued prompts with send/remove/clear. |
| `CommandSearch` | Direct | Flat command palette with group metadata in subtitle/keywords. |
| `SidebarNav` | Compose | Project-scoped session rail with the app-owned filter and lock badges. |
| `ThreadList` | Deferred | Insufficient for project-scoped fuzzy/filter manager; not claimed for that surface. |
| `RecordsTable` | Compose | Used through `FilterTable`; direct session rows remain Tact-owned. |
| `DiffTable` | Direct | A patch touching several files opens a per-file table with accept/reject decisions. |
| `FilterTable` | Compose | Extensions Manager rows and filters. |
| `ComparisonTable` | N/A | No bounded feature-matrix workflow in Tact. |
| `ApprovalCard` | Direct | Permission and destructive-action gates. |
| `PlanCard` | Compose | Goals criteria use the plan-step grammar; the transcript plan card is Tact-owned so it can carry row identity. |
| `RecommendationCard` | Deferred | No current recommendation decision workflow. |
| `ContextCard` | Direct | Retained hook/tool evidence in the Context inspector. |
| `InsightCard` | Compose | Stats and balance summaries. |
| `ChoiceGroup` | Deferred | Single-choice gates use `QuestionFlow`'s option grammar; `ChoiceGroup` is the compact inline candidate once gates lose their confirm step. |
| `QuestionFlow` | Direct | Multi-question clarification sequence; multi-select uses `Toggle`. |

## Unused Components That Reveal Product Gaps

The following are not silently ignored:

- `RecommendationCard`: no decision/recommendation workflow exists.

`SearchResults`, `ArtifactPanel`, `DiffTable` and `ContextCard` were P2
candidates and are now mounted by the readers and the Context inspector
(captures `41`–`44` and `31`).
