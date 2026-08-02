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

## 1. 2026-08-02 — Google Cloud API key 语音转文字 provider

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 相关 | 第 21、23 章 |
| 现象 / 动机 | 语音输入已支持 OpenAI 兼容转写和本地 `whisper.cpp`，但持有 Google Cloud Speech-to-Text API key 的用户无法直接选择 Google provider。 |
| 决策 | 新增 `VoiceProvider::Google`，使用同步 `POST {base_url}/speech:recognize?key=...`，发送 base64 编码的 LINEAR16、单声道、16 kHz WAV JSON。复用 `voice.api_key`、`voice.language`、`voice.model`；默认 `https://speech.googleapis.com/v1` 与 `latest_short`。Google 录音限制为 `1..=60` 秒。Service Account、OAuth、长任务识别、流式识别和自动分段仍不在范围内。 |
| 改后行为 | 配置 `provider = "google"` 后，短录音会发送到 Google Cloud，并将返回的 `results[].alternatives[0].transcript` 合并后沿用现有 TUI 输入流程。缺少 key、HTTP 失败、JSON 错误、空结果和取消都会报告，且不暴露凭证。 |
| 指针 | `crates/tact/src/config/{types.rs,resolve.rs}`；`crates/tact/src/voice/transcriber.rs`；`docs/superpowers/specs/2026-08-02-google-voice-transcription-design.md`；第 21、23 章 |

---

## 1. 2026-08-02 — 压缩交接摘要改为类型化消息 cell

| 字段 | 值 |
|------|------|
| 类型 | `optimization` |
| 相关 | 第 5 章 |
| 现象 / 动机 | Codex 风格重建把交接摘要当成普通 `Role::User` 文本消息追加，唯一的"特殊处理"是字符串前缀匹配（`is_summary_message`）。模型无法区分系统生成的 handoff 与真实用户输入；`[User: summary][User: prompt]` 连续 user 消息有被 provider 合并的风险；检测也很脆弱（仅前缀、非 Text cell 失效）。 |
| 决策 | 让 handoff 成为一等消息 cell：在 `tact_llm::Message` 上加 `MessageKind::Summary`（`#[serde(skip)]`，仅内存——Anthropic wire、OpenAI 转换、JSONL transcript 字节级不变），并在 cell 文本里加 `<context-handoff>` … `</context-handoff>` 包裹。检测优先按类型，SQLite store（只持久化 role + content）重载的会话回退到 `SUMMARY_PREFIX` / 标签字符串匹配。 |
| 改后行为 | `build_compacted_history` / `compacted_context` 产出带包裹、带类型标记的 cell：`<context-handoff>\nThis conversation was compacted…\n\n{summary}\n</context-handoff>`。`collect_user_messages` 按类型跳过它；重载会话按内容重新识别。普通消息的 wire 格式不变；Anthropic 永远看不到 `kind`。 |
| 指针 | `crates/tact_llm/src/content.rs`（`MessageKind`、`Message::with_kind/is_summary`）；`crates/tact/src/compact/mod.rs`（`summary_message`、`is_summary_message`、`build_compacted_history`、`compacted_context`）；`crates/tact/src/store/session_store/sqlite.rs`（`load_session`）；`book/05_chapter_compact_zh.md` |

## 1. 2026-08-02 — DeepSeek 现在可以使用 OpenAI Responses 协议

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 相关 | 第 21、5 章 |
| 现象 / 动机 | `protocol = "responses"` 对除 OpenAI 外的所有 provider 一律拒绝，DeepSeek 因此被钉死在 Chat Completions，尽管 Responses 适配器本身与端点无关，DeepSeek 端点可以服务 `/responses`。 |
| 决策 | 在 `resolve_llm` 中接受 DeepSeek 使用 `responses`，并让 `ProviderInfo::build_client()` 按 protocol 路由：DeepSeek + `chat_completions` 继续使用专用 `DeepSeekAdapter`；DeepSeek + `responses` 构建与 OpenAI 相同的通用 `OpenAiResponsesAdapter`，指向 DeepSeek `base_url`。自动 `context_management` 压缩、由 `thinking_budget` 派生的 `reasoning.effort`、Responses 会话状态续传原样生效。Kimi 与 Anthropic 仍拒绝 `responses`。 |
| 改后行为 | DeepSeek 条目可设置 `protocol = "responses"`；请求发往 `{base_url}/responses`，具备自动压缩与 reasoning 语义。显式 `POST /responses/compact` 在 DeepSeek 端点未实现（2026-08-02 实测），因此 DeepSeek + Responses 走本地摘要压缩并清掉失效基线；OpenAI Responses 仍保持严格"不回退"契约。默认仍为 `chat_completions`。 |
| 指针 | `crates/tact/src/config/resolve.rs`（`resolve_llm` 校验）；`crates/tact_llm/src/provider.rs`（`build_client`）；`docs/superpowers/specs/2026-08-02-deepseek-responses-design.md`；`docs/superpowers/plans/2026-08-02-deepseek-responses.md`；第 21 章（配置）、第 5 章（压缩） |

## 1. 2026-08-01 — Responses 压缩阈值现在会进入普通 `/responses` 请求（原生 `context_management`）

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 5、22、23 章 |
| 现象 / 动机 | `responses_compact_threshold`（以及推导值）虽然被解析并校验，却从未传给 Responses adapter：普通 `stream_message` / `create_message` 构建的 `/responses` body 中 `context_management` 被硬编码禁用（`None`）。因此生产环境下自动 provider 侧压缩被静默关闭，只有显式 `/responses/compact` 路径生效。 |
| 决策 | 把解析后的阈值贯穿整条 配置 → adapter 链路，并让它进入**每一个普通** `/responses` 请求：`LlmSettings.provider_info()` → `ProviderInfo.responses_compact_threshold` → `OpenAiResponsesAdapter` → `create_response`（`context_management: [{ "type": "compaction", "compact_threshold": N }]`）。原生状态会被持久化并回放：不透明基线（`input_items`、`compaction_id`、`logical_context_hash`）与消息在同一事务中提交，后续请求原样回放。缺少原生 Responses 压缩的端点不受支持——**绝不**回落本地摘要。 |
| 改后行为 | 配置或推导出阈值后，每个普通 `/responses` 请求（流式与非流式）都会携带 `context_management`。端点可在对话中途自动压缩基线；返回的 `compaction` item 以不透明状态往返，绝不渲染。显式压缩（`/compact`、自动触发、恢复）发送 `POST /responses/compact` 并原子替换基线；诊断只显示 item 数与 compaction id，绝不显示 `encrypted_content`。回归测试断言：配置阈值时 wire body 包含 `context_management`，未配置时省略。 |
| 指针 | `crates/tact_llm/src/openai/responses/convert.rs`（`create_response` → `context_management`）；`crates/tact_llm/src/openai/responses/mod.rs`（`OpenAiResponsesAdapter::build_wire_request`、wiremock 回归测试）；`crates/tact_llm/src/provider.rs`（`ProviderInfo.responses_compact_threshold`）；`crates/tact/src/config/types.rs`（`LlmSettings::provider_info`）；`crates/tact/src/config/resolve.rs`（阈值推导）；`crates/tact/src/agent/mod.rs`（`compact_responses_native`、原子 `replace_persisted_context_and_state`）；`docs/token_usage_schema.md`（自动 vs 显式压缩记账）；第 5、22、23 章 |

---

## 1. 2026-08-01 — Markdown 列表后的空 fenced block 不再把尾行劫持进代码卡片

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23、24 章 |
| 现象 / 动机 | 在 TUI 日志流式渲染中，如果一个**空语言** fenced block（普通 ```）紧跟在进行中的 markdown 列表/段落之后，系统会过早把它提升成独立 code card。这样围栏后的尾行会被渲染进代码卡片，而不是继续留在普通 markdown 流程里，看起来像尾行被“吞掉”或错位。这是 Tact 自身的渲染 bug，不是 Responses 协议问题。 |
| 决策 | 保留真实流式代码块（例如 ```rust）的 code-card 路径，但当 **空语言** fence 直接出现在进行中的 markdown 段落/列表后时，不再将其提升成 code card，而是继续保留在 markdown paragraph buffer 中，交给普通 markdown renderer 处理。补充一条高层日志回归测试，覆盖 list → empty fence → tail line 场景；并补一条低层 markdown 测试，证明解析层本身并未丢失尾行。 |
| 改后行为 | markdown 列表后出现的空 fence 片段，不会再把后续尾行渲染成 `Click for full code` 卡片内容。真正带语言标签的流式代码块仍保持 code card 渲染。 |
| 指针 | `crates/tui/src/widgets/state/app/agent.rs`（stream fence promotion guard）；`crates/tui/src/render/render_gap_tests.rs`（`log_markdown_list_then_empty_fence_stays_in_markdown_flow`）；`crates/tui/src/render/render_md.rs`（`render_markdown_list_then_fenced_code_then_list_tail`）；第 23、24 章 |

## 1. 2026-07-28 — 主题检测回退使用了错误主题（Ink 而非 Retro）

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23 章 |
| 现象 / 动机 | `detect_terminal_theme()` 文档注释写「Fallback: Retro」，但代码返回的是 `ThemeName::Ink`。对应的单测接受 `Dark`、`Light`、`Retro` 三者之一，`Ink` 不在其中，导致任何未设置 `COLORFGBG` / `COLORTERM` 环境变量且无 macOS 深色模式覆盖的 CI runner 都会稳定失败。 |
| 决策 | 将回退值从 `ThemeName::Ink` 改为 `ThemeName::Retro`，与文档注释及测试预期保持一致。 |
| 改后行为 | 当无终端主题环境变量设置时，`detect_terminal_theme()` 返回 `Retro`（中性暗色），而非 `Ink`。 |
| 指针 | `crates/tui/src/theme_detection.rs` |

---

## 1. 2026-07-28 — Log 左边框滚动条残影

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23 章 |
| 现象 / 动机 | Ink 等主题下，Thinking 卡片标题含宽字符（如 🧠）时，部分终端光标会短暂错位；右侧 accent 色滚动条滑块（原 `█`）的残影会留在 Log 左边框上，看起来像间断的浅蓝「阴影」。因为左边框单元格在后续帧未变，`Buffer::diff` 不再重发，残影会一直挂着。 |
| 决策 | 每帧在内容与滚动条绘制之后强制重印左边框竖线，并标记 `CellDiffOption::AlwaysUpdate`。滑块改为半块 `▐`，降低瞬时错位时的视觉冲击。 |
| 改后行为 | 左边框每帧都会以主题 `border` 色重绘到终端；宽字符标题导致的左侧 accent 残影无法持久残留。 |
| 指针 | `crates/tui/src/render/log.rs`（`restamp_log_left_border`）；`crates/tui/src/render/log_render_tests.rs` |

---

## 1. 2026-07-28 — CRUD 类工具族卡片标签按动作区分

| 字段 | 值 |
|------|------|
| 类型 | `optimization` |
| 相关 | 第 7、13–16、23 章 |
| 现象 / 动机 | Cron / worktree / team 等同族工具共用一个 display 标签（例如所有 cron 操作都显示 `⏰ Cron`）。标题几乎一样，只能靠解析 `arg_summary` JSON 区分。`visual_kind = Generic` 时还会忽略 metadata 的 `display_name`，一律走 TUI fallback 表。 |
| 决策 | 同族标签补上动词（`⏰ Cron Create` / `Delete` / `List`，Worktree / Team / Shutdown 同理）。同步 `tool_display_name` fallback。当 presentation `display_name` 非空且不等于原始 tool id 时优先使用它，让 Generic 工具以 metadata 为准。Task 不改——已有 `format_task_tool_title` 的 `# Task…` 人类标题。 |
| 改后行为 | 工具卡片标题一眼可区分动作。`background_run` / `check_background` 的 fallback 与 metadata 对齐（`$ Bg` / `🔍 Check`）。 |
| 指针 | `crates/tact/src/tool/{cron,worktree,team}.rs`；`crates/tui/src/widgets/tool_widget.rs`（`display_name_from_presentation`、`tool_display_name`） |

---

## 1. 2026-07-28 — Bash 工具卡片标签恢复为 `$ Bash`

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 7、23 章 |
| 现象 / 动机 | 内置工具 `ToolPresentation` 绑到 handler 旁路后，`bash` 的 `display_name` 写成了 `"$ Shell"`。TUI 卡片显示 **Shell**，尽管工具 id 与旧回退仍是 `bash` / `$ Bash`。 |
| 决策 | 将 `BASH_METADATA.presentation.display_name` 改回 `"$ Bash"`。运行时仍用 `sh -c` 启动（不变）。 |
| 改后行为 | `bash` 工具的卡片与标题再次显示 `$ Bash`。 |
| 指针 | `crates/tact/src/tool/bash.rs`；`crates/tui/src/widgets/tool_widget.rs` 回退仍为 `$ Bash` |

---

## 1. 2026-07-28 — 语音快捷键吞掉全部键盘输入

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 21、23 章 |
| 现象 / 动机 | 配置了 `voice.voice_keybind` 后，TUI 用 `if let Some(keybind) = … else if …`：只要 option 存在就把整条分发链占住；未命中快捷键的按键到不了 `handle_insert_mode` / Normal，输入框表现为无法打字。 |
| 决策 | 先精确匹配快捷键；仅命中时跳过常规分发。未命中则照常走 slash / overlay / 模式处理。 |
| 改后行为 | `voice_keybind = "ctrl+g"` 仅对该组合键切换录制。其它键输入与导航与从前一致。未配置快捷键时仍为仅鼠标。 |
| 指针 | `crates/tui/src/lib.rs`（按键分发）；`crates/tui/src/widgets/state/app/voice.rs`（`toggle_voice_recording`）；第 21 章 `[voice]`、第 23 章 §6.6 |

---

## 1. 2026-07-28 — 输入框顶边恢复；语音按钮居中

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23 章 |
| 现象 / 动机 | 用空格填充把语音标签“居中”，并带背景色时，会盖住 Input 标题与 `🎙 Voice` 之间的 Block 顶边单元格，横线看起来被“吃掉”。 |
| 决策 | 左侧 Input 标题与语音标签拆成两个 `Block` title（左对齐 + `Alignment::Center`），不再用带背景的填充空格。点击热区使用同一居中几何。 |
| 改后行为 | 启用语音时，足够宽的终端上 Input 标签与居中语音控件之间顶边可见。过窄时仍可能与左侧标题碰撞（ratatui 左侧 title 后画）。 |
| 指针 | `crates/tui/src/render/input.rs`（`voice_title`、`update_voice_button_area`）；第 23 章 §6.6 |

---

## 1. 2026-07-28 — 可配置语音录制快捷键

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 现象 / 动机 | 语音录制只能通过鼠标点击标题栏按钮触发，键盘用户无法在不使用鼠标的情况下启动。 |
| 决策 | 新增 `voice.voice_keybind` 配置项，支持 `ctrl+<char>` 格式（如 `"ctrl+g"`、`"ctrl+r"`）。配置后，在任意输入模式下按下该快捷键即可切换语音录制（空闲→录制，录制中→停止）。未配置时（默认），语音仍仅支持鼠标操作。在帮助面板（`Ctrl+?`）全局快捷键区动态显示当前配置。仅精确匹配时消费按键事件。 |
| 改后行为 | `[voice] voice_keybind = "ctrl+g"` 启用键盘触发语音。快捷键全局生效（任意输入模式）。未命中的按键仍进入 Insert/Normal。帮助面板动态显示。空字符串、多字符键、非 ctrl 修饰符会在配置解析阶段被拒绝。 |
| 指针 | 配置：`crates/tact/src/config/types.rs`、`config/resolve.rs`、`config.example.toml`；TUI 分发：`crates/tui/src/lib.rs`（全局快捷键部分）、`crates/tui/src/widgets/state/app/voice.rs`；帮助：`crates/tui/src/widgets/help_widget.rs`、`render/popups/help.rs`；国际化：`crates/tui/src/i18n.rs`（`help_voice_record_tmpl`）；第 21、23 章 |

---

## 1. 2026-07-28 — 权限：shell 标为 Write、High 尊重 settings allow、headless ask 默认

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 10 章 |

**症状 / 动机：** 三处逻辑错误：(1) `PermissionPolicy::ShellCommand` 把非提权命令标为 Read，导致 `bash` / `background_run` / `worktree_run` 绕过 Default 提示；(2) headless `ask_user` 一律 deny，无 TUI 时 Default 几乎不可用；(3) High 风险工具忽略 settings **allow**，始终询问。

**决策：** 非提权 shell → Write；`sudo`/`su` → High。非交互 `ask_user(tool, risk)` 对 Write/Read 允许一次、对 High 拒绝。Settings 的 Deny/Allow 适用于所有风险；无 Deny/Allow 的 High 仍 ask，且跳过会话内裸名 allowlist。

**改后行为：** 普通 shell 与其它 write 一样会提示（或 headless 放行）。项目 allow 规则可按输入模式批准 High。无人值守的 High 仍需 Auto 模式或显式 allow 规则。

**指针：** `crates/tact/src/permission/mod.rs`、`crates/tact/src/tool/metadata.rs`、`crates/tact/src/agent/tool_dispatch.rs`；第 10 章。

---

## 1. 2026-07-28 — `/model` 思考预算未同步到状态栏

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 21、23 章 |

**症状 / 动机：** `/model` 已保存新的思考预算（如 32K），底栏仍可能显示旧值（如 `think high(64K)`）。落盘成功，但运行中的 agent 与状态栏未跟上。

**决策：** `UserCommand::SetThinkingBudget` 要等当前任务结束后才处理；进行中任务的旧 `ModelInfo` 会覆盖 TUI 的乐观更新，而 `set_thinking_budget` 此前不会再发 `ModelInfo`。改为在 `set_thinking_budget` 中发出 `ModelInfo`，并在 TUI 应用路径同步/扩展会话 `max_tokens`，使 `out` / `think` 一致。

**改后行为：** 确认预算后状态栏立即更新；排队的 agent 命令执行时再发一次 `ModelInfo`，重新同步 `thinking_budget` 与可能自动扩展的 `max_tokens`。

**指针：** `crates/tact/src/agent/mod.rs`（`set_thinking_budget` / `emit_model_status`）、`crates/tact/src/config/mod.rs`（`update_llm_model_and_thinking_budget`）、`crates/tui/src/handlers/select.rs`（`apply_model_and_budget_pick`）、`crates/tact-ui/src/driver.rs`。

---

## 1. 2026-07-28 — 可点击的语音转文字输入（标题栏）

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 21、23 章；`docs/superpowers/specs/2026-07-28-voice-to-text-design.md`；`docs/superpowers/plans/2026-07-28-voice-to-text-input.md` |

**症状 / 动机：** macOS 上纯键盘输入长提示不便；需要免提录音并在提交前审阅。

**决策：** 增加 `[voice]` 配置（独立 API 密钥）、`tact::voice` 工作线程（cpal 采集 → WAV → OpenAI 兼容转写），以及 TUI 标题栏右侧按钮。成功转写按 UTF-8 光标插入；转写中的 `/help` 在按 Enter 前仅为普通文本。录音/转写在事件循环外执行；`Esc` 或停止可取消。

**改后行为：** 默认 `enabled = false` 隐藏控件。`enabled = true` 显示按钮；缺少 `[voice].api_key` 时点击会提示配置。本版无实时转写、自动提交或本地 Whisper。

**指针：** `crates/tact/src/voice/`、`crates/tui/src/widgets/state/voice.rs`、`crates/tui/src/render/input.rs`、`crates/tui/src/handlers/mouse.rs`、`crates/tui/src/handlers/insert.rs`、`crates/tui/src/lib.rs`、`crates/tact-ui/src/interactive.rs`。

---

## 1. 2026-07-28 — 子 agent 元数据显示在工具卡片头部

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 12、23 章；`docs/token_usage_schema.md` |

**现象 / 动机：** 子 agent 的 `TokenUsage` 和 `ModelInfo` 作为 `ToolProgress` 内联块转发到共享父 UI 通道，在输出流中产生重复的 `⚡ N tokens` 和 `🤖 Model: …` 行。同时 TokenUsage 还会覆写主 agent 的底栏数据。

**决策：** 引入 `AgentUpdate::ToolMeta` — 专用更新路径，将模型名和 token 数量直接写入父级工具卡片的头部行，与现有的阶段/耗时信息并列显示。转发器不再为这些事件生成 `ToolProgress` 块，也不再转发到共享通道。子 agent 调用的工具卡片元数据行现在显示 `🤖 {model} · ⚡ {total}`。

**改后行为：** 底栏始终显示主 agent 的 token 统计。子 agent 的模型和 token 总数出现在工具卡片的元数据行中（如 `⠋ 运行中 · 🤖 deepseek-v3 · ⚡ 4.2K · 3.2s`），通过 `ToolMeta` 实时更新并在完成后保留。输出流中不再出现内联的元数据行。

**指针：** `crates/tact/src/tool/subagent_ui.rs`、`crates/tui/src/widgets/tool_widget.rs`、`crates/tui/src/render/cells/tool.rs`、`crates/tui/src/widgets/state/app/agent.rs`、`crates/protocol/src/agent.rs`；`docs/token_usage_schema.md`；第 12、23 章。

---

## 1. 2026-07-27 — 权限设置持久化（基于 JSON 的动态规则）

| 字段 | 值 |
|------|-----|
| **类型** | docs |
| **相关** | 第 7、21 章；`docs/superpowers/specs/2026-07-27-permission-settings-design.md`；`docs/superpowers/plans/2026-07-27-permission-settings.md` |

**现象 / 动机：** 权限决策仅存储在会话级内存（`always_allowed_tools`）中。「总是允许此工具」每次授予的是裸工具名、无参数感知的全局放行，会话之间不持久化，且无法在不修改 `config.toml` 的前提下预配置 deny 或 ask 规则（TOML 文件不适用于动态规则写入）。

**决策：** 引入基于 JSON 的权限设置，分为全局范围（`$HOME/.tact/settings.json`）和项目范围（`<workdir>/.tact/settings.json`）两层。规则采用 Claude 风格的工具+参数语法（`tool(field:pattern)`）并支持 glob 匹配。优先级为 `deny > ask > allow`，与数组顺序无关。项目写入采用原子操作（临时文件 + rename），保留未知 JSON 字段，去重。格式错误的文件或非法规则视为软失败（告警 + 跳过）。高风险确认始终强制，不受 allow 规则影响。

**改后行为：** 动态 allow/ask/deny 规则存储在 JSON 设置文件中，而非 `config.toml`。「总是允许此工具」会写入一条参数感知的规则（例如 `bash(command:cargo test *)`）到项目文件。缺少文件等同于空策略。TOML `[permission].mode` 继续仅控制模式（`default` | `plan` | `auto`）。Plan 和 Auto 模式的语义保持不变。

**指针：** `crates/tact/src/permission/settings.rs`、`crates/tact/src/permission/mod.rs`、`crates/tact/src/consts.rs`、`crates/tact/src/agent/tool_dispatch.rs`、`crates/tact/src/tool/subagent.rs`、`crates/tact-ui/src/interactive.rs`、`crates/tact-ui/src/headless.rs`；`docs/superpowers/specs/2026-07-27-permission-settings-design.md`；`docs/superpowers/plans/2026-07-27-permission-settings.md`；`docs/state_machines.md §5`；`config.example.toml`；第 7、21 章。

## 1. 2026-07-27 — 日志滚动恢复主题背景

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章；`docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`；`docs/superpowers/plans/2026-07-27-log-scroll-artifact-fix.md` |

**现象 / 动机：** 从 code-card 或其他带样式的 Log 内容滚动离开后，普通文本行可能保留前一帧的背景样式。深色 Ink 主题下该问题尤其明显，文字后方会出现阴影。

**决策：** 保留 Log viewport 的重置，并让 `TextCell` 写入每个普通字形时显式应用当前 `theme.bg`。该规则与主题无关；卡片与 overlay 层保留既有背景和绘制顺序。

**改后行为：** 滚动新露出的任意普通 Log 行都使用当前主题背景，同时保留前景样式和选区反色 modifier。不使用 Ink 专用分支或全局终端清屏策略。

**指针：** `crates/tui/src/render/log.rs`；`crates/tui/src/render/cells/text.rs`；`crates/tui/src/render/log_render_tests.rs`；`docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`；第 23 章。

---

## 1. 2026-07-27 — 子 agent 弹窗显示所用模型

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 12、23 章；`docs/token_usage_schema.md` |

**现象 / 动机：** 实时/完成后的 `spawn_subagent` 弹窗会显示子调用的 token 总数、缓存命中率和 prompt 上下文，却不显示生成这些数据的模型。agent 会发出 `ModelInfo`，但子 agent UI 转发器此前直接丢弃了该事件。

**决策：** 将子级 `ModelInfo` 格式化为弹窗转录中的结构化行：`🤖 Model: {model}`。它只走 `ToolProgress` 路径，不转发到共享的父级 UI 通道。

**改后行为：** 每次子级模型调用都会在该子 agent 弹窗中、既有 token 行旁显示模型名。父级底栏继续保留父 agent 的模型名（配套的 TokenUsage 修复见 2026-07-28）。

**指针：** `crates/tact/src/tool/subagent_ui.rs`；`docs/token_usage_schema.md`；第 12、23 章。

---

## 1. 2026-07-27 — Ink 主题 + 统一弹窗外框
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
