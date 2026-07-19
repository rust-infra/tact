# Tool Popup Text Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add accurate mouse text selection to tool detail popups so `y` copies the selected original text, while preserving full-content copy when no non-empty selection exists.

**Architecture:** `DiffPopup` owns a byte-range selection into `cached_content`. The renderer produces explicit visible display rows plus source-offset hit maps, stores those maps in mouse state for event-time hit testing, and applies selection style from the same offsets. Overlay mouse handling updates only the tool-popup selection and never leaks events to the log or plan.

**Tech Stack:** Rust, Ratatui, Crossterm mouse events, `unicode-width`, existing TUI render test harness.

## Global Constraints

- Scope is tool detail popups only; thinking and code popups remain unchanged.
- `y` copies a non-empty selection, otherwise the complete original popup content.
- Borders, title, line numbers, diff gutter, and scrollbar are never copied or highlighted.
- Source offsets must always be valid UTF-8 boundaries.
- Selection survives scrolling but closes with the popup; drag-edge auto-scroll is out of scope.
- Keep English and Chinese TUI documentation structurally aligned.

---

### Task 1: Popup Selection Model and Copy Precedence

**Files:**
- Modify: `crates/tui/src/widgets/state/tool_state.rs`
- Modify: `crates/tui/src/widgets/state/app/popups.rs`
- Modify: popup constructors/tests in `crates/tui/src/widgets/state/app/popups.rs`, `crates/tui/src/handlers/overlay.rs`, and `crates/tui/src/render/popup_scene_tests.rs`

**Interfaces:**
- Produces: `PopupTextSelection { anchor: usize, active: usize }`
- Produces: `PopupTextSelection::normalized_non_empty(&self, content: &str) -> Option<Range<usize>>`
- Changes: every `DiffPopup` constructor initializes `selection: None`
- Changes: `App::copy_diff_popup()` selects the normalized slice before calling `copy_text`

- [ ] **Step 1: Write failing model tests**

Add tests beside `PopupTextSelection` that assert forward and backward ranges normalize identically, equal endpoints return `None`, offsets clamp to content length, and offsets inside a multibyte scalar move to valid UTF-8 boundaries.

```rust
#[test]
fn popup_selection_normalizes_backward_utf8_range() {
    let text = "a界z";
    let selection = PopupTextSelection::new(text.len(), 1);
    assert_eq!(selection.normalized_non_empty(text), Some(1..5));
}

#[test]
fn popup_selection_ignores_empty_range() {
    assert_eq!(PopupTextSelection::new(2, 2).normalized_non_empty("text"), None);
}
```

- [ ] **Step 2: Run the focused model tests and verify failure**

Run: `cargo test -p tui popup_selection --lib`

Expected: compilation fails because `PopupTextSelection` and `DiffPopup::selection` do not exist.

- [ ] **Step 3: Implement the minimal selection model**

Add the selection type to `tool_state.rs`, normalize endpoint order, clamp each endpoint to `content.len()`, and walk backward while an endpoint is not a character boundary. Add `selection: Option<PopupTextSelection>` to `DiffPopup` and update all constructors.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopupTextSelection {
    pub(crate) anchor: usize,
    pub(crate) active: usize,
}

impl PopupTextSelection {
    pub(crate) fn new(anchor: usize, active: usize) -> Self { Self { anchor, active } }

    pub(crate) fn normalized_non_empty(&self, content: &str) -> Option<std::ops::Range<usize>> {
        let mut start = self.anchor.min(self.active).min(content.len());
        let mut end = self.anchor.max(self.active).min(content.len());
        while start > 0 && !content.is_char_boundary(start) { start -= 1; }
        while end > 0 && !content.is_char_boundary(end) { end -= 1; }
        (start < end).then_some(start..end)
    }
}
```

- [ ] **Step 4: Write failing copy-precedence tests**

Refactor the source choice into a pure helper such as `DiffPopup::copy_content()` so tests do not depend on the host clipboard. Assert a selection returns only the original slice, an empty selection returns all content, and the selection does not contain presentation prefixes.

```rust
#[test]
fn popup_copy_content_prefers_non_empty_selection() {
    let mut popup = inline_popup("first\nsecond");
    popup.cached_content = Some("first\nsecond".into());
    popup.selection = Some(PopupTextSelection::new(6, 12));
    assert_eq!(popup.copy_content(), Some("second".into()));
}
```

- [ ] **Step 5: Implement copy precedence and run focused tests**

Run: `cargo test -p tui popup_selection --lib && cargo test -p tui popup_copy_content --lib`

Expected: all focused tests pass; existing full-content behavior remains green.

---

### Task 2: Visible Row Layout, Unicode Hit Maps, and Selection Rendering

**Files:**
- Modify: `crates/tui/src/render/popups/diff_popup.rs`
- Modify: `crates/tui/src/widgets/state/mouse_state.rs`
- Test: `crates/tui/src/render/popup_scene_tests.rs`
- Test: unit tests in `crates/tui/src/render/popups/diff_popup.rs`

**Interfaces:**
- Produces: `PopupTextHit { start: usize, end: usize }`, where the range is the source scalar under a screen cell and may be empty for an empty line
- Produces: `PopupHitRow { screen_y, text_x, line_start, line_end, cells }`
- Produces: `MouseState::diff_popup_hit_rows: Vec<PopupHitRow>` and `diff_popup_body_area: Rect`
- Consumes: `DiffPopup::selection` from Task 1

- [ ] **Step 1: Write failing geometry tests**

Add pure tests for a row builder/hit resolver. Cover ASCII, `界`, combining/zero-width scalars, empty rows, columns before text, columns after text, a numbered line with green gutter, and a wrapped unified-diff line. The key assertions are that every returned offset is a UTF-8 boundary and prefix columns clamp to the line start.

```rust
#[test]
fn hit_map_excludes_number_and_diff_gutter() {
    let row = test_hit_row("界x", 10, 7);
    assert_eq!(row.hit(7), PopupTextHit::empty(10));
    assert_eq!(row.hit(8), PopupTextHit::new(10, 13));
    assert_eq!(row.hit(9), PopupTextHit::new(10, 13));
    assert_eq!(row.hit(10), PopupTextHit::new(13, 14));
}
```

- [ ] **Step 2: Run geometry tests and verify failure**

Run: `cargo test -p tui diff_popup::tests::hit_map --lib`

Expected: compilation fails because popup hit rows do not exist.

- [ ] **Step 3: Implement explicit visible rows and hit maps**

Build source lines once with byte start/end offsets. For each visible source line, construct display cells with `UnicodeWidthChar::width`, preserving the existing plain/file line numbers, optional gutter, syntax styles, and unified-diff colors. Explicitly split wrapped diff rows to the available body width and render pre-laid-out rows without a second implicit `Paragraph` wrap. Save only the currently visible hit rows in `MouseState` after rendering.

The hit resolver must return the complete byte span of the scalar under the pointer. Columns before selectable text return an empty hit at `line_start`; columns to the right return an empty hit at the represented row end.

- [ ] **Step 4: Write failing render-highlight tests**

Seed a popup with `selection: Some(...)`, render it with the existing `TestBackend`, and inspect buffer cell modifiers. Assert selected source cells have `Modifier::REVERSED`, while the line number and gutter cells do not. Add a wide-character case and a scrolled selection case.

```rust
assert!(body_cell.modifier.contains(Modifier::REVERSED));
assert!(!gutter_cell.modifier.contains(Modifier::REVERSED));
```

- [ ] **Step 5: Apply selection style and run popup render tests**

While grouping display cells into spans, add `Modifier::REVERSED` only when the cell source range intersects the normalized non-empty selection. Preserve existing foreground/background/modifier styling.

Run: `cargo test -p tui diff_popup --lib && cargo test -p tui popup_scene --lib`

Expected: hit-map and highlight tests pass; existing popup snapshots/text assertions remain green.

---

### Task 3: Overlay Mouse Routing, Documentation, and Regression Verification

**Files:**
- Modify: `crates/tui/src/handlers/mouse.rs`
- Modify: `crates/tui/src/widgets/state/mouse_state.rs`
- Modify: `book/23_chapter_tui.md`
- Modify: `book/23_chapter_tui_zh.md`
- Modify: `docs/tool_rendering.md`

**Interfaces:**
- Consumes: `MouseState::diff_popup_hit_rows` from Task 2
- Produces: tool-popup-specific down/drag/up routing
- Produces: drag origin state sufficient to include the scalar at both ends of forward and backward drags

- [ ] **Step 1: Write failing mouse-routing tests**

Add tests in `handlers/mouse.rs` that seed a rendered popup hit map and send Crossterm down/drag/up events. Assert down starts an empty selection, a forward drag includes both endpoint scalars, a backward drag normalizes to the same text, mouse up stops later drags, scrolling preserves the range, and popup interaction does not create a log selection.

```rust
handle_mouse_event(&mut app, mouse_down(text_x, row));
handle_mouse_event(&mut app, mouse_drag(text_x + 4, row));
assert_eq!(app.tools.popup.as_ref().unwrap().selected_text(), Some("alpha"));
```

- [ ] **Step 2: Run mouse tests and verify failure**

Run: `cargo test -p tui handlers::mouse --lib`

Expected: new selection assertions fail because overlay clicks are only consumed for close behavior.

- [ ] **Step 3: Implement overlay-first mouse routing**

Before panel hit handling, route left down/drag/up to the active tool popup. On down, store the source scalar span as the drag origin and set a collapsed selection. On drag right, select `origin.start..current.end`; on drag left, select `origin.end..current.start`. Clamp vertical out-of-body drags to the first/last visible hit boundary. On up, stop popup dragging. Preserve current outside-click close behavior and wheel routing.

- [ ] **Step 4: Update synchronized documentation**

In both `book/23_chapter_tui.md` and `book/23_chapter_tui_zh.md`, add structurally matching popup-selection rows and copy precedence: mouse drag selects original tool text, prefixes are excluded, and `y` prefers the popup selection. Update `docs/tool_rendering.md` section 8 with the same behavior and scope.

- [ ] **Step 5: Run formatting and focused verification**

Run:

```bash
cargo fmt --check
cargo test -p tui popup_selection --lib
cargo test -p tui diff_popup --lib
cargo test -p tui handlers::mouse --lib
cargo test -p tui popup_scene --lib
```

Expected: every command exits 0.

- [ ] **Step 6: Run the full TUI regression suite and inspect the diff**

Run:

```bash
cargo test -p tui --lib
git diff --check
git status --short
```

Expected: the full TUI library suite passes, no whitespace errors are reported, and only the planned implementation, tests, plan, and synchronized documentation are modified.
