# 任务与工具调度（Tasks and Tool Scheduling）

本章说明 LLM 决定行动之后发生什么：Tact 如何将一组 `ToolUse` 块转为已执行命令、结果，以及下一轮对话。

**勿与** [持久任务管理器](./19_chapter_persistent_tasks_zh.md)（`task_create` / `task_list` 工具）或 [子 Agent](./12_chapter_subagent_zh.md) 的 `spawn_subagent` spawn 工具混淆。

---

## 1. 任务即 Agent Loop 的一轮

在 Tact 中，**任务（task）** 指 `Agent::agent_loop`（`crates/tact_extensions/src/agent/mod.rs`）一次迭代中的工作：

```text
┌─────────────┐    LLM call    ┌─────────────────────┐
│ User prompt │ ─────────────► │ assistant response  │
└─────────────┘                │ (text + ToolUses)   │
                               └─────────────────────┘
                                         │
                                         ▼
                               ┌─────────────────────┐
│                              │ execute_tool_call() │
│                              └─────────────────────┘
│                                         │
│          ┌──────────────────────────────┼──────────────────────────────┐
│          ▼                              ▼                              ▼
│    pre-flight                    parallel execution              post-processing
│    (sequential)                  (waves)                          (sequential)
│          │                              │                              │
│          ▼                              ▼                              ▼
│   permission + hooks            tool calls run                results + hooks
│                                 concurrently where safe       appended to context
└────────────────────────────────────────────────────────────────────────────┘
                                         │
                                         ▼
                               next LLM call
```

循环持续直到模型停止、询问用户，或满足完成条件。

---

## 2. 三阶段流水线

`Agent::execute_tool_call`（`crates/tact_extensions/src/agent/tool_dispatch.rs`）将每轮分为三个阶段。

### Phase 1 — 预检（串行）

按模型发出顺序，每个工具各执行一次：

1. 发出 `StepAdded` / `StepStarted` UI 事件。
2. 运行 `PreToolUse` hook。
3. 通过 `PermissionManager` 检查权限。
4. 若权限被拒，生成 blocked 结果而不运行工具。

此阶段必须串行，因为权限提示可能交互式，且 hook 需要 `&mut self`。

### Phase 2 — 执行（按 wave 并行）

所有通过预检的工具交给 `crates/tact_extensions/src/agent/tool_schedule.rs` 中的调度器：

- 无依赖的 read 一起运行。
- 冲突的 read/write 或 write/write 串行化。
- `bash`、`spawn_subagent`、MCP 的**列表**类工具与未知工具为 **barrier** —— 单独运行。
- **MCP 工具不是一刀切 barrier**：同一 server 的工具互相串行、不同 server 可并行（每个 server 用一个 `__mcp__<server>` 写标记表示，见 `mcp_server_resources`）；`read_mcp_resource` / `get_mcp_prompt` 显式给了 `server` 时同样只锁那一个 server，不给 `server` 的列表（`list_mcp_resources` / `list_mcp_prompts` / templates）才是 barrier——列表可能触及每个 server。
- `spawn_subagent` 带 `worktree: true` 时是**唯一一个输入相关**的例外：子 agent 有自己的 worktree，文件影响被限定在泳道内，因此映射为 `Independent` 而不是静态的 `Barrier`，可以和其他工具同波次。

调度器为每个工具分配 **wave 编号**：

```text
wave[i] = max( wave[j] + 1  for every j < i that conflicts with i ), else 0
```

Wave 按序执行；同一 wave 内工具并发运行。

### Phase 3 — 结果组装与后处理

`PostToolUse` hook、`StepFinished`、stats 与 recent files 都发生在 **Phase 2 的完成循环里**：每有一个 future 就绪就立即处理，因此波内按**完成顺序**（不是模型顺序）触发；`StepFinished` 携带 `step_idx`，UI 靠它归位。

所有 wave 结束后，`build_tool_results` 按 `prepared` 数组（模型原始顺序）把输出拼回 `ToolResult` 块，再追加到 `runtime.context`——provider 校验 `tool_use` / `tool_result` 的配对与次序，这一步的顺序就是契约。

---

## 3. 冲突模型与安全

`tool_schedule.rs` 决定哪些工具可重叠，但它不维护一张工具名表：资源来自**工具自己的元数据**。每个工具在 `ToolMetadata.resources`（`ResourcePolicy`，`crates/tact_extensions/src/tool/metadata.rs`）里声明形状，`tool_resources_from_metadata` 调 `ResourcePolicy::resolve` 把它 + 本次输入解析成具体的读/写路径集：

| `ResourcePolicy` | 解析结果 | 使用者 |
|------------------|----------|--------|
| `ReadPath { field }` | 读 `input.<field>` 指向的路径 | `read_file`（`path`）、`read_image`（`file_path`） |
| `WritePath { field }` | 写该路径 | `write_file`、`edit_file`（`path`） |
| `SharedState { scope }` | 与同 scope 的其他调用互斥 | `task_create` / `task_update`（`task`）、team 消息族（`team`） |
| `Independent` | 不触及工作区文件，可与任何工具并行 | `sleep`、`save_memory`、`check_background`、`wait_background`、`check_subagent`、`wait_subagent`、`cancel_subagent`、`load_skill`、`task_get`、`task_list`、`list_teammates`、`read_inbox`、`worktree_list` / `worktree_status` / `worktree_events` |
| `Barrier` | 效果无法界定，独占一波 | `bash`、`ask_user`、`compact`、`background_run`、`worktree_run`、`worktree_create` / `worktree_remove`、`spawn_subagent`（不带 `worktree`）、未知工具 |
| `PatchFiles { … }` | 从 patch 头解析目标路径 | `apply_patch`（模块与元数据仍在，但**当前没有任何 toolset 注册它**，见 §3 末） |

路径规范化为绝对路径并 rooted 于 `work_dir`。两路径重叠当且仅当相等或一方为另一方祖先，因此对 `src/foo.rs` 的 write 与作用域为 `src/` 的 search 冲突。

MCP 工具不走这套元数据：它们的资源由 `mcp_server_resources(server)` 在调度时按 server 名合成（见 §2）。

逐工具的完整声明表、`overlap` 的路径分量语义、wave 算法的三条保证、以及各示例的逐项推导，见 [工具系统](./07_chapter_tool_zh.md) §9（工具侧视角）。

### 示例

模型按序返回：

1. `read A`
2. `read B`
3. `write A`
4. `read C`
5. `read A`

| Wave | Tools | 说明 |
|------|-------|------|
| 0 | `read A`、`read B`、`read C` | 一起运行 |
| 1 | `write A` | 等待第一次 `read A` |
| 2 | `read A` | 等待 write |

`read B` 与 `read C` 不受影响，留在 wave 0。

### 默认 barrier

未知工具视为 barrier。新增工具不会意外引入不安全并行；要让它与别的工具并行，得在它的 `ToolMetadata.resources` 里显式声明 `ReadPath` / `WritePath` / `SharedState` / `Independent`。

`apply_patch` 是一个现状提醒：它的模块、`APPLY_PATCH_METADATA`（`PatchFiles`）都还在，但 `454367d6` 把它从 registry 的 import 里删掉后**没有任何 toolset 再注册它**——`edit_file` 恢复后 apply_patch 就退出了工具面，留下的元数据是孤儿。上面表格里的那一行是"若重新接线会怎样"，不是它现在能跑。

---

## 4. 权限与 Hook

工具进入调度前，预检先按工具**元数据**解析风险（`PermissionPolicy::resolve`，在 `tool_dispatch.rs` 内），再把风险连同工具名与输入交给 `PermissionManager`：

- **只读**：一般允许。
- **Write**：Default 模式询问（除非 allowlist）；Auto 模式自动批准；Plan 模式拒绝。
- **高风险**：首次询问；此后仅当有显式允许覆盖该确切工具与输入时才放行。来自 `PermissionPolicy::High`（如 `spawn_subagent`）、敏感路径目标（`ReadPath` / `WritePath` / `PatchPaths` 命中），以及提到敏感路径或以 `sudo ` / `su ` 开头的 shell 命令。

风险不再按工具名匹配——名字是字符串，策略是数据。完整阶梯、模式与 TUI 审批流程见 [权限模型](./10_chapter_permission_zh.md)。

Hook（`PreToolUse`、`PostToolUse`）在 `crates/tact_extensions/src/hook/mod.rs`，可检查或修改工具输入/输出。它们在并行核心周围串行运行。完整设计见 [Agent 生命周期 Hook](./09_chapter_hook_zh.md)。

---

## 5. 回传给 LLM 的内容

每个完成的工具产生带 JSON 内容的 `ToolResult`。这些作为 `Role::User` 消息追加到 `runtime.context`，保持模型原始 tool-call 顺序。Agent loop 随后将更新后的 context 发给 LLM 进行下一轮。

---

## 6. 可观测性：Tool Schedule Summary

执行后 `persist_tool_schedule` 将 `ToolScheduleSummary` 写入与 LLM 调用相同的 `token_usages` 行。行匹配在 **`persist_llm_call` 时** 捕获的 `last_message_id`（`llm_call_last_message_id` —— 发给模型的最后一条消息，在 assistant 响应行追加之前）。

```json
{
  "total_tools": 5,
  "wave_count": 3,
  "max_parallelism": 3,
  "waves": [
    { "tools": ["read_file", "read_file", "read_file"], "barrier": false },
    { "tools": ["write_file"], "barrier": false },
    { "tools": ["read_file"], "barrier": false }
  ]
}
```

这会将调度策略与 token 成本关联，便于后续分析。

---

## 7. 自定义调度

要使新 native 工具可安全并行：

1. 在该工具的 `ToolMetadata.resources` 里声明 `ResourcePolicy`（`ReadPath` / `WritePath` / `SharedState { scope }` / `Independent`）——声明就在工具自己的元数据里，不在调度器里。
2. 别顺手写 `Barrier`：它是"我无法界定这个工具碰什么"的兜底，不是默认值。
3. 避免在声明资源之外的副作用。

若工具有全局副作用（shell 命令、子 agent、MCP 状态），保持为 barrier。字段名与 `permission` / `permission_prompt` / `argument_summary` 的一致性问题见 [工具系统](./07_chapter_tool_zh.md) §6。

---

## 8. 代码地图

| 文件 | 角色 |
|------|------|
| `crates/tact_extensions/src/agent/mod.rs` | `Agent::agent_loop`、`stream_message`、会话辅助 |
| `crates/tact_extensions/src/agent/tool_dispatch.rs` | `execute_tool_call`、三阶段编排 |
| `crates/tact_extensions/src/agent/tool_schedule.rs` | 资源模型、冲突检测、wave 调度器、`ToolScheduleSummary` |
| `crates/tact_extensions/src/permission/mod.rs` | 权限决策（`check` / `check_with_auto`）与模式；风险本身由 `tool/metadata.rs` 的 `PermissionPolicy` 解析 |
| `crates/tact_extensions/src/hook/mod.rs` | `PreToolUse` / `PostToolUse` hook |
| `crates/tact_extensions/src/tool/mod.rs` | `ToolRouter`、工具注册、native 工具分发 |
| `crates/tact_extensions/src/store/session_store/` | `record_tool_schedule` — 持久化 schedule summary |

---

## Related Docs

- [权限模型](./10_chapter_permission_zh.md)
- [工具系统](./07_chapter_tool_zh.md) — `ToolRouter` 与 native 工具分发
- [上下文压缩](./05_chapter_compact_zh.md) — dispatch 中的 `persist_large_output` 与手动 `compact` 检测
- [后台任务](./13_chapter_background_zh.md) — 同步 `bash` 步骤的异步对应物
- [子 Agent](./12_chapter_subagent_zh.md) — 嵌套 `spawn_subagent` 工具与调度 barrier
- [Parallel Tool Execution](../docs/parallel_tool_execution.md)
- [Tool Rendering](../docs/tool_rendering.md)
- [Token Usage Schema](../docs/token_usage_schema.md)
