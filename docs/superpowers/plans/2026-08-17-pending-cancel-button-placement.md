# Pending Queue Cancel Button Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the pending queue's `[Cancel]` control directly after the pending-submit hint text on the same row, keeping its hit area and queue semantics unchanged.

**Architecture:** Keep `pending_cancel_btn_area` as render-time state owned by `App`. Change only `render_pending_block` so it reserves the button width, truncates the hint accordingly, and places the button immediately after the rendered hint. Existing mouse handling continues to consume the recorded rectangle and clear only the queue.

**Tech Stack:** Rust 2024, ratatui, Unicode display-width helpers, TUI unit tests, Markdown book chapters.

## Global Constraints

- The wide-terminal layout must be `Message will be submitted after the current task [Cancel]`.
- The button must remain on the hint row, not move to a separate row or a queued-message row.
- Clicking `[Cancel]` clears pending messages only and never interrupts the active task.
- Narrow terminals must hide `[Cancel]` and clear `pending_cancel_btn_area`.
- Do not change Enter queueing, automatic flush, `/cancel`, Normal-mode `c`, or Esc semantics.
- Do not stage or modify the existing untracked `docs/diagrams/` directory.
- Run Cargo commands sequentially; unset `http_proxy`, `https_proxy`, and `all_proxy` for local tests.

---

## File Map

- **Modify:** `crates/tui/src/render/input.rs` — reposition the button rectangle, replace the flexible gap, and strengthen the pending-block rendering assertion.
- **Modify:** `book/23_chapter_tui.md` — document that the hint-row `[Cancel]` follows the hint text directly.
- **Modify:** `book/23_chapter_tui_zh.md` — keep the Chinese TUI behavior description aligned with English.
- **Modify:** `book/26_chapter_issue.md` — update the existing 2026-08-17 pending-button entry with the new UX decision.
- **Modify:** `book/26_chapter_issue_zh.md` — apply the same issue-log update in Chinese.

No new production files or dependencies are needed. `crates/tui/src/handlers/mouse.rs` and `crates/tui/src/widgets/state/app/pending.rs` remain unchanged because their contracts already use the rendered hit rectangle and invalidate it correctly.

---

### Task 1: Add the failing adjacency assertion

**Files:**
- Modify: `crates/tui/src/render/input.rs` in `input_box_renders_pending_block_above_input`

**Interfaces:**
- Consumes: existing `render_input_box`, `buffer_text`, and `app.msgs().pending_submit_hint` test helpers.
- Produces: a regression test that fails while `[Cancel]` is separated from the hint by a flexible right-alignment gap.

- [ ] **Step 1: Add an exact adjacency assertion**

After obtaining `text`, keep the existing visibility assertions and add:

```rust
let hint_line = text
    .lines()
    .find(|line| line.contains(app.msgs().pending_submit_hint))
    .expect("pending hint must be rendered on one line");
let expected_hint_with_cancel = format!("{} [Cancel]", app.msgs().pending_submit_hint);
assert!(
    hint_line.contains(&expected_hint_with_cancel),
    "Cancel must immediately follow the hint text: {hint_line:?}"
);
```

- [ ] **Step 2: Run the regression test and verify the expected failure**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui input_box_renders_pending_block_above_input -- --exact --nocapture
```

Expected result: **FAIL** with `Cancel must immediately follow the hint text`, because the current renderer inserts a flexible gap before the button.

---

### Task 2: Move the button beside the hint text

**Files:**
- Modify: `crates/tui/src/render/input.rs:249-302`

**Interfaces:**
- Consumes: `App::pending_messages`, `App::msgs()`, `truncate_to_width`, and the existing pending-block area.
- Produces: the same `pending_cancel_btn_area` contract with a new `x` coordinate immediately after the rendered hint.

- [ ] **Step 1: Compute the hint before the button rectangle**

Replace the current right-aligned calculation with:

```rust
let can_show_cancel =
    !app.pending_messages.is_empty() && inner_width >= cancel_width + 30;
let hint = app.msgs().pending_submit_hint;
let hint_max = if can_show_cancel {
    inner_width.saturating_sub(cancel_width).saturating_sub(1)
} else {
    inner_width
};
let hint_text = truncate_to_width(hint, hint_max);
let hint_width = UnicodeWidthStr::width(hint_text.as_str());
let cancel_area = if can_show_cancel {
    Rect::new(
        area.x + hint_width as u16 + 1,
        area.y,
        cancel_width as u16,
        1,
    )
} else {
    Rect::default()
};
app.set_cancel_button_area(cancel_area);
```

The reserved width guarantees that the hint, one space, and `[Cancel]` fit on the row. The button `x` coordinate is derived from the actual rendered hint width rather than from the right edge.

- [ ] **Step 2: Render exactly one gap column before `[Cancel]`**

Replace the current flexible `gap` calculation with:

```rust
if !cancel_area.is_empty() {
    lines[0].spans.push(Span::styled(
        " ",
        Style::default().bg(app.theme.input_box_bg),
    ));
    lines[0].spans.push(Span::styled(
        format!("[{cancel_label}]"),
        Style::default()
            .fg(app.theme.warning)
            .bg(app.theme.input_box_bg),
    ));
}
```

Do not change the queued-message rows or the pending-block background fill.

- [ ] **Step 3: Run focused rendering and mouse tests**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui pending -- --nocapture
```

Expected result: all pending render and mouse tests pass, including the exact adjacency assertion, narrow-terminal hiding, rendered hit-area click, queue clearing, and active-task preservation.

---

### Task 3: Synchronize user-facing documentation

**Files:**
- Modify: `book/23_chapter_tui.md:341`
- Modify: `book/23_chapter_tui_zh.md:338`
- Modify: `book/26_chapter_issue.md:32-50`
- Modify: `book/26_chapter_issue_zh.md:32-50`

**Interfaces:**
- Consumes: the implemented layout and unchanged queue semantics.
- Produces: bilingual documentation that no longer describes a far-right button or a hit-area workaround.

- [ ] **Step 1: Update Ch 23 English**

In the queued-message paragraph, state that the pending hint row renders `[Cancel]` immediately after the hint text when width permits; narrow terminals hide it. Keep the existing statements about clearing only the queue and `/cancel` being unrelated.

- [ ] **Step 2: Update Ch 23 Chinese**

Make the structurally corresponding change in the Chinese queued-message paragraph: the hint 文案后紧接可点击 `[Cancel]`，窄终端隐藏；保留原有队列和 `/cancel` 语义。

- [ ] **Step 3: Rewrite the existing Ch 26 English entry**

Keep the existing 2026-08-17 entry and replace its symptom/decision/behavior text with:

```markdown
**Symptom / motivation:** The pending queue's `[Cancel]` control was right-aligned at the far edge of the hint row, making it awkward to reach even though it only controls the nearby pending prompt block.

**Decision:** Reserve the button width while truncating the hint, then render `[Cancel]` immediately after the hint text with one space. Keep the existing render-time hit rectangle and queue-only cancellation semantics.

**Behavior after:** On wide terminals the hint reads `Message will be submitted after the current task [Cancel]`; the button is directly beside the explanatory text, clicks clear only queued prompts, and narrow terminals still hide it.
```

- [ ] **Step 4: Rewrite the corresponding Ch 26 Chinese entry**

Use the same facts in Chinese: the old far-right placement was hard to operate; the new layout reserves width, places `[Cancel]` directly after the hint 文案, preserves queue-only cancellation, and hides the button on narrow terminals.

- [ ] **Step 5: Check documentation structure and whitespace**

Run:

```bash
git diff --check
```

Expected result: no whitespace errors, with English and Chinese Ch 23 sections and Ch 26 entries retaining matching structure.

---

### Task 4: Run the complete verification gate

**Files:**
- Test only; no additional source files.

- [ ] **Step 1: Check formatting**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo fmt -- --check
```

Expected: no output and exit code 0.

- [ ] **Step 2: Run Clippy with warnings denied**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo clippy --all-targets -- -D warnings
```

Expected: all workspace targets finish without warnings or errors.

- [ ] **Step 3: Run the repository package tests**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact-ui -p tui -p tact -p tact_llm --verbose
```

Expected: every test result reports zero failures.

- [ ] **Step 4: Review the final diff**

```bash
git diff --check && git status --short --branch
```

Confirm that only the renderer, bilingual book files, and bilingual issue-log files are staged; leave `docs/diagrams/` untracked.

- [ ] **Step 5: Commit the implementation**

```bash
git add crates/tui/src/render/input.rs book/23_chapter_tui.md book/23_chapter_tui_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
git commit -m "fix(tui): place pending cancel beside hint"
```

Do not use `git add -A`, so the existing untracked diagram exports cannot enter the commit.
