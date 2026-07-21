# 权限模型（Permission Model）

> 语言：[中文](./10_chapter_permission_zh.md) · [English](./10_chapter_permission.md)

本章说明 Tact 如何决定每个工具调用是否可执行：按风险做意图分类、三种权限模式、会话内 allowlist，以及通过 TUI 的交互式审批。每个 native 与 MCP 工具都会在 `Agent::execute_tool_call` 的 Phase 1 经过同一道关卡——在 `PreToolUse` hook 之后、并行执行之前。Hook 顺序见 [Agent 生命周期 Hook](./09_chapter_hook_zh.md)。

---

## 1. 权限模型做什么

`PermissionManager`（`crates/tact/src/permission/mod.rs`）对每个工具调用回答一个问题：

> 给定此工具名与输入，我们应 **允许**、**拒绝**，还是 **询问用户**？

它 **不** 执行工具。它分类意图、应用当前模式与 allowlist，并返回 `PermissionDecision`。`crates/tact/src/agent/tool_dispatch.rs` 中的 agent 将其转为调度工具，或合成一条被拦截的 `ToolResult`。

| 层级 | 职责 |
|------|------|
| `normalize_capability()` | 解析 native / MCP 工具名；计算 `CapabilityRisk` |
| `PermissionManager::check()` | 将 risk + mode + allowlist 映射为 `PermissionBehavior` |
| `tool_dispatch.rs` | 通过 TUI `RequestSelect` 或 headless deny 处理 `Ask` |
| `bash` 工具 + `shell.rs` | 在执行时硬拦截一部分危险 shell 命令 |

Shell 命令有 **两层** 防护：高风险模式触发权限提示；更小的一组在 `bash` 工具内即被拒绝，即使用户已批准。

---

## 2. 意图分类

### 核心类型

```rust
pub enum CapabilitySource { Native, Mcp }

pub enum CapabilityRisk { Read, Write, High }

pub struct CapabilityIntent {
    pub source: CapabilitySource,
    pub server: Option<String>,  // MCP server 段（若有）
    pub tool: String,              // 解析后的短工具名
    pub risk: CapabilityRisk,
}
```

`normalize_capability(tool_name, tool_input)` 是唯一入口。它解析工具名，再调用 `classify_risk()`。

### Native 与 MCP 工具名

| 模式 | 示例 | 解析结果 |
|------|------|----------|
| Native | `read_file` | `source = Native`，`tool = "read_file"` |
| MCP | `mcp__demo__db__query` | `source = Mcp`，`server = Some("demo__db")`，`tool = "query"` |

MCP 名使用前缀 `mcp__`，随后 `server__tool`，以 **最右侧** 的 `__` 分割（因此 server ID 可含下划线）。

### 风险规则

分类是启发式的——基于工具名前缀，对 `bash` 则基于命令字符串：

| 风险 | 规则 |
|------|------|
| **Read** | `read_file`；以 `read`、`list`、`get`、`show`、`search`、`query`、`inspect`、`find` 开头的名称 |
| **Read**（bash） | 简单只读命令：`ls`、`pwd`、`cat`、`head`、`tail`、`wc`、`rg`、`grep`；或 `git status` / `diff` / `log` / `show` / `branch`——仅当命令无 shell 元字符（`;`、 `&`、 `\|`、 `` ` ``、`$`、`>`、`<`） |
| **High** | `task`（spawn 具备完整文件系统与 shell 访问的子 agent）；以 `delete`、`remove`、`drop`、`shutdown` 开头；bash 匹配高风险模式（见 [§7 Shell 高风险检测](#7-shell-高风险检测)） |
| **Write** | 其余一切（未知 native 工具与非 read bash 的默认） |

MCP 工具在解析后的 **短** 工具名上遵循相同前缀规则。例如 `mcp__demo__db__query` 因 `query` 匹配 read 前缀而分类为 **Read**。

---

## 3. PermissionBehavior：Allow、Deny、Ask

```rust
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

pub struct PermissionDecision {
    pub behavior: PermissionBehavior,
    pub reason: String,
}
```

| Behavior | 在 `tool_dispatch.rs` 中的含义 |
|----------|----------------------------------|
| **Allow** | 工具进入 Phase 2（并行执行） |
| **Deny** | `PreparedState::Resolved`，附带 `"Permission denied: …"`；模型收到失败的 tool result |
| **Ask** | 交互式提示（TUI）或自动 deny（headless）；见 [§6 TUI RequestSelect 流程](#6-tui-requestselect-流程) |

---

## 4. 权限模式

```rust
pub enum PermissionMode {
    Default,
    Plan,
    Auto,
}
```

显示标签（来自 `PermissionMode` 的 `Display` 实现）：

| 模式 | 标签 | 行为 |
|------|------|------|
| `Default` | `default - ask for writes` | Read 允许；Write 询问（除非在 allowlist）；High 始终询问 |
| `Plan` | `plan - read only` | Read 允许；Write 与 High **拒绝**且不提示 |
| `Auto` | `auto - allow non-high operations` | Read 与 Write 自动批准；High 仍询问 |

### `PermissionManager::check()` 中的决策顺序

检查按此固定顺序执行：

```text
1. Read risk?                    → Allow（所有模式；重置 consecutive_denials）
2. Plan mode + non-Read?         → Deny
3. High risk?                    → Ask（即使工具在 allowlist 上）
4. Tool in always_allowed_tools? → Allow
5. Auto mode?                    → Allow（非 high 的 write）
6. Default (or unreachable Plan) → Ask
```

```mermaid
flowchart TD
    TC["ToolUse { name, input }"] --> Norm["normalize_capability()"]
    Norm --> Risk["CapabilityRisk"]

    Risk -- Read --> Allow["Allow"]
    Risk -- Write / High --> Plan{"Plan mode?"}

    Plan -- Yes --> Deny["Deny"]
    Plan -- No --> High{"High risk?"}

    High -- Yes --> Ask["Ask user"]
    High -- No --> AllowList{"always_allowed_tools?"}

    AllowList -- Yes --> Allow
    AllowList -- No --> Auto{"Auto mode?"}

    Auto -- Yes --> Allow
    Auto -- No --> Ask
```

**High 风险覆盖：** 若用户对 `bash` 选择「Always allow this tool」，后续高风险 bash（如 `sudo ls`）仍返回 `Ask`。allowlist 仅绕过 Default 模式的 write 提示，不能绕过 High 风险分类。

---

## 5. Allowlist 与连续拒绝

### 会话内 allowlist

`PermissionManager` 持有 `always_allowed_tools: Vec<String>`。构造时（`try_new`）预置 `"read_file"`。

用户在 TUI 选择 **「Always allow this tool」** 时，`allow_tool(tool_name)` 追加精确工具名（如 `edit_file`、`bash`）。此后对该名的 **Write** 风险调用会跳过 Default 模式提示。

allowlist **仅内存**——不会持久化到 SQLite 或 TOML 跨会话。

### 连续拒绝

每次用户 **Deny** 使 `consecutive_denials` 加一。Allow once 与 always-allow 将其重置为零。

达到 `max_consecutive_denials`（默认 **3**）次拒绝后，`should_suggest_plan_mode()` 返回 true。非交互模式下 `ask_user()` 向 stderr 打印提示：

```text
[3 consecutive denials -- consider switching to plan mode]
```

目前没有自动切换模式——该消息仅为建议。

---

## 6. TUI RequestSelect 流程

当 `check()` 返回 `Ask` 且 agent 有 UI 通道（`runtime.ui_tx`）时，`tool_dispatch.rs` 发送：

```rust
AgentUpdate::RequestSelect {
    prompt,      // 例如 "Allow bash: {\"command\":\"npm test\"}"
    options,     // ["Allow once", "Deny", "Always allow this tool"]
    respond,     // 回 agent 的 oneshot channel
}
```

TUI（`crates/tui/src/widgets/state/app/agent.rs`）切换到 `InputMode::Select` 并渲染选择弹窗（`log_confirm = false`，避免选择项污染日志）。

| 用户选择 | 索引 | Agent 动作 |
|----------|------|------------|
| Allow once | 0 | 运行工具；在 `StepFinished` 上设置 `permission_label = "Allow once"` |
| Deny | 1（默认） | `PreparedState::Resolved`；`StepFailed` 附带 deny 消息 |
| Always allow this tool | 2 | `allow_tool(name)`；运行工具；`permission_label = "Always allow this tool"` |

`permission_label` 附加到 `StepResult`，并在 TUI 工具 meta 行显示。见 [Tool Rendering](../docs/tool_rendering.md)。

### Headless / 无 UI 通道

若缺少 `ui_tx`，agent 调用 `permission_manager.ask_user()`，**始终 deny** 并记录 stderr：

```text
[permission] non-interactive: denying <tool> (<input preview>)
```

因此 headless 运行相当于持续 deny，除非决策已是无需提示的 `Allow` 或 `Deny`。

---

## 7. Shell 高风险检测

共享逻辑在 `crates/tact/src/shell.rs`：

```rust
pub fn is_high_risk_shell_command(command: &str) -> bool;
pub fn validate_shell_command(command: &str) -> Result<()>;
```

`is_high_risk_shell_command` 将命令小写并检查被拦截子串：

| 模式 | 效果 |
|------|------|
| `sudo`、`shutdown`、`reboot` | High risk |
| `> /dev/`、`>> /dev/` | High risk |
| `rm -rf /`、`rm -fr /`、`rm -rf /*`、… | High risk |
| `rm -rf ~`、`rm -fr $home`、… | High risk |

### 两层

```mermaid
sequenceDiagram
    participant Agent
    participant Perm as PermissionManager
    participant TUI
    participant Bash as bash tool

    Agent->>Perm: check("bash", {command})
    alt High risk (e.g. sudo)
        Perm-->>Agent: Ask
        Agent->>TUI: RequestSelect
        TUI-->>Agent: Allow once
    else Write risk (e.g. npm test)
        Perm-->>Agent: Ask or Allow (mode/allowlist)
    end

    Agent->>Bash: call(command)
    alt validate_shell_command fails
        Bash-->>Agent: Error: Dangerous command blocked
    else OK
        Bash-->>Agent: stdout/stderr
    end
```

1. **权限层** — `classify_risk` 用 `is_high_risk_shell_command` 标记 High risk → 始终 `Ask`（只读 bash 除外）。
2. **执行层** — `bash` 与 `background_run` 在 spawn 前调用 `validate_shell_command`。被拦截的命令即使用户已批准也会失败。

无害的破坏性路径可在执行层通过但仍可能提示：例如 `rm -rf ./build` 通过 `validate_shell_command` 但分类为 **Write**，Default 模式会先询问。

只读 bash 检测拒绝含 shell 元字符的命令——`ls; rm -rf /` 是 **Write**，不是 Read。

---

## 8. 工具流水线中的集成

权限在 `execute_tool_call`（`crates/tact/src/agent/tool_dispatch.rs`）的 **Phase 1** 运行，严格在 hook 之后：

```text
For each ToolUse (sequential):
  stats · cancel check
  StepAdded / StepStarted
  PreToolUse hooks          ← 可变更 input 或 Block
  PermissionManager::check  ← 本章
  Ask → RequestSelect (if needed)
  PreparedState::Run | Resolved

Phase 2: parallel waves (no permission re-check)
Phase 3: build ToolResult blocks in model order
```

```mermaid
sequenceDiagram
    autonumber
    participant LLM
    participant Agent
    participant Hook as PreToolUse
    participant Perm as PermissionManager
    participant TUI
    participant Tool as ToolRouter / MCP

    LLM->>Agent: ToolUse blocks
    Agent->>Hook: invoke_hooks!(PreToolUse)
    alt HookControl::Block
        Hook-->>Agent: blocked message
    else Continue
        Agent->>Perm: check(name, input)
        alt Allow
            Perm-->>Agent: Allow
            Agent->>Tool: execute (Phase 2)
        else Deny
            Perm-->>Agent: Deny
            Agent-->>LLM: ToolResult (permission denied)
        else Ask
            Perm-->>Agent: Ask
            Agent->>TUI: RequestSelect
            TUI-->>Agent: user choice
            opt approved
                Agent->>Tool: execute
            end
        end
    end
```

`PermissionManager` 在 `AgentRuntime`（`crates/tact/src/agent/mod.rs`）上，不在 `ToolContext`。`task` 工具创建的子 agent 有独立 manager（始终 `PermissionMode::Default`），但继承主 agent 的 `ui_tx`，权限弹窗仍可用。

---

## 9. 配置

### TOML

```toml
[permission]
mode = "default"   # "default" | "plan" | "auto"
```

定义于 `PermissionTomlConfig`（`crates/tact/src/config/types.rs`）。省略时默认 `"default"`。

### CLI

`--permission-mode` / `-m` 通过 `config/resolve.rs` → `ResolvedConfig.permission_mode` 覆盖 TOML。

### 当前启动行为

| 入口 | 使用的模式 |
|------|------------|
| `tact-ui headless` | `permission_mode_from_config()` — 读 TOML / CLI；未知值回退 **Auto** |
| `tact-ui`（交互 TUI） | 与 headless 相同 — `permission_mode_from_config()` |

---

## 10. 代码地图

| 文件 | 角色 |
|------|------|
| `crates/tact/src/permission/mod.rs` | `CapabilityRisk`、`PermissionManager`、`normalize_capability`、分类启发式 |
| `crates/tact/src/shell.rs` | 共享高风险 shell 模式；执行时 `validate_shell_command` 拦截 |
| `crates/tact/src/agent/tool_dispatch.rs` | 预检权限；`RequestSelect` 处理；`StepFinished` 上的 `permission_label` |
| `crates/tact/src/agent/mod.rs` | `AgentRuntime.permission_manager` |
| `crates/tact/src/tool/bash.rs` | spawn shell 前调用 `validate_shell_command` |
| `crates/tact/src/background.rs` | 后台 shell 命令同样校验 |
| `crates/tact/src/tool/subagent.rs` | 子 agent 用 `Default` 模式；继承 `ui_tx` |
| `crates/tact-ui/src/permission.rs` | `permission_mode_from_config()` |
| `crates/tact-ui/src/headless.rs`、`interactive.rs` | 会话启动时构造 `PermissionManager` |
| `crates/tact/src/config/types.rs` | `[permission] mode` TOML schema |
| `crates/tui/src/widgets/state/app/agent.rs` | 处理 `AgentUpdate::RequestSelect` |
| `crates/protocol/src/lib.rs` | `AgentUpdate::RequestSelect`、`StepResult.permission_label` |

---

## 11. 当前缺口

| 缺口 | 详情 |
|------|------|
| Allowlist 未持久化 | 「Always allow this tool」仅当前进程有效 |
| 无运行时模式切换 API | 用户须以不同模式重启；连续拒绝后 stderr 仅建议 Plan |
| Headless 自动 deny 所有 Ask | 除 Auto/Plan/Default 逻辑已避免 Ask 外，无非交互批准路径 |
| `PlanStep.need_approval` 已弃用 | 字段标记 `#[deprecated(since = "0.19.0")]`；用 `PlanStep::new()` — 权限由 `PermissionManager` 驱动 |
| 权限与 hook 重叠 | 两者均可拦截工具；hook 先运行，`Block` 时跳过权限 |

---

## Related Docs

- [任务与工具调度](./11_chapter_task_zh.md) — 权限所在的三阶段流水线
- [子 Agent](./12_chapter_subagent_zh.md) — `task` 为 High 风险、独立 `PermissionManager`、继承 `ui_tx`
- [Agent 生命周期 Hook](./09_chapter_hook_zh.md) — PreToolUse 紧接在权限检查之前
- [ARCHITECTURE.md](../ARCHITECTURE.md#3-permission-system) — 架构图与模式表
- [docs/state_machines.md](../docs/state_machines.md) — 权限决策状态机
- [docs/tool_rendering.md](../docs/tool_rendering.md) — `permission_label` 在 TUI 中的展示
- [docs/parallel_tool_execution.md](../docs/parallel_tool_execution.md) — 预检为何保持串行
