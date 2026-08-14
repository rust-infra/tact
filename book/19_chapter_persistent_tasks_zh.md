# 持久化任务管理器

> 语言：[中文](./19_chapter_persistent_tasks_zh.md) · [English](./19_chapter_persistent_tasks.md)

本章涵盖 Tact 的 **durable 工作项跟踪器**：`task/` 模块、`.tact/tact.db` 中的 SQLite 存储，以及四个 agent 工具 `task_create`、`task_get`、`task_list`、`task_update`。

这与以下 **不是** 同一概念：

- [第 11 章 工具调度](./11_chapter_task.md) — 一个 LLM turn 内的并行 **工具** wave 执行
- [第 12 章 Subagents](./12_chapter_subagent.md) — 生成嵌套 agent 的 `spawn_subagent` **工具**

实现：`crates/tact/src/task/mod.rs`，工具封装在 `crates/tact/src/tool/task.rs`。

---

## 1. 用途

TaskManager 给 LLM 一个跨 turn 和会话的 **持久化 checklist**：

- 用 subject / 可选 description 创建项
- 跟踪状态：Pending → InProgress → Completed / Deleted
- 分配 `owner` 字符串（队友约定——未强制）
- 通过 `blockedBy` / `blocks` 边建模 **依赖**

存储使用与会话存储同一个 `tact.db` 中的 SQLite [TaskStore](./01_chapter_store_zh.md#6-session-store-sqlite)（`tasks` + `task_dependencies` 表）。后台任务同样使用 SQLite `background_tasks` 表（见 [Ch 13](./13_chapter_background_zh.md)）。

---

## 2. 数据模型

```rust
pub enum TaskStatus {
    Pending,      // 标记 [ ]
    InProgress,   // 标记 [>]
    Completed,    // 标记 [x]
    Deleted,      // 标记 [-]
}

pub struct TaskRecord {
    pub id: u64,
    pub subject: String,
    pub description: Option<String>,
    pub session_id: String,   // 所属 agent 会话；会话外为空串
    pub status: TaskStatus,
    pub blocked_by: Vec<u64>, // 序列化为 blockedBy
    pub blocks: Vec<u64>,
    pub owner: String,
}
```

ID 由 SQLite `INTEGER PRIMARY KEY AUTOINCREMENT` 分配（不复用，从 1 开始）。`task_create` 从工具上下文填充 `session_id`。旧 JSON 布局（`.tact/tasks/*.json` + `index.json`）不再读取。

---

## 3. 存储布局

`<workdir>/.tact/tact.db` 中的 SQLite 表（由 `SqliteTaskStore::new` 建表）：

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

无外键——依赖边由应用层清理（与会话存储同一约定）。连接 PRAGMA：`busy_timeout = 5000`（跨进程写等待）。

---

## 4. 生命周期操作

| API | 行为 |
|-----|------|
| `create(subject, description, session_id)` | 以 `Pending` 插入记录，id 自动分配 |
| `get(id)` | 加载单条记录 |
| `list()` | 加载所有记录，按 id 排序 |
| `update(id, TaskUpdate)` | 补丁 status、owner、依赖边 |
| `delete(id)` | 将 status 设为 `Deleted`（软删除） |

### 依赖更新

边只存在 `task_dependencies` 表中（无镜像字段）。在 task A 上应用 `add_blocks: [B]` 时，在 `BEGIN IMMEDIATE` 事务内插入一行 `(A, B)`；反向边在读取时推导。task 标记为 **`Completed`** 时，同一事务删除其全部边（`DELETE ... WHERE blocker_id = ? OR blocked_id = ?`）——无需全表扫描，无 ghost 边。

---

## 5. Agent 工具

| 工具 | 输入要点 | 输出 |
|------|----------|------|
| `task_create` | `subject`、可选 `description` | 新记录的格式化 JSON |
| `task_get` | `task_id` | 格式化 JSON |
| `task_list` | （空对象） | 带标记的人类可读列表 |
| `task_update` | `task_id`、可选 `status`、`owner`、`addBlockedBy`、`addBlocks` | 格式化 JSON |

`task_update` 的状态字符串：`pending`、`in_progress`、`completed`、`deleted`（通过 `strum` 的 snake_case）。

列表示例行：

```text
[>] #3: Implement auth owner=alice (blocked by: [1])
```

空列表返回 `"No tasks."`。

---

## 6. 接线

```rust
// tui.rs 启动（async）
let task_manager = SharedTaskManager::new(TaskManager::new(&tact_path.session_db_path()).await?);

// ToolContext
pub task_manager: SharedTaskManager,
```

`SharedTaskManager` 包装 `Arc<TaskManager>` — SQLite 连接池已串行化写入，无需额外 mutex。四个工具通过 `ToolContext` 共享同一 manager。

只在主 `toolset()` 注册——**不在** `subagent_toolset()` 中。

调度：四个工具在 `crates/tact/src/agent/tool_schedule.rs` 中共享合成写作用域（`__tact_tasks__`），因此在同一 LLM turn 内**彼此串行**（避免并行 `task_update` 竞态），但仍可与无关的文件读并行。

---

## 7. 渲染辅助

```rust
pub fn render_task_json(task: &TaskRecord) -> Result<String>;
pub fn render_task_list(tasks: Vec<TaskRecord>) -> String;
```

工具直接返回这些字符串作为 tool 结果（create/get/update 为 JSON，`task_list` 为文本列表）。

成功的 `task_create` / `task_update` 还会发出 [`AgentUpdate::TasksChanged`](./25_chapter_protocol_zh.md)（过滤已删除项的快照），供 TUI 刷新 Log 下方的 sticky 进度条并追加 Log 详情卡片。`task_get` / `task_list` 不发射。设计见 `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md`。

---

## 8. 代码地图

| 文件 | 角色 |
|------|------|
| `crates/tact/src/task/mod.rs` | `TaskManager` 门面（`Box<dyn TaskStore>`）、`TaskRecord`、渲染辅助 |
| `crates/tact/src/store/task_store/mod.rs` | `TaskStore` trait |
| `crates/tact/src/store/task_store/sqlite.rs` | `SqliteTaskStore` — schema、事务、边查询 |
| `crates/tact/src/tool/task.rs` | 四个 `#[tool]` 处理器 |
| `crates/tact/src/tool/mod.rs` | `ToolContext.task_manager` |
| `crates/tact/src/tool/registry.rs` | `toolset()` 中的 `task_*` 工具 |


---

## 9. 当前缺口

| 缺口 | 详情 |
|------|------|
| **无 `task_delete` 工具** | Manager API 有软删除但无暴露工具（通过 update 用 `status: deleted`） |
| **Owner 是不透明字符串** | 未链接 [Team](./14_chapter_team.md) roster 校验 |
| **无自动 unblock 规则** | 只有完成会清边；已删 blocker 留下陈旧边 |
| **列表顺序固定为 id** | 无 priority 或 due date 字段 |
| **第 1 章交叉链接曾误导** | 曾指向第 11 章调度 — 已在 store 章更正 |

---

## 相关文档

- [Store 与持久化](./01_chapter_store_zh.md) — `CollectionStore` / `Store` 支撑
- [任务与工具调度](./11_chapter_task.md) — 无关的并行 tool wave
- [Subagents](./12_chapter_subagent.md) — `spawn_subagent` 运行嵌套 agent；它跑完并**不会**完成任务记录
- [团队协调](./14_chapter_team.md) — 可选 owner 命名约定
- [Worktree 泳道](./15_chapter_worktree_zh.md) — worktree create 上可选 `task_id` 链接
