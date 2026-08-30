# Claude Marketplace 插件全功能兼容设计

> 日期：2026-08-29 · 状态：已批准（用户选择"全部做"，先写设计文档）
> 关联：`crates/tact/src/plugin/`、`crates/tact/src/skill/mod.rs`、`crates/tact/src/hook/`、`crates/tact/src/mcp/mod.rs`、`crates/tact/src/tool/subagent.rs`、`crates/tact-ui/src/{interactive,headless}.rs`

## 1. 背景与动机

Tact 的插件从 Claude marketplace（`claude-plugins-official` 等）安装，但当前只消费 **skills** 一项功能。官方 marketplace 的 39 个插件中：

- 24 个有 `skills/*/SKILL.md`；
- 14 个有 `commands/*.md`（斜杠命令）；
- 13 个有 `agents/*.md`（声明式子代理）；
- ponytail 等第三方插件带 **hooks**（plugin.json `hooks` 字段）；
- example-plugin 带 `.mcp.json`（MCP 服务器）。

更严重的是安装校验**硬性要求 `skills/*/SKILL.md`**，导致 15+ 无 skills 的官方插件（LSP 文档类、`commit-commands`、`code-review`、`code-simplifier`、`security-guidance` 等）**根本无法安装**。

目标：让 Tact 与 Claude Code 的插件功能面对齐——安装校验、命令、代理、钩子、MCP 五类功能全部兼容。

## 2. 范围

### 在内（Phase 1–5）

| Phase | 内容 |
|---|---|
| 1 | 安装/清单兼容：放宽校验、完整解析 plugin.json、InstalledPlugin 记录功能清单 |
| 2 | `commands/*.md` 加载为 `plugin:<name>` 斜杠命令 |
| 3 | MCP：从已安装插件缓存扫描 `.mcp.json` + `mcpServers` 并注册 |
| 4 | `agents/*.md` 声明式子代理注册表 + `spawn_subagent` 按名引用 |
| 5 | plugin.json hooks → 命令型 hook 运行时（SessionStart / UserPromptSubmit / PreToolUse / PostToolUse / SubagentStart） |

### 不在内（文档化为限制）

- **Python SDK 自定义工具**（`tools/` 目录）：官方插件当前没有使用，需要 Python 运行时 + SDK，v1 不做。
- **远程 http/url 类型 MCP**：Tact 的 `McpClient` 仅支持 stdio，http 类型跳过并告警。
- **Notification / Stop / SubagentStop / PreCompact / PostCompact / SessionEnd 事件**：Tact 循环没有对应注入点，v1 不映射。
- **skills 的 `allowed-tools` 预授权 / `model` 覆盖**：解析并保存，但权限系统不强制执行（v1 限制）。**声明式 agents 的 `model` 例外**：`inherit` 不覆盖、`sonnet/opus/haiku` 忽略告警、具体 id 透传。
- **SessionStart hook 修改 system prompt**：Tact 的 SessionStart hook 只返回 Continue/Block，`systemPrompt`/`updatedSystemPrompt` 与纯文本 `additionalContext` 输出仅告警不应用。
- **未映射 hook 事件**：官方部分插件使用 `UserPromptExpansion`、`Stop` 等事件，Tact 未映射（与 Notification 等一样在 v1 限制内），但含默认 `hooks/hooks.json` 的插件仍会被识别为 has_hooks 并加载其中映射到已知事件的 hook。

## 3. 现状分析（差距表）

| Claude 插件功能 | Tact 现状 | 差距 |
|---|---|---|
| `skills/*/SKILL.md` | `get_skill_registry` → `load_plugin_skills` 加载为 `plugin:<name>` | 仅 name/description；缺 argument-hint/allowed-tools/model |
| `commands/*.md` | 无 | 完全不加载 |
| `agents/*.md` | 无 | 不加载；`.tact/agents/*.md` 一直是 deferred |
| hooks | `hook/mod.rs` Rust 闭包型，SessionStart/PreToolUse/PostToolUse 三点 | 无 JSON 命令型 hook、无 UserPromptSubmit/SubagentStart 注入点 |
| MCP | `load_mcp_router` 仅扫 cwd 的 `.claude-plugin/plugin.json` | 不扫已安装插件缓存、不支持 `.mcp.json` |
| 安装校验 | `validate_plugin_candidate` 强制 skills | 无 skills 插件装不上 |
| manifest | `CompatibilityManifest` 只读 name | 缺 description/version/author/hooks/mcpServers |

关键集成点（已核实）：
- skills → `get_skill_registry`（skill/mod.rs）→ `interactive.rs` 构建 `skills_data` → TUI 斜杠弹窗 → `submit_user_task`（handlers/skills.rs）。**commands 只要进入 SkillRegistry 即自动进入整条链**。
- MCP → `load_mcp_router()`（mcp/mod.rs）在 `interactive.rs:103` / `headless.rs:94` 调用，`mcp_router` 进入 `Agent.mcp_router`。
- hooks → `Agent::with_session_start/with_pre_tool/with_post_tool`（agent/mod.rs）在 `interactive.rs:159-161` / `headless.rs:137-139` 注册；`invoke_hooks!` 宏遍历 `Agent.hooks`。
- agents → `spawn_subagent`（tool/subagent.rs）使用固定 `subagent_toolset()`（Bash/ReadFile/Sleep/WriteFile/EditFile）+ 静态 system prompt。

## 4. 设计

### 4.1 Phase 1 — 安装/清单兼容

**完整 manifest 解析**（`crates/tact/src/plugin/install.rs` 的 `CompatibilityManifest` → 更名 `PluginManifest`）：

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: Option<String>,
    #[serde(default)] pub description: Option<String>,
    #[serde(default)] pub version: Option<String>,
    #[serde(default)] pub author: Option<Value>,       // 仅透传展示
    #[serde(default)] pub hooks: Option<String>,        // 相对路径 → hooks JSON
    #[serde(default)] pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
}
```

`McpServerConfig` 复用 `crates/tact/src/mcp/mod.rs` 的（command/args/env，camelCase）。

**功能摘要**（install.rs 的 `validate_plugin_candidate` 返回）：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginFeatures {
    pub skill_count: usize,
    pub command_count: usize,
    pub agent_count: usize,
    pub has_hooks: bool,
    pub has_mcp: bool,
}
impl PluginFeatures { pub fn is_empty(self) -> bool { /* 全零 */ } }
```

校验规则：至少一种功能非空（skills≥1、commands≥1、agents≥1、hooks 指向存在的 JSON、mcpServers 或 `.mcp.json` 存在），否则报"插件不包含 Tact 支持的任何功能"。`hooks` 路径解析：相对于插件根，`${CLAUDE_PLUGIN_ROOT}` 不在此处展开（运行时展开）。**默认发现路径**：manifest `hooks` 字段缺省时回退 `<root>/hooks/hooks.json`（Claude Code 默认路径，官方 6 个 hooks 插件依赖它）。

**InstalledPlugin 扩展**（`plugin/model.rs`，全部 `#[serde(default)]` 兼容旧记录）：

```rust
pub struct InstalledPlugin {
    pub id: String, pub marketplace: String, pub revision: String,
    pub cache_path: PathBuf, pub skill_count: usize,   // 保留
    #[serde(default)] pub command_count: usize,
    #[serde(default)] pub agent_count: usize,
    #[serde(default)] pub has_hooks: bool,
    #[serde(default)] pub has_mcp: bool,
}
```

**展示**：`tact plugin list` 打印功能摘要；TUI 插件列表弹窗（`widgets/state/app/extensions.rs`）显示功能徽章；i18n 新增字符串。

### 4.2 Phase 2 — Commands 加载

Claude Code 官方说明：`commands/*.md` 与 `skills/<name>/SKILL.md` 加载方式相同，只是文件布局不同。Tact 已把插件 skills 当作斜杠命令使用，所以 **commands 进入 SkillRegistry 即可全链路生效**。

- `SkillRegistry` 新增 `load_plugin_commands(&self, commands_dir: &Path, plugin_id: &str)`：遍历 `commands/*.md`，name = 文件名去掉 `.md`（不是目录名），description 取 frontmatter，命名空间 `plugin:<name>`。
- `get_skill_registry` 在加载 plugin skills 后追加加载 plugin commands（遍历已安装插件根，`<cache>/commands` 存在时）。
- 冲突语义：同一插件内 skills 与 commands 同名时，后加载的 commands 覆盖 skills（与 Claude Code 一致：命令优先）。加载顺序定为先 skills 后 commands。
- frontmatter 扩展（`SkillFrontmatter` 增加字段，均 `Option`）：`argument-hint`（serde rename）、`allowed-tools`（逗号分隔字符串）、`model`。存入 `SkillManifest` 备用（v1 不强制执行）。

### 4.3 Phase 3 — MCP 从已安装插件加载

新增（`crates/tact/src/mcp/mod.rs`）：

```rust
/// 扫描已安装插件缓存，返回 (server_name, config)。
/// server_name = "plugin__<plugin_id>__<server>"
pub fn installed_plugin_mcp_servers(home: &PluginHome) -> Result<Vec<(String, McpServerConfig)>>
```

扫描每个有效缓存插件根：
1. `.claude-plugin/plugin.json` 的 `mcpServers`（复用 `PluginManifest`）；
2. 插件根 `.mcp.json`（Claude 项目格式）：

```json
{ "server-name": { "type": "stdio", "command": "...", "args": [...], "env": {...} } }
```

`.mcp.json` 解析器 `McpProjectConfig`：`type == "stdio"` → 构造 `McpServerConfig`；`type == "http"` / 有 `url` 字段 → `tracing::warn!("…http 类型暂不支持，跳过")`。

`load_mcp_router()` 在现有 cwd `PluginLoader` 扫描后追加 `installed_plugin_mcp_servers`（`PluginHome::from_environment()` 可解析时）。服务器命名沿用现有 `{plugin}__{server}` 前缀规范（`build_tool_specs` 生成 `mcp__<server>__<tool>`）。

### 4.4 Phase 4 — 声明式 Agents

**注册表**（新模块 `crates/tact/src/agent_def.rs`）：

```rust
pub struct SubagentDefinition {
    pub name: String,          // 本地名（插件内 = 文件名 stem）
    pub description: String,
    pub body: String,          // 定义正文 = 子代理 system prompt
    pub tools: Option<Vec<String>>,        // 限制子代理工具集
    pub model: Option<String>,             // 覆盖模型
    pub permission_mode: Option<PermissionMode>, // 覆盖权限模式
}
pub type SharedAgentDefinitionRegistry = Arc<Mutex<AgentDefinitionRegistry>>;
```

加载源（加载顺序，同名后者胜）：
1. 项目 `<workdir>/.tact/agents/*.md` → 原名（顺带解决 Ch 12 长期 deferred 项）；
2. 已安装插件 `<cache>/agents/*.md` → `plugin:<name>`。

frontmatter 解析：`name`、`description`、`tools`（逗号分隔）、`model`、`permissionMode`（映射到 `PermissionMode`，不识别则忽略）。

**ToolContext 扩展**：新增 `pub agent_registry: crate::agent_def::SharedAgentDefinitionRegistry`（`interactive.rs`/`headless.rs` 构建，模式同 `skill_registry`）。

**spawn_subagent 扩展**（`tool/subagent.rs`）：

```rust
pub struct SubagentInput {
    ...现有字段,
    #[serde(default)] pub agent: Option<String>,  // 声明式代理名（plugin:<name> 或原名）
}
```

语义：
- `agent: Some(name)` 且解析成功 → system prompt = 定义 body + "\n\n用户任务：\n" + `prompt`；
- `tools` 过滤 `subagent_toolset()`：Read/Glob/Grep → ReadFile，Bash → Bash，Edit → EditFile，Write → WriteFile，Sleep → Sleep；未知名忽略（至少保留一个已知名时）；**全部未知 → 报错**（绝不静默回退默认五件套，避免权限扩大）；`tools: None` 保持默认五件套；
- `model` 覆盖子代理模型（与现有 `settings.agent.subagent` 覆盖逻辑叠加）。Claude 别名处理：`inherit` → 不覆盖（保持父级模型）；`sonnet`/`opus`/`haiku` → 警告并忽略（Tact 无别名映射，透传会导致 LLM 调用失败）；其余按具体模型 id 透传；
- `permission_mode` 覆盖继承快照（仅当定义指定且父级非 Auto 时生效，Auto 保持粘性）；
- `agent` 未找到 → 明确报错（列出可用名字）。

### 4.5 Phase 5 — 命令型 Hook 运行时

**Claude hook JSON 格式**（plugin.json `hooks` 指向的 JSON）：

```json
{
  "hooks": {
    "SessionStart":    [ { "matcher": "startup|resume|clear|compact",
                            "hooks": [ { "type": "command", "command": "node …",
                                         "commandWindows": "…", "timeout": 5,
                                         "statusMessage": "…", "async": false } ] } ],
    "UserPromptSubmit":[ { "hooks": [ { "type": "command", "command": "…" } ] } ],
    "PreToolUse":      [ { "matcher": "Bash|Read", "hooks": [ … ] } ],
    "PostToolUse":     [ { "hooks": [ … ] } ],
    "SubagentStart":   [ { "hooks": [ … ] } ]
  }
}
```

**解析模型**（新模块 `crates/tact/src/plugin/hooks.rs`）：

```rust
pub struct HooksFile  { pub hooks: HashMap<String, Vec<HookMatcher>> }
pub struct HookMatcher{ pub matcher: Option<String>, pub hooks: Vec<HookCommand> }
pub struct HookCommand{ pub ty: String, pub command: Option<String>, pub command_windows: Option<String>,
                        pub timeout: Option<u64>, pub status_message: Option<String>,
                        pub async_: bool /* serde "async" */ }
```

**命令执行器**（`crates/tact/src/plugin/hook_runner.rs`）：

```
run_command_hook(cmd: &HookCommand, plugin_root: &Path, event: HookEvent, input: &mut Value) -> Result<HookOutput>
```

- 命令串展开：`${CLAUDE_PLUGIN_ROOT}` → plugin_root（`commandWindows` 仅在 Windows 使用，Unix 用 `command`）；
- 执行：Unix `sh -c`，注入 env `CLAUDE_PLUGIN_ROOT=<plugin_root>`、`CLAUDE_PROJECT_DIR=<work_dir>`；stdin 写输入 JSON（Claude 协议字段：`session_id`、`transcript_path`、`cwd`、`hook_event_name` + 事件字段）；stdout 收输出 JSON；stderr 收集进日志；`timeout` 秒（默认 60，0 = 不设）；`async_` 时 spawn 后立即返回 Continue；
- 输出解析兼容两种格式：
  - 新版：`{"decision": "approve"|"block", "reason": "…", "additionalContext": "…", "systemPrompt": "…"}`
  - 旧版：`{"hookSpecificOutput": {"hookEventName": "…", "permissionDecision": "allow"|"deny", "permissionDecisionReason": "…", "additionalContext": "…", "updatedSystemPrompt": "…"}}`
  - 超时 / 退出码非 0 / JSON 非法 → 记 warning 并按 Continue 处理（**失败不阻塞**，与 Claude Code 一致；`statusMessage` 透传到 TUI 状态栏可选）。

**Hook 注册**（`crates/tact/src/hook/mod.rs`）：

```rust
pub enum Hook {
    SessionStart(Box<dyn SessionStartFn>),
    UserPromptSubmit(Box<dyn UserPromptSubmitFn>),   // 新增
    SubagentStart(Box<dyn SubagentStartFn>),          // 新增
    PreToolUse(Box<dyn PreToolUseFn>),
    PostToolUse(Box<dyn PostToolUseFn>),
}
pub trait UserPromptSubmitFn: for<'a> Fn(&'a LoopState, &'a mut String) -> …HookControl…
pub trait SubagentStartFn:   for<'a> Fn(&'a LoopState, &'a mut SubagentStartCtx) -> …HookControl…
```

- 命令型 hook 通过适配闭包注册为对应 `Hook` 变体（匹配器在闭包内过滤）；
- 新注入点：
  - `UserPromptSubmit`：`agent_loop` 入口处，对用户消息文本跑 hook，`additionalContext` 追加到消息（实现：提取 Message 文本 → 修改 → 重建）；
  - `SubagentStart`：`spawn_subagent` 内，`SubagentStartCtx { name, prompt, system_prompt }`，`additionalContext` 追加到子代理 system prompt；
  - 现有三点（SessionStart/PreToolUse/PostToolUse）直接复用。

**事件输入字段**：
| 事件 | 额外输入 | matcher 匹配对象 |
|---|---|---|
| SessionStart | `source`（恒为 "startup"） | source |
| UserPromptSubmit | `prompt` | prompt 文本 |
| PreToolUse | `tool_name`, `tool_input` | tool_name |
| PostToolUse | `tool_name`, `tool_input`, `tool_response` | tool_name |
| SubagentStart | `subagent_name`, `prompt` | subagent_name |

**注册时机**：`interactive.rs`/`headless.rs` 构建 Agent 时，从 `PluginHome` 读全部已安装插件 → 解析 hooks 文件 → 为每个事件注册适配闭包。`tact plugin reload` 后新 Agent 会话生效（同 skills 刷新语义，文档注明）。

### 4.6 Phase 5 与现有 hook 语义

- 现有 Rust 闭包 hook（rtk_filter 等）与插件命令 hook **并存**。注册顺序：内置闭包先（`interactive.rs`/`headless.rs` 的 builder 链），插件命令 hook 由 `apply_plugin_hooks` 追加在后；任一 Block 短路（首个 Block 生效）。
- `HookTypes` 枚举同步扩展（strum discriminants 自动跟随）。`SubagentStart` **不进入** `Hook` 枚举——`spawn_subagent` 是工具处理器、无父 `Agent` 句柄，插件 SubagentStart 闭包存于 `ToolContext.subagent_start_hooks`（独立 `SubagentStartFn` trait），由 spawn 路径调用。

## 5. 兼容性与迁移

- `installed.json` 旧记录：新字段全部 `#[serde(default)]`，旧插件 skill_count 保留，command/agent 计数为 0 —— 无需迁移脚本。
- 已安装的无 features 插件（理论上不存在，因为旧校验要求 skills）不受影响。
- 官方 marketplace 重新拉取（`tact plugin marketplace update` / install 时的自动刷新）后即可安装此前被拒的 15+ 插件。
- `skill_count` 字段语义不变；`PluginFeatures` 通过 `From<&InstalledPlugin>` 提供展示。

## 6. 测试策略

- **install**：无 skills 仅有 commands/agents/hooks/mcp 的插件可安装；全空报错；manifest name 冲突仍拒绝；功能计数正确。
- **commands**：`commands/foo.md` → 注册表含 `plugin:foo`；frontmatter 无 name 时用文件名；skills/commands 同名覆盖顺序。
- **agents**：frontmatter 解析（tools/model/permissionMode）；`.tact/agents` 与插件 agents 命名空间不冲突；spawn 按名引用成功/失败路径；tools 过滤。
- **hooks**：hooks JSON 解析（matcher/command/timeout/async）；执行器输入输出 JSON 兼容两种格式；block 决策 → HookControl::Block；超时/失败 → Continue + warning；`${CLAUDE_PLUGIN_ROOT}` 展开；UserPromptSubmit 追加上下文进消息；SubagentStart 追加进子代理 prompt。
- **mcp**：插件缓存 `.mcp.json` stdio 被扫描注册为 `plugin__<id>__<server>`；http 跳过告警；plugin.json mcpServers 兼容。
- **UI**：`tact plugin list` 输出功能摘要；TUI 弹窗显示徽章；i18n。
- 全部走现有 wiremock/tempdir 隔离测试模式，不依赖真实 marketplace 网络。

## 7. 文档同步（AGENTS.md 表）

- Ch 02（skill）：commands 加载、插件功能面；
- Ch 08（mcp）：插件 MCP 扫描、`.mcp.json`；
- Ch 09（hook）：命令型插件 hook、新事件点、Claude 协议；
- Ch 12（subagent）：声明式 agents（.tact/agents + 插件 agents）、`spawn_subagent.agent`；
- Ch 21（config）/ Ch 23（tui）：plugin list 功能摘要；
- Ch 26 issue log（双语）追加条目。

## 8. 风险与取舍

- **hooks 执行安全**：插件 hook 是任意命令执行，与 Claude Code 一致（插件本身就是可执行内容）。文档注明来源信任问题。
- **UserPromptSubmit 消息重建**：需要谨慎处理 Message 的 content blocks（text-only 修改；非文本 block 原样保留）。
- **agents 的 tools 过滤**：Tact 子代理工具集与 Claude 命名不同，映射表需覆盖常用名（Read/Glob/Grep/Edit/Write/Bash）。
- **http MCP / Python tools / 未映射事件**：明确文档化限制，避免用户误以为全支持。
