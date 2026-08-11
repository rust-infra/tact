# Persistent Task Manager
> Language: [English](./19_chapter_persistent_tasks.md) · [中文](./19_chapter_persistent_tasks_zh.md)

This chapter covers Tact's **durable work-item tracker**: the `task/` module, SQLite storage in `.tact/tact.db`, and the four agent tools `task_create`, `task_get`, `task_list`, and `task_update`.

This is **not** the same as:

- [Ch 11 Tool Scheduling](./11_chapter_task.md) — parallel **tool** wave execution in one LLM turn
- [Ch 12 Subagents](./12_chapter_subagent.md) — the `spawn_subagent` **tool** that spawns a nested agent

Implementation: `crates/tact/src/task/mod.rs`, tool wrappers in `crates/tact/src/tool/task.rs`.

---

## 1. Purpose

The TaskManager gives the LLM a **persistent checklist** across turns and sessions:

- Create items with subject / optional description
- Track status: Pending → InProgress → Completed / Deleted
- Assign an `owner` string (convention for teammates — not enforced)
- Model **dependencies** via `blockedBy` / `blocks` edges

Storage uses the SQLite [TaskStore](./01_chapter_store.md#6-session-store-sqlite) in the same `tact.db` as the session store (tables `tasks` + `task_dependencies`). Cron and background tasks still use the JSON [CollectionStore](./01_chapter_store.md).

---

## 2. Data Model

```rust
pub enum TaskStatus {
    Pending,      // marker [ ]
    InProgress,   // marker [>]
    Completed,    // marker [x]
    Deleted,      // marker [-]
}

pub struct TaskRecord {
    pub id: u64,
    pub subject: String,
    pub description: Option<String>,
    pub session_id: String,   // owning agent session; '' outside a session
    pub status: TaskStatus,
    pub blocked_by: Vec<u64>, // serialized as blockedBy
    pub blocks: Vec<u64>,
    pub owner: String,
}
```

IDs are assigned by SQLite `INTEGER PRIMARY KEY AUTOINCREMENT` (no reuse, starts at 1). `task_create` fills `session_id` from the tool context session. The legacy JSON layout (`.tact/tasks/*.json` + `index.json`) is no longer read.

---

## 3. Storage Layout

SQLite tables in `<workdir>/.tact/tact.db` (schema created by `SqliteTaskStore::new`):

```sql
CREATE TABLE tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    subject      TEXT    NOT NULL,
    description  TEXT,
    session_id   TEXT    NOT NULL DEFAULT '',
    status       TEXT    NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending','in_progress','completed','deleted')),
    owner        TEXT    NOT NULL DEFAULT '',
    created_at   INTEGER NOT NULL,
    started_at   INTEGER,
    completed_at INTEGER
);
CREATE INDEX idx_tasks_session_id ON tasks(session_id);

CREATE TABLE task_dependencies (
    blocker_id INTEGER NOT NULL,
    blocked_id INTEGER NOT NULL,
    PRIMARY KEY (blocker_id, blocked_id)
);
CREATE INDEX idx_task_deps_blocked ON task_dependencies(blocked_id);
```

No foreign keys — dependency edges are cleaned up by the application (same convention as the session store). Connection PRAGMAs: `busy_timeout = 5000` for cross-process writers.

---

## 4. Lifecycle Operations

| API | Behavior |
|-----|----------|
| `create(subject, description, session_id)` | Inserts record as `Pending`, id auto-assigned |
| `get(id)` | Loads single record |
| `list()` | Loads all records, sorted by id |
| `update(id, TaskUpdate)` | Patches status, owner, dependency edges |
| `delete(id)` | Sets status to `Deleted` (soft delete) |

### Dependency updates

Edges live only in `task_dependencies` (no mirrored fields). `add_blocks: [B]` on task A inserts one row `(A, B)` inside a `BEGIN IMMEDIATE` transaction; the reverse edge is derived when reading. When a task is marked **`Completed`**, the same transaction deletes all its edges (`DELETE ... WHERE blocker_id = ? OR blocked_id = ?`) — no full scan, no ghost edges.

---

## 5. Agent Tools

| Tool | Input highlights | Output |
|------|------------------|--------|
| `task_create` | `subject`, optional `description` | Pretty JSON of new record |
| `task_get` | `task_id` | Pretty JSON |
| `task_list` | (empty object) | Human-readable list with markers |
| `task_update` | `task_id`, optional `status`, `owner`, `addBlockedBy`, `addBlocks` | Pretty JSON |

Status strings for `task_update`: `pending`, `in_progress`, `completed`, `deleted` (snake_case via `strum`).

Example list line:

```text
[>] #3: Implement auth owner=alice (blocked by: [1])
```

Empty list returns `"No tasks."`.

---

## 6. Wiring

```rust
// tui.rs startup (async)
let task_manager = SharedTaskManager::new(TaskManager::new(&tact_path.session_db_path()).await?);

// ToolContext
pub task_manager: SharedTaskManager,
```

`SharedTaskManager` wraps `Arc<TaskManager>` — the SQLite pool already serializes writes, so no mutex is needed. All four tools share the same manager through `ToolContext`.

Registered in main `toolset()` only — **not** in `subagent_toolset()`.

Scheduling: all four tools share a synthetic write scope (`__tact_tasks__`) in `crates/tact/src/agent/tool_schedule.rs`, so they **serialize with each other** in one LLM turn (no parallel `task_update` races) while still allowing overlap with unrelated file reads.

---

## 7. Rendering Helpers

```rust
pub fn render_task_json(task: &TaskRecord) -> Result<String>;
pub fn render_task_list(tasks: Vec<TaskRecord>) -> String;
```

Tools return these strings directly as tool results (JSON for create/get/update, text list for `task_list`).

Successful `task_create` / `task_update` also emit [`AgentUpdate::TasksChanged`](./25_chapter_protocol.md) (filtered non-deleted snapshots) so the TUI can refresh a sticky progress strip under the Log and append a Log detail card. `task_get` / `task_list` do not emit. Spec: `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md`.

---

## 8. Code Map

| File | Role |
|------|------|
| `crates/tact/src/task/mod.rs` | `TaskManager` facade over `Box<dyn TaskStore>`, `TaskRecord`, render helpers |
| `crates/tact/src/store/task_store/mod.rs` | `TaskStore` trait |
| `crates/tact/src/store/task_store/sqlite.rs` | `SqliteTaskStore` — schema, transactions, edge queries |
| `crates/tact/src/tool/task.rs` | Four `#[tool]` handlers |
| `crates/tact/src/tool/mod.rs` | `ToolContext.task_manager` |
| `crates/tact/src/tool/registry.rs` | Task tools in `toolset()` |
| `crates/tact/src/store/` | `CollectionStore`, `Store` primitives |

---

## 9. Current Gaps

| Gap | Detail |
|-----|--------|
| **No `task_delete` tool** | Soft delete exists on manager API but no exposed tool (use `status: deleted` via update) |
| **Owner is opaque string** | Not linked to [Team](./14_chapter_team.md) roster validation |
| **No automatic unblocking rules** | Only completion clears edges; deleted blockers leave stale edges |
| **List order fixed by id** | No priority or due date fields |
| **Ch 1 cross-link was misleading** | Previously pointed at Ch 11 scheduling — now corrected in store chapter |

---

## Related Docs

- [Store and Persistence](./01_chapter_store.md) — `CollectionStore` / `Store` backing
- [Tasks and Tool Scheduling](./11_chapter_task.md) — unrelated parallel tool waves
- [Subagents](./12_chapter_subagent.md) — `spawn_subagent` runs a nested agent; finishing it does **not** complete a task record
- [Team Coordination](./14_chapter_team.md) — optional owner naming convention
- [Worktree Lanes](./15_chapter_worktree.md) — optional `task_id` link on worktree create
