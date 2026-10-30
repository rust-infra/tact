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
| `ToolChip` | Direct | Compact tool lifecycle in subagent/tool groups. |
| `ToolCall` | Direct | Tool input/output/approval card. |
| `TaskRow` | Direct | Sticky task rows. |
| `SearchResults` | Deferred | Hosted web search currently injects context without a result reader. Candidate P2 evidence surface. |
| `TodoList` | Direct | Sticky task checklist. |
| `ImageGeneration` | N/A | Tact is a coding agent; image generation is not a product tool. |
| `StreamingText` | Direct | Assistant Markdown stream. |
| `CodeBlock` | Direct | Terminal output and code evidence. |
| `CodeDiff` | Direct | Single-file patch review. |
| `SelectionActions` | Compose | Ask / Explain / Rewrite toolbar for selected Markdown. |
| `Chat` | N/A | Its message model cannot interleave tools, reasoning, diffs and gates. |
| `Suggestions` | Direct | Follow-up prompt chips after an answer. |
| `AttachmentStrip` | Direct | Composer attachments. |
| `ArtifactPanel` | Deferred | Generated documents/code artifacts are not yet a Tact workflow. Candidate P2 panel. |
| `FineTuneCard` | N/A | Design-property tuning is not a Tact agent workflow. |
| `PromptBar` | Compose | Native editor/model/mentions/attachments; Tact adds the nested slash overlay. |
| `VoiceControls` | Direct | Composer dictation state (desktop-finish scope). |
| `MessageQueue` | Direct | Queued prompts with send/remove/clear. |
| `CommandSearch` | Direct | Flat command palette with group metadata in subtitle/keywords. |
| `SidebarNav` | Compose | Project pane in the Session Manager. |
| `ThreadList` | Deferred | Insufficient for project-scoped fuzzy/filter manager; not claimed for that surface. |
| `RecordsTable` | Compose | Session rows and task-history rows. |
| `DiffTable` | Deferred | Multi-file proposal review; `CodeDiff` covers the current single-file flow. |
| `FilterTable` | Compose | Extensions Manager rows and filters. |
| `ComparisonTable` | N/A | No bounded feature-matrix workflow in Tact. |
| `ApprovalCard` | Direct | Permission and destructive-action gates. |
| `PlanCard` | Direct | Proposed execution steps. |
| `RecommendationCard` | Deferred | No current recommendation decision workflow. |
| `ContextCard` | Deferred | Could render retained evidence in a future compact evidence panel. |
| `InsightCard` | Compose | Stats and balance summaries. |
| `ChoiceGroup` | Direct | Single-choice clarification. |
| `QuestionFlow` | Direct | Multi-question clarification sequence; multi-select uses `Toggle`. |

## Unused Components That Reveal Product Gaps

The following are not silently ignored:

- `SearchResults`: web-search evidence has no dedicated reader today.
- `ArtifactPanel`: generated artifacts have no inspect/preview surface.
- `DiffTable`: multi-file change review is not covered by the single-file diff.
- `RecommendationCard`: no decision/recommendation workflow exists.
- `ContextCard`: retained evidence is currently rendered inside tool cards only.

These are recorded as P2 candidates unless the product scope adds them earlier.
