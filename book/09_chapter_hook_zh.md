# Agent 生命周期钩子（Agent Lifecycle Hooks）

> 语言：[中文](./09_chapter_hook_zh.md) · [English](./09_chapter_hook.md)

本章说明 Tact 如何在工具执行前后注入自定义逻辑：调用前检查或改写 tool 输入，完成后改写输出，以及（通过注册 API）在会话开始前准备状态。

Hooks 是 **agent 循环**与**工具调度器**之间的扩展点。它们顺序执行，可通过返回 `HookControl::Block` **否决**操作。

---

## 1. 为什么需要 Hooks

并非每种策略都适合写在 tool 实现或 `PermissionManager` 里：

- **横切防护** — 在任何 tool 运行前拦截危险参数模式。
- **输入规范化** — 改写路径、注入默认值，或去掉模型常写错的字段。
- **输出整形** — 截断、脱敏，或在结果进入 context 前附加元数据。
- **集成** — 发指标、审计日志或同步外部系统，而无需 fork 每个 tool。

Hooks 把这些关注点移出核心调度器，仍在流水线可预测的位置运行。

---

## 2. 三种 Hook 类型

定义于 `crates/tact/src/hook/mod.rs`：

| Hook | 注册 | 今天是否调用 | 可否变更 | 可否否决 |
|------|------|--------------|----------|----------|
| `SessionStart` | `Agent::session_start` | **否** — 已注册但 `agent_loop` 尚未调用 | 对 `LoopState`（`Agent`）只读 | 是 |
| `PreToolUse` | `Agent::pre_tool` | 是 — 权限检查之前，按 tool 顺序 | `ToolUse` 输入（`name`、`input` JSON） | 是 |
| `PostToolUse` | `Agent::post_tool` | 是 — 每个 tool 完成后，随结果流入 | `ToolResult` content | 是 |

`LoopState` 是 `Agent` 的类型别名，因此 session hook 看到与循环相同的运行时（context、stats、tool router 等）。

---

## 3. 控制流：`HookControl`

每个 hook 返回其一：

```rust
pub enum HookControl {
    Continue,
    Block(String),
}
```

| 结果 | 含义 |
|------|------|
| `Continue` | 运行同类型下一个 hook，然后继续流水线。 |
| `Block(reason)` | 立即停止 hook 链；该 tool 步骤视为失败，原因为 `reason`。 |

对 `PreToolUse`，block 会跳过执行与权限提示——模型仍会收到解释为何被拦的 `ToolResult`。

对 `PostToolUse`，block 会在结果写入 context 前，用失败消息替换成功的 tool 输出。

若 hook 返回 `Err(...)`，agent 视为 block，附带通用失败消息（`PreToolUse hook failed: …` / `PostToolUse hook failed: …`）。

---

## 4. Hooks 在轮次流水线中的位置

Hooks 包裹 [任务与工具调度](./11_chapter_task.md)（英文）所述的并行核心：

```text
对 assistant 消息中每个 ToolUse（Phase 1 — 顺序）：
  StepAdded / StepStarted
  ──► PreToolUse hooks（顺序，可改 input 或 Block）
  ──► PermissionManager
  ──► 标记 tool 为 Run 或 Resolved（blocked/denied）

Phase 2 — 并行波次（此处无 hooks）

对每个完成的 tool（仍按完成顺序）：
  ──► PostToolUse hooks（顺序，可改 content 或 Block）
  ──► StepFinished UI 事件
  ──► 追加 ToolResult 到 context（Phase 3）
```

要点：

1. **PreToolUse 在权限之前** — hooks 可改写权限随后评估的 input。
2. **PreToolUse 严格有序** — 一次一个 tool，按模型发出顺序。
3. **PostToolUse 按每个完成的 tool 运行** — 波次中每个 future resolve 时，而非整波 join 之后。Hooks 仍在 agent task 上逐个完成顺序执行。
4. **并行 tool 不共享 hook 状态** — 每次调用有独立的 `ToolUse` / `ToolResult` 副本。

---

## 5. 核心类型

```rust
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}
```

Hooks 以 trait object 存在 agent 上：

```rust
pub enum Hook {
    SessionStart(Box<dyn SessionStartFn>),
    PreToolUse(Box<dyn PreToolUseFn>),
    PostToolUse(Box<dyn PostToolUseFn>),
}
```

可直接注册闭包——任何签名正确的 `Send + Sync` 异步闭包都实现对应 trait。

---

## 6. 注册 Hooks

在 `Agent` 上（`crates/tact/src/agent/mod.rs`）：

```rust
agent.pre_tool(|agent, tool_use| {
    Box::pin(async move {
        if tool_use.name == "bash" {
            let cmd = tool_use.input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd.contains("curl") {
                return Ok(HookControl::Block("curl is disabled in this workspace".into()));
            }
        }
        Ok(HookControl::Continue)
    })
});

agent.post_tool(|_agent, tool_use, tool_result| {
    Box::pin(async move {
        if tool_use.name == "read_file" && tool_result.content.len() > 50_000 {
            tool_result.content.truncate(50_000);
            tool_result.content.push_str("\n… (truncated by hook)");
        }
        Ok(HookControl::Continue)
    })
});
```

Hooks 按注册顺序追加到 `Agent.hooks`，每次调用按该顺序执行。

同类型多个 hook 组合：全部须 `Continue`，除非某个 `Block`（首个 block 生效）。

---

## 7. `invoke_hooks!` 宏

定义于 `crates/tact/src/hook/mod.rs`，在 crate 根导出：

```rust
invoke_hooks!(PreToolUse, self, &mut tool_use)
invoke_hooks!(PostToolUse, self, &tool_use, &mut tool_result)
```

行为：

1. 从 `HookControl::Continue` 开始。
2. 过滤 `self.hooks` 到请求的 `HookTypes` 变体。
3. 按注册顺序 await 每个 hook。
4. 首个 `Block` 时停止并返回该控制值。
5. 用 `?` 传播错误。

调用点在 `crates/tact/src/agent/tool_dispatch.rs` 的 `Agent::execute_tool_call` 内。

---

## 8. PreToolUse 详解

**时机：** `execute_tool_call` 的 Phase 1，每个 `ContentBlock::ToolUse` 一次。

**相对其他预检工作的顺序：**

```text
stats.tool_counts += 1
cancel 检查
StepAdded / StepStarted
PreToolUse  ◄── hooks
PermissionManager::check
PreparedState::Run | Resolved(blocked message)
```

**变更 input：** 因 `tool_use` 是 `&mut ToolUse`，hook 可在权限与执行看到之前改 `input`。被调度的 JSON 与日志用的是变更后的版本。

**Blocking：** `Block` 时 agent 设 `PreparedState::Resolved(msg)` — tool 不会进入调度器。模型仍会收到匹配的 `ToolResult` 以满足协议。

---

## 9. PostToolUse 详解

**时机：** 在 wave 执行循环内，原生或 MCP tool 返回后、发出 `StepFinished` 之前。

**典型用途：**

- 从命令输出中脱敏 API key 或 token。
- 为模型规范化错误字符串。
- 附加结构化前缀（`[cached]`、`[retry 2/3]` 等）。

**成功后 block：** 若 tool 返回 `StepStatus::Success` 但 hook block，UI 与 context 会看到失败步骤及 hook 原因。

---

## 10. SessionStart（当前 API）

`Agent::session_start` 接受签名如下的 hooks：

```rust
Fn(&LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + '_>>
```

预期调用点是**每个会话一次**，在 `agent_loop` 中第一次 LLM 请求之前（`ensure_session` 之后、主 `loop` 体之前）。

截至本文写作时，**`agent_loop` 尚未调用 `invoke_hooks!(SessionStart, …)`**。今天可以注册 session hooks，但要接上调用才会运行。PreToolUse 与 PostToolUse 已完全生效。

接上后，session hooks 适合一次性 setup：预热缓存、校验工作区不变量或注入遥测 context。

---

## 11. 设计约束

| 约束 | 理由 |
|------|------|
| Hooks 在 agent task 上运行 | 经 `LoopState` 间接持有 `&mut Agent`；工作宜短或在内部 spawn。 |
| 并行波次内无 hooks | tool 不可变借用 router 时，避免共享 agent 状态的数据竞争。 |
| 首个 `Block` 生效 | 可预测、易推理的否决语义。 |
| 错误即步骤失败 | hook bug 表现为 tool 失败，而非静默 no-op。 |
| 注册顺序 = 运行顺序 | 堆叠多个插件时文档化 hook 优先级。 |

**不要**在 hooks 里做权限 UI——用 `PermissionManager` 与现有 `RequestSelect` 流程。

---

## 12. 代码地图

| 文件 | 职责 |
|------|------|
| `crates/tact/src/hook/mod.rs` | 类型、trait、`Hook` 枚举、`invoke_hooks!` 宏 |
| `crates/tact/src/agent/mod.rs` | `pre_tool`、`post_tool`、`session_start`、`hooks_by_type` |
| `crates/tact/src/agent/tool_dispatch.rs` | `execute_tool_call` 中的 PreToolUse / PostToolUse 调用 |
| `crates/tact/src/permission/mod.rs` | PreToolUse 之后运行；与 hooks 分离 |
| `docs/state_machines.md` | Hook 控制枚举与流水线摘要 |

---

## Related Docs

- [权限模型](./10_chapter_permission.md) — 流水线中紧接 PreToolUse 之后（英文）
- [任务与工具调度](./11_chapter_task.md) — hooks 所包裹的三阶段 tool 流水线（英文）
- [工具系统](./07_chapter_tool_zh.md) — 原生工具与 dispatch
- [ARCHITECTURE.md](../ARCHITECTURE.md) — Hook Engine 章节
- [Tool Rendering](../docs/tool_rendering.md) — TUI 中 blocked/failed 步骤如何显示
- [Parallel Tool Execution](../docs/parallel_tool_execution.md) — hooks **不**运行的位置
