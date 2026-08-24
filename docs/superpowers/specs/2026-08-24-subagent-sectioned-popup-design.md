# Subagent Popup Sectioned Rendering Design

- **Date:** 2026-08-24
- **Status:** Pending approval
- **Related:** `crates/protocol/src/tool_output.rs`; `crates/tact/src/tool/subagent_ui.rs`; `crates/tact/src/tool/subagent.rs`; `crates/agent_tui_kit/src/state/tool_state.rs`; `crates/agent_tui_kit/src/components/tool.rs`; `crates/agent_tui_kit/src/widgets/tool_widget.rs`; `crates/agent_tui_kit/src/render/popups/subagent_popup.rs`; `crates/tui/src/render/popup_scene_tests.rs`

## Goal

Redesign the subagent popup so its content is organized into three stacked sections with headers — **🧠 Thinking**, **🔧 Tools**, **📄 Context** — instead of one flat transcript. The whole document scrolls as a single unit inside the existing popup chrome.

## Scope

### In scope

- Tag every subagent `ToolProgress` chunk with the section it belongs to: thinking blocks → Thinking, tool step lines (start / result / error) → Tools, streamed assistant text + info + errors + the initial prompt → Context.
- Accumulate structured section blocks in `ToolOutputBuffer` alongside the existing flat text (the flat text stays byte-identical so the collapsed tool card preview in the main log is untouched).
- Persist the structured blocks through the live→completed transition into `ToolRenderOutput::detail_sections`.
- Render the subagent popup as three stacked sections with headers; live content stays plain wrapped lines, completed content goes through the same width-aware Markdown pipeline as today.
- Include the subagent prompt (already stored as the card's `arg_full`) at the top of the Context section.
- Keep the fallback path for completed blocks that carry no sections (older transcripts): render the whole `detail_full` as before, without section headers.
- Render section headers only when the document has at least two non-empty sections (a run that only streamed text keeps today's look).

### Out of scope

- Changing the collapsed tool card in the main log (title, meta row, live preview, double-click open).
- Changing popup chrome, mouse hit rows, text selection, or `y`-copy semantics (copy continues to operate on the visible sectioned text).
- Changing subagent spawning, the tagged UI channel's permission-select pass-through, or `ToolMeta` handling.
- Adding tabs / switching between sections; the sections are stacked and scroll together.

## Current Context

`tagged_ui_channel_with_progress` (crates/tact/src/tool/subagent_ui.rs) flattens every subagent `AgentUpdate` into `ToolProgress` chunks: thinking summaries are prefixed with `🧠 Thinking`, tool steps become `→ tool args` / `✓ result` / `✗ error` lines, and streamed text / info / errors are appended as-is. The parent tool card's `ToolOutputBuffer` accumulates all of it into one flat text (`full_detail`), which feeds both the card's rolling preview and — on double-click — the subagent popup. The popup renders that flat text: plain wrapped lines while live, Markdown after completion (via `render_markdown_with_tables` + `markdown_plan` decoration).

Because the structure is destroyed at the transport boundary, the popup cannot distinguish thinking from tool calls from context. This design restores that structure without changing the flat text that drives the card.

## Design

### 1. Protocol: section tag on chunks

In `crates/protocol/src/tool_output.rs`:

```rust
/// Which section of a subagent transcript a chunk belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubagentSection {
    #[default]
    Context,
    Thinking,
    Tool,
}

pub struct ToolOutputChunk {
    pub stream: ToolOutputStream,
    pub text: String,
    /// Section tag consumed by the subagent popup; the flat card stream
    /// ignores it. Non-subagent chunks keep the default (Context).
    pub section: SubagentSection,
}
```

- Existing constructors `stdout` / `stderr` / `other` keep the default section (Context), so bash/read_file/etc. producers are unaffected.
- Add `with_section(self, SubagentSection) -> Self` so the forwarder can tag chunks without new constructors.
- Add `pub const THINKING_SECTION_HEADER: &str = "🧠 Thinking";` — the shared marker the forwarder prefixes to every thinking block and the sectioned popup strips before rendering its own header. It lives here so both sides read the same string.

### 2. Buffer: structured blocks alongside flat text

`ToolOutputBuffer` gains:

```rust
pub struct SubagentSectionBlock {
    pub section: SubagentSection,
    pub text: String,
}

// field: sections: Vec<SubagentSectionBlock>
```

- `push_chunks` runs the existing per-char ANSI filter first (same state machine, same order), then appends the filtered text to the current section block — merging into the last block when the section matches, otherwise pushing a new block. Flat `detail` / `full_detail` accumulation is unchanged.
- Add `sections(&self) -> &[SubagentSectionBlock]` and `take_sections(&mut self) -> Vec<SubagentSectionBlock>` (drain, used by the live→completed handoff).

### 3. Forwarder: tag chunks by source event

`tagged_ui_channel_with_progress` keeps emitting the exact same text (card preview unchanged) but tags each chunk:

- Thinking `Finished` summary → `.with_section(SubagentSection::Thinking)`
- `StepStarted` line, `StepFinished` preview, `StepFailed` error → `.with_section(SubagentSection::Tool)` (stderr stream kept for StepFailed so the card keeps its red styling)
- `StreamChunk`, `Info`, `Error` → default Context

### 4. Live→completed handoff

In `components/tool.rs::on_step_finished`, the subagent branch already calls `active.live_output.take_full_detail()` into `output.detail_full`. Add:

```rust
output.detail_sections = Some(active.live_output.take_sections());
```

`ToolRenderOutput` gains `pub detail_sections: Option<Vec<SubagentSectionBlock>>` (builder defaults to `None`; the three struct literals in tests/helpers gain the field).

### 5. Popup: compose the sectioned document

`prepare_subagent_popup` (crates/agent_tui_kit/src/render/popups/subagent_popup.rs) changes its source:

- **Live:** sections from `live_output.sections()`, prompt from the active block's `arg_full`.
- **Completed:** sections from `output.detail_sections`; when missing/empty, fall back to `detail_full` as a single Context block (legacy transcripts).
- Prompt (`arg_full`) is prepended to the Context section as a labeled block: `**Prompt:**` (Markdown mode) / `Prompt:` bold label (live mode).

Then it builds the display document:

- **Group** blocks into canonical order — Thinking, Tools, Context — keeping only non-empty sections. Strip the shared `🧠 Thinking` marker from the front of each thinking block (the popup renders its own single section header).
- **Indent** every thinking-body line 4 columns: plain spaces in live mode, non-breaking spaces (`\u{00A0}`) in the completed Markdown pass so CommonMark does not reinterpret the 4-space prefix as an indented code block (verified against the kit's pulldown renderer).
- **Live mode:** plain `Line`s; each section header rendered bold in `theme.heading`; body lines plain; blank rows between sections. No header row when the document has fewer than two sections.
- **Completed mode:** build a Markdown source with `## <header>` per section and feed it through `render_markdown_with_tables` + `markdown_plan::decorate_headings` (the existing width-aware pipeline; geometry is computed before chrome, as today).

Layout cache, scroll, selection, mouse hit rows, and the render loop stay as they are — the sectioned document is just a different `display_text`/`styled_lines` input to `plan_markdown_display`.

### 6. Card preview stays identical

The flat text stream (`detail`, `full_detail`, `preview_lines`) is untouched; only the chunk struct grows a tag the card never reads.

## Interaction and Data Flow

```text
subagent AgentUpdate (ThinkingChunk / Step* / StreamChunk / Info / Error)
  → tagged_ui_channel_with_progress: same text, now tagged with SubagentSection
  → ToolProgress { chunks }
  → ToolComponent.on_tool_progress → ToolOutputBuffer::push_chunks
       ├─ flat path (unchanged): preview_lines / detail / full_detail  → card
       └─ sections path (new): Vec<SubagentSectionBlock>
  → on_step_finished (subagent): take_full_detail → detail_full
                                 take_sections  → detail_sections
  → subagent popup prepare: group sections (Thinking/Tools/Context) + prompt
       → live: plain lines with styled headers
       → completed: "## headers" markdown → render_markdown_with_tables
  → plan_markdown_display → PopupLayoutCache → render loop (unchanged)
```

## Testing

- Protocol: `ToolOutputChunk` section tag round-trips; `ToolOutputBuffer::push_chunks` merges same-section chunks and opens new blocks on section change; ANSI stripping applies to section text too; `take_sections` drains.
- Forwarder (`subagent_ui.rs`): thinking chunks carry `Thinking`, step started/finished/failed carry `Tool`, streams/info carry `Context`; text content stays byte-identical to today.
- Component (`tool.rs`): a finished subagent block carries `detail_sections` equal to the live buffer's sections; non-subagent blocks keep `None`.
- Popup rendering (`popup_scene_tests.rs`, buffer-level):
  - A completed popup with thinking + tool + context blocks shows all three headers in order and the section bodies.
  - The prompt appears under the Context header.
  - A single-section transcript renders without headers (legacy look).
  - Live mode renders section headers as styled lines.
  - Existing tests (heading `#` markers, code-tail fills, ordered-list spacers, wide tables) keep passing via the `detail_full` fallback path.
- Run focused TUI tests, then the repository formatting, Clippy, and package test gates (single cargo invocation at a time, proxy env unset per AGENTS.md).

## Documentation

- New design spec + implementation plan under `docs/superpowers/` (this file and its plan).
- Append a newest-first `2026-08-24` entry to `book/26_chapter_issue.md` and `book/26_chapter_issue_zh.md` (same heading hierarchy) describing the sectioned subagent popup.
- Update `book/23_chapter_tui*.md` if the popup list / description mentions the subagent transcript rendering; otherwise leave the chapter-level descriptions intact.
