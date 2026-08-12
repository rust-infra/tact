# Cron 调度

> 语言：[中文](./16_chapter_cron_zh.md) · [English](./16_chapter_cron.md)

本章说明 Tact 如何让 agent **注册定时 prompt**：cron 表达式、prompt 文本和元数据持久化在 `<workdir>/.tact/tact.db`（`cron_tasks` 表）。模型可通过原生工具创建、列出和删除这些记录；存储层通过 `ToolContext` 接入每个主 agent 会话。

**重要范围说明：** 截至本文撰写时，Tact 会持久化定时任务，但 **尚未** 运行后台 tick 循环来求值 cron 表达式并将 prompt 注入 `agent_loop`。`recurring` 和 `durable` 标志会存储并在列表中展示；它们为未来的运行时行为预留。见 [§8 当前缺口](#8-当前缺口)。

---

## 1. Cron 调度的用途

Tact 中的 Cron **不是** shell 作业运行器（那是 [后台任务](../crates/tact/src/background.rs) 通过 `background_run` / `check_background`）。它是 **agent 应按计划接收的 prompt** 的注册表：

| 概念 | 代码中的含义 |
|------|--------------|
| `cron` | Cron 表达式字符串（原样存储；目前不校验也不解析） |
| `prompt` | 计划触发时要注入的用户消息文本 |
| `recurring` | `true` →  recurring 计划；`false` → 一次性（目前仅元数据） |
| `durable` | `true` → 跨会话重启存活；`false` → 会话范围（目前仅元数据） |

当用户要求提醒、每日 check-in 或其他基于时间的跟进时，agent 在一轮中使用 `cron_create`。在运行时调度器存在之前，这些条目是 durable **记录**，agent（或未来的 daemon）可用 `cron_list` 查询。

---

## 2. 架构概览

```mermaid
graph TB
    subgraph Entry["会话启动（tui.rs）"]
        DB[(tact.db)]
        CS[CronScheduler]
        SCS[SharedCronScheduler]
        DB --> CS
        CS --> SCS
    end

    subgraph Agent["主 agent 循环"]
        TC[ToolContext.cron_scheduler]
        TR[ToolRouter]
        SCS --> TC
        TC --> TR
    end

    subgraph Tools["原生工具"]
        CC[cron_create]
        CL[cron_list]
        CD[cron_delete]
        TR --> CC
        TR --> CL
        TR --> CD
    end

    subgraph Store["磁盘上"]
        TBL[cron_tasks 表]
        CC --> TBL
        CL --> TBL
        CD --> TBL
    end

    subgraph Missing["尚未实现"]
        TICK[调度器 tick / cron 解析器]
        LOOP[将 prompt 注入 agent_loop]
    end

    TBL -.-> TICK
    TICK -.-> LOOP
```

子 agent（`subagent_toolset`）**不** 接收 cron 工具——只有主 agent 的完整 `toolset()` 包含它们。

---

## 3. 数据模型

定义于 `crates/tact/src/cron/mod.rs`：

```rust
pub struct ScheduledTaskRecord {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    pub session_id: String,  // 所属 agent 会话；会话外为 ""
    pub created_at: i64,     // Unix 时间戳（UTC，毫秒）
}
```

每个任务对应 `cron_tasks` 表中的一行（`crates/tact/src/store/cron_store/sqlite.rs`）。ID 来自 `INTEGER PRIMARY KEY AUTOINCREMENT`，对外以 8 位十六进制字符串暴露（`format!("{rowid:08x}")`）——与遗留 JSON 索引的线上契约一致。

---

## 4. 持久化

| 项 | 值 |
|----|-----|
| 数据库 | `<workdir>/.tact/tact.db`（与 sessions、tasks、background 共享） |
| 表 | `cron_tasks(id INTEGER PRIMARY KEY AUTOINCREMENT, cron TEXT, prompt TEXT, recurring INTEGER, durable INTEGER, session_id TEXT, created_at INTEGER)` |
| 后端 | `SqliteCronStore`（sqlx 连接池，`busy_timeout` 5 秒） |
| 索引 | `session_id` 上的 `idx_cron_tasks_session_id` |
| 初始化 | 首次打开时 `CREATE TABLE IF NOT EXISTS` |

`CronScheduler::new` 在 `headless.rs` / `interactive.rs` 中以 `tact_path.session_db_path()` 调用。遗留 `cron/scheduled_tasks.json` 文件 **不再读取**；旧条目留在磁盘上，id 从 `00000001` 重新开始。

---

## 5. 调度器生命周期

### 构造

`crates/tact-ui/src/headless.rs` 和 `interactive.rs` 每个进程构建一次调度器：

```text
db_path = tact_path.session_db_path()              // <workdir>/.tact/tact.db
cron_scheduler = SharedCronScheduler::new(CronScheduler::new(&db_path).await?)
tool_context = ToolContext { cron_scheduler, work_dir, … }
agent = Agent::new(client, tool_context, toolset(), …)
```

目前没有单独的 cron daemon 或 tokio 任务。调度器在 agent 进程生命周期内存在，通过 `ToolContext` 在所有工具调用间共享（`Arc<CronScheduler>` 可克隆——SQLite 连接池串行化写入，无需 mutex）。

### `CronScheduler` 与 `SharedCronScheduler`

| 类型 | 角色 |
|------|------|
| `CronScheduler` | 对 `Box<dyn CronStore>` 的异步 CRUD 门面 |
| `SharedCronScheduler` | `Arc<CronScheduler>`；工具和测试调用相同的 async 方法 |

---

## 6. Agent 工具

实现在 `crates/tact/src/tool/cron.rs`，注册于 `toolset()`（`crates/tact/src/tool/registry.rs`）。

### `cron_create`

**输入：**

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `cron` | string | 必填 | Cron 表达式 |
| `prompt` | string | 必填 | 计划触发时要注入的 prompt |
| `recurring` | bool | `false` | Recurring 与一次性 |
| `durable` | bool | `false` | Durable 与会话范围 |

**输出：** 新 `ScheduledTaskRecord` 的格式化 JSON。

### `cron_list`

**输入：** 空对象。

**输出：** 每个任务一行（按 id 排序），或 `"No scheduled tasks."`：

```text
00000000 0 9 * * * [recurring/session]: Daily standup summary
```

方括号标签：`recurring` 或 `one-shot`，加上 `/durable` 或 `/session`。

### `cron_delete`

**输入：** `{ "id": "<task id>" }`。

**输出：** `"Deleted scheduled task {id}"`，或 id 未找到时错误。

这些工具在工具调度器中是 **独立** barrier（不同 id 的行无冲突）。它们不直接触碰 `work_dir`——只触碰 `tact.db` 中的 `cron_tasks` 表。

---

## 7. Cron 与后台任务

两者都是通过 `ToolContext` 注入的工作区范围 manager，但解决不同问题：

```mermaid
flowchart LR
    subgraph Cron["Cron（本章）"]
        C1[存储 prompt + cron 表达式]
        C2[通过 agent 工具 CRUD]
        C3[尚无运行时执行器]
    end

    subgraph BG["后台任务"]
        B1[异步运行 shell 命令]
        B2[tokio::spawn + 轮询]
        B3[check_background 取输出]
    end
```

| | Cron | 后台 |
|---|------|------|
| 模块 | `cron/mod.rs` | `background.rs` |
| 持久化 | 定时 prompt | Shell 命令 + stdout/stderr |
| 今日是否执行 | 否 | 是（`background_run`） |
| 子 agent 访问 | 否 | 否 |

---

## 8. 当前缺口

以下 **尚未** 进入代码库；记录它们可避免与 README 营销文案混淆：

1. **无 cron 求值器** — 表达式是不透明字符串；没有解析或校验。
2. **无 tick 循环** — 没有任务在定时器上唤醒并调用 `agent_loop` 注入存储的 prompt。
3. **`recurring` / `durable` 运行时未用** — 仅持久化并由 `cron_list` 展示。
4. **与会话 store 无集成** — 触发 prompt 需要新接线（TUI 事件、headless 触发或 sidecar 进程）。
5. **无自动清理** — 一次性任务在假设的触发后不会移除。

添加运行时时，可能的触点：`tui.rs` 中的 tokio interval 或读取 `cron_tasks` 的专用模块，以及将用户消息入队到活跃 agent 的路径（类似交互模式下的用户输入）。

---

## 9. 代码地图

| 文件 | 角色 |
|------|------|
| `crates/tact/src/cron/mod.rs` | `ScheduledTaskRecord`、`CronScheduler`、`SharedCronScheduler` |
| `crates/tact/src/store/cron_store/mod.rs` | `CronStore` trait（async：create/delete/list） |
| `crates/tact/src/store/cron_store/sqlite.rs` | `SqliteCronStore` — `cron_tasks` 表、8 位十六进制对外 id |
| `crates/tact/src/tool/cron.rs` | `cron_create`、`cron_list`、`cron_delete` 工具处理器 |
| `crates/tact/src/tool/mod.rs` | `ToolContext.cron_scheduler` |
| `crates/tact/src/tool/registry.rs` | `toolset()` 中的 cron 工具 |
| `crates/tact-ui/src/headless.rs`、`interactive.rs` | 构造调度器并传入 `Agent` |
| `crates/tact/src/store/mod.rs` | `StoreRoot` / JSON store（teammates、worktrees） |
| `crates/tact/src/tool/test_support.rs` | 带临时 `tact.db` 的测试 `ToolContext` |

---

## 相关文档

- [ARCHITECTURE.md](../ARCHITECTURE.md) — 子 agent、团队、任务、worktree 表（Cron 行）
- [任务与工具调度](./11_chapter_task.md) — 模型行动后工具调用如何运行（与 cron 触发正交）
- [crates/tact/tact.md](../crates/tact/tact.md) — 领域 manager 与 `.tact/` 布局
- [docs/state_machines.md](../docs/state_machines.md) — 后台任务生命周期（与 cron 对比）
