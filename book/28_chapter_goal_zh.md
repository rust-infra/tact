# Agent Goal：持久化目标执行

> **文档状态：已实现（2026-10-15）。** 本章描述当前源码提供的 Goal 生命周期、SQLite 持久化、bounded Runner、协议命令与 TUI 投影；仍未覆盖的高级能力在文末单独列出。

本章讨论 Tact 如何支持跨多个 turn、可暂停、可恢复、可监督并可验证完成的长期工作。设计建立在现有 [`TaskManager`](./19_chapter_persistent_tasks_zh.md)、[`agent_loop`](./18_chapter_agent_loop_zh.md)、SQLite session store、权限管理和 TUI 协议之上；它不是对某个 Agent CLI 的复刻，而是 Tact 自身的领域设计。

---

## 1. 核心定义与边界

Goal、Task、Turn 和 PlanStep 是不同层级，不能互相替代：

| 概念 | 职责 | 生命周期 |
|------|------|----------|
| **Goal** | 用户希望最终达成的结果，以及证明结果达成的验收标准 | 可跨 session、跨多个 run 持续；直到成功、失败或取消 |
| **Task** | 为 Goal 服务的可执行工作项、状态和依赖关系 | 由现有持久任务系统管理，可关联 Goal，也可独立存在 |
| **Run** | Runner 一次取得执行权后，对 Goal 的一次有预算的推进过程 | 有明确开始、结束、用量和结束原因；可被恢复或重试 |
| **Turn** | Agent 与模型之间的一次交互迭代，可能包含工具调用 | 属于 Agent loop；不是 Goal 的完成单位 |
| **PlanStep** | 当前执行过程的 UI 计划/步骤展示 | 临时显示状态，不承担跨进程持久化职责 |

核心规则：**Goal 是持久意图；Task 是工作图；Runner 是跨 turn 调度器；Agent 是在既有权限边界内工作的执行者；验收结果决定 Goal 是否成功。**

普通的一次性问题仍然是普通对话，不应自动创建 Goal。用户显式创建或确认 Goal 后，Goal 才进入持久化执行生命周期。

---

## 2. 用户可见行为

1. 用户通过 `UserCommand::CreateGoal` 或 `/goal create <objective>` 创建 Goal；交互式命令没有显式标准时会加入一个必需的人工作用标准。
2. 创建成功后 driver **立即**把 Goal 交给 Runner 启动第一次 activation（`Draft → Queued → Running`），不需要额外的启动命令：Runner 创建一次有预算的 Run，并让 Agent 检查现状、规划和维护关联 Task DAG。该 activation 在命令循环之外的可取消任务里运行，因此运行期间 `Cancel` 与 `/goal list` 仍然可响应。
3. 如果仍有可执行工作、没有用户阻塞且预算充足，Runner 可以启动下一执行周期；单次 `agent_loop` 正常结束只表示本次交互结束，不代表 Goal 完成。
4. 需要用户决定时，Goal 等待用户；需要权限时，继续使用现有权限流程，不推导或扩大授权。
5. Agent 认为目标已满足时，提交**完成提议**及证据，由 Runner 进入验证阶段。模型的完成文字、所有 Task 已完成或某个 turn 正常结束，均不是成功判据。
6. 自动验收通过后可标记成功；需要人工确认的标准由用户确认。验收失败则回到执行，预算耗尽或无法继续则暂停并说明原因。
7. 用户可查看、补充方向、暂停、恢复、取消或确认结果。对已结束 Goal 的重新执行必须是明确的用户操作，并留下事件记录。

当前命令表面：

```text
/goal list [--all]    列出当前工作区的 Goal（默认不含已归档）；为空时在 Log 里提示（此时面板不可见）
/goal create <text>   创建 Goal（文本同时作为标题和 objective）并立即启动第一次 Run，Log 里回报新 Goal ID
/goal show <id>       查看目标、标准、任务和验收摘要
/goal steer [<id>] <text> 补充方向或约束并写入事件（省略 id 时作用于当前运行中的 Goal；无运行中 Goal 时取唯一的非终态 Goal，多个候选则要求显式 id）
/goal pause <id>      暂停执行
/goal resume <id>     恢复执行
/goal cancel <id>     取消 Goal
/goal accept <id>     接受人工验收项 / 完成提议
/goal archive <id>    把已结束的 Goal 归档（移出面板与默认列表，记录不删）
/goal unarchive <id>  取消归档，重新出现在面板里
```

**归档（2026-10-16）：** 已结束的 Goal 会一直躺在 sticky 面板与 `/goal list` 里，列表迟早被历史占满。`archive` 把它们移出**默认**列表：面板与 `/goal list` 只画未归档的 Goal，记录、Run、事件、标准全部保留。它是 `Goal` 聚合上的一个时间戳（`archived_at`）而不是新的 `GoalStatus` —— **状态是结局，不能被改写**（一个 `failed` 的 Goal 归档后仍报 `failed`），而新增状态会牵动状态机、转换表与既有行。只有终态 Goal 能归档（`Running` 之类会被拒绝并说明原因：把还在跑的活从面板上藏起来、活却继续跑），重复归档只是状态回报而不是错误；`unarchive` 走同一条 `GoalUpdate` 路径把戳清掉。归档也是**唯一**被允许写终态 Goal 的操作：`Goal::update` 对终态 Goal 只接受 `is_archive_only()` 的更新，标题/目标/标准/证据/状态照旧冻结。可达性：`/goal show <id>` 仍能打开归档的 Goal（正文里标 `**Archived:** yes`），`/goal list --all` 把它们放回面板并标 ` · archived`。

收 `<id>` 的八个动词在 TUI 里可以直接从补全菜单里挑 ID：打完 `/goal show ` 这样的前缀空格后，弹窗列出当前工作区里**该动词能作用**的 Goal，路径列是 ID、右列是面板同一行的状态标记与标题（可按 ID 或标题模糊匹配）。面板行不显示 ID、公告行会翻上去，补全菜单是不必回滚去找 ID 的地方。

这些命令映射为带 Goal ID 和用户文本的 typed `UserCommand`；list/show 在 Agent activation 进行时也直接从 `SharedGoalManager` 读取。

---

## 3. 生命周期与执行阶段

当前 Goal 状态：

```text
draft → queued → running → verifying → succeeded
                    │          │
                    ├─ waiting_user
                    ├─ waiting_permission
                    ├─ paused
                    ├─ failed
                    └─ cancelled
```

| 状态 | 含义 |
|------|------|
| `draft` | 目标和验收标准仍在确认，尚未授权 Runner 开始推进 |
| `queued` | 用户已确认，等待 Runner 获得执行 lease |
| `running` | Runner 持有 lease，Agent 正在推进工作 |
| `waiting_user` | 需要用户补充信息、决定方向或完成人工验收 |
| `waiting_permission` | 保留状态，当前没有生产者（见 §12）：非交互模式下权限门自行裁决，交互式弹窗在 activation 内被回答 |
| `paused` | 用户暂停、预算耗尽、无进展保护或可恢复错误导致暂停 |
| `verifying` | Agent 已提交完成提议，必需验收项正在核验 |
| `succeeded` | 所有必需验收条件均有有效证据并通过 |
| `failed` | 已确认无法继续且 Goal 以失败结束；暂时性错误不应直接记为失败 |
| `cancelled` | 用户明确取消，或确认终止 Goal |

`phase` 是独立字段，用于显示当前 `planning`、`execution` 或 `verification` 阶段；不要将 phase 与生命周期状态塞进同一个枚举。

每次 Run 结束都带明确 outcome，而不是让 driver 从 `Result<()>` 或最后一段模型文字猜测：

```text
progress_made | needs_user | needs_permission | completion_proposed
budget_exhausted | no_progress | cancelled | transient_error | fatal_error
```

Runner 根据 outcome、任务图、验收状态和预算决定状态转移。暂停、等待和 run 结束都要先落库，再通知 UI。

当前真正会被写出的 outcome：`progress_made`（有可观察进展，继续下一周期）、`no_progress`（只说了话、没做可观察的事，本次 Run 结束、Goal 保持 `running`）、`budget_exhausted`、`completion_proposed`、`needs_user`、`cancelled`、`transient_error`、`fatal_error`。`needs_permission` 与状态 `waiting_permission` 一样属于保留项（见 §12）。

---

## 4. 持久化模型

Goal 数据与会话 transcript 分开：transcript 记录对话事实；Goal 表记录当前可查询的结构化状态；事件表保留审计轨迹。领域记录沿用同一个工作区的 `.tact/tact.db`。

实际存储采用「**整行 JSON + 少量索引列**」：领域结构体（含标准、证据、预算用量、checkpoint）序列化进 `data` 列，查询和排序只依赖几列不变量。进化领域字段不需要迁移；契约稳定的是聚合语义与索引列。

标识符由 `goal::new_id()` 统一铸造：Goal、标准、Run、Event 共用 **8 位十六进制**短 ID（`git log` 风格）。这些 ID 同时出现在用户要读要敲的地方（`/goal show`、创建提示、`/goal <id>` 命令）和模型要回填的地方（`goal_propose_completion` 的 `criterion_id`、每次 activation 注入的 `<goal-context>`），完整 UUID v4 的 36 字符在这两处都是纯浪费。Goal 按工作区划分且数量很少，32 位空间足够；ID 在所有代码路径上都是不透明字符串，从不解析、不校验长度，因此历史库里更长的 UUID 行继续可用。

事件读取全部**有界**（2026-10-16）：每周期的进度判定用 `count_events` 计数比较，不 materialize 事件；注入上下文只取最近若干条（`list_recent_events`）；checkpoint、暂停原因、完成提议各是一条定向查询（`latest_event_of_kind` / `latest_event_with_status` / `has_event_of_kind_since`，均带 `ORDER BY seq DESC LIMIT 1`， newest-first 命中即止）。一次 activation 不再重读并 JSON-parse Goal 的全部历史——多周期循环下那曾是每周期两次全量读取。事件表本身仍不裁剪，`list_events` 保留给审计与展示。

### `goals`

```text
id TEXT PRIMARY KEY, data TEXT NOT NULL,
status TEXT NOT NULL, version INTEGER NOT NULL, updated_at INTEGER NOT NULL
```

索引：`idx_goals_status`、`idx_goals_updated_at`。

`data` 是 `Goal` 聚合的序列化：标题、objective、约束、phase、session 关联、完成时间与摘要，以及**验收标准列表**——每条标准含描述、`required`、`verification_kind`、`status` 和带 `criteria_version` 标签的证据。标准随聚合整体提交，`version` 即标准/证据版本号：用户修改目标或标准时递增，旧版本证据自动失效。CRUD 路径经 `GoalManager`，不允许绕过它直写。

Goal 属于工作区，不只属于创建它的 session。创建 session 与当前执行 session 分开记录，允许用户在后续 session 中查看和恢复。

### `goal_criteria`（不存在独立的表）

标准不是独立表：它嵌在 `goals.data` 的聚合里，随版本号一起写入。这样证据失效只需比较 `criteria_version`，不存在「标准改了、证据还在旧表里」的分叉。

「交互式 `/goal create` 未提供标准时由 driver 加入一个必需的人工标准」就是往这个聚合里写一条 `Human` 标准。

### `goal_runs`

```text
id TEXT PRIMARY KEY, goal_id TEXT NOT NULL, data TEXT NOT NULL,
state TEXT NOT NULL CHECK (state IN ('running', 'completed', 'interrupted')),
started_at INTEGER NOT NULL, finished_at INTEGER, stop_reason TEXT
```

索引：`idx_goal_runs_state`、`idx_goal_runs_goal_id`、`idx_goal_runs_started_at`。

`data` 是 `GoalRun` 记录：outcome、`turns_used` / `tool_calls_used` / `elapsed_ms` / `tokens_used`、checkpoint 和结束原因。Run 记录一次 lease 周期和资源消耗。进程异常退出时可以将过期的活动 Run 标为 `interrupted`，并在创建新 Run 时保留关联关系，不覆盖旧记录。

### `goal_events`

```text
seq INTEGER PRIMARY KEY AUTOINCREMENT,
id TEXT NOT NULL UNIQUE, goal_id TEXT NOT NULL,
data TEXT NOT NULL, occurred_at INTEGER NOT NULL
```

索引：`idx_goal_events_goal_seq`（`goal_id, seq`）——按 Goal 取区间事件。

追加式事件日志：创建、用户修改目标或标准、状态转移、Task 关联、Run 开始/结束、验收结果、steering、恢复、取消等。`seq` 保证全局顺序。事件用于诊断和审计；不能只依赖内存队列或 TUI 收到过的事件来恢复状态。

### `goal_leases`

```text
singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
owner TEXT NOT NULL, epoch INTEGER NOT NULL, expires_at INTEGER NOT NULL
```

工作区内**单行** lease：`singleton = 1` 由 CHECK 约束保证只能有一个写入者。`epoch` 是递增的 fencing 代，所有状态写入必须携带当前 epoch。

### Task 关联

现有 `tasks` 表增加可空 `goal_id`，并建 `idx_tasks_goal_id_id` 索引。既有 Task 不需迁移出虚构 Goal；没有 `goal_id` 的任务继续照常工作。任务依赖仍由现有 DAG 管理。

Goal 生命周期更新、Run outcome 写入和事件追加应在可行时使用同一 SQLite 事务，避免 UI 看见已暂停但 run 仍显示活跃等不一致状态。迁移必须幂等，兼容已有数据库。

---

## 5. Runner 与 Agent 主循环

`GoalRunner` 属于 session/command driver 的编排层，不属于 `Agent::agent_loop` 内的无限循环：

```mermaid
flowchart TD
    user[用户创建 / 恢复 Goal] --> claim[Runner 获取 workspace lease]
    claim --> load[加载 Goal、验收标准、未完成 Tasks 与 checkpoint]
    load --> run[创建有预算的 Run]
    run --> agent[Agent 执行一个受控周期]
    agent --> outcome{Run outcome}
    outcome -->|有进展且仍可执行| load
    outcome -->|需要用户| wait[持久化 waiting_user 并释放 lease]
    outcome -->|完成提议| verify[Runner 执行验收]
    verify -->|通过| success[succeeded]
    verify -->|失败且可继续| load
    verify -->|需人工确认| wait
    outcome -->|无进展| keep[Run 结束，Goal 保持 running]
    outcome -->|预算耗尽| pause[持久化 paused 并释放 lease]
    outcome -->|取消 / 不可恢复失败| terminal[cancelled / failed]
```

Agent loop 仍负责单次模型交互、工具 dispatch、hooks、权限、会话消息与 compaction。Goal 模式额外返回显式的 yield outcome；普通对话维持原有任务语义。

**一次 Run 是"一个 lease 周期 + 一到多个受限 activation"**：`GoalRunner::run_with_lease` 在同一个 Run 内循环执行 activation，只有上一个 activation 留下可观察进展（执行过工具调用或追加过 Goal 事件）且 Run 的累计预算仍有剩余时才进入下一个周期；否则以 `progress_made` / `no_progress` 结束该 Run。所有周期共享同一条 Run 记录、同一次 lease 和同一次收尾事务——不会每个周期各写一条 Run。

每个新周期开始时，Runner 都重新注入精简且权威的 Goal 上下文：目标、硬约束、验收标准摘要、关联 Task 状态、最近 checkpoint 和暂停原因（Goal 工具在周期内追加的 checkpoint/事件，下一个周期就能看到）。完整事件历史按需查询，不应每轮无界地注入 prompt。上下文应以明确标签与边界传递，不能让来自历史/tool output 的内容覆盖用户约束。

Agent 可报告 checkpoint、提出 Task 变更、请求用户输入、请求权限或提交完成提议；Goal 状态的最终写入权归 Runner/GoalManager。Subagent 可继承只读 Goal 摘要；只有明确委派的任务可以在父 Goal 范围内推进，子 Agent 不得直接结束或重开 Goal。

---

## 6. 验收：成功必须有证据

- Goal 启动前应有可检查的必需验收标准；交互式 `/goal create` 未提供时由 driver 加入默认人工作用标准，API 调用者可直接提供标准。
- 自动验收由 Runner 调用已有安全工具/权限体系执行；Goal 不提供绕过 permission manager 的“可信命令”通道。
- 人工验收由用户显式确认，并记录确认者、时间及针对的标准版本。
- Agent 提交完成提议后进入 `verifying`。Runner 检查所有 `required = true` 的标准是否通过，且证据与当前 Goal/标准版本匹配。
- 全部必需自动检查通过且没有待确认标准，才可自动转为 `succeeded`。
- 有人工标准时转为 `waiting_user`；用户接受后成功，拒绝后将反馈带回 `running`。
- 验收失败时回到 `running` 修复；预算耗尽则转 `paused` 并列出失败项。不得把“任务清单清空”当作验收通过。
- 用户修改目标或验收条件时提升版本号；旧版本的证据不能默认为新版本通过，必须按影响重新验证。

---

## 7. 预算、并发与恢复

### 预算与进展保护

**一次 Run 由一到多个受限 activation 组成。** 整个 Run 的 turn 上限默认是 500（`GoalLimits::max_turns`，可用 `[goal].max_turns` 覆盖；每个 cycle 只拿到剩余额度），由 `agent_loop` 自己的 turn cap 在运行中强制；一个 activation 结束后，只有它在**可观察进展**（执行过工具调用，或追加过 Goal 事件——checkpoint、Task 关联、证据）且 Run 的累计预算仍有剩余时，Runner 才接着跑下一个 activation。纯文本回合（模型只说话、没有落地动作）结束本次 Run。

Run 的累计预算同样是 `GoalLimits` 的 500 turns / 600 秒墙钟时间（`[goal].max_turns` / `max_elapsed_secs`），跨 activation 累加到同一条 Run 记录上；达到上限时保存 checkpoint、用量和 stop reason，Goal 进入 `paused`。每个 cycle 传给 agent loop 的是**剩余** turn 数（`max_turns - turns_used`），且上限在 `turns_taken` 自增之前检查，所以停止时那次“探测”不会被记成一次模型调用——多 cycle 的 Run 不会花到约 2× 预算。工具调用数只作为用量记录（`GoalRun::tool_calls_used`），不设单独上限：每次工具调用本来就发生在一次模型调用之内，turn 上限已经约束了总扇出，单独计数只会让有用的 Run 提前结束。因为累计预算是按 turn 计的，周期数天然有界（每个周期至少消耗一次模型调用）。注意 `Agent::max_turns` 约束的是**进程累计**的 `turns_taken`，所以 Runner 传给循环的是「当前累计值 + 剩余预算」，而不是裸的预算数字。

**最后一轮会提醒模型写 checkpoint**（2026-10-17）：turn cap 在模型调用**之前**检查，所以最后一轮是模型唯一还能落盘的机会。该轮请求的 system prompt 末尾会追加 `<goal-budget-reminder>`，要求模型调用 `goal_checkpoint` 记录「已完成 / 仍待办 / 下一步」，避免 Run 在思考中途被截断、下一次 activation 从零重新发现。提醒只存在于该次请求的 system prompt 中，不进入会话历史；预算仍是硬约束，提醒只是软引导。

**Lease 在 Run 执行期间持续心跳**（2026-10-17）：`max_elapsed` 只在 activation 之间检查，单个 activation 可能远长于 lease TTL。Runner 在 cycles 运行期间按 `TTL/3` 刷新 lease，退出时 abort 并 await 心跳任务；收尾与释放只校验 owner+epoch（fencing），不再要求 lease 仍未过期——否则一个只是因为跑得久而过期、但未被接管的 lease 会丢掉整个 Run 的结果。

**activation 自己的判定优先于预算**（2026-10-16）：取消（`cancelled`）、被 hook 拦截（`fatal_error`/`failed`）、需要用户（`needs_user`）、验收通过（`succeeded`）这类终局判定即使与预算耗尽同时发生，也按判定记录——用户在最后一次流式响应期间按下的 Esc 不会被改记成预算暂停。预算只在「本来还会继续」（`progress_made`，或验收被拒）时把 Run 标成 `budget_exhausted`/`paused`。

无进展检测只依赖可观察变化（工具调用、Goal 事件），不依赖模型自述：只有文字的周期记为 `no_progress` 并结束本次 Run，Goal 保持 `running` 等待下一次 activation——它不会因此被暂停，也不会被误记成 `progress_made`。单次失败或偶发网络错误同样不判定 Goal 失败：activation 报错记为 `transient_error` 并结束 Run，Goal 状态不变。人工验收不要求 Goal 恰好停在 `waiting_user`：`waiting_user`、`paused`（用户暂停或恢复把 stranded `running` 落到 paused）、以及没有活动 Run 的 `running` 都可以被 `/goal accept` 推进；只有活动 Run 在途时拒绝。这样用户消息把 Goal 唤醒后，刚提示的验收不会立刻失效，验收仍由 `GoalVerifier` 决定。

### Lease 与写入隔离

同一执行目录默认只允许一个写入型 Goal Runner。lease 应包含 owner、过期时间/心跳和递增 fencing epoch；所有状态写入校验当前 epoch，避免旧进程失去 lease 后继续提交覆盖新执行者的结果。独立 worktree 可按独立执行目录并发；是否允许多个 Goal 对同一目录并行写，首版默认禁止。

### 崩溃恢复与取消

- 启动或恢复时先检查活动 Run 与 lease；lease 过期后将旧 Run 标记为 `interrupted`，然后创建新的恢复 Run。
- 恢复以 SQLite Goal/Run/Event 状态和 session transcript 为依据，不依赖进程内 future、channel 或 pending queue。
- 只有完成幂等性检查后才重试可能已执行但结果未落库的操作；外部副作用不得因为恢复而盲目重放。
- 取消先持久化取消请求，再触发当前 Agent 的 `cancel_flag`；等待活动工具按现有取消语义退出，记录 Run 终态并释放 lease。
- 进程退出或 TUI 断开不能把 Goal 静默标成成功；无 UI 时遇到用户决策/权限需求应暂停并可由后续 session 恢复。
- 挂起的权限请求不能在进程重启后被视为已批准；恢复时须重新经过当前权限流程。

---

## 8. 权限与安全边界

Goal 授权的是“持续尝试达成这个目标”，不是对将来所有副作用的预先批准：

- 文件写入、shell、MCP、网络和敏感路径访问继续由现有权限系统及安全扫描器控制。
- Goal 验收命令也走相同的工具和权限路径；验收配置不能执行任意 shell 绕过审批。
- 用户 steering、目标变更、标准变更和人工验收必须作为用户来源事件记录；不能让模型工具伪造用户确认。
- Goal 工具只允许访问当前执行上下文中的 Goal；Task 关联也默认限定在该 Goal，避免跨 Goal 修改。
- 工具结果和恢复 checkpoint 可能包含不可信内容，重新注入时保留来源标签和信任边界。
- Runner 负责取消、预算、lease 和调度；它不更改现有 permission mode，也不静默降低审批要求。

---

## 9. TUI / Headless 协议

TUI 使用 `AgentUpdate::GoalsChanged` 携带完整 `GoalSnapshot` 集合，并通过 `UserCommand` 的 `CreateGoal`、`ListGoals`、`ShowGoal`、`SteerGoal`、`PauseGoal`、`ResumeGoal`、`CancelGoal` 和 `AcceptGoal` 控制生命周期。快照包含状态、phase、标准摘要、Task 进度、等待原因、预算和验收计数；重新连接或恢复时仍以 SQLite 为权威来源。

Goal 面板可以接入现有 Tasks / Subagent / Background sticky host，概览显示：目标标题、Goal 状态与 phase、当前 Run、关联 Task 进度、阻塞原因、预算用量和验收摘要。展开视图提供标准逐项状态、证据、最近事件和暂停/恢复/取消/确认操作。

Headless 与交互式模式共用同一个 GoalRunner 和状态机。Headless 遇到必须由用户决定的事项时保存 `waiting_user` 并退出当前 Run，不能等待 UI responder；后续通过命令或 API 补充输入后恢复。

---

## 10. 实时代码边界

当前代码地图：

| 路径 | 职责 |
|------|------|
| `crates/tact/src/goal/` | `Goal`、标准、证据、状态机、`GoalManager`、`GoalVerifier` |
| `crates/tact/src/store/goal_store/` | `GoalStore` trait、SQLite schema、事务、lease 与事件持久化 |
| `crates/tact/src/task/`、`store/task_store/` | Task 可选 `goal_id`、Goal 范围查询及旧记录兼容 |
| `crates/tact/src/tool/` | 注入 `SharedGoalManager`；Goal 感知工具与受限 Task 作用域 |
| `crates/tact-ui/src/session_bootstrap.rs` | 为 Agent 构建并注入共享 Goal manager |
| `crates/tact-ui/src/goal_runner.rs`、`driver.rs` | bounded activation、预算、lease、恢复、命令串行化 |
| `crates/protocol/src/agent.rs` | Goal 快照、`GoalsChanged` 和用户命令协议 |
| `crates/tui/`、`crates/agent_tui_kit/` | Goals sticky panel、状态投影、鼠标切换与背景测试 |
| `book/`、`docs/superpowers/` | 本设计章节与实现前 spec/plan；代码交付时同步实际行为 |

实现为迁移、状态机合法转移、lease/fencing、Run 预算、验收证据、无 UI 等待、取消及崩溃恢复提供了聚焦测试。跨 channel 等待的异步测试使用 timeout；Cargo 验证必须串行运行。

---

## 11. 与现有领域的关系

- [第 1 章 Store](./01_chapter_store_zh.md)：Goal、Task、Run、事件和 session transcript 共用工作区 SQLite，但各有明确领域边界。
- [第 10 章权限](./10_chapter_permission_zh.md)：Goal 不改变风险分级、审批模式或敏感路径守卫。
- [第 18 章 Agent 主循环](./18_chapter_agent_loop_zh.md)：`agent_loop` 仍是单次执行内核；Goal 跨周期调度由外层 Runner 负责。
- [第 19 章持久任务](./19_chapter_persistent_tasks_zh.md)：Goal 关联既有 Task DAG，不替代独立任务。
- [第 23 章 TUI](./23_chapter_tui_zh.md) 与 [第 25 章协议](./25_chapter_protocol_zh.md)：Goal UI 是协议驱动的投影；持久 store 才是恢复依据。
- [第 26 章工程问题日志](./26_chapter_issue_zh.md)：Goal 的交付记录见 2026-10-15 条目。

## 12. 当前限制

- `/goal create` 的文本命令只提供默认人工作用标准；复杂的自动标准仍需通过 API/领域层创建。
- `/goal resume <id>` 既用于恢复 `paused` / `waiting_user` / `waiting_permission` 的 Goal，也用于推进 `draft` / `queued`；终态（`succeeded` / `failed` / `cancelled`）不能直接继续。
- `waiting_permission` 目前**没有生产者**：非交互模式下权限门自行裁决（高风险拒绝、其余放行），交互式弹窗在 activation 内被回答，因此被权限挡住的 activation 不会把 Goal 停在该状态。该状态保留在状态机里，供将来"权限感知的停止原因"使用。
- 一个 Run 以 `progress_made` 结束时 Goal 仍是 `running`（没有活动 Run）。此时没有自动的下一周期——下一个 activation 由 `/goal resume` 或下一次进程启动后的恢复逻辑（`running` 且无活动 Run → `paused`）接管。
- 一次 `/goal resume` 会跑到该 Run 的累计预算用完（或达到验收/等待用户等终态）为止；`running` 但空闲的 Goal 表示"上一次 Run 用完了预算或没有可观察进展"，继续推进仍是显式操作。
- `needs_permission`（run outcome）与 `waiting_permission`（状态）一样是保留项，没有生产者，原因见上。
- 当前协议提供完整 Goal 集合快照，没有单独的 `GoalRunChanged` 增量事件；运行预算在活动 run 中展示，完成 run 的详细审计保存在事件表。
- `/goal pause` / `/goal cancel` 这类**不带 lease** 的状态写入在 Run 活跃期间会被拒绝（`ActiveRunExists`）：从 `start_run` 到收尾事务，Goal 的状态归 Run 所有，第二个进程中途暂停会留下「已暂停但 Run 仍活跃」的矛盾观测。同一会话内命令循环本就串行在 Run 之后，因此只有跨进程写入会撞到这道门（2026-10-16）。
- Goal sticky panel 提供状态、预算、Task 进度、验收摘要、等待原因和标准列表；具体标准操作仍通过 `/goal accept` 等命令完成。

---

## 相关文档

- [书籍目录](./index_zh.md)
- [存储与持久化](./01_chapter_store_zh.md)
- [权限模型](./10_chapter_permission_zh.md)
- [Agent 主循环](./18_chapter_agent_loop_zh.md)
- [持久化任务管理器](./19_chapter_persistent_tasks_zh.md)
- [TUI 与 Agent–TUI 协议](./23_chapter_tui_zh.md)、[第 25 章](./25_chapter_protocol_zh.md)
