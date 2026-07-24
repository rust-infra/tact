# Task Progress Panel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface persistent-task progress in the TUI via `AgentUpdate::TasksChanged`: a sticky strip under the Log (outer layout split, Log internals unchanged) plus a Log detail card on each mutating update.

**Architecture:** Protocol carries filtered task snapshots. `task_create` / `task_update` emit over `ToolContext.ui_tx` after success. TUI keeps `task_panel` state, appends a system Log row, and splits `render_main_area` into `log Rect` + optional sticky Rect. Clicks on the sticky title toggle expand.

**Tech Stack:** Rust, `tact_protocol::AgentUpdate`, ratatui layout/`Paragraph`, existing TUI `add_system_message` / mouse hit helpers.

**Spec:** `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md`

## File map

| File | Responsibility |
|------|----------------|
| `crates/protocol/src/agent.rs` | `TaskStatusSnapshot`, `TaskSnapshot`, `TasksChangeReason`, `AgentUpdate::TasksChanged` |
| `crates/protocol/src/lib.rs` | Re-export new types |
| `crates/tact/src/task/mod.rs` | `to_snapshots` / `emit_tasks_changed` helpers (filter deleted, map records) |
| `crates/tact/src/tool/task.rs` | Call emit after successful create/update |
| `crates/tui/src/widgets/state/task_panel.rs` | **Create** — panel state + height/format helpers |
| `crates/tui/src/widgets/state/mod.rs` | Wire `task_panel` module + `App.task_panel` |
| `crates/tui/src/widgets/state/app/construct.rs` | Default `TaskPanelState` |
| `crates/tui/src/widgets/state/app/agent.rs` | Handle `TasksChanged` |
| `crates/tui/src/widgets/state/mouse_state.rs` | `task_panel_area: Rect` |
| `crates/tui/src/render/task_panel.rs` | **Create** — sticky renderer |
| `crates/tui/src/render/mod.rs` | Export task panel render |
| `crates/tui/src/render/layout.rs` | Outer split before `render_log_panel` |
| `crates/tui/src/render/test_harness.rs` | Same split so scene tests match production |
| `crates/tui/src/handlers/mouse.rs` | Hit sticky → toggle expand |
| `crates/tui/src/i18n.rs` | EN/ZH strings for Log card + sticky title |
| `book/19_chapter_persistent_tasks*.md` | UI + event note; narrow “no TUI” gap |
| `book/23_chapter_tui*.md` | Main-area sticky strip |
| `book/25_chapter_protocol*.md` | `TasksChanged` |
| `book/26_chapter_issue*.md` | Newest-first changelog when shipping |

## Global constraints

- Do **not** change Log Phase 0–3 / wrap caches / visual scroll math.
- Do **not** emit on `task_get` / `task_list` / failed mutations.
- Do **not** show sticky on startup/resume from disk (`session_seen` starts false).
- Sticky hide when no `Pending`/`InProgress`; clear `expanded`.
- Expand cap: title + `min(snapshot.len(), 6)` body rows; overflow `… +K`.
- `owner`: omit from sticky/Log lines when empty (v1).
- `docs/superpowers/` is gitignored — use `git add -f` when committing specs/plans under that tree.

---

### Task 1: Protocol types

**Files:**
- Modify: `crates/protocol/src/agent.rs`
- Modify: `crates/protocol/src/lib.rs`
- Test: add unit test in `crates/protocol/src/agent.rs` `#[cfg(test)]` (or existing test module)

- [ ] **Step 1: Write failing test for `TasksChanged` construction**

```rust
#[test]
fn tasks_changed_snapshot_round_trips_fields() {
    let update = AgentUpdate::TasksChanged {
        tasks: vec![TaskSnapshot {
            id: 1,
            subject: "Fix auth".into(),
            status: TaskStatusSnapshot::InProgress,
            owner: String::new(),
        }],
        reason: TasksChangeReason::Created,
    };
    match update {
        AgentUpdate::TasksChanged { tasks, reason } => {
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].id, 1);
            assert_eq!(tasks[0].status, TaskStatusSnapshot::InProgress);
            assert!(matches!(reason, TasksChangeReason::Created));
        }
        other => panic!("unexpected {other:?}"),
    }
}
```

- [ ] **Step 2: Run test — expect compile/link failure**

Run: `cargo test -p tact_protocol tasks_changed_snapshot_round_trips_fields -- --nocapture`  
Expected: FAIL (unknown types / unknown enum variant)

- [ ] **Step 3: Add types + enum arm**

In `crates/protocol/src/agent.rs` (near other public types):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatusSnapshot {
    Pending,
    InProgress,
    Completed,
}

impl TaskStatusSnapshot {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[>]",
            Self::Completed => "[x]",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksChangeReason {
    Created,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub id: u64,
    pub subject: String,
    pub status: TaskStatusSnapshot,
    pub owner: String,
}
```

Add to `AgentUpdate`:

```rust
    /// Persistent task list changed (`task_create` / `task_update`).
    /// `tasks` excludes soft-deleted records.
    TasksChanged {
        tasks: Vec<TaskSnapshot>,
        reason: TasksChangeReason,
    },
```

Re-export from `crates/protocol/src/lib.rs`:

```rust
pub use agent::{
    AgentErrorKind, AgentUpdate, ModelCallParams, PlanStep, StepResult, StepStatus,
    TaskSnapshot, TaskStatusSnapshot, TasksChangeReason, ThinkingChunk, TokenUsageInfo,
    UserCommand,
};
```

Fix any exhaustive `match` on `AgentUpdate` that the compiler flags (ignore arm or handle explicitly). Prefer `_` only where already present; in TUI `handle_agent_update` add a real arm in Task 3.

- [ ] **Step 4: Run test — expect PASS**

Run: `cargo test -p tact_protocol tasks_changed_snapshot_round_trips_fields -- --nocapture`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/protocol/src/agent.rs crates/protocol/src/lib.rs
# plus any compile-fix match arms required by this task
git commit -m "$(cat <<'EOF'
feat(protocol): add AgentUpdate::TasksChanged snapshots

EOF
)"
```

---

### Task 2: Emit from `task_create` / `task_update`

**Files:**
- Modify: `crates/tact/src/task/mod.rs`
- Modify: `crates/tact/src/tool/task.rs`
- Test: `crates/tact/src/tool/task.rs` (extend existing `#[cfg(test)]`) and/or `crates/tact/src/task/mod.rs` tests

- [ ] **Step 1: Write failing tests**

In `crates/tact/src/task/mod.rs` tests:

```rust
#[test]
fn to_ui_snapshots_filters_deleted_and_maps_status() {
    let mut pending = TaskRecord::new(1, "a".into(), None);
    let mut active = TaskRecord::new(2, "b".into(), None);
    active.status = TaskStatus::InProgress;
    let mut gone = TaskRecord::new(3, "c".into(), None);
    gone.status = TaskStatus::Deleted;
    let snaps = to_ui_snapshots(vec![pending, active, gone]);
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[1].status, tact_protocol::TaskStatusSnapshot::InProgress);
}
```

In `crates/tact/src/tool/task.rs` tests (pattern like existing router tests; attach `ui_tx`):

```rust
#[tokio::test]
async fn task_create_emits_tasks_changed() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let router = ToolRouter::new().route(TaskCreateTool);
    let mut context = test_context("task_create_emits");
    context.ui_tx = Some(tx);

    router
        .call(
            &context,
            "task_create",
            serde_json::json!({ "subject": "Ship panel" }),
        )
        .await
        .unwrap();

    let update = rx.try_recv().expect("TasksChanged");
    match update {
        AgentUpdate::TasksChanged { tasks, reason } => {
            assert!(matches!(reason, TasksChangeReason::Created));
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].subject, "Ship panel");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn task_list_does_not_emit_tasks_changed() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let router = ToolRouter::new()
        .route(TaskCreateTool)
        .route(TaskListTool);
    let mut context = test_context("task_list_no_emit");
    context.ui_tx = Some(tx.clone());
    let _ = router
        .call(&context, "task_create", serde_json::json!({ "subject": "x" }))
        .await
        .unwrap();
    while rx.try_recv().is_ok() {}

    context.ui_tx = Some(tx);
    router
        .call(&context, "task_list", serde_json::json!({}))
        .await
        .unwrap();
    assert!(rx.try_recv().is_err(), "task_list must not emit");
}
```

Also add `task_update` emit / filter-deleted coverage similarly (`TasksChangeReason::Updated`).

- [ ] **Step 2: Run tests — expect FAIL**

Run: `cargo test -p tact to_ui_snapshots_filters_deleted -- --nocapture`  
Run: `cargo test -p tact task_create_emits_tasks_changed -- --nocapture`  
Expected: FAIL (missing helpers / no emit)

- [ ] **Step 3: Implement helpers + emit**

In `crates/tact/src/task/mod.rs`:

```rust
pub fn to_ui_snapshots(tasks: Vec<TaskRecord>) -> Vec<tact_protocol::TaskSnapshot> {
    tasks
        .into_iter()
        .filter(|t| t.status != TaskStatus::Deleted)
        .map(|t| tact_protocol::TaskSnapshot {
            id: t.id,
            subject: t.subject,
            status: match t.status {
                TaskStatus::Pending => tact_protocol::TaskStatusSnapshot::Pending,
                TaskStatus::InProgress => tact_protocol::TaskStatusSnapshot::InProgress,
                TaskStatus::Completed => tact_protocol::TaskStatusSnapshot::Completed,
                TaskStatus::Deleted => unreachable!(),
            },
            owner: t.owner,
        })
        .collect()
}

pub fn emit_tasks_changed(
    ui_tx: &Option<tokio::sync::mpsc::UnboundedSender<tact_protocol::AgentUpdate>>,
    tasks: Vec<TaskRecord>,
    reason: tact_protocol::TasksChangeReason,
) {
    let Some(tx) = ui_tx else { return };
    let _ = tx.send(tact_protocol::AgentUpdate::TasksChanged {
        tasks: to_ui_snapshots(tasks),
        reason,
    });
}
```

In `task_create` after `create(...)?`:

```rust
let listed = ctx.task_manager.list().unwrap_or_default();
crate::task::emit_tasks_changed(&ctx.ui_tx, listed, TasksChangeReason::Created);
```

In `task_update` after successful `update(...)?` use `TasksChangeReason::Updated` and `list()`.

Keep returning `render_task_json` as today (tool result unchanged).

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p tact task_create_emits_tasks_changed task_list_does_not_emit -- --nocapture`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tact/src/task/mod.rs crates/tact/src/tool/task.rs
git commit -m "$(cat <<'EOF'
feat(tact): emit TasksChanged from task_create/update

EOF
)"
```

---

### Task 3: TUI state + Log detail on `TasksChanged`

**Files:**
- Create: `crates/tui/src/widgets/state/task_panel.rs`
- Modify: `crates/tui/src/widgets/state/mod.rs`
- Modify: `crates/tui/src/widgets/state/app/construct.rs`
- Modify: `crates/tui/src/widgets/state/app/agent.rs`
- Modify: `crates/tui/src/i18n.rs`
- Test: unit tests in `task_panel.rs` + handler tests in `agent.rs`

- [ ] **Step 1: Write failing pure-helper + handler tests**

In `task_panel.rs`:

```rust
#[test]
fn has_open_items_detects_pending_and_in_progress() {
    assert!(!has_open_items(&[]));
    assert!(has_open_items(&[snap(1, TaskStatusSnapshot::Pending)]));
    assert!(!has_open_items(&[snap(1, TaskStatusSnapshot::Completed)]));
}

#[test]
fn sticky_height_collapsed_and_expanded_cap() {
    let many: Vec<_> = (0..10).map(|i| snap(i, TaskStatusSnapshot::Pending)).collect();
    assert_eq!(sticky_height(false, &many), 1);
    assert_eq!(sticky_height(true, &many), 1 + 6); // CAP=6
}

#[test]
fn format_checklist_caps_with_ellipsis() {
    let many: Vec<_> = (0..8)
        .map(|i| snap(i, TaskStatusSnapshot::Pending))
        .collect();
    let text = format_checklist_lines(&many, 6).join("\n");
    assert!(text.contains("… +2") || text.contains("... +2"));
}
```

In `agent.rs` tests:

```rust
#[test]
fn tasks_changed_shows_panel_and_appends_log() {
    let mut app = /* make_app or existing test harness */;
    assert!(!app.task_panel.visible);
    app.handle_agent_update(AgentUpdate::TasksChanged {
        tasks: vec![TaskSnapshot {
            id: 1,
            subject: "Fix auth".into(),
            status: TaskStatusSnapshot::InProgress,
            owner: String::new(),
        }],
        reason: TasksChangeReason::Created,
    });
    assert!(app.task_panel.session_seen);
    assert!(app.task_panel.visible);
    assert!(!app.task_panel.expanded);
    assert!(
        app.raw_messages.iter().any(|m| m.contains("Fix auth")),
        "Log detail should mention subject"
    );
}

#[test]
fn tasks_changed_hides_when_no_open_items() {
    let mut app = /* … */;
    app.handle_agent_update(AgentUpdate::TasksChanged {
        tasks: vec![TaskSnapshot {
            id: 1,
            subject: "done".into(),
            status: TaskStatusSnapshot::Completed,
            owner: String::new(),
        }],
        reason: TasksChangeReason::Updated,
    });
    assert!(app.task_panel.session_seen);
    assert!(!app.task_panel.visible);
    assert!(!app.task_panel.expanded);
}
```

- [ ] **Step 2: Run tests — expect FAIL**

Run: `cargo test -p tui has_open_items_detects sticky_height tasks_changed_shows -- --nocapture`  
Expected: FAIL

- [ ] **Step 3: Implement state + handler + i18n**

`TaskPanelState` fields per spec: `snapshot`, `session_seen`, `visible`, `expanded`. Constants: `pub(crate) const STICKY_BODY_CAP: usize = 6`.

`apply_tasks_changed(&mut self, tasks, reason)`:
1. store snapshot  
2. `session_seen = true`  
3. `visible = has_open_items(&snapshot)`  
4. if `!visible { expanded = false }`  
5. build Log body via `format_tasks_log_card(msgs, reason, &snapshot)`  
6. `app.add_system_message(...)`  
7. caller sets `dirty` (already true at start of `handle_agent_update`)

i18n (EN/ZH) examples:
- `tasks_log_created_tmpl`: `"Tasks · {}/{} created"` / `"任务 · {}/{} 已创建"`
- `tasks_log_updated_tmpl`: `"Tasks · {}/{} updated"` / `"任务 · {}/{} 已更新"`
- `tasks_sticky_title`: `"Tasks"` / `"任务"`

Counts: `completed` / `snapshot.len()` (non-deleted).

Wire `AgentUpdate::TasksChanged { .. }` arm in `handle_agent_update` (content-producing — leave it in the flush-thinking / remove-placeholder path).

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p tui tasks_changed_shows_panel_and_appends_log tasks_changed_hides_when_no_open_items -- --nocapture`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/widgets/state/task_panel.rs \
  crates/tui/src/widgets/state/mod.rs \
  crates/tui/src/widgets/state/app/construct.rs \
  crates/tui/src/widgets/state/app/agent.rs \
  crates/tui/src/i18n.rs
git commit -m "$(cat <<'EOF'
feat(tui): apply TasksChanged to panel state and Log card

EOF
)"
```

---

### Task 4: Sticky render + outer layout split

**Files:**
- Create: `crates/tui/src/render/task_panel.rs`
- Modify: `crates/tui/src/render/mod.rs`
- Modify: `crates/tui/src/render/layout.rs`
- Modify: `crates/tui/src/render/test_harness.rs`
- Modify: `crates/tui/src/widgets/state/mouse_state.rs` (store `task_panel_area`)
- Test: scene/layout tests in `layout.rs` or `task_panel.rs`

- [ ] **Step 1: Write failing scene test**

```rust
#[test]
fn main_area_shows_sticky_when_tasks_visible() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::TasksChanged {
        tasks: vec![TaskSnapshot {
            id: 1,
            subject: "Fix auth".into(),
            status: TaskStatusSnapshot::InProgress,
            owner: String::new(),
        }],
        reason: TasksChangeReason::Created,
    });
    let text = render_app_text(&mut app, 100, 30);
    assert!(
        text.contains("Fix auth") && (text.contains("Tasks") || text.contains("任务")),
        "sticky/log should show task progress, got:\n{text}"
    );
}
```

- [ ] **Step 2: Run test — expect FAIL or missing sticky chrome**

Run: `cargo test -p tui main_area_shows_sticky_when_tasks_visible -- --nocapture`  
Expected: FAIL or assertion miss on sticky title row (Log card alone may already contain subject — assert sticky glyph `▸` / collapsed marker if Log card uses different prefix)

Prefer asserting collapsed marker `▸` only rendered by sticky:

```rust
assert!(text.contains('▸') || text.contains('▼'));
```

- [ ] **Step 3: Implement renderer + split**

`render_task_panel(frame, area, app)`:
- Collapsed: one `Paragraph` line `▸ {title} {done}/{total} · {focus}  ▼`
- Expanded: title + checklist lines from helpers
- Set `app.mouse.task_panel_area = area`

In `render_main_area` (and mirror in `test_harness`):

```rust
let sticky_h = if app.task_panel.visible {
    app.task_panel.sticky_height() as u16
} else {
    0
};
if sticky_h == 0 {
    app.mouse.task_panel_area = Rect::default();
    app.mouse.log_area = area;
    render_log_panel(frame, area, app);
} else {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(sticky_h)])
        .split(area);
    app.mouse.log_area = chunks[0];
    render_log_panel(frame, chunks[0], app);
    render_task_panel(frame, chunks[1], app);
}
```

Keep history/help early-returns unchanged (no sticky there).

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p tui main_area_shows_sticky_when_tasks_visible -- --nocapture`  
Also: `cargo test -p tui --lib` (smoke existing log tests)  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/render/task_panel.rs crates/tui/src/render/mod.rs \
  crates/tui/src/render/layout.rs crates/tui/src/render/test_harness.rs \
  crates/tui/src/widgets/state/mouse_state.rs
git commit -m "$(cat <<'EOF'
feat(tui): render sticky task strip under Log via outer split

EOF
)"
```

---

### Task 5: Click-to-expand

**Files:**
- Modify: `crates/tui/src/handlers/mouse.rs`
- Test: `crates/tui/src/handlers/mouse.rs` `#[cfg(test)]`

- [ ] **Step 1: Write failing mouse test**

```rust
#[test]
fn click_task_panel_toggles_expanded() {
    let mut app = make_app();
    app.task_panel.visible = true;
    app.task_panel.expanded = false;
    app.task_panel.snapshot = vec![/* one in_progress */];
    app.mouse.task_panel_area = Rect::new(0, 10, 40, 1);

    handle_mouse_event(&mut app, mouse_down(5, 10));
    assert!(app.task_panel.expanded);

    handle_mouse_event(&mut app, mouse_down(5, 10));
    assert!(!app.task_panel.expanded);
}
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo test -p tui click_task_panel_toggles_expanded -- --nocapture`  
Expected: FAIL

- [ ] **Step 3: Implement hit handling**

Extend `MousePanelHit` with `in_task_panel: bool`.

In `handle_mouse_down`, **before** log click:

```rust
if hit.in_task_panel && app.task_panel.visible {
    app.task_panel.expanded = !app.task_panel.expanded;
    app.dirty = true;
    return;
}
```

Scroll: only `in_log` scrolls Log (sticky wheel ignored / no-op is fine for v1).

- [ ] **Step 4: Run test — expect PASS**

Run: `cargo test -p tui click_task_panel_toggles_expanded -- --nocapture`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/handlers/mouse.rs
git commit -m "$(cat <<'EOF'
feat(tui): toggle task sticky expand on click

EOF
)"
```

---

### Task 6: Docs sync

**Files:**
- Modify: `book/19_chapter_persistent_tasks.md` + `_zh.md`
- Modify: `book/23_chapter_tui.md` + `_zh.md`
- Modify: `book/25_chapter_protocol.md` + `_zh.md`
- Modify: `book/26_chapter_issue.md` + `_zh.md`
- Modify: `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md` (status → implemented; plan link)

- [ ] **Step 1: Update subsystem chapters (bilingual, same headings)**

Ch 19: note `TasksChanged`, sticky under Log, Log detail card; remove/narrow gap “no TUI progress”.  
Ch 23: document main-area outer split + sticky collapse/expand.  
Ch 25: document `AgentUpdate::TasksChanged` fields.

- [ ] **Step 2: Add Ch 26 newest-first entry (both languages)**

Include: date `2026-07-24`, type `optimization` (or feature), symptom (tasks only in tool JSON), decision (sticky + Log card + event), pointers to Ch 19/23/25 and this spec/plan.

- [ ] **Step 3: Point spec at this plan; mark status**

```markdown
> Status: implemented  
> Plan: `docs/superpowers/plans/2026-07-24-task-progress-panel.md`
```

(Only mark `implemented` after code tasks 1–5 are merged; if docs land in the same PR as code, set it in the final docs commit.)

- [ ] **Step 4: Commit**

```bash
git add book/19_chapter_persistent_tasks.md book/19_chapter_persistent_tasks_zh.md \
  book/23_chapter_tui.md book/23_chapter_tui_zh.md \
  book/25_chapter_protocol.md book/25_chapter_protocol_zh.md \
  book/26_chapter_issue.md book/26_chapter_issue_zh.md
git add -f docs/superpowers/specs/2026-07-24-task-progress-panel-design.md
git commit -m "$(cat <<'EOF'
docs: sync task progress panel across book and spec

EOF
)"
```

---

## Spec coverage checklist

| Spec item | Task |
|-----------|------|
| `TasksChanged` protocol types | 1 |
| Emit on create/update; not get/list | 2 |
| Filter deleted; map statuses | 2 |
| Panel state + session gate + hide rule | 3 |
| Log detail card on each change | 3 |
| Outer layout split; Log internals untouched | 4 |
| Collapsed default; expand cap 6 | 3–4 |
| Click title toggles expand | 5 |
| Docs Ch 19/23/25/26 | 6 |
| No bottom-bar summary / `/tasks` / edit-in-panel | (non-goals — no task) |

## Placeholder / consistency notes

- Package name for protocol crate in commands: use workspace name from `Cargo.toml` (`tact_protocol` vs `protocol`) — adjust `cargo test -p …` to the actual package name if different.
- Ellipsis character: prefer `…` (U+2026) consistently in helpers and assertions.
- `make_app` / `render_app_text`: use existing `crates/tui/src/render/test_harness.rs` helpers.
