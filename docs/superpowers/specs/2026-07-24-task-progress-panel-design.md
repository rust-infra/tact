# Task Progress Panel Design

> Date: 2026-07-24  
> Status: approved — plan ready  
> Related: `book/19_chapter_persistent_tasks.md`, `book/23_chapter_tui.md`, `book/25_chapter_protocol.md`  
> Plan: `docs/superpowers/plans/2026-07-24-task-progress-panel.md`

## Goal

Give persistent tasks (`task_create` / `task_update` / …) a dedicated TUI progress surface without digging into the Log scroll/wrap pipeline: a **sticky strip** visually attached to the bottom of the Log region (outer layout split), plus a **Log timeline detail card** on each progress change. Drive both from one protocol event.

## Problem

`TaskManager` and the four agent tools already persist work items under `.tact/tasks/`, but the TUI has no progress chrome. Users only see ordinary tool JSON/text in the Log. There is no always-visible “where are we” checklist, and no structured timeline card for task mutations.

## Decision summary

| Choice | Decision |
|--------|----------|
| Data path | **`AgentUpdate::TasksChanged`** snapshot from agent after successful mutating tools (not TUI parsing / not direct store reads) |
| Sticky placement | **Outer split** of the main content area: `scrollable log Rect` + optional `sticky tasks Rect` — Log internals unchanged |
| Visual framing | One continuous main block (Log chrome / shared region); sticky sits at its bottom so it does not feel like a third sandwich strip between Log and Input |
| Sticky default | **Collapsed** one-liner; **click title row** to expand |
| Log on update | Each `TasksChanged` **appends** a detail card (does not replace the `task_*` tool row) |
| Hide sticky when | No unfinished items (`pending` + `in_progress` empty); clear expand state |
| Session gate | **Do not** show sticky on resume/startup from disk; wait until first `TasksChanged` in **this** session |
| Read-only tools | `task_get` / `task_list` do **not** emit `TasksChanged` |
| Out of scope (v1) | Bottom-bar task summary, `/tasks` popup, click-to-edit status, auto-expand-on-update, changing Log Phase 0–3 / wrap caches |

---

## 1. Protocol

Add types in `crates/protocol` (TUI depends on protocol only, not `tact::task`):

```rust
pub enum TasksChangeReason {
    Created,
    Updated,
}

pub struct TaskSnapshot {
    pub id: u64,
    pub subject: String,
    pub status: TaskStatusSnapshot, // Pending | InProgress | Completed
    pub owner: String,              // optional on wire; may be empty
}

// AgentUpdate arm:
TasksChanged {
    tasks: Vec<TaskSnapshot>, // non-deleted only
    reason: TasksChangeReason,
}
```

Notes:

- `Deleted` records are omitted from `tasks` (soft-delete via `task_update` still emits `TasksChanged` with the filtered list).
- Snapshot omits full `description` text; UI uses `subject` + status markers.
- Wire `TaskStatusSnapshot` may mirror `tact`’s snake_case statuses used for UI markers `[ ]` / `[>]` / `[x]`.

---

## 2. Emit points (`tact`)

After a successful:

- `task_create`
- `task_update` (including `status: deleted`)

…map current manager list → filtered snapshots → `emit(AgentUpdate::TasksChanged { … })` on the same update channel as other tool lifecycle events, ordered next to the corresponding step completion so the TUI can show tool row + tasks card coherently.

Do **not** emit on:

- `task_get`, `task_list`
- failed create/update

---

## 3. TUI state

```text
task_panel:
  snapshot: Vec<TaskSnapshot>
  session_seen: bool      // set true on first TasksChanged this process/session UI
  visible: bool           // session_seen && has_open_items(snapshot)
  expanded: bool          // default false; click toggles
  mouse_area: Rect        // sticky hit target
```

`has_open_items` = any status in `{ Pending, InProgress }`.

### Handler for `TasksChanged`

1. Replace `snapshot`
2. `session_seen = true`
3. Recompute `visible`; if becoming hidden, set `expanded = false`
4. Append Log detail message/card from snapshot (+ reason for title copy)
5. `dirty = true` (do not force `expanded`)

---

## 4. Layout (outer split only)

Current vertical stack remains conceptually:

```text
top bar | main | input | bottom bar
```

Inside `main` (today: full Log):

```text
┌─ main (shared visual block) ─────────────┐
│  render_log_panel(shorter Rect)          │  // existing pipeline
│  ─ sticky tasks (0 rows if !visible) ─   │  // new renderer
└──────────────────────────────────────────┘
```

Implementation touchpoints:

- `crates/tui/src/lib.rs` draw path and/or `render_main_area` / test harness layout helpers
- New small module e.g. `render/task_panel.rs` (name flexible)
- Mouse handler: clicks in `task_panel.mouse_area` toggle expand; wheel continues to target Log when over log Rect

**Do not** reserve space inside `render_log_panel` visual viewport, wrap caches, or logical scroll math.

Sticky height:

- Collapsed: **1** row (title summary)
- Expanded: **1 + min(snapshot.len(), CAP)** with `CAP = 6` (snapshot is already non-deleted; includes completed items for context); overflow shown as `… +K`

---

## 5. Sticky UI copy

Collapsed:

```text
▸ Tasks {completed}/{completed+open} · {focus_subject}     ▼
```

- `focus_subject`: first `InProgress` subject, else first `Pending`
- If somehow visible with no focus (should not happen under hide rule), show counts only

Expanded: title row + lines using existing markers:

```text
[x] …
[>] …
[ ] …
… +K
```

v1: rows are display-only (no per-row click actions).

---

## 6. Log detail card

On each `TasksChanged`, append a dedicated Log entry (system/card style — follow nearest existing pattern for non-tool structured rows):

- Title like `Tasks · {completed}/{total_non_deleted} updated` (adjust for `Created` vs `Updated` if i18n needs distinct strings)
- Body: compact checklist, same markers, same cap / `… +K`
- Prefer highlighting the in-progress / just-changed subject when cheap; full diff UI is out of scope
- Does **not** replace or suppress the normal `task_*` tool cell

Uses existing message append APIs so scroll/history behave like other Log rows.

---

## 7. Testing

| Layer | Cases |
|-------|--------|
| protocol | `TasksChanged` constructible / matchable |
| tool/agent | create/update success emits; get/list do not; delete via update emits filtered list |
| TUI | first change shows sticky; all complete hides; click toggles expand; Log gains a detail row; existing log tests still pass with shorter log Rect |

---

## 8. Docs (same change or immediately after)

| Doc | Update |
|-----|--------|
| `book/19_chapter_persistent_tasks*.md` | Note UI surfaces + `TasksChanged`; gap “no TUI” removed/narrowed |
| `book/23_chapter_tui*.md` | Main-area outer split + sticky panel |
| `book/25_chapter_protocol*.md` | New `AgentUpdate` arm |
| `book/26_chapter_issue*.md` | Newest-first entry when behavior ships |

---

## 9. Non-goals / follow-ups

- Statusline / bottom-bar duplicate summary
- `/tasks` full-screen popup
- Editing tasks from the sticky panel
- Auto flash-expand on update then collapse
- Embedding sticky inside Log Phase 0–3

---

## Open items for implementation plan only

- Exact i18n string keys (EN/ZH)
- Whether `owner` is shown on expanded rows in v1 (default: omit if empty)
- Precise message-type enum variant name for the Log detail row
