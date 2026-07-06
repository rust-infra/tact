# Persistent Task Manager

This chapter covers Tact's **durable work-item tracker**: the `task/` module, JSON file storage under `.claude/tasks/`, and the four agent tools `task_create`, `task_get`, `task_list`, and `task_update`.

This is **not** the same as:

- [Ch 11 Tool Scheduling](./11_chapter_task.md) — parallel **tool** wave execution in one LLM turn
- [Ch 12 Subagents](./12_chapter_subagent.md) — the `task` **tool** that spawns a nested agent

Implementation: `crates/tact/src/task/mod.rs`, tool wrappers in `crates/tact/src/tool/task.rs`.

---

## 1. Purpose

The TaskManager gives the LLM a **persistent checklist** across turns and sessions:

- Create items with subject / optional description
- Track status: Pending → InProgress → Completed / Deleted
- Assign an `owner` string (convention for teammates — not enforced)
- Model **dependencies** via `blockedBy` / `blocks` edges

Storage uses the same [CollectionStore](./01_chapter_store.md) primitives as cron and background tasks.

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
    pub status: TaskStatus,
    pub blocked_by: Vec<u64>,   // JSON: blockedBy
    pub blocks: Vec<u64>,
    pub owner: String,
}

pub struct TaskIndex {
    pub next_id: u64,
}
```

IDs monotonically increase from `next_id` in `tasks/index.json` (starts at 1).

---

## 3. Storage Layout

```text
.claude/
└── tasks/
    ├── index.json          # { "next_id": N }
    ├── task_1.json
    ├── task_2.json
    └── …
```

Each task is one JSON file keyed `task_{id}`. `TaskManager::new` initializes `index.json` if missing.

---

## 4. Lifecycle Operations

| API | Behavior |
|-----|----------|
| `create(subject, description)` | Allocates id, writes record as `Pending` |
| `get(id)` | Loads single record |
| `list()` | Loads all records, sorted by id |
| `update(id, TaskUpdate)` | Patches status, owner, dependency edges |
| `delete(id)` | Sets status to `Deleted` (soft delete) |

### Dependency updates

When `add_blocks: [B]` is applied on task A:

1. A's `blocks` list gains B
2. B's `blocked_by` list gains A (reverse edge written automatically)

When a task is marked **`Completed`**, `clear_dependency` removes its id from every other task's `blocked_by` list.

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
// tui.rs startup
let task_manager = SharedTaskManager::new(TaskManager::new(&store_root)?);

// ToolContext
pub task_manager: SharedTaskManager,
```

`SharedTaskManager` wraps `Arc<Mutex<TaskManager>>` — all four tools lock the same manager through `ToolContext`.

Registered in main `toolset()` only — **not** in `subagent_toolset()`.

Scheduling: treated as **independent** in `crates/tact/src/agent/tool_schedule.rs` (safe to parallelize with other non-conflicting reads/writes).

---

## 7. Rendering Helpers

```rust
pub fn render_task_json(task: &TaskRecord) -> Result<String>;
pub fn render_task_list(tasks: Vec<TaskRecord>) -> String;
```

Tools return these strings directly as tool results (JSON for create/get/update, text list for `task_list`).

---

## 8. Code Map

| File | Role |
|------|------|
| `crates/tact/src/task/mod.rs` | `TaskManager`, `TaskRecord`, dependency logic, render helpers |
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
| **No automatic unblocking rules** | Only completion clears `blocked_by`; deleted blockers leave stale edges |
| **List order fixed by id** | No priority or due date fields |
| **Ch 1 cross-link was misleading** | Previously pointed at Ch 11 scheduling — now corrected in store chapter |

---

## Related Docs

- [Store and Persistence](./01_chapter_store.md) — `CollectionStore` / `Store` backing
- [Tasks and Tool Scheduling](./11_chapter_task.md) — unrelated parallel tool waves
- [Subagents](./12_chapter_subagent.md) — `task` tool name collision
- [Team Coordination](./14_chapter_team.md) — optional owner naming convention
- [Worktree Lanes](./15_chapter_worktree.md) — optional `task_id` link on worktree create
