# Thinking Live Card Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render streaming thinking as a direct log card whose body grows from one to three lines and collapses to one summary line on completion.

**Architecture:** Replace thinking's title/end-range model with active and completed records anchored at one placeholder row. A new `ThinkingCell` renders those records through `LogColumnRenderer`, like `ToolCell`; the old thinking overlay and visibility filtering disappear. Full plain text and cached Markdown remain in state for the existing detail popup and copy command.

**Tech Stack:** Rust, Ratatui `Renderable`/`LogColumnRenderer`, Crossterm mouse handling, existing `render_markdown_tui`, TUI test backend.

## Global Constraints

- Scope is thinking only; do not change bash/tool live-output behavior or create a generic live-card abstraction.
- Active thinking body height is one, then two, then three logical lines; after three lines it is a fixed tail of the latest three.
- A non-empty unterminated stream fragment is rendered as the current last line.
- Completed thinking is one summary row using the latest non-empty logical line, at a UTF-8-safe truncation boundary.
- Active and completed popup/copy paths expose complete content; active popup uses content buffered so far.
- No thinking overlay and no per-thinking source rows are stored in shared `messages`/`raw_messages`.
- Keep English and Chinese TUI chapters structurally aligned and update rendering documentation.

---

### Task 1: Replace Range-Based Thinking State With Card Records

**Files:**
- Modify: `crates/tui/src/widgets/state/thinking_state.rs`
- Modify: `crates/tui/src/widgets/state/app/visibility.rs`
- Modify: `crates/tui/src/widgets/state/app/agent.rs`
- Modify: `crates/tui/src/widgets/state/app/popups.rs`
- Test: unit tests in `crates/tui/src/widgets/state/thinking_state.rs` and `crates/tui/src/widgets/state/app/agent.rs`

**Interfaces:**
- Produces: `ActiveThinkingBlock { phys_idx, content, pending_line, completed_tail, started_at }`
- Produces: `ThinkingBlock { phys_idx, content, summary, cached_markdown, elapsed }`
- Produces: `ThinkingState { active: Option<ActiveThinkingBlock>, blocks: Vec<ThinkingBlock>, popup: Option<ThinkingPopup> }`
- Produces: `ActiveThinkingBlock::push_delta(&mut self, delta: &str)` and `display_tail(&self) -> Vec<String>`
- Changes: `ThinkingPopup` identifies content by `phys_idx`, not a completed-block array index

- [ ] **Step 1: Write failing stream-model tests**

Add pure `ActiveThinkingBlock` tests before changing production behavior. Test a first unterminated delta, completion of a buffered line, a fourth completed line replacing the three-line tail, and whitespace-only content.

```rust
#[test]
fn active_thinking_tail_grows_then_keeps_latest_three_lines() {
    let mut active = ActiveThinkingBlock::new(8, Instant::now());
    active.push_delta("one\ntwo\nthree\nfour\n");
    assert_eq!(active.display_tail(), ["two", "three", "four"]);
}

#[test]
fn active_thinking_tail_includes_unterminated_fragment() {
    let mut active = ActiveThinkingBlock::new(8, Instant::now());
    active.push_delta("one\ntwo");
    assert_eq!(active.display_tail(), ["one", "two"]);
}
```

- [ ] **Step 2: Run the model tests and verify RED**

Run: `cargo test -p tui active_thinking_tail --lib`

Expected: compilation fails because `ActiveThinkingBlock` and its tail API do not exist.

- [ ] **Step 3: Implement active/completed state and finalization**

Replace `buffer`, `title_added`, `active_start`, `active_end`, range-based `ThinkingBlock`, `cached_preview`, and `scroll_offset`. `push_delta` always appends to `content`, moves completed `\n`-terminated lines through a three-entry `VecDeque<String>`, and exposes the tail plus a non-empty `pending_line`.

Create one blank `RawMessageType::LLMThinking` placeholder on `Started` or first `Delta`; do not append one message per thinking line. On finish, remove the placeholder for `content.trim().is_empty()`. Otherwise retain its index, calculate the latest non-empty `summary`, cache `render_markdown_tui(&content, &theme)`, and move the record to `blocks`.

```rust
pub(crate) fn begin_thinking_block(&mut self) {
    if self.thinking.active.is_some() { return; }
    let phys_idx = self.messages.len();
    self.append_blank(RawMessageType::LLMThinking);
    self.thinking.active = Some(ActiveThinkingBlock::new(phys_idx, Instant::now()));
}
```

- [ ] **Step 4: Write failing lifecycle and popup/copy tests**

Assert missing `Started` opens one placeholder, `Finished` keeps the same physical index, empty/whitespace thinking removes its placeholder, and popup/copy lookup succeeds for both active and completed content by `phys_idx`.

```rust
#[test]
fn thinking_finished_collapses_at_the_existing_placeholder() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("x\n".into())));
    let phys = app.thinking.active.as_ref().unwrap().phys_idx;
    app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
    assert_eq!(app.thinking.blocks[0].phys_idx, phys);
}
```

- [ ] **Step 5: Add placeholder resizing and index shifting, then verify GREEN**

Add `thinking_visual_rows`, `push_thinking_placeholder_rows`,
`resize_thinking_placeholder_rows`, and `refresh_thinking_log_scroll` in the
existing app visibility/state helpers. Active card placeholder rows resize only
for visible-tail transitions 1→2 and 2→3; completion resizes to the completed
one-summary card height. Extend `shift_phys_indices_from` and loading removal
to update the single `phys_idx` in `thinking.active` and every completed block.

Run:

```bash
cargo test -p tui active_thinking --lib
cargo test -p tui thinking_finished_collapses --lib
cargo test -p tui thinking_chunk --lib
```

Expected: focused state/lifecycle tests pass; no thinking row range is needed.

---

### Task 2: Add `ThinkingCell` and Remove Overlay Rendering

**Files:**
- Modify: `crates/tui/src/render/cells/thinking.rs`
- Modify: `crates/tui/src/render/cells/mod.rs`
- Modify: `crates/tui/src/render/log.rs`
- Modify: `crates/tui/src/render/layout.rs`
- Modify: `crates/tui/src/widgets/state/app/visibility.rs`
- Test: `crates/tui/src/render/log_render_tests.rs`, `crates/tui/src/render/popup_scene_tests.rs`, and tests in the new `thinking.rs`

**Interfaces:**
- Produces: `ThinkingCell::active(&ActiveThinkingBlock, spinner, theme, msgs)`
- Produces: `ThinkingCell::completed(&ThinkingBlock, theme, msgs)`
- Produces: `thinking_visual_rows(body_lines: usize) -> usize`
- Consumes: `ActiveThinkingBlock::display_tail()` and completed `summary`
- Removes: `render_thinking_cards` and thinking-specific `is_message_visible` collapse logic

- [ ] **Step 1: Write failing direct-cell render tests**

Write tests that construct an active cell with one, two, three, and four logical lines. Assert its height grows for the first three states, remains the same for four lines, and its rendered text changes from `one/two/three` to `two/three/four`. Add a completed-cell test that shows exactly the summary line, not the three-line tail.

```rust
#[test]
fn active_thinking_cell_stops_growing_after_three_lines() {
    let three = active_cell(&["one", "two", "three"]);
    let four = active_cell(&["one", "two", "three", "four"]);
    assert_eq!(three.height(80), four.height(80));
    assert!(render_text(&four).contains("two"));
    assert!(render_text(&four).contains("four"));
}
```

- [ ] **Step 2: Run render tests and verify RED**

Run: `cargo test -p tui active_thinking_cell --lib`

Expected: compilation fails because the direct `ThinkingCell` and its layout API do not exist.

- [ ] **Step 3: Implement the cell using `Renderable`**

Create a direct `ThinkingCell` with a two-row header (title/status) and a
bordered body. Its active body takes `display_tail().len().clamp(1, 3)` rows;
completed body takes exactly one. Use `render_partial` to preserve normal log
scroll clipping, following `ToolCell`'s row-slice pattern. Use UTF-8-safe
character truncation for each body line and existing thinking colors/messages.

- [ ] **Step 4: Write failing Phase 3 integration tests**

Build an app with active thinking, render the log panel, and assert a thinking
card is present without raw `│ ` source rows. Finish thinking and assert the
same log position renders the one-line summary. Assert old overlay rendering
does not draw a second card.

```rust
assert!(text.contains("Thinking"));
assert!(!text.contains("│ one"));
assert_eq!(count_occurrences(&text, "Thinking"), 1);
```

- [ ] **Step 5: Integrate in log Phase 3 and remove overlay/visibility code**

In `render/log.rs`, match the physical placeholder against `thinking.active`
and `thinking.blocks`, calculate the original visual start when the viewport
opens inside placeholder rows, push `ThinkingCell`, skip its owned placeholder
rows, and continue. Remove `render_thinking_cards` from `render/log.rs`; remove
the thinking overlay call path and the title/end-range branch from
`is_message_visible`. Keep `render/layout.rs`'s detail-popup call.

Run:

```bash
cargo test -p tui thinking_cell --lib
cargo test -p tui log_render --lib
cargo test -p tui popup_scene --lib
```

Expected: direct card height/tail/completion rendering passes, and no duplicate
thinking output remains in the log.

---

### Task 3: Rewire Popup, Mouse, Keyboard, Documentation, and Regression Coverage

**Files:**
- Modify: `crates/tui/src/widgets/state/app/popups.rs`
- Modify: `crates/tui/src/handlers/mouse.rs`
- Modify: `crates/tui/src/handlers/normal.rs`
- Modify: `crates/tui/src/render/popups/thinking_popup.rs`
- Modify: `crates/tui/src/render/render_gap_tests.rs`
- Modify: `crates/tui/src/render/popup_scene_tests.rs`
- Modify: `book/23_chapter_tui.md`
- Modify: `book/23_chapter_tui_zh.md`
- Modify: `docs/tui_rendering.md`
- Modify: `docs/tool_rendering.md`

**Interfaces:**
- Consumes: `App::find_thinking_at_logical(line_idx) -> Option<(phys_idx, logical_start, rows)>`
- Changes: `App::open_thinking_popup(phys_idx)` resolves active or completed record by stable placeholder
- Changes: `App::copy_thinking_popup()` resolves complete active/completed content by popup `phys_idx`

- [ ] **Step 1: Write failing popup and input tests**

Add tests proving an active card double-click opens a popup containing buffered
content, `y` copies that content, completion keeps an already-open popup valid,
and completed popup renders cached Markdown. Add mouse tests proving one click
on a direct thinking card clears log selection and double click opens it. Add
normal-mode `t` tests proving it chooses the current visible active/completed
thinking placeholder.

```rust
#[test]
fn active_thinking_popup_uses_buffered_content() {
    let mut app = active_thinking_app("draft reasoning");
    let phys = app.thinking.active.as_ref().unwrap().phys_idx;
    app.open_thinking_popup(phys);
    assert_eq!(app.thinking_popup_content(), Some("draft reasoning".into()));
}
```

- [ ] **Step 2: Run popup/input tests and verify RED**

Run: `cargo test -p tui active_thinking_popup --lib`

Expected: existing title-index lookup cannot resolve active card content.

- [ ] **Step 3: Implement stable-placeholder interaction lookup**

Make `ThinkingPopup` carry `phys_idx`. Add one lookup helper that returns
active content first, then completed content; use it for popup open, popup
render, copy, mouse double click, and `t`. `render_thinking_popup` displays
plain active content line-by-line until completion and completed cached Markdown
afterward. Keep `j/k`, `Esc`, outside-click dismissal, and `y` overlay routing.

- [ ] **Step 4: Update paired documentation and run focused verification**

Update the English and Chinese TUI chapters with the same section hierarchy and
the same direct-card behavior: active 1→3 lines, fixed three-line tail,
completed one-line summary, and popup access. Update `docs/tui_rendering.md`
and `docs/tool_rendering.md` to remove overlay descriptions and distinguish
thinking from bash's fixed five-line live tail.

Run:

```bash
cargo fmt --check
cargo test -p tui thinking --lib
cargo test -p tui handlers::mouse --lib
cargo test -p tui popup_scene --lib
git diff --check
```

Expected: focused behavior and documentation-adjacent scene tests pass with no
formatting or whitespace errors.

- [ ] **Step 5: Run full regression verification**

Run: `cargo test -p tui --lib`

Expected: all TUI library tests pass. Inspect `git status --short` and verify
the diff contains only the direct-thinking-card state/render/input/doc changes
described above.
