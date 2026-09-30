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

## 2. Hook 类型

定义于 `crates/tact/src/hook/mod.rs`：

| Hook | 注册 | 今天是否调用 | 可否变更 | 可否否决 |
|------|------|--------------|----------|----------|
| `SessionStart` | `Agent::session_start` | 是 — 每会话一次，初始化后（`dispatch_session_start_hooks`）；它收集的 context 在第一轮之前被记录 | `&mut SessionStartContext`（追加注入的 context） | 是 |
| `UserPromptSubmit` | `Agent::user_prompt_submit` | 是 — 用户回合消息进入 `agent_loop` 时 | prompt 文本（追加 `additionalContext`） | 是 |
| `PreToolUse` | `Agent::pre_tool` | 是 — 权限检查之前，按 tool 顺序 | `ToolUse` 输入（`name`、`input` JSON） | 是 |
| `PostToolUse` | `Agent::post_tool` | 是 — 每个 tool 完成后，随结果流入 | `ToolResult` content | 是 |
| `PostToolUseFailure` | `Agent::post_tool_failure` | 是 — tool **失败**后，在 `PostToolUse` 之外触发 | 只读 `LoopState` + `ToolUse` + 错误文本 | 仅记录（tool 已失败） |
| `Notification` | `Agent::notification` | 是 — agent 呈现用户通知时（目前仅 `permission_prompt`） | 只读 `LoopState` + `NotificationContext` | 仅记录（观测性） |
| `TaskCompleted` | `Agent::task_completed` | 是 — 每个完成的用户任务一次（`dispatch_task_completed_hooks`） | 只读 `LoopState` | 仅记录（任务已完成） |
| `Stop` | `Agent::stop` | 是 — 在外层回合边界调用一次（`dispatch_stop_hooks`） | 对 `LoopState` 只读 | 是 — `Block(reason)` 表示用 `reason` 作为下一条 prompt *继续*该回合 |
| `SessionEnd` | `Agent::session_end` | 是 — 拆除时调用一次（`dispatch_session_end_hooks`） | 对 `LoopState` 只读 | 仅记录（会话即将结束） |
| `PreCompact` | `Agent::pre_compact` | 是 — `compact_history` 路由到 native/local 之前 | 只读 `LoopState` + `CompactTrigger` | 是 — `Block` 否决压缩 |
| `PostCompact` | `Agent::post_compact` | 是 — 压缩成功后，每条路径调用一次 | 只读 `LoopState` + `CompactTrigger` | 仅记录（已提交） |

`SubagentStart` 与 `SubagentStop` 是**独立**的 hook trait（`SubagentStartFn` / `SubagentStopFn`），不是 `Agent.hook` 变体：`spawn_subagent` 是工具处理器、没有父 `Agent` 句柄，所以闭包挂在 `ToolContext.subagent_start_hooks` / `ToolContext.subagent_stop_hooks` 上，由 spawn 路径调用。`SubagentStart` 修改子代理的 system prompt；`SubagentStop` 在子代理结束后运行，可改写回传给父级的 summary。

`Stop` 是唯一一个 `Block` 语义反转的事件：被「block」的「操作」是「停下」本身，因此 `Block(reason)` 表示*继续*该回合（Codex continuation-fragment 语义），而不是否决。其余所有 hook 都用 `Block` 来否决。

`LoopState` 是 `Agent` 的类型别名，因此 session hook 看到与循环相同的运行时（context、stats、tool router 等）。

---

## 3. 控制流：`HookControl`

每个 hook 返回其一：

```rust
pub enum HookControl {
    Continue,
    Allow,
    Block(String),
}
```

| 结果 | 含义 |
|------|------|
| `Continue` | 运行同类型下一个 hook，然后继续流水线。 |
| `Allow` | hook 自行回答了审批询问（Codex 的 `permissionDecision: "allow"`）。只有 `PreToolUse` 与 `PermissionRequest` 会对它采取行动；其余事件把它当作 `Continue`。 |
| `Block(reason)` | 立即停止 hook 链；该 tool 步骤视为失败，原因为 `reason`。 |

**block 永远优先。** 宏在遇到 `Allow` 后仍继续扫描，因此后面的 hook 依然可以拒绝，而拒绝不会被另一个 hook 的批准撤销——与权限规则一致的 `deny > allow` 优先级。对于没有询问可回答的事件，`Allow` 是无操作而非错误，因此同一个 hook 文件可以服务多个事件。

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
        if tool_use.name == "bash" && tool_result.content.len() > 50_000 {
            tool_result.content.truncate(50_000);
            tool_result.content.push_str("\n… (truncated by hook)");
        }
        Ok(HookControl::Continue)
    })
});
```

Hooks 按注册顺序追加到 `Agent.hooks`，每次调用按该顺序执行。

同类型多个 hook 组合：全部须 `Continue`，除非某个 `Block`（首个 block 生效）。

### 命令 hook

hook 来自六处，且**所有匹配的 hook 都会运行**——高层不会取代低层，与 Codex 叠加 user / project / managed 的方式一致：

| 来源 | 路径 | `${PLUGIN_ROOT}` | `${PLUGIN_DATA}` |
|---|---|---|---|
| user 文件 | `~/.tact/hooks.json` | 该文件所在目录 | `~/.tact` |
| project 文件 | `<workdir>/.tact/hooks.json` | 该文件所在目录 | `<workdir>/.tact` |
| 已安装插件 | bundle 的 `hooks/hooks.json`，或 manifest 内联 `hooks` 映射 | bundle 根 | 该插件的数据目录 |
| user 配置 | `~/.tact/config.toml` 的 `[hooks]` 表 | 该文件所在目录 | `~/.tact` |
| project 配置 | `<workdir>/config.toml` 的 `[hooks]` 表 | 该文件所在目录 | `<workdir>/.tact` |
| project 配置 | `<workdir>/.tact/config.toml` 的 `[hooks]` 表 | 该文件所在目录 | `<workdir>/.tact` |

注册顺序即上表顺序——插件、user 文件、project 文件，然后是 `config.toml` 的各张表——位于既有 Rust 闭包之后。这个顺序只影响哪一段 `SessionStart` 上下文先被拼接，而「追加在最后」正是保持所有既有顺序不变的做法，因此新增入口不会打乱插件或 user 文件的上下文顺序。

**`[hooks]` 表是第三种写法，而不是第三套机制。** 它反序列化进 JSON 文件用的同一个 `HooksFile`，因此字段、校验与审核都是同一份实现：

```toml
# ~/.tact/config.toml
[[hooks.PreToolUse]]
matcher = "bash"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "policy.sh"
timeout = 5
```

每个声明了 `[hooks]` 的配置文件都成为**它自己的**一个来源，并以路径作为标签——绝不合并，尽管配置加载器会合并这些文件的其它取值。hook 是按身份审核的，而审核必须点名它来自哪个文件；合并还会让「只批准这个文件的 hook」变得不可能。由于收集器复用了同一个加载器，所有既有性质原样成立：条目一开始处于**未审核**状态，未审核**绝不注册**，因此仓库附带的 `config.toml` 无法仅凭被克隆就执行任何东西。没有 `[hooks]` 表的文件完全不产生来源，而格式错误的文件会被警告并跳过，与格式错误的 `hooks.json` 完全一致。

文件名用 `hooks.json`（Codex 的），不是 `.hooks.json`：从 `bm hook install --harness codex` 拷出来的文件可以原样使用。Tact 依然不读 `~/.codex/`。

条目声明两种**类型**之一，而下游的一切都是共享的：同样的输出归一化、同样的决策契约（`decision` / `hookSpecificOutput` / 裸 `exit 2`）、同样的 `additionalContextLimit`。

| `type` | 运行什么 | 字段 |
|---|---|---|
| `"command"`（或缺省） | 一条 shell 命令，JSON payload 走 stdin | `command`、`timeout`、`async` 等 |
| `"mcp_tool"` | **已连接 MCP server** 上的一个工具 —— 经由 agent 用的同一个 `MCPToolRouter` 抵达 | `server`、`tool`、`arguments` |

```json
{ "type": "mcp_tool", "server": "policy", "tool": "gate",
  "arguments": { "path": "secrets/.env" } }
```

第二种类型的意义在于：策略可以住在那个已经持有集成工具的 MCP server 里，返回与 shell 脚本相同的 `{"decision": …}` / `{"hookSpecificOutput": …}` 形状 —— 而不是写一个脚本，再用它所用的语言重新实现 payload、决策契约与 `additionalContext` 解析。回答 `{"decision":"block","reason":…}` 的工具会阻断；回答 `additionalContext` 的会注入上下文。

`arguments` 是**静态**的：hook payload 刻意不合并进去。一个输入随事件漂移的工具调用会让被审核的定义变成谎言，而定义正是用户批准的东西。

server 不可达、工具不存在或工具报错，都会被上报并**继续** —— 与所有其他 hook 失败一致：hook 不能靠自身损坏来中止循环。

由于身份哈希与审核列表都读取这份定义，一个 `mcp_tool` 条目以 `mcp_tool <server>/<tool> <arguments>` 标识。若对（不存在的）`command` 求哈希，同一来源里的每个 `mcp_tool` 条目都会得到同一个身份 —— 批准一个就等于批准其余全部，修改 `tool` 或 `arguments` 也不会让批准失效，而且审核界面会在本该显示定义的地方显示一行空白。缺少 `server` 或 `tool` 的条目会被上报为无法运行，而不是静默失效。

### 运行前的审核

hook 文件是可执行的配置，而仓库可以附带 `.tact/hooks.json`——所以克隆一个仓库绝不能执行它的命令。每个 hook 定义默认处于**未审核**状态，未审核的 hook **绝不注册**：文件会被读取，然后被拒绝。这是 Codex 的 `trusted_hash` 模型，配上 Tact 自己的存储。

- **身份**就是定义本身：来源标签、事件、matcher 与命令，用 SHA-256 哈希。改动命令即作废此前的批准。
- **存储**是 `~/.tact/hooks-state.json`（`{"version":1,"trusted":{hash:描述}}`）。它刻意不是 `config.toml`：手改配置不该能授予执行权。无法解析的存储等同空存储，于是所有 hook 回到待审核。
- **审核**用 `tact-ui hooks list`（看有哪些、状态如何），随后 `tact-ui hooks trust --all` 或 `tact-ui hooks trust --source <label>`；`tact-ui hooks forget --all` 撤销全部。同一套审核也能在 hook 实际触发的地方完成：TUI 里的 `/hooks list`、`/hooks trust --all`、`/hooks trust --source <标签>`、`/hooks forget --all`，措辞完全一致；`/hooks list` 仅在空闲时可执行，因为 driver 会把非 fast 命令串行排到进行中的 turn 之后。TUI 写法同样要求显式的 `--all` / `--source`，没有“全部批准”的捷径。
- **生效**发生在 hook 注册时，所以批准从下一次会话开始起作用；正在运行的会话保留它启动时的决定。
- **告知读者**从不省略：未审核的 hook 会在 TUI 里走 `AgentUpdate::Info` 通道，headless 下走 stderr（`[hooks] …`）——与 MCP 加载报告相同的两条通道。

输出契约以 **Codex**（`codex-rs/hooks`）为准：`decision` / `reason`、`hookSpecificOutput.additionalContext`、`suppressOutput`、`continue` 与 `command` handler 才是 Tact 建模并遵守的部分。只属于 Claude 的输出有意不实现——这里没有 `systemPrompt` 处理，因为没有插件能合法发出它（Claude 的 SessionStart 文档列的是 `additionalContext` / `initialUserMessage` / `watchPaths` / `sessionTitle` / `reloadSkills`，而 Codex 的 schema 恰好只有 `hookEventName` + `additionalContext`）。

已安装的 marketplace 插件可通过 `.codex-plugin/plugin.json`（`"hooks": "./hooks/hooks.json"`）声明命令 hook。`apply_plugin_hooks_with_report`（`crates/tact/src/plugin/hooks.rs`）在 `interactive.rs` / `headless.rs` 中把**已审核**的那些注册到 `Agent` 上，覆盖十五个映射事件中的十三个（`SubagentStart` / `SubagentStop` 改由 `plugin_subagent_*_hooks` 构建 `ToolContext` 闭包）：

- `SessionStart` — matcher 与真实的 `source`（`startup` / `resume` / `compact`）匹配；`additionalContext`（JSON，或纯 stdout —— 参考实现 `basic-memory` 插件正是以这种形式打印它的简报）会在第一轮之前被记录为一条合成的 `<hook-context>` user 消息；`continue: false` 会跳过这一轮（该 schema 里没有 `decision`）。
- `UserPromptSubmit` — matcher 匹配 prompt 文本；`additionalContext` 输出追加到用户 prompt。
- `PreToolUse` — matcher 匹配工具名；`additionalContext` 会记录为下一轮请求之前的对话上下文；`block` 阻止执行，而 `permissionDecision: "allow"` 会在不询问的情况下执行该调用。
- `PermissionRequest` — 只在 Tact **即将询问审批**时运行（matcher 匹配工具名），因此一个只负责批准的策略 hook 不必为无需审批的调用付出代价：`allow` 跳过询问，`block` 带理由拒绝，其余把决定留给用户。
- `Interrupt` — 仅观测，matcher 匹配字面量 `interrupt`；用户取消（`/cancel`）时每轮触发一次，用于记日志或冲刷。
- `PostToolUse` — matcher 匹配工具名；`additionalContext` 会记录为下一轮请求之前的对话上下文；`suppressOutput` 清空结果；`block` 使其变为失败。
- `PostToolUseFailure` — matcher 匹配工具名；仅观测（`tool_name`、`tool_input`、`tool_use_id`、`error`）。
- `Notification` — matcher 匹配通知类型（`permission_prompt`）；仅观测（`notification_type`、`title`、`message`）。
- `TaskCompleted` — matcher 忽略（对齐）；仅观测（`task_description` = 最后一条 assistant 消息）。
- `SubagentStart` — `plugin_subagent_start_hooks` 构建 `ToolContext` 闭包；`additionalContext` 追加到子代理 system prompt。
- `SubagentStop` — `plugin_subagent_stop_hooks` 构建 `ToolContext` 闭包；子代理结束后运行；仅观测（`block` 无法恢复已结束的子代理）。
- `Stop` — matcher 忽略（与 Codex 对齐）；`block` 以 `reason` 作为下一条 prompt 继续该回合。
- `SessionEnd` — matcher 匹配 `reason`（`"other"`）；仅观测。
- `PreCompact` — matcher 匹配 trigger 字符串（`auto`/`manual`/`recovery`/`command`）；`block` 否决压缩。
- `PostCompact` — matcher 匹配 trigger 字符串；仅观测。

每条 hook 是一个 shell 命令（Unix 用 `sh -c`，`commandWindows` 暂不处理），注入 `CLAUDE_PLUGIN_ROOT` / `CLAUDE_PROJECT_DIR` 环境变量，stdin 输入 Claude 输入 JSON（`session_id`、`transcript_path`、`cwd`、`hook_event_name` 及事件字段），stdout 输出 JSON 同时兼容新版 `decision` / `reason` / `additionalContext` 与旧版 `hookSpecificOutput` 格式。`timeout` 默认 60s，`async: true` 即发即忘，`additionalContextLimit` 以 token 为单位限制单个 hook 能注入多少上下文——作用在上下文产生处，而不是看会话还能装多少。

**`exit 2` 是最简单的阻断契约。** 理由是 hook 的 **stderr** 文本，而它阻断什么按事件区分，与 Codex 的定义一致：

| 事件 | `exit 2` 的含义 |
|---|---|
| `PreToolUse`、`PermissionRequest` | 阻断，`stderr` 即理由 |
| `PostToolUse` | 该 tool 结果变为携带 `stderr` 的失败 |
| `Stop`、`SubagentStop`、`UserPromptSubmit` | 携 `stderr` 作为下一轮 prompt 继续 |
| 其余事件 | fail-open，但仍会上报 |

JSON 决定永远优先于裸 `exit 2`：打印了决定的 hook 已经表明了意图，只有**什么都没决定**的 hook 才回退到退出码。`additionalContext` 与 `systemMessage` 算附加而非决定，因此可以与 `exit 2` 并存。

其余所有失败——超时、启动失败、非 `2` 的非零退出、看着像 JSON 却解析失败——仍然只告警、显示 `[plugin hook <Event> failed] …` 并 **继续**（fail-open，与 Claude Code 一致）。

插件 hook 在既有 Rust 闭包之后按声明顺序执行；`Block` 短路。

堆叠多个来源时该顺序是确定的。插件按 `<marketplace>/<plugin>` 键序访问——即 `installed.json` 的 `BTreeMap` 顺序，字典序，**不是**安装时间；同一插件内部按 hooks 文件（或 manifest 内联映射）中该事件 matcher 的声明顺序。没有 priority 字段：跨插件顺序由该键固定，你能控制的是单个插件内部的声明顺序。

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

## 10. SessionStart

`Agent::session_start` 接受签名如下的 hooks：

```rust
Fn(&LoopState, &mut SessionStartContext) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + '_>>
```

它们**每会话运行一次，且发生在第一轮**——不在启动时。插件 hook 是一个子进程，可能要跑好几秒（参考实现 `basic-memory` 的 hook 实测热启动约 8 秒、`uv` 缓存冷启动约 100 秒），在首帧之前等它会让每次启动都变慢。`AgentRuntime::session_start_hooks_pending` 保证这组 hook 只跑一次，而它们之间是**并发**执行的，收集到的内容仍按注册顺序排列。

hook 收集到的 context 落在 `AgentRuntime::pending_session_context` 上，而不是直接进入对话：`dispatch_session_start_hooks` 发生在 `ensure_session` 之前，此时 `push_message` 会让 context 非空、从而抑制历史恢复。`agent_loop` 在**本轮预压缩之后**、本轮用户消息之前把它取走，每个片段各记为一条合成的 `<hook-context>` user 消息，携带 `MessageKind::HookContext`。必须在压缩之后：`build_compacted_history` 只保留真实 user turn，早于压缩注入的 cell 会被它伴随的那次压缩直接丢掉。

超过约 2,500 token 的片段会全文写到 `<temp_dir>/hook_outputs/<session>/`，模型看到的则是头尾预览加一句 `Full hook output saved to: <path>`（Codex 的 `HookOutputSpiller` 及其默认上限）；TUI 仍显示 hook 的完整文本。

插件这条路径还会补齐 Codex 的 `session-start.command.input` 在 Claude 基础字段之外要求的两个字段——`model` 与 `permission_mode`（后者用 Tact 映射过去的 Claude Code 词汇：`default` / `plan` / `acceptEdits`）——并通过 `AgentUpdate::Info` 把插件自己的 `statusMessage` 显示出来，让要跑几秒的 hook 有反馈而不是一片沉默。

这个时机与「一片段一条消息」的规则与 Codex 一致：它的 `SessionStart` 处理把每个 `additionalContext` 各记为一条 `developer` 角色消息，而它的 start hooks 也跑在 `run_pre_sampling_compact` 之后。Tact 的消息模型只有 user/assistant，所以改由 `<hook-context>` 标记来承载来源信息 —— 而且与内存中的 kind 不同，这些标记在重新加载后依然存在。stdout 看起来像 JSON 却解析失败时，按失败的 hook 处理而非注入，与 Codex 的 `looks_like_json` 检查一致。

matcher 匹配的是**真实的** `source`：全新会话是 `startup`，`ensure_session` 恢复了历史则是 `resume`，而压缩重新排队这批 hook 时是 `compact`。最后一种正是插件在上下文被摘要掉之后重新定位的手段——参考实现 `basic-memory` 插件就是这样请求一份需要人工撰写的 checkpoint——代价是每次压缩多跑一次 hook，与 Codex 完全一致。

`SessionStart` hook 返回 `continue: false` 会**跳过这一轮**：`agent_loop` 在用户消息入队之前就返回，与 Codex 的 `return Ok(None)` 一致；读者看到的是它的 `stopReason`。那个 schema 里没有 `decision: block`，所以它不是停止会话的手段。

### hook 的 payload

每个事件 stdin 上的 payload 都带齐了 Codex 各 schema 要求的字段，照着那些 schema 写的插件不会再读到 `null`：

| 字段 | 值 |
|---|---|
| `session_id` | 当前会话 id |
| `cwd`、`hook_event_name` | 同以前 |
| `model` | `Agent::model()`——当前模型，会跟随 `/model` |
| `permission_mode` | Claude Code 词汇：`default` / `plan` / `acceptEdits` |
| `turn_id` | `Agent::turns_taken` |
| `transcript_path` | **`null`**——Tact 把会话放在 SQLite，只在压缩时才写 transcript，因此没有单一的活动文件（Codex 指向它的 rollout 文件）。此前这里报的是 transcripts **目录**，而插件会照着去打开它。 |
| `tool_use_id` | `PreToolUse` / `PostToolUse` 上有：被标注的那次调用 |

失败的 hook（非零退出、超时、命令起不来）是 fail-open 的，而且从这次改动起**可见**：agent 会发出 `[plugin hook <Event> failed] <error>`，因为单靠 `tracing::warn!` 只会写进一个默认会话永远不会写的日志文件。

### 哪些事件会把 `additionalContext` 带进对话

Codex 一共只有四条这样的通道——`SessionStart` / `SubagentStart`（两者共用一个 outcome 类型）、`UserPromptSubmit`、`PreToolUse`、`PostToolUse`——Tact 现在覆盖同一集合：

| 事件 | context 的去处 |
|---|---|
| `SessionStart` | 第一轮用户消息之前的一条 `<hook-context>` 消息 |
| `PreToolUse` / `PostToolUse` | 下一轮请求之前的一条 `<hook-context>` 消息，与它所标注的工具调用相邻 |
| `SubagentStart` | 追加到子代理的 system prompt（Claude Code 语义） |
| `UserPromptSubmit` | 追加到 prompt 文本本身 |

其余九个事件只承担控制/观察职责；它们的 `additionalContext` 也不是 Codex 定义的通道。特别地，`PreToolUse` 的 context **不会**写进工具参数——早先那个 `tool_use.input["_hook_context"]` 键没有任何读取者，于是 context 消失、字段还泄漏进了权限检查与工具本身能看到的内容里。

session hooks 也适合一次性 setup：预热缓存、校验工作区不变量或注入遥测 context。

---

## 11. 设计约束

| 约束 | 理由 |
|------|------|
| Hooks 在 agent task 上运行 | 经 `LoopState` 间接持有 `&mut Agent`；工作宜短或在内部 spawn。 |
| 并行波次内无 hooks | tool 不可变借用 router 时，避免共享 agent 状态的数据竞争。 |
| 首个 `Block` 生效 | 可预测、易推理的否决语义。 |
| 错误即步骤失败 | hook bug 表现为 tool 失败，而非静默 no-op。 |
| 注册顺序 = 运行顺序 | 堆叠插件时 hook 优先级为 `<marketplace>/<plugin>` 键序（见 §6）；没有 priority 字段可设。 |

**不要**在 hooks 里做权限 UI——用 `PermissionManager` 与现有 `RequestSelect` 流程。

---

## 12. 代码地图

| 文件 | 职责 |
|------|------|
| `crates/tact/src/hook/mod.rs` | 类型、trait、`Hook` 枚举、`invoke_hooks!` 宏 |
| `crates/tact/src/agent/mod.rs` | `pre_tool`、`post_tool`、`session_start`、`hooks_by_type` |
| `crates/tact/src/agent/tool_dispatch.rs` | `execute_tool_call` 中的 PreToolUse / PostToolUse 调用 |
| `crates/tact/src/permission/mod.rs` | PreToolUse 之后运行；与 hooks 分离 |
| `crates/tact/src/plugin/hooks.rs` | `collect_hook_sources` / `config_hook_paths`（六个来源）、`HooksFile::{from_file, from_toml_file}`、`admit_trusted`、`HookTrust`、`survey_hooks`、`trust_hooks`、`run_hook`（按 `HookCommand::kind` 分发）、`run_command_hook`、`run_mcp_tool_hook`、`definition_text`、`build_payload` |
| `crates/tact-ui/src/hooks_cli.rs` | `tact-ui hooks list` / `trust` / `forget` 及其渲染函数 |
| `crates/tui/src/handlers/hooks.rs` | `/hooks list` / `trust` / `forget` —— 解析与空闲门控；实际工作由 driver 执行 |
| `crates/tact-ui/src/driver.rs` | `UserCommand::Hooks{List,Trust,Forget}` → `survey_hooks` / `trust_hooks` / `forget_hook_trust`，经 `Info` / `MdInfo` 通道上报 |
| `docs/state_machines.md` | Hook 控制枚举与流水线摘要 |

---

## 13. 有意留下的缺口

| 缺口 | 原因 |
|-----|-----|
| `SessionStart` 的 `clear` / `fork` 来源 | Codex 会上报它们，但 Tact 没有清空历史的命令、也没有会话 fork，因此这两个变体会不可达。词表是 `startup` / `resume` / `compact`——Tact 真正区分的那三个。 |


| 受管/企业 hook、`bypass_trust` | 管理员下发的 hook 包、以及关闭审核的开关，都是信任模型层面的决定，目前这里没有消费者。`tact-ui hooks trust --all` 是可脚本化的等价物。 |

---

## Related Docs

- [权限模型](./10_chapter_permission.md) — 流水线中紧接 PreToolUse 之后（英文）
- [任务与工具调度](./11_chapter_task.md) — hooks 所包裹的三阶段 tool 流水线（英文）
- [工具系统](./07_chapter_tool_zh.md) — 原生工具与 dispatch
- [ARCHITECTURE.md](../ARCHITECTURE.md) — Hook Engine 章节
- [Tool Rendering](../docs/tool_rendering.md) — TUI 中 blocked/failed 步骤如何显示
- [Parallel Tool Execution](../docs/parallel_tool_execution.md) — hooks **不**运行的位置
