# Claude Marketplace 插件全功能兼容实现计划

> 日期：2026-08-29 · 设计：`docs/superpowers/specs/2026-08-29-claude-plugin-compat-design.md`
> 分支：`feat/plugin-compat`（在 `feat/async-subagent` 之后）
> 目标：Claude marketplace 插件的 skills / commands / agents / hooks / MCP 五类功能全部兼容。

## 执行顺序总览

1. T1–T3（Phase 1）：安装/清单兼容 —— 放宽校验、完整 manifest、功能摘要。
2. T4–T5（Phase 2）：commands 加载。
3. T6–T7（Phase 3）：MCP 从已安装插件加载。
4. T8–T12（Phase 4）：声明式 agents + `spawn_subagent.agent`。
5. T13–T19（Phase 5）：命令型 hook 运行时 + 新注入点。
6. T20：UI/CLI 展示 + i18n。
7. T21：文档同步（Ch 02/08/09/12/21/23 + Ch 26 双语）。
8. T22：全量验证（cargo test + clippy + 手工冒烟）。

---

## Phase 1 — 安装/清单兼容

### T1 放宽安装校验 + 功能摘要

- `crates/tact/src/plugin/install.rs`
  - `CompatibilityManifest` → `PluginManifest`（name/description/version/author/hooks/mcp_servers，camelCase，全 `Option`/`default`）。
  - `validate_plugin_candidate` 返回 `PluginFeatures { skill_count, command_count, agent_count, has_hooks, has_mcp }`；至少一种非空，否则报"插件不包含 Tact 支持的任何功能"。
  - hooks 判定：manifest.hooks 相对路径存在（或 `hooks/` 目录存在含 `.json`）。
  - mcp 判定：manifest.mcp_servers 非空 或 插件根 `.mcp.json` 存在。
- `crates/tact/src/plugin/model.rs`：`InstalledPlugin` 增加 `command_count` / `agent_count` / `has_hooks` / `has_mcp`（serde default）；新增 `PluginFeatures` 模型（可 `From<&InstalledPlugin>`）。
- 测试：无 skills 仅 commands 可安装；仅 agents 可安装；仅 hooks 可安装；仅 mcp 可安装；全空报错；功能计数正确；旧 installed.json 反序列化兼容。

### T2 install/update 记录新字段

- `install_source` 用返回的 `PluginFeatures` 填 `InstalledPlugin`。
- `update` 时新修订重新计算并覆盖。
- 测试：install 后 `command_count`/`has_hooks` 等正确。

### T3 CLI list 功能摘要

- `crates/tact-ui/src/plugin_cli.rs`：`tact plugin list` 输出 `skills=N commands=M agents=K hooks hooks=Y/N mcp=Y/N`。
- 测试：plugin_cli_tests 更新。

## Phase 2 — Commands 加载

### T4 SkillRegistry 加载 plugin commands

- `crates/tact/src/skill/mod.rs`
  - `load_plugin_commands(commands_dir, plugin_id)`：遍历 `commands/*.md`，name = 文件 stem，命名空间 `plugin:<stem>`。
  - `get_skill_registry` 在 plugin skills 之后追加 plugin commands（需要 `PluginStore` 提供已安装插件根列表——复用 `installed_skill_roots` 的结构，新增 `installed_plugin_roots` 返回所有有效缓存根）。
  - 同一插件内 skills/commands 同名：后加载 commands 覆盖。
- `SkillFrontmatter` 增加 `argument-hint` / `allowed-tools` / `model`（parse 并存入 `SkillManifest`，v1 不强制）。
- 测试：commands 加载为 `plugin:foo`；无 name frontmatter 用文件名；同名覆盖顺序；frontmatter 新字段解析。

### T5 plugin store 提供通用根

- `crates/tact/src/plugin/store.rs`：新增 `installed_plugin_roots() -> Vec<PluginRoot { plugin_id, root: PathBuf }>`（供 commands/agents/hooks/mcp 复用），`installed_skill_roots` 改由它派生（保持现有 API/测试）。
- 测试：根列表仅含有效缓存。

## Phase 3 — MCP 从已安装插件加载

### T6 `.mcp.json` 解析 + 插件缓存扫描

- `crates/tact/src/mcp/mod.rs`
  - `McpProjectConfig`：`{ type, command, args, env, url }`（serde camelCase + `type` serde rename）。
  - `installed_plugin_mcp_servers(home: &PluginHome) -> Result<Vec<(String, McpServerConfig)>>`：遍历已安装插件根，读 `.claude-plugin/plugin.json` mcpServers + `.mcp.json`；stdio → `McpServerConfig`，http/url → warn 跳过；server 名 `plugin__<id>__<server>`。
- 测试：stdlib server 注册；http 跳过；plugin.json mcpServers 兼容；无插件时为空。

### T7 接入 load_mcp_router

- `load_mcp_router()` 在 cwd PluginLoader 后追加 `installed_plugin_mcp_servers`（`PluginHome::from_environment()` 可解析时），合并进 `McpServerConfig` 集合后统一连接。
- 测试：现有 mcp 测试保持；新增含插件服务器的路由器构建测试（mock service）。

## Phase 4 — 声明式 Agents

### T8 注册表与加载器

- 新模块 `crates/tact/src/agent_def.rs`：
  - `SubagentDefinition`（name/description/body/tools/model/permission_mode）。
  - `AgentDefinitionRegistry` + `SharedAgentDefinitionRegistry`；`get_agent_definition_registry(workdir)` 加载 `.tact/agents/*.md`（原名）+ 已安装插件 `agents/*.md`（`plugin:<name>`）。
  - frontmatter 解析：name/description/tools(逗号)/model/permissionMode。
- `crates/tact/src/lib.rs` 导出；`consts.rs` `TactPath` 增加 `agents_dir()`（`<workdir>/.tact/agents`）。
- 测试：两种根加载与命名空间；同名覆盖；frontmatter 解析；非法 permissionMode 忽略。

### T9 ToolContext 携带注册表

- `crates/tact/src/tool/mod.rs`：`ToolContext` 增加 `pub agent_registry: SharedAgentDefinitionRegistry`。
- `interactive.rs` / `headless.rs`：构建并注入（同 skill_registry 模式）。
- 测试：孤儿上下文默认空注册表可用。

### T10 spawn_subagent 按名引用

- `crates/tact/src/tool/subagent.rs`：`SubagentInput.agent: Option<String>`（serde default）。
- 解析流程：agent 名 → 注册表查 `plugin:<name>` 与原名 → system prompt = body + 用户任务；`tools` 过滤子代理工具集（Read/Glob/Grep→ReadFile, Bash→Bash, Edit→EditFile, Write→WriteFile, Sleep→Sleep，未知名忽略）；`model` 覆盖；`permission_mode` 覆盖（父级 Auto 保持粘性）。
- 未找到 → 明确报错并列出可用名。
- 测试：agent 引用成功（system prompt 含 body、工具集被过滤、model/permissionMode 生效）；未知 agent 报错。

### T11 系统提示与工具描述更新

- `SPAWN_SUBAGENT_METADATA.description` 补充 `agent` 字段说明。

### T12 文档字段表更新

- 设计文档 §4.4 落地后同步 Ch 12（T21 统一做）。

## Phase 5 — 命令型 Hook 运行时

### T13 hook JSON 解析模型

- 新模块 `crates/tact/src/plugin/hooks.rs`：`HooksFile` / `HookMatcher` / `HookCommand`（ty/command/commandWindows/timeout/statusMessage/async，serde camelCase + "async" rename）。
- `PluginManifest.hooks` 相对路径解析（相对插件根）。
- 测试：官方 ponytail hooks JSON 可解析；matcher/command/timeout/statusMessage 字段；非法 JSON 报错。

### T14 命令执行器

- 新模块 `crates/tact/src/plugin/hook_runner.rs`：
  - `run_command_hook(cmd, plugin_root, work_dir, event, input_json) -> Result<HookOutput>`；
  - `${CLAUDE_PLUGIN_ROOT}` 展开；env `CLAUDE_PLUGIN_ROOT` / `CLAUDE_PROJECT_DIR`；Unix `sh -c`（保留 commandWindows 字段仅 Windows 用）；
  - stdin JSON（session_id/transcript_path/cwd/hook_event_name + 事件字段）；stdout JSON；stderr 日志；timeout（默认 60s，0=不限）；async 立即返回；
  - 输出解析：新版 `decision/reason/additionalContext/systemPrompt` 与旧版 `hookSpecificOutput.permissionDecision/permissionDecisionReason/additionalContext/updatedSystemPrompt` 双兼容；
  - 失败/超时/非法 JSON → warning + Continue（不阻塞）。
- 测试：mock 命令脚本（`sh -c 'echo "{\"decision\":\"block\",...}"'`）验证输入输出、展开、env、block/continue、超时、async。

### T15 Hook 枚举扩展 + 新注入点

- `crates/tact/src/hook/mod.rs`：
  - `Hook::UserPromptSubmit(Box<dyn UserPromptSubmitFn>)`（`Fn(&LoopState, &mut String) -> …HookControl`）；
  - `Hook::SubagentStart(Box<dyn SubagentStartFn>)`（`Fn(&LoopState, &mut SubagentStartCtx) -> …HookControl`）；
  - `HookTypes` 自动跟随；`invoke_hooks!` 复用。
- `agent_loop`（agent/mod.rs）：用户消息进入时跑 UserPromptSubmit hooks，`additionalContext` 追加到消息文本（text-only 修改，非文本 block 保留）。
- `spawn_subagent`（tool/subagent.rs）：`SubagentStartCtx { name, prompt, system_prompt }`，hooks 追加 context 到 system prompt。
- 测试：UserPromptSubmit 追加进消息；SubagentStart 追加进子代理 prompt；Block 短路。

### T16 插件 hook 注册适配器

- 新模块 `crates/tact/src/plugin/hook_register.rs`（或并入 hooks.rs）：
  - `register_plugin_hooks(agent_builder 相关, home: &PluginHome, work_dir)`：遍历已安装插件 → 解析 hooks → 按事件注册适配闭包（matcher 过滤在闭包内）；
  - SessionStart 输出 `systemPrompt`/`updatedSystemPrompt` → warn 不应用；
  - PreToolUse 输出 `additionalContext` → 追加进 tool_input；
  - PostToolUse 输出 `suppressOutput` → 清空 content（Block → 失败内容）。
- `interactive.rs` / `headless.rs`：Agent 构建后调用注册。
- 测试：注册后各事件触发；matcher 命中/不命中；事件输入字段正确。

### T17 状态消息透传（可选）

- `statusMessage` 通过 `progress_reporter` / 日志透传；TUI 状态栏显示（若成本低）。
- 测试：仅日志断言。

### T18 超时/失败语义测试

- 超时命令、退出码非 0、非法 JSON 三种失败路径 → Continue + warning，不阻塞循环。
- 测试覆盖。

### T19 与内置闭包 hook 并存

- 顺序：插件命令 hook 先、内置闭包后；任一 Block 短路。
- 测试：rtk_filter 与插件 hook 并存行为。

## Phase 6 — UI/CLI 展示

### T20 TUI 插件列表功能徽章 + i18n

- `crates/tui/src/widgets/state/app/extensions.rs`：`show_plugin_list` 显示功能摘要（skills/commands/agents/hooks/mcp）。
- `crates/agent_tui_kit/src/i18n.rs`：新增字符串（en/zh）。
- `crates/tui/src/render/popups/select.rs`（如涉及）。
- 测试：popup_scene_tests 或扩展列表测试。

## Phase 7 — 文档同步

### T21 文档同步（双语）

- `book/02_chapter_skill*.md`：commands 加载、插件功能面、frontmatter 新字段。
- `book/08_chapter_mcp*.md`：插件 MCP 扫描、`.mcp.json`、http 限制。
- `book/09_chapter_hook*.md`：命令型插件 hook、新事件点、Claude 协议、失败语义。
- `book/12_chapter_subagent*.md`：声明式 agents（.tact/agents + 插件）、`spawn_subagent.agent`。
- `book/21_chapter_config.md` / `book/23_chapter_tui*.md`：plugin list 功能摘要。
- `book/26_chapter_issue*.md`：追加 2026-08-29 条目（双语同结构）。

## Phase 8 — 验证

### T22 全量验证

- `env -u http_proxy -u https_proxy -u all_proxy cargo test`（工作区）。
- `cargo clippy` / `cargo fmt --check`。
- 手工冒烟：
  - 安装一个无 skills 的官方插件（如 `commit-commands`）成功，`/plugin:commit` 可用；
  - 安装 `ponytail`（hooks 插件）后会话启动/用户提交触发 hook（观察日志）；
  - 安装带 agents 的插件（如 `claude-security`）后 `spawn_subagent agent=…` 生效；
  - `.mcp.json` stdio 服务器出现在 `mcp__` 工具集。
- 提交 + push（代理 unset）。

## 里程碑

- M1（T1–T5）：官方 39 个插件全部可安装，commands 可用。
- M2（T6–T7）：插件 MCP 可用。
- M3（T8–T12）：声明式 agents 可用。
- M4（T13–T19）：命令型 hooks 可用。
- M5（T20–T22）：展示、文档、验证、发布。
