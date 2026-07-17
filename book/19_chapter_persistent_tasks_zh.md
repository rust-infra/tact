# 持久化任务管理器

> 语言：[中文](./19_chapter_persistent_tasks_zh.md) · [English](./19_chapter_persistent_tasks.md)

本章涵盖 Tact 的 **durable 工作项跟踪器**：`task/` 模块、`.claude/tasks/` 下的 JSON 文件存储，以及四个 agent 工具 `task_create`、`task_get`、`task_list`、`task_update`。

这与以下 **不是** 同一概念：

- [第 11 章 工具调度](./11_chapter_task.md) — 一个 LLM turn 内的并行 **工具** wave 执行
- [第 12 章 Subagents](./12_chapter_subagent.md) — 生成嵌套 agent 的 `task` **工具**

实现：`crates/tact/src/task/mod.rs`，工具封装在 `crates/tact/src/tool/task.rs`。

---

## 1. 用途

TaskManager 给 LLM 一个跨 turn 和会话的 **持久化 checklist**：

- 用 subject / 可选 description 创建项
- 跟踪状态：Pending → InProgress → Completed / Deleted
- 分配 `owner` 字符串（队友约定——未强制）
- 通过 `blockedBy` / `blocks` 边建模 **依赖**

存储使用与 cron、后台任务相同的 [CollectionStore](./01_chapter_store_zh.md) 原语。

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
    pub status: TaskStatus,
    pub blocked_by: Vec<u64>,   // JSON: blockedBy
    pub blocks: Vec<u64>,
    pub owner: String,
}

pub struct TaskIndex {
    pub next_id: u64,
}
```

ID 从 `tasks/index.json` 中的 `next_id` 单调递增（从 1 开始）。

---

## 3. 存储布局

```text
.claude/
└── tasks/
    ├── index.json          # { "next_id": N }
    ├── task_1.json
    ├── task_2.json
    └── …
```

每个 task 是一个 JSON 文件，键为 `task_{id}`。`TaskManager::new` 在缺失时初始化 `index.json`。

---

## 4. 生命周期操作

| API | 行为 |
|-----|------|
| `create(subject, description)` | 分配 id，以 `Pending` 写入记录 |
| `get(id)` | 加载单条记录 |
| `list()` | 加载所有记录，按 id 排序 |
| `update(id, TaskUpdate)` | 补丁 status、owner、依赖边 |
| `delete(id)` | 将 status 设为 `Deleted`（软删除） |

### 依赖更新

在 task A 上应用 `add_blocks: [B]` 时：

1. A 的 `blocks` 列表增加 B
2. B 的 `blocked_by` 列表增加 A（自动写入反向边）

当 task 标记为 **`Completed`** 时，`clear_dependency` 从其 id 从每个其他 task 的 `blocked_by` 列表中移除。

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
// tui.rs 启动
let task_manager = SharedTaskManager::new(TaskManager::new(&store_root)?);

// ToolContext
pub task_manager: SharedTaskManager,
```

`SharedTaskManager` 包装 `Arc<Mutex<TaskManager>>` — 四个工具通过 `ToolContext` 锁定同一 manager。

只在主 `toolset()` 注册——**不在** `subagent_toolset()` 中。

调度：在 `crates/tact/src/agent/tool_schedule.rs` 中视为 **独立**（可与其他无冲突读/写并行）。

---

## 7. 渲染辅助

```rust
pub fn render_task_json(task: &TaskRecord) -> Result<String>;
pub fn render_task_list(tasks: Vec<TaskRecord>) -> String;
```

工具直接返回这些字符串作为 tool 结果（create/get/update 为 JSON，`task_list` 为文本列表）。

---

## 8. 代码地图

| 文件 | 角色 |
|------|------|
| `crates/tact/src/task/mod.rs` | `TaskManager`、`TaskRecord`、依赖逻辑、渲染辅助 |
| `crates/tact/src/tool/task.rs` | 四个 `#[tool]` 处理器 |
| `crates/tact/src/tool/mod.rs` | `ToolContext.task_manager` |
| `crates/tact/src/tool/registry.rs` | `toolset()` 中的 task 工具 |
| `crates/tact/src/store/` | `CollectionStore`、`Store` 原语 |

---

## 9. 当前缺口

| 缺口 | 详情 |
|------|------|
| **无 `task_delete` 工具** | Manager API 有软删除但无暴露工具（通过 update 用 `status: deleted`） |
| **Owner 是不透明字符串** | 未链接 [Team](./14_chapter_team.md) roster 校验 |
| **无自动 unblock 规则** | 只有完成会清 `blocked_by`；已删 blocker 留下陈旧边 |
| **列表顺序固定为 id** | 无 priority 或 due date 字段 |
| **第 1 章交叉链接曾误导** | 曾指向第 11 章调度 — 已在 store 章更正 |

---

## 相关文档

- [Store 与持久化](./01_chapter_store_zh.md) — `CollectionStore` / `Store` 支撑
- [任务与工具调度](./11_chapter_task.md) — 无关的并行 tool wave
- [Subagents](./12_chapter_subagent.md) — `task` 工具名碰撞
- [团队协调](./14_chapter_team.md) — 可选 owner 命名约定
- [Worktree 泳道](./15_chapter_worktree_zh.md) — worktree create 上可选 `task_id` 链接
