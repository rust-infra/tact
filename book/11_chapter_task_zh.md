# 任务与工具调度（Tasks and Tool Scheduling）

> 语言：[中文](./11_chapter_task_zh.md) · [English](./11_chapter_task.md)

本章说明 LLM 决定行动之后发生什么：Tact 如何将一组 `ToolUse` 块转为已执行命令、结果，以及下一轮对话。

**勿与** [持久任务管理器](./19_chapter_persistent_tasks_zh.md)（`task_create` / `task_list` 工具）或 [子 Agent](./12_chapter_subagent_zh.md) 的 `task` spawn 工具混淆。

---

## 1. 任务即 Agent Loop 的一轮

在 Tact 中，**任务（task）** 指 `Agent::agent_loop`（`crates/tact/src/agent/mod.rs`）一次迭代中的工作：

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

`Agent::execute_tool_call`（`crates/tact/src/agent/tool_dispatch.rs`）将每轮分为三个阶段。

### Phase 1 — 预检（串行）

按模型发出顺序，每个工具各执行一次：

1. 发出 `StepAdded` / `StepStarted` UI 事件。
2. 运行 `PreToolUse` hook。
3. 通过 `PermissionManager` 检查权限。
4. 若权限被拒，生成 blocked 结果而不运行工具。

此阶段必须串行，因为权限提示可能交互式，且 hook 需要 `&mut self`。

### Phase 2 — 执行（按 wave 并行）

所有通过预检的工具交给 `crates/tact/src/agent/tool_schedule.rs` 中的调度器：

- 无依赖的 read 一起运行。
- 冲突的 read/write 或 write/write 串行化。
- `bash`、MCP、子 agent 与未知工具为 **barrier** —— 单独运行。

调度器为每个工具分配 **wave 编号**：

```text
wave[i] = max( wave[j] + 1  for every j < i that conflicts with i ), else 0
```

Wave 按序执行；同一 wave 内工具并发运行。

### Phase 3 — 后处理（串行）

所有 wave 完成后：

1. 运行 `PostToolUse` hook。
2. 按模型原始顺序发出 `StepFinished` UI 事件。
3. 更新 bookkeeping：recent files、stats、压缩触发。
4. 将 tool results 追加到 `runtime.context`。

---

## 3. 冲突模型与安全

`tool_schedule.rs` 决定哪些工具可重叠。每个已知工具声明其触及的工作区资源：

| Tool | Resource | Mode |
|------|----------|------|
| `read_file` | `input.path` | read |
| `batch_read` | `input.files[].path` | read |
| `search_code` | directory scope | read |
| `write_file`, `edit_file` | `input.path` | write |
| `web_search`, `web_fetch`, `lsp`, `sleep` | — | independent |
| `bash`, `apply_patch`, subagent, MCP, unknown | — | barrier |

路径规范化为绝对路径并 rooted 于 `work_dir`。两路径重叠当且仅当相等或一方为另一方祖先，因此对 `src/foo.rs` 的 write 与作用域为 `src/` 的 search 冲突。

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

未知工具视为 barrier。新增工具不会意外引入不安全并行；须在 `tool_schedule.rs` 的 `tool_resources` 中显式 opt-in。

---

## 4. 权限与 Hook

工具进入调度前，`PermissionManager` 分类其意图：

- **只读**：一般允许。
- **Write**：Default 模式询问（除非 allowlist）；Auto 模式自动批准；Plan 模式拒绝。
- **高风险**：始终询问（即使 allowlist）；包括 `task`、破坏性工具名与危险 bash 模式。

分类规则、模式与 TUI 审批流程见 [权限模型](./10_chapter_permission_zh.md)。

Hook（`PreToolUse`、`PostToolUse`）在 `crates/tact/src/hook/mod.rs`，可检查或修改工具输入/输出。它们在并行核心周围串行运行。完整设计见 [Agent 生命周期 Hook](./09_chapter_hook_zh.md)。

---

## 5. 回传给 LLM 的内容

每个完成的工具产生带 JSON 内容的 `ToolResult`。这些作为 `Role::User` 消息追加到 `runtime.context`，保持模型原始 tool-call 顺序。Agent loop 随后将更新后的 context 发给 LLM 进行下一轮。

---

## 6. 可观测性：Tool Schedule Summary

执行后 `persist_tool_schedule` 将 `ToolScheduleSummary` 写入与 LLM 调用相同的 `token_usages` 行。行匹配在 **`persist_llm_call` 时** 捕获的 `last_message_id`（`llm_call_last_message_id` —— 发给模型的最后一条消息，在 assistant 响应行追加之前）。

```json
{
  "tool_count": 5,
  "wave_count": 3,
  "max_parallelism": 3,
  "waves": [
    { "wave": 0, "tools": ["read_file", "read_file", "read_file"], "barrier": false },
    { "wave": 1, "tools": ["write_file"], "barrier": false },
    { "wave": 2, "tools": ["read_file"], "barrier": false }
  ]
}
```

这会将调度策略与 token 成本关联，便于后续分析。

---

## 7. 自定义调度

要使新 native 工具可安全并行：

1. 在 `crates/tact/src/agent/tool_schedule.rs` 的 `tool_resources()` 中添加其资源模式。
2. 返回正确的 `ToolResourceMode`（`Read`、`Write` 或 `Independent`）。
3. 避免在声明资源之外的副作用。

若工具有全局副作用（shell 命令、子 agent、MCP 状态），保持为 barrier。

---

## 8. 代码地图

| 文件 | 角色 |
|------|------|
| `crates/tact/src/agent/mod.rs` | `Agent::agent_loop`、`stream_message`、会话辅助 |
| `crates/tact/src/agent/tool_dispatch.rs` | `execute_tool_call`、三阶段编排 |
| `crates/tact/src/agent/tool_schedule.rs` | 资源模型、冲突检测、wave 调度器、`ToolScheduleSummary` |
| `crates/tact/src/permission/mod.rs` | 意图分类与权限决策 |
| `crates/tact/src/hook/mod.rs` | `PreToolUse` / `PostToolUse` hook |
| `crates/tact/src/tool/mod.rs` | `ToolRouter`、工具注册、native 工具分发 |
| `crates/tact/src/store/session_store/` | `record_tool_schedule` — 持久化 schedule summary |

---

## Related Docs

- [权限模型](./10_chapter_permission_zh.md)
- [工具系统](./07_chapter_tool_zh.md) — `ToolRouter` 与 native 工具分发
- [上下文压缩](./05_chapter_compact_zh.md) — dispatch 中的 `persist_large_output` 与手动 `compact` 检测
- [后台任务](./13_chapter_background_zh.md) — 同步 `bash` 步骤的异步对应物
- [子 Agent](./12_chapter_subagent_zh.md) — 嵌套 `task` 工具与调度 barrier
- [Parallel Tool Execution](../docs/parallel_tool_execution.md)
- [Batch Tools Flow](../docs/batch_tools_flow.md)
- [Tool Rendering](../docs/tool_rendering.md)
- [Token Usage Schema](../docs/token_usage_schema.md)
