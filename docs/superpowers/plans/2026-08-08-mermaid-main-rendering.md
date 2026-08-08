# Mermaid Main-Area Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render supported Mermaid fenced blocks as themed terminal diagrams in the TUI main area while preserving existing Markdown, code-card, scrolling, and fallback behavior.

**Architecture:** Keep `tui-markdown` as the normal Markdown renderer. Add a narrow Mermaid renderer in `render_md.rs` that extracts complete top-level `mermaid` fences, calls the already-enabled `ratatui-markdown::mermaid::render_mermaid`, and falls back to the existing renderer when parsing fails. Reuse the width-aware `MarkdownCell` path for content that must re-layout and add a Mermaid flag to the streaming fence state so valid completed diagrams do not become `CodeBlock` overlays.

**Tech Stack:** Rust, ratatui `Line`, existing `tui-markdown`, existing `ratatui-markdown` Mermaid feature, TUI unit/render-gap tests.

## Global Constraints

- Do not add a dependency or a new Mermaid parser.
- Supported diagram types are limited to those implemented by the pinned `ratatui-markdown` revision.
- Ordinary Markdown, tables, headings, and non-Mermaid code cards must retain their current output and behavior.
- Invalid or unsupported Mermaid must remain readable through the existing code rendering path.
- Run Cargo commands serially; unset `http_proxy`, `https_proxy`, and `all_proxy` for tests.
- Follow TDD: each production change is preceded by a focused failing test.
- Update `book/23_chapter_tui.md`, `book/23_chapter_tui_zh.md`, `book/26_chapter_issue.md`, and `book/26_chapter_issue_zh.md` in the same change because the main-area rendering contract is user-visible.

---

## File Map

- Modify `crates/tui/src/render/render_md.rs`: shared `RichTextTheme` adapter, Mermaid block renderer, and Markdown/fence routing tests.
- Modify `crates/tui/src/widgets/state/task_dag.rs`: reuse the shared theme adapter without changing DAG output.
- Modify `crates/tui/src/render/cells/markdown.rs`: ensure the width-aware whole-Markdown cell uses the Mermaid-aware router and add cell rendering tests.
- Modify `crates/tui/src/widgets/state/stream_state.rs`: record whether the currently buffered fence is Mermaid.
- Modify `crates/tui/src/widgets/state/app/agent.rs`: route streaming Mermaid completion to diagram lines or code fallback.
- Modify `crates/tui/src/widgets/state/app/visibility.rs`: route incomplete-stream flush through the same Mermaid/code finalization behavior.
- Modify `crates/tui/src/render/render_gap_tests.rs`: main-area streamed Mermaid and fallback regressions.
- Modify `book/23_chapter_tui.md` and `book/23_chapter_tui_zh.md`: describe Mermaid fenced-block behavior and fallback.
- Modify `book/26_chapter_issue.md` and `book/26_chapter_issue_zh.md`: add the newest user-visible behavior entry.
- Create `docs/superpowers/plans/2026-08-08-mermaid-main-rendering.md`: this plan.

---

### Task 1: Add the shared Mermaid theme adapter and renderer

**Files:**
- Modify: `crates/tui/src/render/render_md.rs`
- Modify: `crates/tui/src/widgets/state/task_dag.rs`

**Interfaces:**
- Produce `pub(crate) fn render_mermaid_block(source: &str, theme: &Theme, width: usize) -> Option<Vec<Line<'static>>>`.
- Produce a shared `RichTextTheme` implementation in `render_md.rs` that maps the existing `Theme` fields to the colors required by `ratatui-markdown`.
- Consume the existing `ratatui_markdown::mermaid::render_mermaid` API.

- [ ] **Step 1: Write the failing unit test for a sequence diagram.**

Add a test beside the existing `render_markdown_*` tests:

```rust
#[test]
fn render_mermaid_sequence_returns_terminal_lines() {
    let source = "sequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi";
    let lines = render_mermaid_block(source, &theme(), 80).expect("valid sequence diagram");
    let text = lines.iter().map(Line::to_string).collect::<Vec<_>>().join("\n");

    assert!(text.contains("Alice"), "participant missing: {text}");
    assert!(text.contains("Bob"), "participant missing: {text}");
    assert!(text.contains("Hello"), "message missing: {text}");
    assert!(text.contains('─') || text.contains('>'), "diagram art missing: {text}");
    assert!(!text.contains("sequenceDiagram"), "raw source leaked: {text}");
}
```

- [ ] **Step 2: Run the focused test and verify it fails for the missing helper.**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_md::tests::render_mermaid_sequence_returns_terminal_lines -- --exact
```

Expected: compilation failure because `render_mermaid_block` does not exist.

- [ ] **Step 3: Implement the smallest shared adapter and helper.**

Move/generalize the color mapping currently implemented by `DagTheme` into `render_md.rs`. The helper should follow this shape:

```rust
pub(crate) fn render_mermaid_block(
    source: &str,
    theme: &Theme,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    ratatui_markdown::mermaid::render_mermaid(
        source,
        width.max(1),
        None,
        &TuiRichTextTheme { theme },
    )
}
```

Keep the adapter `Copy`/borrowed, map `get_mermaid_theme()` with `MermaidTheme::for_background(theme.bg)`, and preserve the existing `DagTheme` color choices exactly. Change `task_dag.rs` to import the shared adapter and use it when calling `MarkdownRenderer::render`; do not change `tasks_to_mermaid`, popup width handling, source-copy behavior, or tests.

- [ ] **Step 4: Run the focused test and existing DAG tests.**

Run serially:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_md::tests::render_mermaid_sequence_returns_terminal_lines -- --exact
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib widgets::state::task_dag::tests
```

Expected: the sequence test and all task-DAG tests pass.

- [ ] **Step 5: Commit the shared helper.**

```bash
git add crates/tui/src/render/render_md.rs crates/tui/src/widgets/state/task_dag.rs
git commit -m "feat(tui): add shared Mermaid renderer"
```

---

### Task 2: Route complete Mermaid fences through Markdown rendering

**Files:**
- Modify: `crates/tui/src/render/render_md.rs`
- Modify: `crates/tui/src/render/cells/markdown.rs`

**Interfaces:**
- Keep `render_markdown_tui` and `render_markdown_with_tables` call sites source-compatible.
- Add an internal fence router that accepts `(text, theme, width)` and returns `(Vec<Line<'static>>, Vec<String>)`.
- The router must delegate non-Mermaid content to the existing `tui-markdown`/table logic and invoke `render_mermaid_block` only for a closed, top-level `mermaid` fence.

- [ ] **Step 1: Write failing tests for flowchart, fallback, and width-aware cell routing.**

Add tests in `render_md.rs`:

```rust
#[test]
fn render_markdown_mermaid_flowchart_uses_box_art() {
    let md = "```mermaid\nflowchart TD\n  A[Start] --> B[End]\n```";
    let (lines, raw) = render_markdown_tui(md, &theme());
    let text = raw.join("\n");

    assert!(lines.iter().any(|line| line.to_string().contains('─') || line.to_string().contains('│')),
        "expected flowchart box art: {text}");
    assert!(!text.contains("flowchart TD"), "raw Mermaid leaked: {text}");
}

#[test]
fn render_markdown_invalid_mermaid_falls_back_to_code() {
    let md = "```mermaid\nnot a valid diagram\n```";
    let (_lines, raw) = render_markdown_tui(md, &theme());
    let text = raw.join("\n");

    assert!(text.contains("not a valid diagram"), "fallback lost source: {text}");
}
```

Add a `MarkdownCell` test in `cells/markdown.rs`:

```rust
#[test]
fn markdown_cell_renders_mermaid_at_the_requested_width() {
    let cell = MarkdownCell::new(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```",
        &dark(),
    );
    let text = render_text(&cell, 60);

    assert!(text.contains("Alice") && text.contains("Bob"), "diagram missing: {text}");
    assert!(!text.contains("sequenceDiagram"), "raw Mermaid leaked: {text}");
}
```

- [ ] **Step 2: Run the new focused tests and verify they fail.**

Run serially:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_md::tests::render_markdown_mermaid_flowchart_uses_box_art -- --exact
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_md::tests::render_markdown_invalid_mermaid_falls_back_to_code -- --exact
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::cells::markdown::tests::markdown_cell_renders_mermaid_at_the_requested_width -- --exact
```

Expected: tests fail because the current `tui-markdown` path emits the Mermaid source as code.

- [ ] **Step 3: Add a narrow top-level fence extractor.**

Implement a line-oriented helper in `render_md.rs` that:

- treats a line whose trimmed form starts with ````` as a fence opener;
- recognizes `mermaid` case-insensitively after the opening fence;
- collects source lines until a trimmed closing line equal to `````;
- flushes preceding/following prose to the existing renderer;
- calls `render_mermaid_block` with `width.max(1)`;
- if rendering returns `None`, sends the original opening fence, source, and closing fence through the existing code renderer;
- leaves non-Mermaid fences untouched for the existing parser;
- treats an unclosed Mermaid fence as ordinary Markdown/code content rather than dropping it.

Keep the existing `render_markdown_tui` output unchanged when no Mermaid fence is present. Make `render_markdown_with_tables` invoke the Mermaid extractor before its pipe-table scanner so `|` lines inside a diagram are not mistaken for a table. Use the existing `format_table` implementation for prose/table segments.

- [ ] **Step 4: Wire `MarkdownCell` to the same width-aware router.**

Replace the cell's current call to `render_markdown_with_tables` only as needed so the cell passes its content width into the Mermaid route. Preserve `wrap_line` and the existing width cache; Mermaid output should be regenerated when `MarkdownCell::height`/`render_partial` is called at a new width.

- [ ] **Step 5: Run the focused and adjacent Markdown tests.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_md::tests
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::cells::markdown::tests
```

Expected: all existing Markdown/table/cell tests and the new Mermaid tests pass.

- [ ] **Step 6: Commit Markdown routing.**

```bash
git add crates/tui/src/render/render_md.rs crates/tui/src/render/cells/markdown.rs
git commit -m "feat(tui): render Mermaid fences in Markdown"
```

---

### Task 3: Route streamed Mermaid blocks without creating code-card overlays

**Files:**
- Modify: `crates/tui/src/widgets/state/stream_state.rs`
- Modify: `crates/tui/src/widgets/state/app/agent.rs`
- Modify: `crates/tui/src/widgets/state/app/visibility.rs`
- Modify: `crates/tui/src/render/render_gap_tests.rs`

**Interfaces:**
- Add `StreamState::code_block_is_mermaid: bool`, defaulting to `false`.
- Reuse `render_mermaid_block` and the existing message splice/placeholder APIs.
- Valid completed Mermaid becomes ordinary log content or a width-aware Markdown cell, but never adds an entry to `app.code_blocks`.
- Invalid Mermaid and ordinary fences retain the existing `CodeBlock` fallback.

- [ ] **Step 1: Write the failing main-area regression tests.**

Add to `render_gap_tests.rs`:

```rust
#[test]
fn log_renders_streamed_mermaid_without_code_card() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 30);

    assert!(app.code_blocks.is_empty(), "valid Mermaid must not become a code card");
    assert!(text.contains("Alice") && text.contains("Bob"), "diagram missing: {text}");
    assert!(!text.contains("sequenceDiagram"), "raw Mermaid leaked: {text}");
}

#[test]
fn log_falls_back_to_code_card_for_invalid_streamed_mermaid() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nnot valid Mermaid\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 30);

    assert_eq!(app.code_blocks.len(), 1, "invalid Mermaid should use code fallback");
    assert!(text.contains("not valid Mermaid"), "fallback lost source: {text}");
}
```

Add a test for a closing Mermaid fence without a trailing newline alongside `flush_consumes_closing_fence_without_trailing_newline`; it must assert a valid diagram and `app.code_blocks.is_empty()`.

- [ ] **Step 2: Run the focused tests and verify they fail.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_gap_tests::log_renders_streamed_mermaid_without_code_card -- --exact
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_gap_tests::log_falls_back_to_code_card_for_invalid_streamed_mermaid -- --exact
```

Expected: the first test fails because the current path creates a `CodeBlock`; the second may pass only for source retention, but must be kept as a regression for the shared fallback path.

- [ ] **Step 3: Mark Mermaid when opening a stream fence.**

When `apply_stream_chunk` extracts the language, assign:

```rust
self.stream.code_block_is_mermaid = lang.eq_ignore_ascii_case("mermaid");
```

Keep buffering lines exactly as today. The preview may retain the existing streaming code styling, but do not create a `CodeBlock` object for a valid completed Mermaid block.

- [ ] **Step 4: Centralize close/finalize behavior.**

Extract the close logic currently duplicated between `apply_stream_chunk` and `flush_stream_pending` into one helper with this behavior:

```rust
fn finish_stream_code_block(&mut self, lang: String, lines: Vec<String>, start: usize, end: usize) {
    let source = format!("```{lang}\n{}\n```", lines.join("\n"));
    if lang.eq_ignore_ascii_case("mermaid")
        && let Some(diagram) = render_mermaid_block(
            &lines.join("\n"),
            &self.theme,
            self.log_scroll.width.max(1) as usize,
        )
    {
        let raw = diagram.iter().map(Line::to_string).collect::<Vec<_>>();
        self.splice_msgs(start..end, diagram, raw, RawMessageType::LLM);
        return;
    }

    const MAX_CODE_PREVIEW: usize = 30;
    let (styled, _) = render_markdown_tui(&source, &self.theme);
    let placeholder_count = styled.len().min(MAX_CODE_PREVIEW) + 2;
    let placeholders: Vec<Line<'static>> =
        (0..placeholder_count).map(|_| Line::from("")).collect();
    let raw_placeholders: Vec<String> =
        (0..placeholder_count).map(|_| String::new()).collect();
    self.splice_msgs(
        start..end,
        placeholders,
        raw_placeholders,
        RawMessageType::LLM,
    );
    self.code_blocks.push(CodeBlock {
        start_idx: start,
        end_idx: start + placeholder_count,
        lang,
        content: lines.join("\n"),
        styled,
    });
}
```

The actual implementation should use the existing `code_block_start_idx`, `code_block_line_count`, `splice_msgs`, and `CodeBlock` fields rather than introducing a second overlay type. Reset `code_block_is_mermaid` whenever the buffered block is finalized.

- [ ] **Step 5: Make incomplete flush use the same helper.**

In `flush_stream_pending`, preserve the current handling of a final pending ` ``` ` without newline, collect any remaining content, and call the same finalization helper. If the stream ends with an unclosed Mermaid fence, the helper receives no closing fence and must take the fallback code-card branch; it must not discard buffered source.

- [ ] **Step 6: Run streaming and existing code-card tests.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::render_gap_tests
# Run only after the previous command exits:
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib widgets::state::app::agent::lifecycle_tests
```

Expected: Mermaid stream tests pass, existing Rust/text code-card tests remain green, and no test reports leaked closing fences.

- [ ] **Step 7: Commit streaming support.**

```bash
git add crates/tui/src/widgets/state/stream_state.rs crates/tui/src/widgets/state/app/agent.rs crates/tui/src/widgets/state/app/visibility.rs crates/tui/src/render/render_gap_tests.rs
git commit -m "feat(tui): render streamed Mermaid diagrams"
```

---

### Task 4: Synchronize documentation and verify the complete TUI suite

**Files:**
- Modify: `book/23_chapter_tui.md`
- Modify: `book/23_chapter_tui_zh.md`
- Modify: `book/26_chapter_issue.md`
- Modify: `book/26_chapter_issue_zh.md`

**Interfaces:**
- Documentation must describe the shipped main-area behavior, not the old “all explicit fences become code cards” rule.
- English and Chinese Ch 23 sections must remain structurally aligned.
- Ch 26 gets one newest-first entry with date, type, symptom, decision, post-change behavior, and pointers to the spec/plan/code/tests.

- [ ] **Step 1: Write the documentation assertions as a checklist before editing.**

The updated Ch 23 Markdown section must state all of these facts:

```text
- A complete ```mermaid fenced block is rendered as a terminal diagram in the main log.
- Supported types follow the pinned ratatui-markdown Mermaid renderer.
- Streaming buffers the block until closure and does not create a code card when rendering succeeds.
- Invalid/unsupported Mermaid falls back to the normal code block/card.
- Ordinary explicit-language fences retain code-card behavior.
- Width changes and viewport scrolling use the existing log layout/cache behavior.
```

- [ ] **Step 2: Edit the paired English/Chinese Ch 23 sections.**

Replace only the stale fenced-content paragraph and add a short Mermaid bullet/subsection in both files. Keep heading hierarchy and section numbering identical; translate content rather than changing structure.

- [ ] **Step 3: Add the newest Ch 26 issue entry in both languages.**

Use date `2026-08-08`, type `optimization` or `bugfix` according to the final implementation classification, and point to:

```text
crates/tui/src/render/render_md.rs
crates/tui/src/widgets/state/app/agent.rs
crates/tui/src/widgets/state/app/visibility.rs
docs/superpowers/specs/2026-08-08-mermaid-main-rendering-design.md
docs/superpowers/plans/2026-08-08-mermaid-main-rendering.md
```

Describe the before/after observable behavior and the code-fallback invariant in both languages.

- [ ] **Step 4: Run documentation consistency checks.**

```bash
git diff --check
rg -n "Mermaid|mermaid|时序图|代码卡片|code card" book/23_chapter_tui.md book/23_chapter_tui_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
```

Expected: both language files mention the same behavior and no whitespace errors are reported.

- [ ] **Step 5: Run the complete TUI verification suite.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib
```

Expected: all tests pass, including the baseline 505 tests plus the new regressions.

- [ ] **Step 6: Inspect the final diff and commit documentation.**

```bash
git diff --stat HEAD~3..HEAD
git status --short --branch
git diff --check
git add book/23_chapter_tui.md book/23_chapter_tui_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
git commit -m "docs: document main-area Mermaid rendering"
```

Expected: the worktree is clean and only the planned Mermaid code/tests/docs are changed.

---

## Plan Self-Review

- **Spec coverage:** shared renderer and adapter are covered by Task 1; normal Markdown routing, fallback, and width-aware cell behavior by Task 2; streaming, incomplete flush, and code-card compatibility by Task 3; bilingual docs and issue log by Task 4.
- **Placeholder scan:** no `TBD`, `TODO`, ellipsis, or unspecified implementation step remains.
- **Type consistency:** `render_mermaid_block` returns `Option<Vec<Line<'static>>>`; all tasks use that signature. Stream finalization consumes `String`, `Vec<String>`, and index bounds already represented by `StreamState`; raw lines are derived with `Line::to_string`.
- **Scope check:** all changes belong to one TUI rendering subsystem plus its required bilingual docs; no independent subsystem needs a separate spec.
