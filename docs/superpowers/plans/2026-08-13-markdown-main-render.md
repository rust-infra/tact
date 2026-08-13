# Markdown Main-Area Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix Markdown list wrapping and verify the TUI main-area rendering path without replacing the existing streaming Markdown pipeline.

**Architecture:** Keep `App::apply_stream_chunk`, `render_markdown_with_tables`, and `tui-markdown` unchanged as the parsing/streaming pipeline. Add one reusable width-aware wrapper for styled lines that preserves the first list marker and inserts a hanging indent on continuation rows; use it only when the main log renders ordinary Markdown lines. Cover the behavior with main-area regression tests, then update the bilingual issue log because this changes visible TUI behavior.

**Tech Stack:** Rust 2024, ratatui `Line`/`Span`, `unicode-width`, `cargo test -p tui --lib`.

## Global Constraints

- Do not implement a complete CommonMark incremental AST.
- Do not replace `tui-markdown` or change the existing stream state machine.
- Do not add yellow-border/background callout rendering.
- Do not change table, code-block, Mermaid, popup, tool-card, or sticky-task-panel architecture.
- Never run multiple Cargo commands in parallel; unset `http_proxy`, `https_proxy`, and `all_proxy` for local tests.
- Keep the change focused and do not commit unless explicitly requested.

---

### Task 1: Add failing main-area regression tests

**Files:**
- Modify: `crates/tui/src/render/log_render_tests.rs`
- Modify: `crates/tui/src/render/render_gap_tests.rs`

**Interfaces:**
- Consume existing `make_app`, `render_log_panel_terminal`, `render_log_panel_text`, `render_main_area_text`, and `buffer_column_of` helpers.
- Produce regression tests that fail against the current `wrap_line` behavior and pass after the wrapper is integrated.

- [ ] **Step 1: Write the failing list continuation test**

Add this test next to the existing log wrapping tests in `crates/tui/src/render/log_render_tests.rs`:

```rust
#[test]
fn log_wrapped_ordered_list_continuation_hangs_under_item_text() {
    let mut app = make_app();
    app.add_system_message(
        "4. The stashes listed here belong to older branches and are not part of this change, so I left them untouched."
            .into(),
    );

    let terminal = render_log_panel_terminal(&mut app, 44, 12);
    let buffer = terminal.backend().buffer();
    let first_x = buffer_column_of(buffer, "4. The stashes").expect("ordered marker row");
    let continuation_x = buffer_column_of(buffer, "branches").expect("wrapped continuation row");

    assert_eq!(
        continuation_x,
        first_x + 3,
        "continuation text should start below item text, not at the panel edge"
    );
}
```

- [ ] **Step 2: Write the failing nested/CJK width test**

Add a test that verifies nested Markdown output is preserved and every visible row fits the content width:

```rust
#[test]
fn log_nested_cjk_list_wraps_without_losing_text_or_width() {
    let mut app = make_app();
    app.add_system_message(
        "- 通用提醒\n  1. 香港散户在持牌平台上只能交易 BTC、ETH 等主要代币，长文本需要继续换行而不能丢失。\n  2. 大额交易建议咨询专业人士。"
            .into(),
    );

    let text = render_log_panel_text(&mut app, 36, 18);
    assert!(text.contains("通用提醒"), "parent item missing:\n{text}");
    assert!(text.contains("香港散户"), "nested CJK item missing:\n{text}");
    assert!(text.contains("专业人士"), "second nested item missing:\n{text}");

    for line in text.lines() {
        assert!(
            unicode_width::UnicodeWidthStr::width(line) <= 36,
            "rendered row exceeds terminal width: {line:?}"
        );
    }
}
```

- [ ] **Step 3: Write the failing stream-boundary test**

Add a focused test in `crates/tui/src/render/render_gap_tests.rs` that proves the fix does not require changing stream buffering:

```rust
#[test]
fn streamed_markdown_list_keeps_tail_and_renders_after_flush() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "- first item\n- second item with a long tail".into(),
    ));

    let before_flush = render_main_area_text(&mut app, 44, 18);
    assert!(before_flush.contains("second item"), "live tail missing:\n{before_flush}");

    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));
    let after_flush = render_main_area_text(&mut app, 44, 18);
    assert!(after_flush.contains("first item"), "first list item missing:\n{after_flush}");
    assert!(after_flush.contains("long tail"), "flushed list tail missing:\n{after_flush}");
}
```

- [ ] **Step 4: Run only the new tests and verify the expected failure**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::
```

Expected: the ordered-list test fails because the existing main-area wrapper starts the continuation at column zero; the stream test may pass and is retained as a guard. If a test fails for a fixture/setup reason instead, correct only the test before proceeding.

---

### Task 2: Implement width-aware hanging-indent wrapping

**Files:**
- Modify: `crates/tui/src/render/util.rs`
- Modify: `crates/tui/src/render/log.rs`
- Test: `crates/tui/src/render/log_render_tests.rs`

**Interfaces:**
- Add `pub(crate) fn list_hanging_indent(text: &str) -> usize` in `render/util.rs`; it returns terminal display columns from the beginning of a Markdown list line through the whitespace after `-`, `*`, `+`, `•`, or an ordered marker such as `12.`. Return `0` for non-list lines.
- Add `pub(crate) fn wrap_line_with_hanging_indent(line: &Line<'_>, max_width: usize, hanging_indent: usize) -> Vec<Line<'static>>` in `render/util.rs`; first visual row uses `max_width`, later rows reserve `hanging_indent` cells and wrap the remaining content to `max_width - hanging_indent`.
- Update the ordinary-message branch in `render_log_panel` to call the new wrapper with `list_hanging_indent(&app.raw_messages[phys_idx])`; preserve the existing `nested_log_indent` subtraction and all special handling for separators and MarkdownCell/tool rows.

- [ ] **Step 1: Implement list marker detection**

Use display width, not byte length, for the marker prefix. The helper must recognize leading spaces, unordered markers, and ordered markers, and must return zero when there is no whitespace after the marker because that is not a Markdown list marker.

- [ ] **Step 2: Implement the styled-line wrapper**

Refactor the existing `wrap_line` splitting loop into a small internal function that accepts a first-row width and a continuation-row width. Keep the existing span style behavior: use the line style patched with the first span style, never emit a row wider than the requested display width, and handle wide Unicode scalars that cannot fit in the remaining cells by emitting them on their own row. Prefix continuation rows with `Span::raw(" ".repeat(hanging_indent))` and ensure the prefix is included in the width budget.

The non-list `wrap_line` API must delegate to the same implementation with `continuation_indent = 0`, so existing callers keep their current behavior.

- [ ] **Step 3: Integrate only into ordinary main-log rows**

In `render_log_panel`, replace the ordinary row call:

```rust
wrap_line(&line, wrap_width.saturating_sub(indent).max(1))
```

with:

```rust
let content_width = wrap_width.saturating_sub(indent).max(1);
let hanging_indent = list_hanging_indent(&app.raw_messages[phys_idx]);
wrap_line_with_hanging_indent(&line, content_width, hanging_indent)
```

Do not apply the helper to separators, stream rows, `MarkdownCell` placeholders, tools, or code overlays until a test demonstrates that those paths need it.

- [ ] **Step 4: Run the new tests and verify green**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::
```

Expected: the new tests and all existing render tests pass, and every row remains within the terminal width.

- [ ] **Step 5: Run the focused existing render suites**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib render::
```

Expected: all focused tests pass with no new warnings.

---

### Task 3: Verify adjacent Markdown boundaries and documentation

**Files:**
- Modify: `book/26_chapter_issue.md`
- Modify: `book/26_chapter_issue_zh.md`
- Review only: `docs/superpowers/specs/2026-08-13-markdown-main-render-design.md`

**Interfaces:**
- Documentation records the visible main-area list wrapping bug and the final behavior.
- No production API changes are introduced.

- [ ] **Step 1: Run the complete TUI library test suite**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib
```

Expected: all tests pass. Do not start another Cargo command until this command exits.

- [ ] **Step 2: Inspect adjacent-path behavior using existing tests**

Confirm the existing tests still cover:

- Markdown tables and table wrapping;
- streamed and unclosed code fences;
- Mermaid routing before table scanning;
- main-area narrow-terminal rendering;
- scroll-cache rebuilds after width changes;
- tool cards and sticky task layout.

If the full suite exposes a regression, add one minimal failing test beside the owning renderer before changing production code.

- [ ] **Step 3: Add newest-first bilingual Ch 26 bugfix entries**

Add matching entries dated `2026-08-13` to both issue logs with type `bugfix`, describing:

- symptom: long Markdown list items in the main log wrapped continuation text at the left edge and could mislead or clip nested CJK lists;
- decision: preserve the existing streaming/tui-markdown path and add display-width-aware hanging indentation in the main log wrapper;
- observable behavior: ordered, unordered, nested, and wide-character list continuation rows align under item text and remain within the panel width;
- pointers: `crates/tui/src/render/util.rs`, `crates/tui/src/render/log.rs`, the new render tests, and this design/plan.

- [ ] **Step 4: Re-read the final diff and verify worktree scope**

Run:

```bash
git diff --check
git status --short
git diff --stat
git diff -- crates/tui/src/render/util.rs crates/tui/src/render/log.rs crates/tui/src/render/log_render_tests.rs crates/tui/src/render/render_gap_tests.rs book/26_chapter_issue.md book/26_chapter_issue_zh.md
```

Expected: only the listed renderer, test, documentation, and design/plan files are changed; no dependency or unrelated layout changes appear.

- [ ] **Step 5: Final verification**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo fmt --all -- --check
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib
```

Expected: formatting check passes and the complete TUI library suite passes.
