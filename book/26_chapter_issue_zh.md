# 工程问题与优化日志

> Language: [English](./26_chapter_issue.md) · [中文](./26_chapter_issue_zh.md)

本章是一份**按时间倒序的优化与 bugfix日志**，记录有用户可见或 API 可见行为变化的改动。它不是教程：每条写清问题、决策与代码 / 设计文档位置，避免后续重复踩坑。

相关流程：`AGENTS.md`（何时追加条目）、`docs/superpowers/specs/`（设计）、`docs/superpowers/plans/`（实现计划）。

---

## 0. 目的

| 目标 | 说明 |
|------|------|
| 连续性 | 记录*为什么*改，而不只是*改了哪些文件* |
| 交叉引用 | 指向设计 spec、PR，以及讲解子系统的 book 章节 |
| 控制膨胀 | 每个已交付的行为变更一条；纯重构、仅测试改动不记 |

### 条目模板

最新条目在前。每条应包含：

1. **日期 / ID** — `YYYY-MM-DD` 与可选 PR 号  
2. **类型** — `optimization` · `bugfix` · `removal` · `docs`  
3. **现象 / 动机** — 改前错在哪里或代价是什么  
4. **决策** — 最终契约（不必展开全部否决方案）  
5. **改后行为** — agent / 用户可依赖的可观察规则  
6. **指针** — 代码路径、spec、相关 book 章节  

---

## 1. 2026-07-27 — Log 滚动恢复主题背景

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章；`docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`；`docs/superpowers/plans/2026-07-27-log-scroll-artifact-fix.md` |

**现象 / 动机：** 从 code-card 或其他带样式的 Log 内容滚动离开后，普通文本行可能保留前一帧的背景样式。深色 Ink 主题下该问题尤其明显，文字后方会出现阴影。

**决策：** 保留 Log viewport 的重置，并让 `TextCell` 写入每个普通字形时显式应用当前 `theme.bg`。该规则与主题无关；卡片与 overlay 层保留既有背景和绘制顺序。

**改后行为：** 滚动新露出的任意普通 Log 行都使用当前主题背景，同时保留前景样式和选区反色 modifier。不使用 Ink 专用分支或全局终端清屏策略。

**指针：** `crates/tui/src/render/log.rs`；`crates/tui/src/render/cells/text.rs`；`crates/tui/src/render/log_render_tests.rs`；`docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`；第 23 章。

---
## 1. 2026-07-27 — Ink 主题 + 统一弹出层 Chrome

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 21、23 章；`docs/tui_rendering.md` |

**现象 / 动机：** 默认主题为 `retro`；弹出层覆窗口边框类型不一致、颜色硬编码、缺乏共享 chrome。

**决策：** 添加 `ink`/`ink-light` 主题，颜色精确匹配像素；新增 `heading`/`version`/`muted` Theme 字段；所有 overlay 统一使用 `render_popup_chrome`。默认主题改为 `ink`。

**改后行为：** 默认主题为 `ink`；所有 overlay 弹窗共享一致的边框、标题栏（粗体标题、`[x]` 提示）与底栏布局；弹窗代码 DRY。

**指针：** `crates/tui/src/theme.rs`、`crates/tui/src/render/popups/mod.rs`、`crates/tui/src/render/render_md.rs`、`crates/tact/src/config/resolve.rs`

---

## 1. 2026-07-26 — 子 agent 工具改名 `task` → `spawn_subagent`

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 7、10、11、12、19 章 |

**现象 / 动机：** spawn 子 agent 的工具原名 `task`，与四个持久化任务工具（`task_create` / `task_get` / `task_list` / `task_update`）共享前缀，语义却完全不同。模型与读者会把「`task` 工具跑完」当成「任务记录已完成」——实际观测到一次：子 agent 已返回，清单项仍停在 Pending。第 1、11、12、19 章各挂一句免责说明作为绕过。

**决策：** 工具改名 `spawn_subagent`（动词 + 对象，与 description 一致）；包装类型 `TaskTool` → `SpawnSubagentTool`，handler `task()` → `spawn_subagent()`。持久化任务工具保留 `task_*` 前缀。`spawn_subagent` 仍为 `CapabilityRisk::High`，仍是调度 barrier。

**改后行为：** 面向模型的工具名为 `spawn_subagent`，不再存在名为 `task` 的工具。含历史 `task` tool_use 块的旧 session 仍可恢复 —— `load_history` 只渲染 `Text` 块，router 仅在实时 dispatch 时按名解析，缺名不会报错。内存态 `always_allowed_tools` 按会话重建，无需迁移。

**指针：** `crates/tact/src/tool/subagent.rs`、`crates/tact/src/tool/registry.rs`、`crates/tact/src/permission/mod.rs`

---

## 1. 2026-07-26 — `TasksChanged` 不再追加 Log 卡片

| 字段 | 值 |
|------|-----|
| **类型** | removal |
| **相关** | 第 19、23 章 |

**现象 / 动机：** `on_tasks_changed` 原会追加一条 `📋 # Task.N · …` 系统消息，与已渲染同样标题的 `task_*` 工具行重复。commit `4116c23` 把这段发送逻辑注释掉（属于该 commit 的误伤）而非删除，于是 `format_tasks_log_card` 挂着 `#[allow(dead_code)]` 空转，`tasks_changed_shows_panel_and_appends_log` 长期变红。

**决策：** Log 中只保留工具行这一种表示。删除 `format_tasks_log_card`、`focus_changed_task`、`primary_action_for_change`；测试改为断言 sticky 已更新且 Log 长度不变。`AgentUpdate::TasksChanged` 保留 `reason` 字段 —— 生产端与协议不变。

**改后行为：** 一次 `task_create` / `task_update` 只产生一条 Log 行（工具卡）加一次 sticky 刷新，不会出现两条。

**指针：** `crates/tui/src/widgets/state/app/agent.rs`、`crates/tui/src/widgets/state/task_panel.rs`

---

## 1. 2026-07-26 — sticky 主机分隔 tab 与正文

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章 |

**现象 / 动机：** `sticky_host_content_height` 只预留 `1 + body` 行，渲染器把正文画在 `inner.y + 1`，于是 tab 行（`[Tasks] [Subagent] …`）紧贴 `── Pending ──` / 子 agent 日志，上方又紧邻 Log 框边框，整体挤成一块。

**决策：** 多预留一行（Tasks 为 `2 + body`，Subagent 为 `3 + header + lines`），并在 tab 行与正文之间画一条全宽淡色 `─` 分隔线。

**改后行为：** 展开的 sticky 依次为 tab、分隔线、内容。折叠高度仍为 1 行。

**指针：** `crates/tui/src/render/task_panel.rs`

---

## 1. 2026-07-26 — Bash 非 0 退出记为 Failed

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 7 章 |

**现象 / 动机：** `bash` 收集了 `ExitStatus` 却未使用，`cargo test` 失败等非 0 退出仍显示 `Success · …`，而输出里已是错误信息。

**决策：** 进程正常结束后若 `!status.success()`，经 `error_with_partial` 返回 `Err`（`exit code N` 或 `terminated by signal`），映射为 `StepStatus::Failed`，并保留已捕获输出给模型。

**改后行为：** shell 非 0 退出在 TUI 显示 Failed；0 退出不变。

**指针：** `crates/tact/src/tool/bash.rs`

---

## 1. 2026-07-25 — Subagent sticky tab（主 Log 保持干净）

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 12 / 23 章；`docs/superpowers/specs/2026-07-25-subagent-sticky-pane-design.md` |

**现象 / 动机：** 子 agent 共用父级 `ui_tx`，Stream/Step/Thinking 混进主 Log，子级 `TokenUsage` 覆盖底栏。

**决策：** 子更新打成 `AgentUpdate::Subagent`；sticky 主机 tab：Tasks | Subagent；主 Log 只留父 `task` 工具行；`RequestSelect*` 透传；首次自动切 tab，之后仅角标。

**改后行为：** 嵌套工作在 Subagent 可见；`task` 期间主 Log 与 ctx 仪表保持父级语义。

**指针：** `crates/tact/src/tool/subagent_ui.rs`、`crates/tui/src/widgets/state/subagent_pane.rs`、`crates/tui/src/render/task_panel.rs`

---

## 1. 2026-07-25 — 子 agent session 经 `ref_id` 关联

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 1 / 12 章；`docs/superpowers/specs/2026-07-25-subagent-session-ref-design.md` |

**现象 / 动机：** `task` 子 agent 无 `session_id` / store — 轮次、token 用量与 DeepSeek `user_id` 隔离都缺失；`task` 中途崩溃则子历史全丢。

**决策：** 每个子 agent 新建 session 行，`sessions.ref_id` = 父 id（父无 session 则为 `''`）。`list_sessions` 只返回顶层（`ref_id = ''`）。`delete_session` 级联删子。子会话不抢 `SessionLock`。

**改后行为：** 子 agent 消息 / `token_usages` 落在子 id 下；`--list-sessions` 仍只见父；删父带走其子。

**指针：** `crates/tact/src/tool/subagent.rs`、`crates/tact/src/store/session_store/sqlite.rs`、`ToolContext.session_id` / `session_store`

---

## 1. 2026-07-25 — 低占用时 ctx 进度条可见

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章；`docs/token_usage_schema.md` |

**现象 / 动机：** 上下文窗口为 1M 时，约 1%（`13.7K/1M`）会画 `▏`（1/8 格）。紧挨 `·` 时这条发丝几乎看不见，数字已是 `1%` 但条看起来仍是空的。

**决策：** 任意正小数格至少钳到 `▍`（3/8）；`frac > 0` 时不再回退成 `·`。

**改后行为：** 非零 ctx 占用在 `[…]` 内必有清晰半格（例如 1% → `[▍·······]`）。

**指针：** `crates/tui/src/render/bar.rs`（`partial_block_char` / `render_usage_bar`）

---

## 1. 2026-07-25 — Task 工具标题、Log 短卡、sticky 树、`/tasks-dag`

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 11 / 19 / 23 / 25 章；`docs/superpowers/specs/2026-07-25-task-tool-ui-redesign.md` |

**现象 / 动机：** `task_*` 工具行是 raw JSON；Log 卡重复整板 checklist；终端里难看依赖关系。

**决策：** 可读 tool 标题（`# Task.N · …`）；sticky 默认展开为 `blocks` 树并带 `#id`；`/tasks-dag` 用 meraid 弹窗渲 Mermaid Unicode（节点仅状态+id）。`TaskSnapshot` 携带 `blocks`/`blocked_by`。Log **不再**追加任务系统卡（进度看 sticky + tool 行）。

**改后行为：** tool 行可读；sticky 树形；slash 可看 DAG；Log 不再刷任务系统消息。

**指针：** `crates/tact/src/task/display.rs`、`crates/tui/src/widgets/state/task_panel.rs`、`crates/tui/src/widgets/state/task_dag.rs`

---

## 1. 2026-07-25 — 任务清单完整渲染（去掉 `… +N`）

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 19 / 23 章 |

**现象 / 动机：** Log 详情卡与 sticky 展开最多只显示 6 行（`… +N`），8 条任务时即使已全部更新也像未完成。

**决策：** 去掉 `STICKY_BODY_CAP`；sticky 高度与 Log 卡列出全部任务。

**改后行为：** sticky 展开与每次 `TasksChanged` Log 卡均显示完整清单。

**指针：** `crates/tui/src/widgets/state/task_panel.rs`

---

## 1. 2026-07-25 — 同一 turn 内串行持久化 `task_*` 工具

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 11 / 19 章 |

**现象 / 动机：** 模型常在一轮里发出大量 `task_update` / `task_create`。若落在同一 wave 并行执行，TaskManager 更新与 `TasksChanged` UI 事件会交错，Log 挤成一团，进度卡也不完整。

**决策：** 将 `task_create` / `task_update` / `task_get` / `task_list` 标为合成资源 `__tact_tasks__` 的写者，保证分属不同 wave（保序），但仍可与无关的 `read_file` 重叠。

**改后行为：** 同一 assistant 工具批次内，task 工具逐个执行；每次 mutating 调用可按序各自发出 `TasksChanged`。

**指针：** `crates/tact/src/agent/tool_schedule.rs`

---

## 1. 2026-07-24 — 持久任务 sticky 进度 + Log 详情卡

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 19 / 23 / 25 章；`docs/superpowers/specs/2026-07-24-task-progress-panel-design.md` |

**现象 / 动机：** 持久任务（`task_create` / `task_update`）只以普通 tool JSON/文本出现在 Log，没有常驻 checklist，也没有结构化变更时间线。

**决策：** mutating 工具成功后发射 `AgentUpdate::TasksChanged`。TUI 用 **外层切分** 在 Log 下挂 sticky 条（不改 Log wrap/scroll 内核），默认收起、点击展开；每次变更追加 Log 详情卡。无 pending/in_progress 时隐藏；resume 后等到本会话首次 `TasksChanged` 再显示。

**改后行为：**

- sticky 一行：`▸ 任务 done/total · 当前项`（点击展开完整清单）
- 每次 `TasksChanged` 追加 system Log checklist
- `task_get` / `task_list` 不发射

**指针：** `crates/protocol/src/agent.rs`、`crates/tact/src/tool/task.rs`、`crates/tui/src/render/task_panel.rs`、`crates/tui/src/render/layout.rs`

---

## 1. 2026-07-24 — 底栏去掉冗余 `[Log]`

| 字段 | 值 |
|------|-----|
| **类型** | removal |
| **相关** | 第 23 章 |

**现象 / 动机：** 底栏第 1 行总是以 `[Log]` 开头，但界面已永久单列日志，焦点标签无信息量，只占空间。

**决策：** 从 `render_bottom_bar` 第 1 行去掉 focus 段。顶栏如需仍可提 Log；底栏从 cwd / 运行时间起排。

**改后行为：** 第 1 行不再显示 `[Log]`；首段为工作区路径（随后 uptime、分支、可选账户）。

| 指针 | 路径 |
|------|------|
| 代码 | `crates/tui/src/render/bar.rs` |

---
## 2. 2026-07-24 — Slash 弹窗 Esc 提示 + 优先于 overlay

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章 |

**现象 / 动机：** Agent 忙碌时打开 `/` 容易感觉「卡住」：标题没有 Esc 关闭提示，
且 Esc 可能被 thinking/diff overlay 先吃掉，关不掉 slash 列表。

**决策：** 标题追加共用的 `popup_close_hint`（`[Esc] 关闭`，含无匹配态）。
Insert + slash 活跃时，按键路由优先于 `handle_overlay_key`，保证 Esc 先关 slash。

**改后行为：** Slash 标题显示 Esc 关闭；Esc 关掉弹窗且保留已输入内容；overlay
的 Esc 仅在 slash 关闭后生效。

| 指针 | 路径 |
|------|------|
| 代码 | `crates/tui/src/render/popups/slash_command.rs`、`crates/tui/src/lib.rs` |

---
## 3. 2026-07-24 — 空闲底栏 `Up` 低开销走秒

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章 |

**现象 / 动机：** 完全 Idle 时 poll 超时不 dirty，`Up MM:SS` 会一直停住，直到
下一次按键/鼠标/agent 事件。

**决策：** Idle 约 1000 ms 醒一次，且仅当显示的整秒变化才 dirty。活跃态仍为
spinner dirty；轮询间隔不变。Done 继续靠 `should_repaint` 强制重绘。

**改后行为：** 空闲时 `Up` 大约每秒走一格；不会更快空转刷屏。

| 指针 | 路径 |
|------|------|
| 代码 | `crates/tui/src/lib.rs`（`on_poll_timeout`） |

---
## 4. 2026-07-24 — 任务耗时挪到 task-end 分隔线

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 23 章 |

**现象 / 动机：** 底栏 `Elapsed` 与路径/分支/余额挤在一起，和它度量的那次
回复距离远，不好扫。

**决策：** 冻结耗时写入 task-end sentinel（`\x07tact-task-end\x1f{secs}`），在
强调色分隔线上居中渲染（`──── 耗时 00:03 ────`）；底栏不再显示耗时。

**改后行为：** 完成/取消的任务在尾部分隔线显示耗时；底栏第 1 行不再有
`Elapsed`/`耗时`。

| 指针 | 路径 |
|------|------|
| 代码 | `crates/tui/src/render/cells/separator.rs`、`widgets/state/app/popups.rs`、`render/bar.rs` |

---

## 5. 2026-07-24 — 底栏可读性回补

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 23 章、`docs/token_usage_schema.md` |

**现象 / 动机：** 图标-only polish 之后，底栏难解读（`8K/32K`、裸 `∑` / `▣`、
偏淡的 ` · ` 分隔）。thinking 档位虽已有 `model_reasoning_effort`，却未外显。

**决策：** 图标旁补短 i18n 标签；thinking 显示档位+budget（`high(32K)`）；第 1
行用 ` │ `、第 2 行两个空格；缓存为 `缓存%` / `cache%`；上次合计为 `∑ₜₒₖ`；
ctx 进度条填充改用中线高度 `■` / `·`，避免溢出 `[]`。

**改后行为：** 两行底栏无需图例即可读；token/cache 计算不变。窄屏丢弃顺序：
缓存 → 运行 → 路径 → ∑ → ctx。

| 指针 | 路径 |
|------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-readability-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-readability.md` |
| 代码 | `crates/tui/src/render/bar.rs`、`crates/tui/src/i18n.rs` |

---

## 6. 2026-07-24 — Slash 弹出：Tab 补全，Enter 运行 skill

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 2 章、第 23 章 |

**现象 / 动机：** 恢复 Insert 模式 `Tab` 给 slash 弹出后，对 skill 来说 **Tab**
与 **Enter** 仍相同（都只填 `/name `），分不清「补全」和「执行」。

**决策：** Slash 弹出 **Tab** 始终只自动补全为 `/name `；**Enter** 立即 Invoke
skill / 执行内置命令。需要子命令的 `/plugin` 仍只补全。命令面板对 skill 的
Enter 仍预填 Insert（便于 undo）。

**改后行为：** `/` 选中 skill → Tab 可改 args，或 Enter 立刻跑。

**指针：** `crates/tui/src/handlers/insert.rs`、第 2 章 §7、第 23 章 slash skills。

---

## 7. 2026-07-24 — 移除 TUI 左侧 Execution Plan 面板

| Field | Value |
|-------|-------|
| **类型** | removal |
| **相关** | Ch 23、Ch 25 |

**症状 / 动机：** 左侧 plan 面板与 log 中已有信息重复（tool block 在
`StepStarted` 时已出现在 log 中），却额外带来 `Tab` 焦点切换、`e` 可见性切换、
可拖拽 divider，以及大多数用户从未用过的 `panel_split_ratio` 布局参数。面板
焦点状态还让鼠标 hit test 与键盘处理更复杂。

**决策：** 完全移除面板 UI；保留 `PlanStep` 追踪为无面板的内部存储
（`app.plan.steps` / `steps_set`），以便未来消费者仍可用到 step 数据。Log
现在永久单列。`FocusedPanel` 仅保留 `Log` variant。删除 `Tab` 焦点切换、`e`
切换与 divider 拖拽/resize；`j`/`k`/`g`/`G`/`y`/`Y`/`V` 现在始终作用于 log。
Insert 模式下 `Tab` 用于 slash-command 自动补全（此前被全局 `Tab` handler
遮蔽）现在能正常触发，因为 `lib.rs` 中已无更早的 `Tab` 拦截。

**变更后行为：** `render_main_area` 始终以全宽渲染 log 面板；顶栏或底栏都不再
有 plan 面板、divider 或面板焦点指示。`StepAdded` 仍会更新 `app.plan.steps`
作内部记录，但从不绘制专用面板。

**指针：** `crates/tui/src/widgets/state/plan_panel.rs`、
`crates/tui/src/render/layout.rs`、`crates/tui/src/widgets/state/mod.rs`
（`FocusedPanel`）、`crates/tui/src/handlers/normal.rs`、
`crates/tui/src/handlers/mouse.rs`、`book/23_chapter_tui*.md`。

---

## 8. 2026-07-24 — 项目配置文件 `tact.toml` → `config.toml`

| 字段 | 值 |
|------|-----|
| **类型** | docs |
| **相关** | 第 21 章 |

**现象 / 动机：** 自动发现列表里是 `./tact.toml`，而用户全局 / `.tact/` 路径已是
`config.toml`，容易放错文件名。

**决策：** 搜索 `./config.toml` 替代 `./tact.toml`；示例文件改名为
`config.example.toml`。

**改后行为：** 发现顺序为 `./.tact/config.toml`、`./config.toml`、
`~/.tact/config.toml`。显式 `--config` 不变。

**指针：** `crates/tact/src/config/load.rs`、`book/21_chapter_config*.md`、
`config.example.toml`。

---

## 9. 2026-07-24 — Session Stats GFM 单元格填充以对齐纯文本

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**现象 / 动机：** 会话结束时 `eprintln` 打印的 `SessionStats::summary()` 是未填充
的 GFM（短标签与长标签混排），`tact-ui` 退出后终端里 `|` 列对不齐。

**决策：** 仍用 GFM pipe 表供 tui-markdown 渲染；按列最大宽度填充单元格（数值列
依分隔行 `:` 右对齐）。

**改后行为：** CLI / headless / TUI 退出摘要在等宽字体下对齐；`/stats` 弹窗仍走
tui-markdown 框线表。

**指针：** `crates/tact/src/stats.rs`、`docs/token_usage_schema.md`
（Session Stats Display）。

---

## 10. 2026-07-24 — 额外 `skill_dirs` + 项目本地 `.tact/skills`

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-extra-skill-dirs-design.md` |

**现象 / 动机：** 原先只有固定 skill 根；无法挂共享 / vendor 目录。旧的
`<workdir>/skills/` 也落在 `.tact/` 之外。

**决策：** `<workdir>/skills/` 改为 `<workdir>/.tact/skills/`。新增可选
`[agent].skill_dirs = [...]`（相对 workdir；`~` 展开）。加载顺序：
`.tact/skills` → `~/.tact/skills` → `~/.agents/skills` → `.claude/skills` →
配置额外目录 → 插件 cache。缺失目录软跳过。

**改后行为：** 配置可追加 skill 根并覆盖同名独立 skill。不再扫描裸
`<workdir>/skills/`。

**指针：** `crates/tact/src/consts.rs`、`crates/tact/src/skill/mod.rs`、
`crates/tact/src/config/types.rs`、`config.example.toml`、第 2 章。

---

## 11. 2026-07-24 — `/skills` 列表改用 tui-markdown（不用 pipe 表）

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |

**现象 / 动机：** `/skills` 经 `format_table` 画 Skill/Description 表。长
frontmatter 描述使行宽超过 log 面板，视觉换行把 `|` 列拆碎，难以阅读。

**决策：** 保留标题块与空行分隔。输出易换行的 markdown（`**\`name\`**` + 描述
段落），经 `render_markdown_tui` / tui-markdown 渲染。此处**不用** GFM 表（与
Session Stats 不同）：目录描述对 log 固定列宽来说太宽。

**改后行为：** `/skills` 每个 skill 一块名称 + 描述；任意面板宽度下自然折行。
命名空间名（`plugin:skill`）不变。

**指针：** `crates/tui/src/handlers/mod.rs`（`show_skills_command`、
`skills_list_markdown`）。

---

## 12. 2026-07-24 — Session Stats 用 GFM 表格 + tui-markdown 渲染

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**现象 / 动机：** `/stats` 把 comfy-table UTF8 框线文本丢进 `render_markdown_tui`。
软换行变空格，整张表挤成一行再 wrap，弹窗里乱成一团。

**决策：** 保持 `SessionStats::summary() -> String`。输出 **GFM pipe 表格**
（数值列右对齐）。TUI 继续走 `render_markdown_tui` /
[tui-markdown](https://github.com/joshka/tui-markdown) 的表格渲染（Unicode 框线）。
移除 `comfy-table` 依赖。CLI / headless 打印同一份 markdown 源。

**改后行为：** Session Statistics 弹窗显示对齐框线表；退出摘要为 GFM markdown。
计数与显隐规则不变。

**指针：** `crates/tact/src/stats.rs`、
`crates/tui/src/widgets/state/app/agent.rs`、`docs/token_usage_schema.md`。

---

## 13. 2026-07-24 — Session Stats 用 comfy-table 排版

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-session-stats-table.md` |
| **被取代** | §7（GFM + tui-markdown） |

**现象 / 动机：** 会话结束时的 Tool calls 行靠空格对齐，工具名与耗时变长后列错位。

**决策：** 保持 `SessionStats::summary() -> String`。先输出 Metric/Value 表，再按需输出 Tool calls 表（`Tool | Count(s/f) | Total | Avg`），最后用尾部 Metric/Value 表放工具汇总 / cache / reasoning。*（最初用 `comfy-table` UTF8 框线；与 TUI markdown 冲突，见 §7。）*

**改后行为：** 计数与显隐规则不变；排版改为对齐表格。

**指针：** `crates/tact/src/stats.rs`、`docs/token_usage_schema.md`（Session Stats Display）。

---

## 14. 2026-07-24 — `/model` 从 `/v1/models` 补充配置

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-openai-models-api-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-openai-models-api.md` |

**现象 / 动机：** `/model` 需要手写维护 `models = [...]` 列表；而 providers 已经提供了 `GET /v1/models`。

**决策：** Config 保持优先；API 附加不在 config 中的 id；冲突时 config 保持；每个 `(base_url, api_key)` 在首次 `/model` 时仅获取一次；跳过 Anthropic；失败时降级为仅用 config 或空提示。

**改后行为：** 见第 21 章 `/model` 节。

**指针：** `crates/tact_llm/src/models.rs`、`crates/tui/src/handlers/select.rs`、第 21 章、第 22 章（账户类查询）。

---

## 15. 2026-07-24 — `read_file` 分页与删除 `batch_read`

| 字段 | 值 |
|------|-----|
| **类型** | optimization + removal |
| **PR** | [#50](https://github.com/rust-infra/tact/pull/50) |
| **Spec** | `docs/superpowers/specs/2026-07-24-read-file-pagination-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-read-file-pagination.md` |

### 6.1 现象

`read_file` 用 `read_to_string` 整文件读入，再以 `chars().take(50000)` **静默**丢掉尾部。这与按行的 `offset` / `limit` 语义冲突，模型没有续读信号（幻觉风险见 [第 20 章](./20_chapter_hallucination_zh.md)），并与 dispatch 层的 `persist_large_output`（30k 字符 → `<persisted-output>`）形成双重、不一致的大小策略。

`batch_read` 是第二套多文件 API，另有 200k 字符硬顶，并在调度 / recent-file 上重复特例。

### 6.2 决策

1. 删除 `batch_read`。多文件并行读取改为同一 wave 内多个 `read_file`。  
2. 用 Tokio `BufReader` 按行流式读取（不为整页缓冲整文件）。  
3. 在 `read_file.rs` 用带前缀的常量封顶：

```rust
const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;
```

Token 估算：现有 `approx_token_count`（`ceil(UTF-8 字节数 / 4)`）。  
4. 不限制单行字符数（单行本身超预算则报错，绝不静默砍半行）。  
5. **未显式**指定范围 / 走默认页且未读完时，返回带引导的标记：

```text
[PARTIAL view — lines {start}-{end}; continue with offset={next}]

{joined lines}
```

6. **显式**传了 `offset` 和/或 `limit` 仍超 token 预算 → **报错**（不静默返回少于请求的范围）。  
7. `run_native_tool` 在 `name == "read_file"` 时 **跳过** `persist_large_output`。  
8. 工具 `description` 保持简短——限制在运行时强制，不在 schema 文案里重复。

### 6.3 改后行为

| 场景 | 结果 |
|------|------|
| 小文件、无参数 | 全文，无 PARTIAL |
| 超过 2000 行、无参数 | 前 2000 行 + PARTIAL（`offset=2001`） |
| 隐式读取触达 token 预算 | 已装下的完整行 + PARTIAL 与下一 `offset` |
| 显式范围超 token 预算 | `Err`，提示缩小 `limit` / 区间 |
| 单行本身超预算 | `Err`（无法靠行 offset 恢复行内后缀） |
| offset 越过 EOF | 空字符串 |
| 大 `read_file` vs bash / MCP | `read_file` 不会包 `<persisted-output>`；其它工具仍可能 |

### 6.4 指针

| 区域 | 路径 |
|------|------|
| 实现 | `crates/tact/src/tool/read_file.rs` |
| persist 豁免 | `crates/tact/src/agent/tool_dispatch.rs`（`run_native_tool`） |
| 工具注册 | `crates/tact/src/tool/registry.rs`（无 `BatchReadTool`） |
| 近似 token | `crates/tact/src/utils/truncate.rs` |
| 工具章 | [第 7 章](./07_chapter_tool_zh.md) |
| 压缩 / spill | [第 5 章](./05_chapter_compact_zh.md)、`docs/compaction.md` |

---

## 16. 2026-07-24 — 底部栏视觉优化

| 字段 | 值 |
|------|-----|
| **类型** | optimization |

**动机：** 底部栏混合使用 emoji、长双语标签（`Elapsed:`、`Balance:`、`cache hit:`）和混合分隔符（`│` / `|`）。两行均使用单一 `Paragraph` 样式，颜色层级扁平，难以快速浏览。

**决策：** 用窄 Unicode 图标（`◷`、`⊙`、`⎇`、`¤`、`∑`、`▣`）替换 emoji。统一分隔符为 ` · `。模型限制压缩为 `8k/32k` 格式，余额/配额信息精简。使用 ratatui `Line` / `Span` 渲染：图标和分隔符暗色、主值亮色、分支强调色、余额成功/错误色。

**变更后：** 双行底部栏具有一致的图标和颜色层级。纯格式化函数（`format_model_compact`、`format_balance_entry`、`format_quota_window`、`format_cache_pct`）可无终端进行单元测试。窄屏丢弃顺序：第 1 行去掉运行时间 → 路径；第 2 行去掉缓存 → 令牌总数 → 上下文计量器。

| 区域 | 路径 |
|------|------|
| 设计规格 | `docs/superpowers/specs/2026-07-24-bottom-bar-polish-design.md` |
| 实现计划 | `docs/superpowers/plans/2026-07-24-bottom-bar-polish.md` |
| 实现 | `crates/tui/src/render/bar.rs`、`crates/tui/src/i18n.rs` |
| 文档 | `docs/tui_rendering.md`（底部栏章节） |
| 渲染框架 | [第 23 章](./23_chapter_tui_zh.md) |

---

## Related Docs

- [工具系统](./07_chapter_tool_zh.md)
- [上下文压缩](./05_chapter_compact_zh.md)
- [Agent 循环中的幻觉](./20_chapter_hallucination_zh.md)
- [AGENTS.md](../AGENTS.md) — 含本章的文档同步触发条件
