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

## 1. 2026-08-16 — Codex 风格排队消息：agent 忙时提交"当前任务结束后自动提交"

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | Ch 23; `crates/tui/src/handlers/insert.rs`、`crates/tui/src/handlers/skills.rs`、`crates/tui/src/widgets/state/app/pending.rs`、`crates/tui/src/render/input.rs`、`crates/tui/src/lib.rs` |

**Symptom / motivation:** agent 处于 `Planning`/`Executing` 时按 Enter 只会闪现"⏳ 上一个任务还在处理中"并丢弃输入——工具运行期间打的消息全丢了。Codex CLI 的做法是排队："Messages to be submitted after next tool call (press esc to interrupt and send immediately)"。

**Decision:** 用 Codex 风格队列取代忙时拒绝。`App.pending_messages` 存放 `PendingMessage { display, agent_task }`；`Planning`/`Executing` 时按 Enter 清空输入并入队（字符长度校验在入队时执行）。主循环在排空 `agent_rx` 后调用 `handlers::skills::flush_pending_when_idle`：状态进入 `Idle`/`Done` 后，按序把每条排队消息各自派发为一个 `SubmitTask`——tact-ui 命令驱动本就会串行处理在途 `SubmitTask`，因此每条都成为下一个用户回合。提交**纯自动**——不存在"立即发送"路径（`[Send now]` 按钮与 Normal 模式 `s` 键按用户要求移除："send now 去掉吧，自动处理即可"）。丢弃排队消息的**唯一**途径是 pending 块的 `[Cancel]` 按钮（`pending_cancel_btn_area` 命中测试在 mouse handler）——只清空队列、不影响运行中的任务。`/cancel` 与 Normal 模式 `c` 与队列**无关**：只取消在途任务，与功能引入前一致（用户决定："/cancel 也不用处理 prompt 队列"）。Esc 保持原语义（始终退出插入模式——误触不会中断任务）。提示行与 `↳ 消息` 行渲染在输入框上方（`render/input.rs` 的 `render_pending_block`，窄终端隐藏按钮）；布局把 `pending_display_lines()`（提示 + 每条一行，上限 4 行）计入输入高度。`submit_user_task` 拆成 `task_within_limits` / `dispatch_user_task`，使排队与自动提交共用派发。`/compact` 仍保留旧的忙时闪现（`input_busy_msg`）。

**Behavior after:** 忙时输入的消息被排队，显示在输入框上方并带 Codex 风格提示，当前任务结束后自动提交（包括被 `/cancel` 结束的任务）；`[Cancel]` 按钮只丢弃队列。多条排队消息按顺序各自成为独立回合。超长消息在入队时即被拒绝。忙时提交不再丢失。

**Pointers:** `handlers/insert.rs`（`handle_enter_submit`、Esc 分支）、`handlers/skills.rs`（`submit_user_task`、`flush_pending_when_idle`、`interrupt_and_submit_pending`）、`widgets/state/app/pending.rs`、`render/input.rs`（`render_pending_block`、`truncate_to_width`）；测试 `submit_queued_while_agent_busy`、`esc_with_pending_interrupts_and_submits_immediately`、`flush_pending_when_idle_submits_all_queued_in_order`、`input_box_renders_pending_block_above_input`；Ch 23 §6.6。

---

## 1. 2026-08-16 — 行内代码改用强调色文字，不再绘制背景补丁

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/render/pulldown.rs`、`crates/tui/src/render/log_style.rs` |

**现象 / 动机：** 上一个修复已经阻止含行内代码的正文行被重绘成整行代码块，但行内代码 span 自身仍携带窄的 `code_block_bg` 背景补丁。这个矩形背景在普通正文和列表中仍然显得过重，换行后尤其明显。

**决策：** 行内代码使用 `theme.accent` 作为前景色，不再绘制背景。日志重绘阶段对旧的行内代码背景 span 应用相同规则；真正的围栏代码行继续使用 `code_block_bg` 与 `code_block_fg`。

**变更后行为：** 行内代码通过强调色文字区分，不再出现矩形背景补丁。围栏代码块仍保留主题背景和前景色，因此代码块边界仍然清晰。

**指针：** `crates/tui/src/render/pulldown.rs`（`push_inline_code`）、`crates/tui/src/render/log_style.rs`（`restyle_log_line_with_skills`）；两个模块中的 `inline_code_uses_accent_without_background` 测试；Ch 23。

---

## 1. 2026-08-16 — 含行内代码的正文/列表行不再整行绘制代码背景块

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/render/log_style.rs` |

**现象 / 动机：** `restyle_log_line_with_skills` 只要某行的 span 里出现 `theme.code_block_bg()` 就把整行当作围栏代码行，重绘成整行代码背景。像 `- run `cargo build`` 这种常见列表项因此渲染成一整块高亮背景；当条目换行时，`wrap_line` 会把背景重新切片到每一续行，帧间残留成阴影状色带（反复出现的 "shadow" 类问题）。

**决策：** 只有**每个** span 都带代码背景的行（即 `flush_code_block` 产生的真正围栏代码行）才按代码块重绘。混合正文/列表行保留原样式；行内代码 span 保留其窄背景补丁（文字/前景特殊渲染仍在）。

**变更后行为：** 含行内代码的列表项与段落不再渲染成整行代码块，换行续行不再出现阴影色带。围栏代码块仍带主题代码背景（浅色主题下仍通过 `restyle_code_line` 修正前景色）。

**指针：** `crates/tui/src/render/log_style.rs`（`restyle_log_line_with_skills`、`restyle_code_line`）；测试 `inline_code_line_keeps_narrow_patch_not_full_block_bg`；Ch 23 渲染管线。

---

## 1. 2026-08-16 — 任务统计行支持多语言，并去掉过宽的 📊 图标

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/i18n.rs`、`crates/tui/src/widgets/state/app/messages.rs`、`crates/tui/src/handlers/mouse.rs` |

**现象 / 动机：** 每轮结束的统计行被硬编码为 `📊 任务统计：…`（`messages.rs` 中的 `TASK_STATS_PREFIX`），即使 UI 切到英文模式仍是中文，且过宽的 `📊` emoji 在日志里显得太大。`[copy]` 按钮同样是一段硬编码英文。

**决策：** 前缀与复制按钮移入 i18n `Messages` 表：EN `Task stats:` / `[copy]`，ZH `任务统计：` / `[复制]`，不再带 emoji 图标——过宽的 `📊` 前缀与 model 前的 `🧠` 都被移除。复制按钮渲染在**统计正文之前**，只有点击按钮字形本身才触发复制（其余位置正常做文本选择）。`add_task_stats_block` 通过 `self.msgs()` 读取；`is_task_stats_line` 现在识别所有支持语言的前缀（可带前置按钮），**并兼容旧的 `📊 任务统计：` 行**（保证旧会话里 `[copy]` 仍然可用）；鼠标处理器通过 `find_task_stats_copy_button` 定位本地化按钮的字节区间。

**变更后行为：** 统计行在英文下渲染为 `[copy]  Task stats:⏱ mm:ss · model · N tokens …`，中文下为 `[复制]  任务统计：⏱ mm:ss · model · N tokens …`，前缀与 model 前都不再有宽 emoji。旧会话的统计行仍可复制。

**指针：** `crates/tui/src/i18n.rs`（`task_stats_prefix`、`task_stats_copy_btn`）、`crates/tui/src/widgets/state/app/messages.rs`（`is_task_stats_line`、`find_task_stats_copy_button`、`add_task_stats_block`）、`crates/tui/src/handlers/mouse.rs`；测试 `task_stats_block_localizes_prefix_and_copy_button`、`task_stats_line_detection_covers_all_languages_and_legacy_rows`。

---

## 1. 2026-08-16 — `install.sh` 不再报 `tmp: unbound variable`，也不再泄漏克隆目录

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `scripts/install.sh` |

**现象 / 动机：** 在 `set -u` 下，成功安装 release 后安装器打印 `bash: line 351: tmp: unbound variable`。`try_install_release` 对 `local tmp` 设置了 `trap 'rm -rf "$tmp"' RETURN`；RETURN trap 会被继承，并在调用者 `main` 返回时再次触发，此时 `tmp` 已越界，未加保护的 `$tmp` 展开因 `nounset` 中断。另外，`main` 把 `work` 声明为 `local` 并用 `EXIT` trap 清理；该 trap 在脚本退出时触发，此时 `main` 的局部变量已销毁，`${work:-}` 恒为空，导致 `git clone` 目录在源码构建 / 非仓库目录路径下泄漏。

**决策：** (1) release 临时目录 trap 改为 `trap '[[ -n "${tmp:-}" ]] && rm -rf "$tmp"' RETURN` —— 加保护后，继承到调用者上下文的那次触发成为 no-op，同时在 `try_install_release` 自身返回时仍能正确清理。(2) `work` 不再是 `local`，改为初始化为 `""` 的全局变量，使已有的 `EXIT` trap 能在脚本退出时真正删除克隆目录（包括 `die` / `exit 1` 路径）。

**变更后行为：** `curl … | bash`（以及 `./scripts/install.sh`）以 `Done. Run: tact-ui --help`、退出码 0 收尾，无 `unbound variable` 报错，release 临时目录与克隆仓库目录均被删除。

**指针：** `scripts/install.sh`（`try_install_release`、`main`）。

---

## 1. 2026-08-16 — `plugin install` 不再 panic，并支持官方 `url` 类型的插件源

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 02；`crates/tact-ui/src/main.rs`、`crates/tact/src/plugin/marketplace.rs` |

**现象 / 动机：** `tact-ui plugin install <plugin>@claude-plugins-official` 以两种方式失败。其一，主入口的自更新提前返回用了 `args.command.take()`，它会把*任何*非 `Upgrade` 的命令消费掉并置 `args.command = None`；于是 `Plugin`（以及 `Headless`）落到 `run_interactive`，而 `plugin` 命令解析配置时不会初始化 LLM provider（`install_without_llm`），交互路径因此在 `get_provider()` panic：`LLM provider not initialized`。其二，官方 `anthropics/claude-plugins-official` 目录使用了 `"source": "url"` 对象形式（286 个插件中的 150 个：`url` + `sha`，部分带 `path`），而 `PluginSource::from_catalog_value` 无法识别，导致整个目录解析失败，报 `invalid marketplace source: url`。

**决策：** (1) 自更新提前返回改为先 `matches!(args.command, Some(CliCommand::Upgrade { .. }))` 判断，仅在命中分支内 `take()`，非 upgrade 命令得以进入后续分发。(2) `from_catalog_value` 将 `git-subdir` 与 `url` 统一视为 Git 仓库源（克隆 `url`、可选 `path`、锁定修订），优先使用 `sha` 锁定，回退到 `ref`。

**变更后行为：** `plugin install`（以及 `headless`）正确分发；`plugin install frontend-design@claude-plugins-official` 会克隆官方目录、解析全部 286 条并安装 `frontend-design`（1 个 skill），锁定到其固定修订。插件命令从不依赖 LLM provider。

**指针：** `crates/tact-ui/src/main.rs`（自更新提前返回）、`crates/tact/src/plugin/marketplace.rs`（`RawPluginSource::Object.sha`、`PluginSource::from_catalog_value`）；测试 `parses_url_plugin_source`、`parses_url_plugin_source_with_subdirectory`、`git_source_falls_back_to_named_ref_without_a_sha`；spec `docs/superpowers/specs/2026-07-20-plugin-install-design.md`。

---

## 1. 2026-08-16 — 覆盖式列表弹窗限定在主区域内

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/lib.rs`、`crates/tui/src/render/test_harness.rs` |

**现象 / 动机：** 主帧循环里渲染的四个覆盖式弹窗（`command_palette`、`select`、`file_picker`、`slash_command`）以**整屏**为基准居中，高度上限是 `frame.height - 4`。终端较矮、命令/文件/选项较多时，弹窗高过日志面板，盖住命令行输入框和底栏；弹窗列表行与输入框边框字形（弹窗 `Clear` 矩形之外的部分）交错，看起来像一团阴影/错乱，用户也看不到正在输入的过滤词。

**决策：** 弹窗调用点传入**主区域**（`chunks[1]`：下方是状态栏、上方是输入框）而不是整帧。弹窗只在主区域内居中并限制高度。

**变更后行为：** palette / select / 文件选择 / 斜杠命令弹窗无论列表多长、终端多矮，都保持在输入框和底栏之上；过滤输入时输入框始终完整可见。

**指针：** `crates/tui/src/lib.rs`（帧循环弹窗调用）、`crates/tui/src/render/test_harness.rs`（`draw_full_ui`，同步保持）、`crates/tui/src/render/popup_scene_tests.rs`（`full_frame_palette_popup_stays_inside_main_area`）；Ch 23。

---

## 1. 2026-08-16 — 主区域标题不再绘制高亮色块

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/render/log_style.rs` |

**现象 / 动机：** restyle 通道（pulldown-cmark 迁移 #69 时加入）会给 H1 标题涂上 `theme.highlight` 背景——这是 tui-markdown 直接给 H1 上背景的遗留行为。日志面板里色块只覆盖标题的字形列；标题换行时每一行都会带色块，于是「长标题 + 列表」在文字后面形成一大片类似阴影的色块。整段 Markdown 路径（`MarkdownCell`，如 `/skills` 分页）从不绘制该色块，两条主区域路径表现不一致。

**决策：** restyle 通道不再给标题 span 赋任何背景（pulldown 渲染器本身就不带背景）。同时删除已失效的 `Color::Rgb(70, 90, 140)` → `theme.highlight` 映射（fork 移除后无来源）。

**变更后行为：** H1 标题在两条主区域路径中都渲染为无背景的加粗+下划线标题色文本；（换行的）列表标题后面不再出现高亮阴影块。

**指针：** `crates/tui/src/render/log_style.rs`（`restyle_log_line_with_skills`、`heading_keeps_no_background`）、`crates/tui/src/render/log_render_tests.rs`（`heading_rows_carry_no_highlight_band`）；Ch 23 渲染管线。

---

## 1. 2026-08-15 — `/stats` 弹窗直接用 ratatui-markdown 渲染

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | system-prompt 弹窗（`/stats` 会话统计与组装后的 system prompt 视图共用）此前走 pulldown-cmark 管线 + Tact 自研 width-aware pipe 表格，并按弹窗内容宽度布局。对一个快速统计弹窗而言，这套额外布局机制不值得。 |
| 决策 | `render_system_prompt_popup` 改为通过 `render_markdown_ratatui`（`crates/tui/src/render/render_md.rs`）渲染：按弹窗内容宽度使用普通 `ratatui_markdown::markdown::MarkdownRenderer`，复用与 Mermaid 渲染器相同的 `TuiRichTextTheme`。width-aware 表格与 Mermaid 路由保留给主区域 Markdown cell。 |
| 改后行为 | `/stats` 与 system-prompt 弹窗由 ratatui-markdown 默认渲染器布局（含表格）；弹窗测试（`session_stats_popup_renders_gfm_table`）原样通过。 |
| 指针 | `crates/tui/src/render/popups/system_prompt_popup.rs`、`crates/tui/src/render/render_md.rs`（`render_markdown_ratatui`）；`docs/token_usage_schema.md` Session Stats Display。 |

---

## 1. 2026-08-15 — 自动压缩的摘要调用不再开启 thinking

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | 本地压缩摘要调用此前会转发 agent 的 Claude 式 `thinking_budget`（`with_thinking`，并限制在线上 `max_tokens` 之下）与显式 `reasoning_effort`，并在文本预算之上按 effort 分档预留 reasoning 份额。对手交摘要而言思考价值不大，且会从同一个 `max_tokens` 信封（effort 模型）中占用输出 token——用户要求自动压缩时不再开启 think。 |
| 决策 | 摘要请求不再携带任何 thinking：不转发 `thinking` 块、不转发 `reasoning_effort`（主循环的 thinking 配置不受影响），输入预留也不再扣除 thinking budget。**服务端默认** reasoning 预留仅保留给 DeepSeek / Kimi K3（固定为文本预算的 75% 追加在文本之上）：即使请求省略 effort，它们服务端默认 thinking 开启 + effort high，没有该预留其强制 reasoning 会挤占摘要文本、触发截断续写。原生 `/responses/compact` 请求本就只带 `{model, input}`，其无效的 `.with_reasoning_effort` 一并移除。 |
| 改后行为 | 摘要调用为普通非流式 `create_message`，`max_tokens` = 经典文本预算（OpenAI / Anthropic）或文本 + 75% 预留（DeepSeek / Kimi K3）；压缩期间发出的 `AgentUpdate::ModelInfo` 不报告 thinking/effort。 |
| 指针 | `crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode`、`compact_summary_reasoning_reserve_percent`、`compact_responses_native`）、book [Ch 5](./05_chapter_compact_zh.md) §摘要调用。 |

---

## 1. 2026-08-15 — 底栏 `out` 更名为 `max_out_token`，显示真实输出额度

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | 底栏输出段此前标记为 `out`/`输出`，直接显示原始 `max_tokens` 信封。对 effort 语义模型（openai / deepseek / kimi k3），reasoning 与输出文本算在同一个信封内，`think high` 旁边的 `out 128K` 高估了真正留给文本的 token——用户要求该段显示 **max output token** 值，并扣除 reasoning 份额。 |
| 决策 | 标签改为 `max_out_token`（两种语言统一用该标识符），数值改为真正的文本输出额度：effort 语义模型按压缩预留的同一分档约定扣除 reasoning 份额（预留为文本预算的百分比并追加在文本之上 → 文本 = 信封 × `100/(100+pct)`；128K 信封 + `high` → 73K）。budget 语义模型（Anthropic 式 `thinking_budget`）的 thinking 走独立信封，仍显示完整 `max_tokens`。在 TUI 内由 `status_bar.model_max_tokens` + `model_thinking_budget` + `model_reasoning_effort` 计算，无协议改动。 |
| 改后行为 | 底栏第 2 行显示 `max_out_token {n}` 取代 `out {n}`；无 think / budget 语义时 `n` = `max_tokens`，显示 effort 时 `n` = `max_tokens × 100/(100+pct)`（`none`/无 effort → 不变，`low` → 80%，`medium` → ~67%，`high` → ~57%，`xhigh`/`max` → 50%）。 |
| 指针 | `crates/tui/src/render/bar.rs`（`format_max_out_tokens`）、`crates/tui/src/i18n.rs`（`bottom_out`）、book [Ch 23](./23_chapter_tui_zh.md) §6.6、`docs/token_usage_schema.md`。 |

---

## 1. 2026-08-15 — 模型→上下文窗口映射覆盖手工 `model_context_window` 配置

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | `agent.model_context_window` 此前完全手工指定（CLI/TOML，默认 `200_000`），没有任何模型推断。使用 `deepseek-v4-pro`（真实窗口 1M）时，底栏 `ctx` 计量因残留的 256k 配置显示 `…/256K`，自动压缩也在 ~80%（约 205k）处触发而非 ~800k，导致过早压缩。`max_tokens` 已有按模型的默认值可参照（`kimi_k2x → 32_000`），而窗口没有等价机制。 |
| 决策 | 在 `resolve.rs` 新增 `model_context_window_for_model(model)`，并按 **模型→窗口映射（最高）→ CLI/TOML → 默认 `200_000`** 解析窗口。数值依据官方模型文档（2026-08）：OpenAI `gpt-5.6` 系列 + `gpt-5.5` → `1_050_000`、`gpt-5.4` → `1_000_000`、`gpt-5`…`gpt-5.3`/`gpt-5.4-mini` → `400_000`、`gpt-4o` 系列 → `128_000`；Anthropic（API 与 Claude Code 同 ID）`claude-sonnet-5`/`claude-fable-5`/`claude-opus-5`/`claude-opus-4-8`/`claude-opus-4-7`/`claude-opus-4-6`/`claude-sonnet-4-6` → `1_000_000`、`claude-sonnet-4-20250514`/`claude-opus-4-20250514`/`claude-haiku-4-5`/`claude-haiku-4-20250514` → `200_000`；DeepSeek V4 → `1_000_000`、`k3-256k` → `256_000`。命中映射时**刻意**覆盖用户文件配置，避免过时的手工窗口低估已知模型。 |
| 改后行为 | `ctx` 底栏计量与派生的自动压缩阈值（窗口的 80%）对已映射模型使用映射后的窗口。GPT-5.6/5.5 系列显示 `…/1.05M`、Claude 1M 模型（含 Claude Code ID）`…/1M`、DeepSeek V4 `…/1M`、GPT-5.x `…/400K`、GPT-4o `…/128K`、`k3-256k` `…/256K`。手工 `model_context_window` 仅对无内置映射的模型生效。非零 `model_context_window > max_tokens` 的校验仍作用于解析后的最终值。 |
| 指针 | `crates/tact/src/config/resolve.rs`（`model_context_window_for_model`，解析位于 ~`:587`）；`config.example.toml` `[agent]`；book [Ch 21](./21_chapter_config_zh.md) §5、[Ch 5](./05_chapter_compact_zh.md) 设置表。 |

---

## 1. 2026-08-15 — Markdown 正文迁移到 pulldown-cmark；ratatui-markdown 仅保留 Mermaid

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Plan | `docs/superpowers/plans/2026-08-15-pulldown-cmark-migration.md` |
| Symptom / motivation | 下方条目所述的整合方案最终落在本地 `ratatui-markdown` fork 上，仅为了让 Tact 的正文渲染对齐 `tui-markdown` 的输出，就携带了约 350 行、跨 8 个文件的补丁（H4–H6、有序编号、硬换行、嵌套强调、CJK 左翼、主题代码色槽）。fork 是 rebase 负担，且是只在本机可解析的 path 依赖；`steer` 与 xAI 的 `grok-build` 都用 `pulldown-cmark` 解析 CommonMark、在自己代码里渲染，而非 fork 一个 Markdown 库。 |
| Decision | 用 `pulldown-cmark` 0.13 的事件循环（`crates/tui/src/render/pulldown.rs`）替代 fork 的块渲染器，复用 Tact 的宽度感知管道表 `format_table`、`▎` 引用块 gutter、fenced-code 无框主题样式与 Mermaid 路由。`ratatui-markdown` 改为上游 git 依赖（`celestia-island/ratatui-markdown` @ `3a8bcbe`，仅 `mermaid` feature），只用于非 sequence 的 Mermaid 图；`sequenceDiagram` 仍是 Tact 自研的 `mermaid_sequence.rs`。删除 `feat/tact` fork 与 `TuiRenderHooks`/`RenderHooks` 适配器。 |
| Behavior after | 正文/标题/列表/任务/表格/引用块渲染由 Tact 依据 `pulldown-cmark` 事件自持。刻意关闭 `ENABLE_SMART_PUNCTUATION`，使 `...` 不被转成 `…`（系统消息与用户文本保持字节稳定）。GFM 任务列表现在同样作用于有序列表（`1. [X]` → `1. ☑`）。Mermaid 输出不变。 |
| Pointers | `crates/tui/src/render/pulldown.rs`、`render_md.rs`；`Cargo.toml`（`ratatui-markdown` git 依赖 + `pulldown-cmark`）；book [Ch 23](./23_chapter_tui_zh.md) §6.7；下方条目记录了中间的 fork 方案。 |

---

## 1. 2026-08-15 — 主区域 Markdown 渲染统一到 ratatui-markdown

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 计划 | `docs/superpowers/plans/2026-08-15-ratatui-markdown-migration.md` |
| 症状 / 动机 | TUI 主区域并行维护两套 Markdown 栈：`tui-markdown` 0.3.x（crates.io）渲染日志面板的正文 / 标题 / 列表，`ratatui-markdown`（celestia-island git fork，按分支固定）渲染 Mermaid 与 `/tasks-dag` 弹窗。两套样式适配器、两套调色板，以及各种 fork 规避（任务列表标记转义、`log_style.rs` 中的硬编码色重映射、围栏标记簿记）都必须同步维护。 |
| 决策 | 统一到 `ratatui-markdown`（本地 fork 位于 `../ratatui-markdown`，分支 `feat/tact`，基于 `chore/update-ratatui-0.30` @ `3a8bcbe`）并补齐能力：H4–H6 标题、有序列表保留编号、每级 4 列嵌套缩进、递归嵌套强调（`**bold _x_ italic**`）、链接保留 URL 后缀、行内代码 / 围栏代码的主题色槽位、软换行折叠为空格 + 硬换行保留、连续空格保留、CJK 标点友好的强调判定。Tact 侧：`render_plain_markdown` 用 fork 的 parse+render 替换 `tui_markdown::from_str_with_options`，并通过 `TuiRenderHooks`（隐藏围栏 / 边框装饰、直接上代码背景）；引用 `▎` gutter 与 H1 高亮背景移入后处理 / restyle 阶段；表格通过 `table` RenderHooks 适配器委托给 Tact 的宽度感知 `format_table`（管道风格），因此 fork 自带的 `render_table` 不再被使用、保持上游原样；三击代码块检测改为匹配代码背景而非 ````` ``` ````` 标记；diff 弹窗直接使用 `syntect` 高亮（同一 Base16 Ocean Dark 主题），从而移除 `tui-markdown` 与直接依赖的 `pulldown-cmark`。 |
| 行为后 | 日志面板经单一 crate 渲染 Markdown。无序列表渲染为 `•`，任务项渲染为 `☐` / `☑`（原先为字面 `-` / `[ ]` 文本）；围栏代码保持主题背景且无围栏标记；raw 复制行镜像渲染文本（标记在解析阶段被消费）；`/stats` 的 GFM 表格经 `format_table` 渲染为管道表；diff 弹窗保留语法高亮。 |
| 指针 | `crates/tui/src/render/render_md.rs`（`TuiRenderHooks`、`render_plain_markdown`、`apply_blockquote_indicator`）、`crates/tui/src/render/log_style.rs`（H1 高亮规则）、`crates/tui/src/render/popups/diff_popup.rs`（syntect）、`crates/tui/src/widgets/state/app/popups.rs`（`find_code_block_containing_logical`）、`Cargo.toml`（`ratatui-markdown` path 依赖 + `syntect`）；fork 仓库 `../ratatui-markdown` `feat/tact`；[Ch 23](./23_chapter_tui_zh.md) §6.7。 |

---

## 1. 2026-08-15 — Thinking、命令输出与 Read 卡片顶部移除重复行数，统一由底部栏承载

| 字段 | 内容 |
|------|------|
| 类型 | `removal` |
| 症状 / 动机 | Thinking 卡片把总行数显示了两遍——顶部标题（`🧠 Thinking (N lines)`）与底部栏（`↕ 可见/N 行 …`）各一次；`bash` 命令输出卡片同样重复（顶部 `Live output (N lines)` / `Command output (N lines)`，底部 `preview/total 行` 提示）；`read_file` 卡片也是如此（顶部 `Read <路径> (N lines)`）。两者同时可见时，顶部计数与底部栏数字冗余。 |
| 决策 | 卡片顶部标题不再携带行数：`🧠 Thinking`（active 与 completed 一致）、`Live output`（运行中 bash）、`Command output`（已完成 bash）、`Read <路径>`（read_file）。底部栏成为唯一计数来源（Thinking 的 `↕ visible/total 行`；命令输出溢出预览时的 `preview/total 行`）。删除不再使用的 `thinking_card_title_pl` 字段；`tool_live_output_title_tmpl` 去掉 `{}` 占位符并更名为 `tool_live_output_title`。 |
| 改后行为 | Thinking 卡片显示 `🧠 Thinking` / `🧠 思考中`；运行中的 bash 卡片显示 `Live output` / `实时输出`；完成的命令卡片显示 `Command output`；Read 卡片显示 `Read <路径>`。所有行数都在卡片底部栏。Popup 标题不变（本就用命令文本或裸 `Command output`）。 |
| 指针 | `crates/tui/src/i18n.rs`、`crates/tui/src/render/cells/thinking.rs`、`crates/tui/src/widgets/tool_widget.rs`（`detail_card_title`）、`crates/tui/src/render/cells/tool.rs`（`card_bottom_text`）；测试 `live_output_total_excludes_command_prefix_but_popup_keeps_it`、`log_tool_card_renders_when_scrolled_into_placeholder_rows`；[Ch 23](./23_chapter_tui.md) §render pipeline。 |

## 1. 2026-08-15 — Log 按词边界折行；文字选择交互对称化

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | （1）`wrap_line` 在显示宽度处硬切每行（`split_at_display_width`），长 URL、路径与单词从中间断开且无续行提示；（2）跨折行的部分选择在续行上丢失 REVERSED 高亮——旧折行路径把所有 span 摊平为一个基础样式；（3）双击选词只认 ASCII，双击中文选不中任何内容；（4）选择交互不对称：点击/拖入整段 Markdown 行会产生看不见的选择（MarkdownCell 渲染器不画叠加层），点空白区或面板外则保留过期选择；（5）鼠标 hit-test 按面板宽度模拟硬折行，而渲染按宽度 − 缩进折行，缩进行上最多偏移一个缩进宽。 |
| Decision | 新增共享的 `wrap_break_offsets` 一次性计算视觉行起始偏移，`wrap_line` 与 `visual_pos_to_byte_offset` 共用，渲染与 hit-test 不可能再分歧。折行改为贪心词边界折行：在最后一个放得下的空白处断开；仅当连续词超过行宽才硬切；尾随空白留在上一行（不可见），保证分段字节连续。`wrap_line` 按分段重新切片原始样式 span，使逐段样式（含 REVERSED）延续到续行。`find_word_bounds` 按光标下字符分类为 ASCII 词或 CJK 连续段（汉字/假名/谚文）并在同类内扩展。`handle_log_click`/`handle_mouse_drag`/`handle_log_triple_click` 拒绝在 Markdown 行上开始或扩展选择；点击日志下方空白或面板外的任意位置清除选择。hit-test 的折行宽度减去行缩进。 |
| Behavior after | 单词不再从中间断开（URL/路径/CJK 保持完整直到确实超宽）；选择高亮在每条续行都可见；双击可选中整段中文；Markdown 卡片不再被"静默选中"；误点击清除过期选择而非保留；缩进行上的点击映射到正确字节。 |
| Pointers | `crates/tui/src/render/util.rs`（`wrap_break_offsets`、`wrap_line`、`visual_pos_to_byte_offset`、`col_to_byte_offset`）、`crates/tui/src/widgets/state/app/visibility.rs`（`find_word_bounds`、`is_markdown_row`、`byte_offset_from_log_position`）、`crates/tui/src/handlers/mouse.rs`（点击/拖拽/三击守卫与面板外点击清除）、`crates/tui/src/render/cells/text.rs`；测试 `wrap_break_offsets_prefers_word_boundaries`、`wrap_line_keeps_word_intact_and_preserves_span_styles`、`wrap_break_offsets_agree_with_byte_offset_hit_testing`、`partial_selection_reverses_target_span_across_wrapped_lines`、`double_click_selects_cjk_run`、`click_below_last_message_clears_selection`、`click_on_markdown_row_does_not_create_invisible_selection`、`drag_into_markdown_row_does_not_extend_selection`、`click_outside_log_clears_selection`；[第 23 章](./23_chapter_tui_zh.md) 渲染管线节。 |

## 1. 2026-08-15 — Log 滚动改为视觉行；`/skills` 分页

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Log 面板按逻辑消息行滚动（`j`/`k`/滚轮每格 `log_scroll.offset ± 1`）。高于 viewport 的整段 Markdown 消息——例如 `/skills` 约 60 个 skill、400+ 渲染行的管道表格——只能看到首尾两屏：`resolve_visual_scroll` 在最大逻辑偏移处钉底，位于中间的行（按字母序排列的 `lark-*`）双向都滚不到。 |
| Decision | viewport 的首条可见**视觉**行成为权威状态（`LogScroll.visual_top`，`usize::MAX` = 钉底哨兵）；`offset` 变为派生的逻辑镜像，仅供只读消费方（鼠标 hit-test、code 弹窗）使用。纯函数 `visual_step_up/down` 在高于 viewport 的 cell 内部按 `j`/`k` 半屏、滚轮 3 行步进，其余情况按行边界跳转；从下方进入高 cell 时落在其底部，保证向上遍历连续。删除 `resolve_visual_scroll` / `effective_max_logical_scroll`。`/skills` 输出额外按 15 个 skill 一页分块，每块一条带 `(n/k)` 标题的 Markdown 消息。 |
| Behavior after | 任何高于 viewport 的 cell（长表格、展开的工具卡片）都可用 `j`/`k`/滚轮双向完整遍历；`g`/`G` 仍跳转顶/底，自动跟随流式输出保持可用（`is_log_pinned_to_bottom` 改为比较视觉位置）。`/skills` 每页渲染 15 个 skill 并带页码标题。 |
| Pointers | `crates/tui/src/widgets/state/app/scroll.rs`（步进函数与滚动 API）、`crates/tui/src/widgets/state/log_scroll.rs`（`visual_top`）、`crates/tui/src/render/log.rs`（视觉钳制与镜像派生）、`crates/tui/src/handlers/{normal,mouse,mod}.rs`（按键、滚轮、`/skills` 分页）、`crates/tui/src/widgets/state/app/{agent,messages,visibility}.rs`（钉底辅助）；回归测试 `tall_markdown_cell_is_fully_traversable`、`skills_command_paginates_long_lists`；[第 23 章](./23_chapter_tui_zh.md) 渲染管线节。 |

## 1. 2026-08-15 — 主区域渲染打磨：Markdown 缩进、主题化链接、代码背景、隐藏标记

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | 渲染路径审查发现：(1) 整段 Markdown 消息（`MarkdownCell`，如 `/skills`）贴左边渲染，而流式回复/工具卡有缩进；(2) 链接使用硬编码调色板 `Blue`，永不随主题变化；(3) `is_user_message_line` 每渲染一行就向块头回退扫描，长段粘贴时呈平方复杂度；(4) `TextCell` 把所有 span 背景压平为面板底色，流式回复里的围栏代码丢失背景（与 `MarkdownCell` 不一致）；(5) 原始 Markdown 标记（`# `、`> `、``` 围栏）泄漏进渲染文本。 |
| Decision | (1) `append_markdown` 统一使用 `LOG_THINKING_INDENT + 1` 缩进；(2) 链接改用 `theme.heading`，restyle 里旧 `Blue` 重映射；(3) 每帧预计算单遍 `user_line_mask`，restyle 与缩进共用；(4) `TextCell` 保留 span 自带背景（代码 bg、H1 highlight），restyle 仅把代码背景的 span 当代码处理；(5) styled 行隐藏围栏行（渲染为空白）并剥除 `#{1,6} ` / `> ` 前缀，`raw_messages` 保留原始 Markdown 供复制、代码块检测与 hit-test；流式文本改用与最终行一致的 fg，回复完成时不再变色。 |
| Behavior after | 流式回复中的代码块有背景；H1 保留 highlight 色带；引用渲染为 `▎ text`；标题不带 `## `；链接随主题适配；长粘贴不再触发平方级回退扫描；`/skills` 等 Markdown 通知与回复对齐。 |
| Pointers | `crates/tui/src/render/{log.rs,log_style.rs,render_md.rs}`、`crates/tui/src/render/cells/{text.rs,markdown.rs}`、`crates/tui/src/widgets/state/app/{popups.rs,visibility.rs}`；测试 `span_backgrounds_survive_rendering`、`heading_keeps_no_background`、`user_line_mask_matches_the_per_row_walk`、`hardcoded_blue_links_remap_to_theme_heading`、`render_markdown_fenced_code_block`、`render_markdown_heading_markers_are_stripped`、`indented_cell_shifts_content_right`；[第 23 章](./23_chapter_tui_zh.md) 渲染管线节。 |

## 1. 2026-08-14 — 移除 cron 调度功能

| 字段 | 内容 |
|------|------|
| 类型 | `removal` |
| 现象 / 动机 | cron 功能（`cron_create` / `cron_list` / `cron_delete`）只持久化调度记录；没有任何代码解析表达式或将存储的 prompt 注入 `agent_loop`。用户要求设提醒后得到的是错误的安全感——记录存在但永远不会触发。进程内 tick loop 被判定为方向错误：需要交互式 TUI 进程常驻，且与早已可靠运行的系统 cron 重复造轮子。 |
| 决策 | 整体移除：`crates/tact/src/cron/`（调度器 + 模拟）、`crates/tact/src/store/cron_store/`（trait + SQLite 实现）、`crates/tact/src/tool/cron.rs`（工具）、`ToolContext.cron_scheduler` 字段、registry 路由、`headless.rs` / `interactive.rs` 启动接线、文档中的 `cron_tasks` 表行、TUI 工具名映射与相关测试。删除 book 第 16 章（中英）；清理第 1 / 4 / 7 / 12 / 14 / 15 / 19 / 23 章、`index.md`、`mindmap.md`、`ARCHITECTURE.md` 中的 cron 引用。 |
| 改后行为 | 不再有 `cron_*` 工具；模型无法再创建定时提示。存量 `cron_tasks` 行与遗留 `.tact/cron/` 文件保留在磁盘上不动（死数据，可手动清理）。 |
| 指针 | 已删除：`crates/tact/src/cron/*`、`crates/tact/src/store/cron_store/*`、`crates/tact/src/tool/cron.rs`、`book/16_chapter_cron*.md`；已编辑：`crates/tact/src/lib.rs`、`crates/tact/src/tool/{mod,registry}.rs`、`crates/tact/src/tool/test_support.rs`、`crates/tact/src/store/mod.rs`、`crates/tact-ui/src/{headless,interactive}.rs`、`crates/tact-ui/tests/{subsystem_tools.rs,harness/mod.rs}`、`crates/tui/src/widgets/tool_widget.rs`、`book/01_chapter_store*`；[Ch 1](./01_chapter_store_zh.md)、[Ch 7](./07_chapter_tool_zh.md)。 |

## 1. 2026-08-14 — 后台输出改为混合存储：全量日志文件 + 记录上的 `output_path`

| 字段 | 内容 |
|------|------|
| 类型 | `optimization` |
| 现象 / 动机 | `background_run` 的输出只存在 `background_tasks` DB 记录里，且上限为前 50,000 字符——cap 保留的是长日志的*开头*（通常最没用），丢弃的结尾恰恰是报错所在。模型无法深挖大输出：轮询 `check_background` 会把整个 ≤50k JSON 拉进 context，而 `grep`/`tail` 因为没有文件而无从谈起。 |
| 决策 | 混合存储：DB 记录保留元数据 + 前 50k 字符（不变，轮询便宜），**全量** stdout+stderr 流随到达即追加写入 `<workdir>/.tact/background/<id>.log`。`BackgroundTaskRecord` 新增 `output_path`（SQLite 列 `output_path TEXT NOT NULL DEFAULT ''`，对存量库用 `PRAGMA table_info` + `ALTER TABLE` 迁移）。日志文件创建为 best-effort——失败时退化为仅截断记录。`check_background` 列表每行追加 `(log: <path>)`。 |
| 改后行为 | 轮询 JSON 带 `output_path`；agent 可用 `bash tail <path>` / `grep error <path>` 深挖全量日志，而非吞下 50k blob。日志文件从任务启动即存在（spawn 前记录已写入路径），长任务可实时查看。 |
| 指针 | `crates/tact/src/background.rs`（`BackgroundTaskRecord.output_path`、`open_log_file`、`log_write`、`run_background_process`）、`crates/tact/src/store/background_store/sqlite.rs`（schema + 迁移 + upsert/读取）、`crates/tact/src/tool/background_run.rs`（列表）；测试 `run_writes_full_output_to_log_file_and_truncates_db_record`、`migrates_legacy_table_without_output_path`；[Ch 13](./13_chapter_background_zh.md) §2、§3、§6、§8；[Ch 1](./01_chapter_store_zh.md)。 |

## 1. 2026-08-13 — Plan mode 只读 shell 分类加固：拒绝换行符命令分隔

| 字段 | 内容 |
|------|------|
| 类型 | `bugfix` |
| 现象 / 动机 | `split_plain_command` 把 `\n` / `\r` 当作普通空白跳过，但对 `sh -c` 而言裸换行（以及 CRLF 输入中的 `\r`）是命令分隔符而非词分隔符。于是 `ls\nrm file` 能通过纯命令切分，白名单首词 `ls` 被归为 Read，在 plan mode 下自动放行——第二条变更命令被静默携带执行。 |
| 决策 | 切分器在扫描分隔符时一旦遇到裸 `\n` / `\r` 立即返回 `None`，任何含换行的多命令字符串保持未分类（回退到 `Write` / 提示）。单/双引号内的字面换行仍是词字符，继续放行。同一次改动把 git 全局选项处理统一进单张 `GIT_GLOBAL_OPTIONS` 表（每条记录是否消费下一个 token），同时驱动 `find_git_subcommand` 的跳过逻辑与 `git_has_unsafe_global_option`，避免两处检查再次漂移。 |
| 改后行为 | `ls\nrm file`、`echo hi\nrm -f x`、CRLF 变体及行首/行尾裸换行一律归为 **Write**（plan mode 下提示/拒绝）；`echo "line1\nline2"`、`cat "file\nname"`（引号内字面换行）仍为 Read。 |
| 指针 | `crates/tact/src/tool/readonly_shell.rs`（`split_plain_command`、`GIT_GLOBAL_OPTIONS`、`find_git_global_option`、`git_has_unsafe_global_option`）；同文件回归测试；[Ch 10](./10_chapter_permission_zh.md) §7。 |

## 1. 2026-08-13 — OpenAI 兼容 Chat Completions 将传输失败以 `LlmError::Request` 呈现

| 字段 | 内容 |
|------|------|
| 类型 | `bugfix` |
| 现象 / 动机 | OpenAI 兼容适配器把发送/连接/读响应失败报成 `LlmError::Unsupported("HTTP request failed: …")`，混淆了"端点不支持"与"请求根本没发出去"；token 数用 `u64 as u32` 强转（超限时截断误导）；工具调用 `arguments` 载荷非法 JSON 时被静默替换为 `{}`，无任何痕迹。 |
| 决策 | 新增 `LlmError::Request(String)` 变体承载请求传输/反序列化错误（API HTTP 错误仍走 `HttpError`）。流式与非流式路径都改发已序列化的 JSON 字节（`body` + 显式 `Content-Type`），不再用 `.json()` 二次序列化。token 数 `u64 → u32` 饱和转换（`u32_token_count`）。非法工具参数在 `debug` 级记录（error、工具名、原始 args）后回退为空对象。 |
| 改后行为 | 端点不可达/连接断开时显示 `request error: …` 而非 `unsupported: …`；超限 token 数饱和而非回绕；非法工具参数可在 debug 日志中看到。 |
| 指针 | `crates/tact_llm/src/error.rs`（`LlmError::Request`）、`crates/tact_llm/src/openai/compatible/mod.rs`（`OpenAiAdapter` chat/流式路径、`u32_token_count`、`tool_use_block_from_parts`）；[Ch 22](./22_chapter_llm_zh.md)。 |

## 1. 2026-08-13 — TUI 输入框长行软换行，光标随折行后的显示行定位

| 字段 | 内容 |
|------|------|
| 类型 | `bugfix` |
| 现象 / 动机 | 输入框高度与行数统计只数显式 `\n`，单条超长行会溢出 3 行上限；光标/滚动按逻辑行计算，与实际渲染不一致（长输入时光标画错行列）。 |
| 决策 | `render/input.rs` 新增 `wrap_line`（按字符边界软换行、CJK 双宽感知；`Paragraph` 保持不换行、逐行绘制这些切分）与 `caret_in_wrapped`（逻辑光标列 → 显示行列）。框高、行数统计、滚动钳制与光标定位全部改用显示行。 |
| 改后行为 | 长行在框内折行而非溢出；高度随折行自动扩展（1–3 显示行 + border）；光标与滚动跟随折行行。提交文本不变。 |
| 指针 | `crates/tui/src/render/input.rs`（`wrap_line`、`caret_in_wrapped`）；`crates/tui/src/lib.rs`（输入框高度）；测试见 `input.rs`（`wrap_line_splits_at_column_width`、`caret_in_wrapped_maps_logical_column_to_display_row`、`input_box_soft_wraps_overlong_line`、`input_box_scrolls_to_caret_on_wrapped_line`）；[第 23 章](./23_chapter_tui_zh.md) §6.2、§6.6。 |

## 1. 2026-08-13 — Plan mode 可运行可证明只读的 shell 命令（`ls`、`grep` 等）

| 字段 | 内容 |
|------|------|
| 类型 | `optimization` |
| 现象 / 动机 | Plan mode 拒绝一切归类为 `Write` 的工具，而 shell 命令此前一律归为 `Write`（仅 `sudo ` / `su ` 开头为 `High`），导致 `ls` / `grep` 这类规划 agent 最需要的探查命令在 plan mode 下也被硬拒绝。 |
| 决策 | `PermissionPolicy::ShellCommand::resolve` 现在在命令**可证明只读**时将其归为 `Read`，依托新的保守分类器 `crates/tact/src/tool/readonly_shell.rs`：(1) 纯命令切分，拒绝任何 shell 元字符（管道、重定向、`$`、反引号、glob、转义等），保证分类不会与 `sh -c` 实际执行内容产生分歧；(2) 白名单程序（仅凭选项无法写入），如 `ls`、`grep`、`cat`、`head`、`tail`、`wc`、`git status/log/diff/show/branch`，以及排除危险旗标的 `find`/`rg`/`base64`/`sed` 等，镜像 OpenAI Codex 的 `is_known_safe_command`（`codex-rs/shell-command/src/command_safety/is_safe_command.rs`）。任何含糊输入一律保持 `Write` ——分类器刻意偏向漏判，确保变更类命令不可能在 plan mode 下被静默执行。 |
| 改后行为 | Plan mode 下 `ls -la`、`grep -rn x .`、`git status` 无需提示即可运行；`cargo test`、管道、重定向、未知程序与不安全选项（`find -delete`、`git push` 等）仍被拒绝。`bash` 与 `background_run` 共用同一分类，因此只读命令在 Default 模式下也会自动放行。 |
| 指针 | `crates/tact/src/tool/readonly_shell.rs`；`crates/tact/src/tool/metadata.rs`（`ShellCommand::resolve`）；测试见 `crates/tact/src/tool/readonly_shell.rs` 与 `crates/tact/src/permission/mod.rs`（`plan_mode_allows_readonly_shell_commands_and_denies_others`）；[Ch 10](./10_chapter_permission_zh.md) §2、§4、§7。 |

## 1. 2026-08-12 — async-openai 从 `vendor/async-openai` 切换到本地维护的 fork `../async-openai`

| 字段 | 值 |
|------|-----|
| 类型 | `docs`（依赖管理） |
| 症状 / 动机 | `vendor/async-openai` 下的 vendored 拷贝（2026-08-10 条目）能用，但把整个 crate 复制进了仓库：每次同步上游都要 diff、重新打补丁，还要在 Tact 的最小 feature 集下保持 crate 级 doctest 可编译。 |
| 决策 | fork 改为独立仓库，位于 `../async-openai`（克隆自 `https://github.com/rust-infra/async-openai`，分支 `feat/tact`，commit `ca74607` = 上游 main 0.41.3），直接维护并包含四个本地提交：(1) `CreateResponse` 增加类型化字段 `context_management: Option<Vec<ContextManagementParam>>`；(2) `ReasoningEffort` 增加 `Max` 变体；(3) 两个调用 `client.chat()` 的 doctest 增加 feature gate；(4) package 改名为 `async-openai-local` 并显式 `[lib] name = "async_openai"`（fork workspace 移除 examples，因为它们仍引用上游包名）。Tact workspace 依赖变为 `async-openai-responses = { package = "async-openai-local", path = "../async-openai/async-openai", version = "0.41.3", features = ["responses", "byot"] }`；删除 `vendor/async-openai/`。代码仍 `use async_openai_responses::…`，无需改引用。 |
| 改后行为 | 无用户可见变化：配置阈值时 wire body 仍携带 `context_management`。维护移到仓库外：直接改本地 fork（`/Users/rg/Projects/async-openai`，分支 `feat/tact`），不再 re-vendor。 |
| 指针 | `/Users/rg/Projects/async-openai`（fork，`feat/tact` 上提交 `7de8bb4` / `5e22785` / `12488eb`）；`Cargo.toml` 的 `async-openai-responses` 依赖；`crates/tact_llm/src/openai/responses/convert.rs`（`create_response` builder 注入）；[Ch 22](./22_chapter_llm_zh.md) §6.2。 |

## 1. 2026-08-12 — Worktree 存储从 JSON 文件迁移到 SQLite（`WorktreeStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | worktree 元数据 + 审计日志以单一 JSON 索引持久化（`worktrees/index.json`，`Store<WorktreeIndex>`），读-改-写无事务；重名检查与索引写入存在竞态。 |
| 决策 | worktree 状态移入现有 `<workdir>/.tact/tact.db`，建 `worktrees` + `worktree_events` 表，通过新的异步 `WorktreeStore` trait（`crates/tact/src/store/worktree_store/`，sqlx 实现的 `SqliteWorktreeStore`）访问。`worktrees.name` UNIQUE（并发兜底）；自增 `id` 保持插入顺序；`worktree_events` 按自身 `id` 排序。新增 `session_id` 列与索引，`worktree_create` 时从工具上下文填充。`WorktreeManager` 变为 `Box<dyn WorktreeStore>` 之上的 async 门面；`SharedWorktreeManager` 去掉 mutex（`Arc<WorktreeManager>`，连接池已串行化写入——`worktree_run` 不再阻塞其他 worktree 工具）。遗留 `worktrees/index.json` 不再读取、留在磁盘。至此无领域模块使用 JSON store（`StoreRoot`/`Store`/`CollectionStore` 作为通用原语保留，自带单元测试）。 |
| 改后行为 | 泳道与事件持久化在 `tact.db`（旧 `worktrees/index.json` 条目若不手动导出则丢失）；`worktree_*` 表面不变；`session_id` 出现在 worktree 记录中。 |
| 指针 | `crates/tact/src/store/worktree_store/{mod,sqlite}.rs`、`crates/tact/src/worktree/mod.rs`、`crates/tact/src/tool/worktree.rs`、`crates/tact-ui/src/{headless,interactive}.rs`；[Ch 1](./01_chapter_store_zh.md) §5–6、[Ch 15](./15_chapter_worktree_zh.md) §2–5。 |

## 1. 2026-08-12 — Team 存储从 JSON 文件迁移到 SQLite（`TeamStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | roster 以单一 JSON 索引持久化（`team/config.json`，`TeamConfig` 包装），inbox 为每个 owner 一个 JSONL 文件（`team/inbox/{owner}.json`）。两者都是无事务的读-改-写、无跨进程锁；重名检查与 roster 写入存在竞态。 |
| 决策 | team 状态移入现有 `<workdir>/.tact/tact.db`，建 `teammates` + `inbox_messages` 表，通过新的异步 `TeamStore` trait（`crates/tact/src/store/team_store/`，sqlx 实现的 `SqliteTeamStore`）访问。`teammates.name` 为 PRIMARY KEY；重复 spawn 用 `INSERT OR IGNORE` + `rows_affected == 0` 拒绝（保留 `teammate {name} already exists` 错误且无竞态）。`inbox_messages` 增加自增 `id` 以保持读取的插入顺序（遗留 JSONL 追加语义）+ `owner` 索引。`TeammateManager` 变为 `Box<dyn TeamStore>` 之上的 async 门面；`SharedTeammateManager` 去掉 mutex（`Arc<TeammateManager>`，连接池已串行化写入）。旧 JSON 文件不再读取、留在磁盘。 |
| 改后行为 | roster 与 inbox 持久化在 `tact.db`（旧 `team/` JSON 条目若不手动导出则丢失）；`spawn_teammate` / `broadcast` / `read_inbox` / `plan_approval` / `shutdown_*` 表面不变；跨进程 inbox 写入不再在文件追加上竞态。 |
| 指针 | `crates/tact/src/store/team_store/{mod,sqlite}.rs`、`crates/tact/src/team.rs`、`crates/tact/src/tool/team.rs`、`crates/tact-ui/src/{headless,interactive}.rs`；[Ch 1](./01_chapter_store_zh.md) §5–6、[Ch 14](./14_chapter_team_zh.md) §3–5。 |

## 1. 2026-08-12 — Cron 与后台任务从 JSON 文件迁移到 SQLite（`CronStore` / `BackgroundStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | cron 以单一 JSON 索引持久化（`cron/scheduled_tasks.json` 含 `next_id` 计数器），后台以每条记录一个 JSON 文件持久化（`background/tasks/{id}.json` 加内存 `Mutex<HashMap>` 镜像）。两者都是无事务的读-改-写、无跨进程锁；后台 manager 持有磁盘 + 内存双份状态，可能漂移。 |
| 决策 | cron 与后台移入现有 `<workdir>/.tact/tact.db`，建 `cron_tasks` + `background_tasks` 表，通过新的异步 trait `CronStore`（`crates/tact/src/store/cron_store/`）与 `BackgroundStore`（`crates/tact/src/store/background_store/`）访问，仿照 `TaskStore` 模式。`cron_tasks` 的 id 由 `INTEGER PRIMARY KEY AUTOINCREMENT` 分配、对外以 8 位十六进制字符串暴露（`format!("{rowid:08x}")`）——与遗留索引的线上契约一致；`background_tasks` 保留时间戳毫秒 hex `id`，`status` 带 `CHECK` 约束。两张表都新增 `session_id` 列与索引，`cron_create` / `background_run` 时从工具上下文填充。`CronScheduler` / `BackgroundManager` 变为 async 门面；`SharedCronScheduler` 去掉 mutex（`Arc<CronScheduler>`），`BackgroundManager` 去掉内存镜像（DB 为唯一数据源；spawn 的 tokio 任务通过克隆的 store 句柄写回）。旧 JSON 文件不再读取、留在磁盘；`TactPath::cron_dir()` / `CRON_SUBDIR` 作为死代码删除。 |
| 改后行为 | cron id 从 `00000001` 重新开始（旧条目若不从 `.tact/cron/` 手动导出则丢失）；`cron_*` / `background_*` / `/background` 表面不变；启动孤儿修复（`running` → `error`）改为扫表；`session_id` 出现在 cron JSON 与后台记录中。 |
| 指针 | `crates/tact/src/store/cron_store/{mod,sqlite}.rs`、`crates/tact/src/store/background_store/{mod,sqlite}.rs`、`crates/tact/src/cron/mod.rs`、`crates/tact/src/background.rs`、`crates/tact/src/tool/{cron,background_run}.rs`、`crates/tact-ui/src/{headless,interactive,driver}.rs`；[Ch 1](./01_chapter_store_zh.md) §5–6、[Ch 13](./13_chapter_background_zh.md) §2–5。（Ch 16 已于 2026-08-14 随 cron 功能一并删除。） |

## 1. 2026-08-11 — 任务存储从 JSON 文件迁移到 SQLite（`TaskStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | 任务以每条记录一个 JSON 文件（`tasks/task_{id}.json`）加 `tasks/index.json` 的 next-id 计数器持久化。ID 分配与依赖边（`blockedBy` / `blocks` 在两条记录上互相镜像）都是无事务的读-改-写，且没有跨进程锁；完成任务需要 O(n) 全表扫描来清理边。 |
| 决策 | 任务移入现有 `<workdir>/.tact/tact.db`，建 `tasks` + `task_dependencies` 表，通过新的 `TaskStore` trait（`crates/tact/src/store/task_store/`，sqlx 实现的 `SqliteTaskStore`）访问。边为行（复合主键，`INSERT OR IGNORE`），无镜像字段、无外键；每次变更都在 `BEGIN IMMEDIATE` 事务内，完成时用一条 `DELETE` 清边。ID 由 `INTEGER PRIMARY KEY AUTOINCREMENT` 分配（删除 `TaskIndex`）。`TaskManager` 变为 `Box<dyn TaskStore>` 之上的 async 门面；`SharedTaskManager` 去掉 mutex（`Arc<TaskManager>`，连接池已串行化写入）。新增 `session_id` 列与索引，`task_create` 时从工具上下文填充。旧 JSON 文件不再读取、留在磁盘。`crates/tact/Cargo.toml` 的 tokio features 增加 `macros` + `rt-multi-thread`，使 `-p tact` 单独构建时 `#[tokio::test]` 可用。 |
| 改后行为 | 新任务 ID 从 1 开始（旧的 1–233 条记录若不从 `.tact/tasks/` 手动导出则丢失）；依赖更新原子化；`task_*` 工具表面不变（`session_id` 出现在任务 JSON / 快照中）。 |
| 指针 | `crates/tact/src/store/task_store/{mod,sqlite}.rs`、`crates/tact/src/task/mod.rs`、`crates/tact/src/tool/task.rs`；[Ch 1](./01_chapter_store_zh.md) §6、[Ch 19](./19_chapter_persistent_tasks_zh.md) §2–3。 |

## 1. 2026-08-11 — 摘要器 thinking budget 限制在 `max_tokens` 之下；Kimi K3 默认 reasoning 预留

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 2026-08-10 的摘要预算改动与主循环一样用 `with_thinking(self.thinking_config())` 把配置的 Claude 式 thinking budget 转发给压缩摘要请求，但摘要器的 `max_tokens` 独立封顶为 `min(窗口 × 20%, 2,000)`。Anthropic 在 wire 上要求 `budget_tokens < max_tokens`，于是默认 8k/32k 的 thinking budget 会生成非法请求（`thinking.budget_tokens = 8,000` 而 `max_tokens = 2,000`），导致所有开启 thinking 的 Anthropic 用户本地压缩以 400 失败。另外，reasoning 预留只把 DeepSeek 视为默认开启 reasoning，但 Kimi K3 服务端同样默认 thinking 开启 + effort high，未显式配置 effort 时 Kimi 摘要仍可能被 reasoning 挤占。 |
| 决策 | 新增 `compact_summary_thinking(configured_budget, summary_max_tokens)`，把转发的 budget 限制为 `summary_max_tokens - 1`（输出预算退化到 ≤ 1 token 时完全禁用 thinking），并通过一个小的 builder 闭包同时应用于首次与续写的摘要请求。输入侧预留仍按配置的 budget 扣除（偏保守）。`compact_summary_reasoning_reserve_percent` 现在对 `ProviderKind::Kimi` 与 DeepSeek 一样预留默认 high 档（75%）。 |
| 改后行为 | 使用大 thinking budget 的 Anthropic 压缩会发送 `budget_tokens = max_tokens - 1` 而非以 400 失败；本来就放得下的 budget 原样透传。未显式配置 effort 的 Kimi K3 获得与 DeepSeek 相同的 75% reasoning 预留。 |
| 指针 | `crates/tact/src/agent/mod.rs` 中 `compact_summary_thinking` 与 `compact_summary_reasoning_reserve_percent`（`compact_history_local_with_mode`）；测试 `compact_summary_thinking_clamps_below_max_tokens`、`local_compact_clamps_thinking_budget_below_summary_max_tokens`、`compact_summary_reasoning_reserve_percent_tiers`；[Ch 5](./05_chapter_compact_zh.md) §5 步骤 3。 |

## 1. 2026-08-10 — 本地 vendor async-openai 为 `async-openai-local`，获得类型化 `context_management`

| 字段 | 值 |
|------|-----|
| 类型 | `docs`（依赖管理） |
| 症状 / 动机 | OpenAI Responses API 官方支持 `context_management` / `compact_threshold`（服务端压缩），官方 Python/Node SDK 也有类型化支持；但 Rust 的 `async-openai` crate（截至 2026-08-10 最新 0.41.3）只定义了 `ContextManagementParam` 类型，从未把它接进 `CreateResponseArgs` builder——Tact 只能通过 byot JSON 路径注入（`body["context_management"] = serde_json::json!(...)`）。 |
| 决策 | 将 async-openai 0.41.3 源码 vendor 到 `vendor/async-openai`，并把 package 名改为 `async-openai-local`，使 path 依赖只命中 Responses 协议的 0.41.x、不会与 Chat Completions 路径使用的旧版 `async-openai 0.20` 冲突（workspace 清单：`async-openai-responses = { package = "async-openai-local", path = "vendor/async-openai", ... }`；代码仍 `use async_openai_responses::…`，无需改引用）。与上游源码有两处差异：给 `CreateResponse` 加类型化字段 `context_management: Option<Vec<ContextManagementParam>>`；给 `ReasoningEffort` 加 `Max` 变体（上游只到 `Xhigh`；DeepSeek / Kimi K3 接受 `max`）。`convert.rs` 现在全部通过类型化 builder 构造——`context_management(...)` setter，以及 `Reasoning { effort: request.reasoning_effort.map(Into::into), summary }`（经 `crates/tact_llm/src/types.rs` 中的 `impl From<OpenAiReasoningEffort> for ReasoningEffort`）——不再使用 `serde_json::json!` / `Value::String` 注入。`vendor/async-openai/README.fork.md` 记录与上游同步的方法。 |
| 改后行为 | 无用户可见变化：配置阈值时 wire body 仍携带 `context_management`。维护改为本地化：新的 Responses 字段可以直接加到 vendor，不必等待上游 Rust crate。 |
| 指针 | `vendor/async-openai/`（`README.fork.md`、`src/types/responses/response.rs`）；`Cargo.toml` 的 `async-openai-responses` 依赖；`crates/tact_llm/src/openai/responses/convert.rs`（`create_response` builder 注入）；[Ch 22](./22_chapter_llm_zh.md) §6.2。 |

## 1. 2026-08-10 — Responses 端点未实现 `/responses/compact` 时给出明确报错

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 兼容 `/responses` 端点（如 `opencode.ai/zen/go/v1`）往往不实现 `POST /responses/compact`，返回 404 HTML 页面。SDK 的 `compact_byot` 随后会抛出把整个 HTML body 塞进错误的 JSON 反序列化错误；且该消息可能命中瞬时错误重试列表（"unavailable"），导致无意义的退避重试后才失败。 |
| 决策 | Responses 适配器的 `compact()` 改为通过共享原始 HTTP client 发送 compact 请求（与 SDK 同一传输层），从而可以检查状态码。HTTP 404/405 映射为 `LlmError::Unsupported("endpoint does not support POST /responses/compact (HTTP {status}): native Responses compaction is not implemented by base URL {base_url}")` —— 措辞刻意避开瞬时错误关键词，避免进入重试循环。其它非 2xx 状态沿用既有 `LlmError::HttpError { status, body }`。 |
| 改后行为 | 在未实现 `/responses/compact` 的端点上触发压缩时，会立即显示点名缺失端点和 base URL 的明确错误（无 HTML 倾倒、无重试）；会话状态保持不变。 |
| 指针 | `crates/tact_llm/src/openai/responses/mod.rs` 的 `compact()`；测试 `compact_reports_missing_endpoint_clearly`；[Ch 22](./22_chapter_llm_zh.md) §6.2、[Ch 5](./05_chapter_compact_zh.md)。 |

## 1. 2026-08-10 — Responses 适配器在兼容端点无终态事件关闭流时恢复

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 兼容 `/responses` 端点（如 `opencode.ai/zen/go/v1`）偶尔会在没有任何终态事件（`response.completed` / `response.incomplete` / `response.failed`）的情况下关闭 SSE 流。`ResponsesStreamState::finish()` 会以 `unsupported response state: OpenAI Responses stream ended without a terminal event` 硬失败，即使流已经交付了完整的 `output_item.done` 序列或可见文本，也会中止整个 agent 回合。 |
| 决策 | 当流本身完整时，无终态事件的干净 EOF 现在被视为终态：若所有已 announce 的 item 均完成（`output_item.done` 序列连续且无 pending `added`），则从 done 序列重建输出；否则恢复已流式输出的可见文本（与既有兼容端点恢复同一分支）。合成一个最小 completed `Response` 后走既有的 normalize/恢复路径，因此 stop reason 推断（含工具调用时的 `ToolUse`）与 provider-state baseline 构建保持不变。缺失 compaction 边界（`pending_compactions` 非空）与空流仍然是硬协议错误——恢复绝不能静默丢弃已压缩的 baseline。 |
| 改后行为 | 之前因 "stream ended without a terminal event" 直接失败的回合，现在在响应已完整交付时从 done 序列/流式文本正常完成；真正空流或 compaction 不完整的流仍会大声失败。 |
| 指针 | `crates/tact_llm/src/openai/responses/stream.rs` 的 `finish()`；测试 `no_terminal_event_recovers_from_complete_done_sequence`、`no_terminal_event_recovers_visible_text`、`no_terminal_event_empty_stream_is_error`、`no_terminal_event_with_pending_compaction_is_error`；[Ch 22](./22_chapter_llm_zh.md) §6.2。 |

## 1. 2026-08-10 — 本地压缩为 reasoning / thinking token 预留输出预算

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | `compact_history_local_with_mode`（所有非 OpenAI-Responses 压缩的摘要器）把摘要请求的 `max_tokens` 定为 `min(窗口 × 20%, 2,000)` 并当作**文本**预算，但 reasoning-effort 类 provider（OpenAI o 系 / DeepSeek / Kimi K3）把 reasoning token 计入同一个 `max_tokens` 信封。配置 `high`/`max` effort（或 DeepSeek 服务端默认 thinking 开启 + effort high）时，reasoning 会烧掉 2,000 预算的大半，留给摘要文本的额度不足，每次调用都撞 `StopReason::MaxTokens`，续写循环（≤3 次，且每次都共享同一上限）最终只能接受 best-effort 的部分摘要。摘要请求也从未转发配置的 Claude 式 thinking budget（主循环会），输入侧预留同样忽略了它。 |
| 决策 | 拆分摘要输出预算：摘要**文本**沿用经典 `min(窗口 × 20%, 2,000)`；当配置了 reasoning effort（或 provider 为 DeepSeek 且未显式配置 effort）时，在文本预算**之上**追加分档预留（minimal\|low / medium / high / xhigh\|max 分别为文本预算的 25/50/75/100%；DeepSeek 默认 ≈ high = 75%），使 wire 上的 `max_tokens` = 文本 + 预留，reasoning 不再挤占文本额度。摘要请求现在会转发配置的 thinking budget（`with_thinking(self.thinking_config())`，与主循环一致），并从输入侧预留中扣除。不改变任何 effort 语义 —— 只做预算核算。 |
| 改后行为 | 配置了 reasoning effort（或在 DeepSeek 上）的压缩摘要会获得更大的 wire `max_tokens`（例如 128k 窗口 + high effort → 2,000 + 1,500 = 3,500），而文本部分仍拿到完整经典预算；摘要请求携带与主循环相同的 thinking 配置；输入预留同时计入 reasoning 与 thinking 余量，当窗口在扣除这些预留后放不下提示词时，仍以原有的 "too small" 错误提前失败。 |
| 指针 | `compact_summary_reasoning_reserve_percent` 与 `crates/tact/src/agent/mod.rs` 中 `compact_history_local_with_mode` 的预算计算；测试 `compact_summary_reasoning_reserve_percent_tiers`、`local_compact_reserves_reasoning_budget_and_forwards_thinking`、`local_compact_input_reservation_subtracts_thinking_budget`；[Ch 5](./05_chapter_compact_zh.md) §5 步骤 3。 |

## 1. 2026-08-10 — `background_run` 实时输出到工具卡片（类 bash）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | `background_run` 用 `Command::output()` 一次性缓冲全部输出，工具卡片立即终结（"started"），用户不轮询 `check_background` 就看不到任何内容 —— 与 `bash` 卡片实时流出输出完全不同。 |
| 决策 | 新增 keep-live 卡片契约：`ToolPresentationInfo.keep_live`（由新 `LiveOutputPolicy::Background` 映射）让 TUI 在 `StepFinished` 后仍保留卡片活动；manager 改为增量读取 stdout/stderr（`read_pipe` + `Utf8Decoder` + 约 50ms 节流的 `ToolProgress`，实时预览保留最近 ~4 KB），并以新 `AgentUpdate::BackgroundTaskFinished { tool_id, success, message, output }` 关闭卡片，携带 ✓/✗、耗时与有上限的最终输出。`background_run` 将 `BackgroundProgressSink`（tool_id + `ui_tx`）传入 `SharedBackgroundManager::run`；记录持久化与 120s 超时不变。 |
| 改后行为 | `background_run cargo build` 在 TUI 卡片中显示 spinner + 实时构建输出，进程退出时以 ✓/✗ 与耗时收尾 —— 即使 agent 那一轮早已结束。模型仍无完成 push，须轮询 `check_background`。 |
| 指针 | `crates/tact/src/background.rs`（`BackgroundProgressSink`、`run_background_process`）；`crates/tact/src/tool/background_run.rs`；`crates/tact/src/tool/metadata.rs` 的 `LiveOutputPolicy::Background`；`crates/protocol/src/agent.rs` 的 `AgentUpdate::BackgroundTaskFinished`；TUI `on_step_finished` / `on_background_task_finished`（`crates/tui/src/widgets/state/app/agent.rs`）；[Ch 13](./13_chapter_background_zh.md)、[Ch 25](./25_chapter_protocol_zh.md)。 |

## 1. 2026-08-10 — `/background` slash 命令查看后台任务状态

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | `background_run` 启动的后台任务只能通过让模型调用 `check_background` 工具来查看；TUI 没有直接入口，用户要轮询任务必须专门输入一句 prompt。 |
| 决策 | 新增 TUI slash 命令 `/background`（`/background <id>` 查看单个任务），由新协议变体 `UserCommand::QueryBackground(Option<String>)` 承载。命令 driver（`crates/tact-ui/src/driver.rs`）调用共享的 `ToolContext.background_manager.check(id)` —— 与 `check_background` 工具同一代码路径 —— 并发出 `AgentUpdate::MdInfo`，内容为 `## ⚙️ Background Tasks` 围栏代码块（未知 id 则发出 `AgentUpdate::Error`）。命令加入 `PALETTE_COMMANDS`、`i18n.rs`（中/英）本地化，面板图标为 `🖥`。 |
| 改后行为 | `/background` 每行输出一个任务（id、状态、命令）；`/background <id>` 输出该任务 pretty JSON；未知 id 显示错误。不新增状态、不做完成推送 —— 命令只读取持久化/内存中的记录。 |
| 指针 | `crates/protocol/src/agent.rs` 中的 `UserCommand::QueryBackground`；`crates/tact-ui/src/driver.rs` 的 driver 分支；`crates/tui/src/widgets/state/mod.rs` 的 `PALETTE_COMMANDS`；`crates/tui/src/handlers/mod.rs` 的 `execute_palette_command`；[Ch 13](./13_chapter_background_zh.md)、[Ch 23](./23_chapter_tui_zh.md) §3。 |

## 1. 2026-08-09 — OpenAI Responses 托管 web search（`protocol = "responses"`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| PR | https://github.com/laohanlinux/tact/pull/62（分支 `feat/responses-web-search`） |
| 症状 / 动机 | Responses adapter 只发送 function tools，OpenAI `protocol = "responses"` 会话没有托管（provider 执行）web search；用户只能自接 MCP `web_search` function tool，或退回 Chat Completions。 |
| 决策 | Hosted web search 是 **Responses 协议级能力**，与协议背后的端点/provider 无关：只要选择 `protocol = "responses"`，adapter 就在每次普通 `/responses` 请求中注入 `Tool::WebSearch`（`create_response(..., native_web_search = true)`；只有 `/responses/compact` 传 `false`——压缩端点不接受 tools）——OpenAI、DeepSeek 与 custom OpenAI-compatible 端点一视同仁，没有按 provider 的开关（`OpenAiResponsesAdapter` 不再有 `native_web_search` 标志；`ResponsesCapabilities::hosted_tools` 对每个 Responses 端点都包含 `WebSearch`）。Provider 在服务端执行搜索，Tact 只通过真实 Step 事件渲染工具卡片（`output_item.added` → `StepStarted`，每个 index 首次 `output_item.done` → `StepFinished`/`StepFailed`；`done` 时仍为 `in_progress`/`searching` 一律判失败）。`web_search_call` 永远不会变成 `ContentBlock::ToolUse`，stop reason 保持 `completed`。兼容端点若在 search action 返回 `queries` 数组而非单数 `query`，由 `wire::normalize_web_search_call_query` 处理（仅在 typed 解析时回填 `query`，原始 item 按原样回放）。`AgentUpdate::StepFailed` 新增 `arg_summary`，失败卡片标题能保留 query。DeepSeek 保留代码路径，但配置解析仍按 #57 拒绝，直到其 Responses 支持重新启用。 |
| 改后行为 | 任意 `protocol = "responses"` 会话——OpenAI、DeepSeek 或 custom OpenAI-compatible——都自动获得托管 web search；TUI 显示 `🔍 Web Search` 卡片，标题为 query，sources 为可展开详情；失败携带 status/query/action 诊断。 |
| 指针 | `crates/tact_llm/src/openai/responses/{convert,stream,wire,mod}.rs`、`crates/tact_llm/src/provider.rs`（`build_openai_responses`）、`crates/tui/src/widgets/tool_widget.rs`、AGENTS.md "Hosted tools (Provider-executed) — design invariants"、[Ch 22 §6.2.1.1](./22_chapter_llm_zh.md)。 |

## 1. 2026-08-09 — 任务统计行 `[copy]` 复制最近一轮

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 回合结束后，用户无法从任务统计行一键复制本轮对话。 |
| 决策 | 在每条 `📊 任务统计：` 行追加 `[copy]` 按钮；点击后复制「上一轮统计行之后（或会话开头）到当前统计行之前」的日志文本，跳过空行与任务结束分隔线。 |
| 变更后行为 | 点击统计行的 `[copy]` → 剪贴板为本轮用户/助手内容；不包含更早回合。 |
| 指针 | `messages.rs` 的 `add_task_stats_block` / `copy_turn_ending_at_stats`；`handlers/mouse.rs` 命中；回归测试 `copy_turn_ending_at_stats_copies_last_turn_only`。 |

## 1. 2026-08-09 — Mermaid 图双击弹窗复制源码

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 成功渲染的 Mermaid 在把 ASCII 图拼进日志时丢掉了 fence 正文，用户无法再取回源码以便编辑。 |
| 决策 | 每次成功渲染保留 `MermaidBlock { start_idx, end_idx, source }`；双击打开 Mermaid 弹窗；弹窗 `y` 复制源码。主区选区 yank 仍为 ASCII。 |
| 变更后行为 | 双击任意 diagram 行 → 源码弹窗（`y` / `j/k` / `Esc`）；失败 Mermaid 仍走 code-card 路径。 |
| 指针 | Spec `docs/superpowers/specs/2026-08-09-mermaid-diagram-copy-popup-design.md`；`finish_stream_code_block`；`popups/mermaid_popup.rs`；回归测试 `log_renders_streamed_mermaid_without_code_card`、`mermaid_popup_copy_uses_source_not_ascii`。 |

## 1. 2026-08-09 — Mermaid 时序图自消息改为 U 形回环

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 自消息（`A->>A`）只画成单格 `<│◀`，看起来像断开的尖括号，而不像指向自己的回环箭头。 |
| 决策 | 将自消息画成两行盒线回环（`│──┐` / `│◀─┘`；末列参与者用左侧 `┌──│` / `└─▶│`），标签放在回环旁。 |
| 变更后行为 | 自调用在生命线上呈现清晰的 U 形折返；末列自消息向左回环，避免画出图外。 |
| 指针 | `crates/tui/src/render/mermaid_sequence.rs`（`self_loop_rows`）；回归测试 `self_message_draws_u_shaped_loop`、`self_message_on_last_participant_loops_left`。 |

## 1. 2026-08-09 — Mermaid 时序图标签不再掉字或错位

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 自有 `sequenceDiagram` 渲染器会丢弃落在生命线上的标签字形（如 `submitTask` → `ubmitTask` / `submi│Task`），且每个 2 列宽 CJK 字形后留下幽灵空格，使标签行比生命线/箭头行更宽——多参与者图上生命线看起来断裂，箭头也像缺段。 |
| 决策 | 在 `label_row` 中把宽字形的续格清空为空 span，并将标签字符绕过已占用的生命线单元格重排，而不是直接跳过。 |
| 变更后行为 | 长 ASCII / CJK 箭头标签保留全部字符（必要时在 `│` 两侧拆开），各行显示宽度一致，生命线保持纵向对齐。 |
| 指针 | `crates/tui/src/render/mermaid_sequence.rs`（`label_row`）；回归测试 `cjk_label_keeps_same_display_width_as_lifeline_row`、`long_ascii_label_is_not_eaten_by_lifelines`、`self_message_keeps_lifeline_intact`。 |

## 1. 2026-08-08 — TUI 使用自有渲染器绘制 Mermaid 时序图

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 上游 `ratatui-markdown` 的时序图渲染器对三类常见输入处理有误：`participant A as 用户` 别名被原样展示；`+`/`-` 激活简写（`A->>+B`）会产生幻影参与者列（`+B`、`-B` 等）；2 列宽的 CJK 箭头描述可能盖住生命线（或被丢弃），导致带描述的箭头看起来对不齐。 |
| 决策 | 在 TUI 中把 `sequenceDiagram` 代码块路由到 Tact 自有的渲染器（`crates/tui/src/render/mermaid_sequence.rs`）；其他 Mermaid 图类型继续使用 `ratatui-markdown`。新渲染器解析 `as` 别名，在参与者查找前去除 `+`/`-` 激活前缀，并按显示列放置标签字形，仅当字形宽度内所有单元格都空闲时才绘制。 |
| 变更后行为 | 只有声明的参与者渲染为列；`A->>+B` 指向参与者 `B`；CJK 标签在生命线之间居中，且不会覆盖 `│`。无法解析的源码仍回退到普通代码渲染。 |
| 指针 | `crates/tui/src/render/mermaid_sequence.rs`；路由：`crates/tui/src/render/render_md.rs`（`render_mermaid_block`）；回归测试位于 `mermaid_sequence.rs`。 |

## 1. 2026-08-08 — Subagent 模型选择器使用自身 provider

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | `/model-subagent` 会把 subagent 配置的模型与主 agent 当前 provider 的 API 模型列表合并；当两者使用不同 provider 时，选择器可能显示错误的模型。 |
| 决策 | 使用已解析的 subagent provider 的 `base_url` 和 `api_key` 查询 `/models`；保留 provider 配置中的 `models = [...]` 作为主要候选，并继续按 `(base_url, api_key)` 缓存。 |
| 变更后行为 | Subagent 选择器只显示属于 subagent provider 的配置模型和 API 发现模型；主 agent 的 `/model` 选择器仍使用主 provider。 |
| 指针 | `crates/tact_llm/src/models.rs`、`crates/tui/src/handlers/select.rs`；回归测试 `explicit_provider_model_query_uses_subagent_credentials`；设计：`docs/superpowers/specs/2026-08-08-subagent-model-picker-provider-design.md`；计划：`docs/superpowers/plans/2026-08-08-subagent-model-picker-provider.md`。 |

## 1. 2026-08-08 — DeepSeek 与 Kimi Responses 保持配置门控

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 通用 Responses adapter 可以为 OpenAI-compatible 端点构造，但 DeepSeek/Kimi 的原生压缩与状态续传尚未验证达到生产契约；若直接允许正常配置，会把未支持的 fallback 行为误认为已支持。 |
| 决策 | 继续在配置解析阶段拒绝 DeepSeek/Kimi 的 `protocol = "responses"`。底层 adapter 构造仍可用于隔离端点测试；生产配置在原生 Responses 能力验证完成前使用 Chat Completions。 |
| 变更后行为 | DeepSeek/Kimi 用户会得到明确的配置错误，不会进入未经验证的 Responses 路径。OpenAI 与明确配置的自定义 OpenAI-compatible provider 保留现有 Responses 路由。 |
| 指针 | `crates/tact/src/config/resolve.rs`；provider 构造：`crates/tact_llm/src/provider.rs`；相关设计：`docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`；压缩行为：第 5 章。 |


## 1. 2026-08-08 — OpenAI Responses 保留未知 wire item

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | typed `async-openai` Responses 枚举会在 Tact 有机会保留新 output item 之前直接拒绝它，使 provider state 无法前向兼容；同时共享的 Chat/Anthropic 请求模型没有明确的 Responses 专用字段扩展边界。 |
| 决策 | 在 typed normalization 之前先解析 raw Responses envelope；已知 item 正常转换，未知 input/output item 作为 raw JSON 保留。增加只由 Responses adapter 消费的 `ResponsesRequestOptions`，并提供保守的 provider capability metadata；只有出现可复现的 SDK 阻塞时才 fork `async-openai`。 |
| 变更后行为 | 无害的未知流事件不再中断响应。未知 output item 可以跨普通/流式 turn、session state 序列化和下一次 Responses 请求保留。Responses 专用请求字段不会出现在 Chat Completions 或 Anthropic payload 中。 |
| 指针 | `crates/tact_llm/src/openai/responses/wire.rs`、`request_options.rs`、`stream.rs`、`provider.rs`；设计：`docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`；计划：`docs/superpowers/plans/2026-08-08-responses-compatibility-foundation.md`；压缩：第 5 章与 `docs/compaction.md`。 |


## 1. 2026-08-08 — 主区域 Markdown 将完整 Mermaid fence 渲染为终端图

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 相关 | `crates/tui/src/render/render_md.rs`、`crates/tui/src/widgets/state/app/agent.rs`、`crates/tui/src/widgets/state/app/visibility.rs`、`crates/tui/src/widgets/state/stream_state.rs`、第 23 章 §6.7 |
| 症状 / 动机 | 所有带显式语言标签的流式 fence——包括 ```mermaid——闭合时都会被提升为 `CodeBlock` card overlay，因此 Mermaid 源码只显示为语法着色的代码，而不是图。 |
| 决策 | 在 `render_md.rs` 中把完整、顶层 `mermaid` fence 路由到共享的 `render_mermaid_block` 辅助函数（`ratatui-markdown::mermaid::render_mermaid` + 应用主题适配器）；在 `stream_state.rs` 中标记当前缓冲的流式 fence 是否为 Mermaid；`agent.rs` / `visibility.rs` 在合法闭合 fence 时直接把 diagram 行拼接进日志，而不是 push `CodeBlock`。无效、不支持或未闭合的 Mermaid 保留 code-card 回退，绝不丢弃源码。 |
| 变更后行为 | 完整的 ```mermaid fence 以日志宽度渲染为带主题的终端图（定宽 Markdown 路径使用名义 80 列）；合法的流式 Mermaid block 闭合后不再创建 code card；无效/不支持/未闭合的 Mermaid 与普通显式语言 fence 仍走原有 code-card 路径；宽度重排与视口滚动沿用现有 log 布局/缓存行为。 |
| 指针 | `render_md.rs`（`render_mermaid_block`、`route_mermaid_fences`）及测试 `render_mermaid_sequence_returns_terminal_lines`、`render_markdown_mermaid_flowchart_uses_box_art`、`render_markdown_invalid_mermaid_falls_back_to_code`、`render_markdown_unclosed_mermaid_fence_keeps_source`；`stream_state.rs`（`code_block_is_mermaid`）；`agent.rs`（`finish_stream_code_block`）、`visibility.rs`（`flush_stream_pending`）；`render_gap_tests.rs` 回归测试（`log_renders_streamed_mermaid_without_code_card`、`log_falls_back_to_code_card_for_invalid_streamed_mermaid`、`flush_renders_streamed_mermaid_without_trailing_newline`、`flush_falls_back_to_code_card_for_unclosed_streamed_mermaid`）、`cells/markdown.rs`（`markdown_cell_renders_mermaid_at_the_requested_width`）；spec `docs/superpowers/specs/2026-08-08-mermaid-main-rendering-design.md`；plan `docs/superpowers/plans/2026-08-08-mermaid-main-rendering.md`；文档 `book/23_chapter_tui*.md` §6.7 |

---


## 1. 2026-08-06 — OpenAI Responses 显示详细 reasoning summary

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 相关 | `crates/tact_llm/src/openai/responses/convert.rs`、Responses reasoning 请求构造 |
| 症状 / 动机 | 普通 OpenAI Responses 请求发送 `reasoning.summary = auto`，因此即使启用了 reasoning，流式 thinking block 也可能只有 provider 自动选择的简短摘要。 |
| 决策 | 保留 Responses API 的 `summary` 字段，但在 Tact 启用 reasoning 时请求 `ReasoningSummary::Detailed`（`"detailed"`）。流式解析无需修改，因为它已经消费 reasoning summary delta。 |
| 变更后行为 | OpenAI Responses 的 thinking block 请求并显示详细 reasoning summary，不再使用自动摘要级别。 |
| 指针 | 请求转换与回归断言：`crates/tact_llm/src/openai/responses/convert.rs`；相关 Responses 适配器：`crates/tact_llm/src/openai/responses/`。 |



| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 相关 | `crates/tui/src/render/log.rs`、`crates/tui/src/render/cells/text.rs`、`crates/tui/src/render/log_render_tests.rs` |
| 症状 / 动机 | 主日志区域先按整个面板内容宽度换行，绘制普通消息时再增加左侧缩进；满行消息因此会在右边界被截掉几列，且选区重绘使用了错误的换行宽度。 |
| 决策 | 缓存换行时预先扣除该消息实际缩进；流式回复使用相同的回复缩进。`TextCell` 的选区换行直接使用扣除缩进后的可用宽度。 |
| 变更后行为 | 主区域满行的普通、嵌套和流式文本会在实际可绘制宽度内换行，右侧字符不再丢失。 |
| 指针 | 日志布局与换行缓存见 `render/log.rs`；文本绘制见 `render/cells/text.rs`；回归测试 `log_full_width_nested_line_wraps_before_indentation_clip`。 |


| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact/src/agent/mod.rs`（`set_thinking_budget` / `set_reasoning_effort`）、`crates/tact/src/config/mod.rs`（`update_llm_model_and_*`、`update_subagent_*`）、`crates/tact/src/config/persist.rs`（TOML 移除）、`crates/tui/src/render/bar.rs`（`format_think_segment`）、第 23 章 §6.6 |
| Symptom / motivation | 使用 OpenAI Chat Completions（effort 语义）时，底栏显示 `think high(32K)`：`high` 是真实的 `reasoning_effort`，而 `32K` 是之前预算语义模型（claude / kimi-for-coding）残留的 `thinking_budget`。预算对 effort 模型毫无意义、也绝不会发到线上，但由于运行时 setter、内存配置更新函数、TOML 持久化路径都不会清掉另一字段，它仍会显示在 effort 旁边。 |
| Decision | 端到端地让 effort 与预算**互斥**：`set_thinking_budget` 清掉 `reasoning_effort`，`set_reasoning_effort` 将 `thinking_budget` 归零；配置更新函数（`update_llm_model_and_thinking_budget` / `update_llm_model_and_reasoning_effort` 及其 subagent 对应项）同样处理；TOML 持久化函数从 provider/subagent 条目中移除相反键。状态栏 `format_think_segment` 改为 effort 优先（有 effort → `think high`，忽略残留预算；只有预算 → `think 32K`），且仅有 effort 时仍会渲染而不是消失。 |
| Behavior after | effort 语义模型显示 `think high`（绝不会是 `think high(32K)`）；预算语义模型显示 `think 32K`。选择 effort 会从 config.toml 移除已存的 `thinking_budget`；选择预算会移除 `reasoning_effort`。旧配置若同时包含两个字段，显示只取 effort，并在下次 `/model` 持久化时自愈。 |
| Pointers | `format_think_segment` 及其测试位于 `crates/tui/src/render/bar.rs`；setter 位于 `crates/tact/src/agent/mod.rs`；配置更新函数位于 `crates/tact/src/config/mod.rs`；TOML 移除及其测试位于 `crates/tact/src/config/persist.rs`；driver 测试 `set_reasoning_effort_clears_stale_thinking_budget`；TUI 测试 `applying_effort_pick_clears_stale_thinking_budget`；文档 `book/23_chapter_tui*.md` §6.6 |

---

## 1. 2026-08-06 — 恢复重试消息附带底层错误

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact/src/recovery.rs`（`error_summary`）、`crates/tact/src/agent/mod.rs`（backoff / compact retry 消息点）、第 6 章恢复消息 |
| Symptom / motivation | `[Recovery] backoff (1/10): retrying in 1.9s` 只说明了*何时*重试，没有说明*为什么*——底层传输错误（超时、连接重置、限流……）完全不可见，用户看到 8 次以上退避却不知道哪里失败。压缩摘要的重试消息（`[compact retry 1/3] retrying in 1.9s`）也有同样的问题。 |
| Decision | 在 `recovery.rs` 新增 `error_summary`：将空白/换行折叠为单行，超过 200 字符以省略号截断。主循环 backoff 消息追加完整 anyhow 链（外层上下文 → 根因，以 `": "` 连接）；两处压缩重试消息追加客户端错误字符串。原有标签、计数与延时文本保持不变，因此匹配 `contains("Recovery") && contains("backoff")` 的测试仍可通过。 |
| Behavior after | 恢复重试会报告原因，例如 `[Recovery] backoff (2/10): retrying in 4.3s — http request failed: error sending request for url`。 |
| Pointers | `error_summary` 及其单元测试位于 `crates/tact/src/recovery.rs`；消息点在 `crates/tact/src/agent/mod.rs`；文档 `book/06_chapter_recovery*.md` 恢复消息一节 |

---

## 1. 2026-08-06 — 未知 provider 名称放行为自定义 OpenAI 兼容 provider

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 相关 | `crates/tact_llm/src/types.rs`（`ProviderKind::Custom`、`FromStr`）、`crates/tact_llm/src/provider.rs`（`build_client`、`model_uses_effort`）、`crates/tact_llm/src/hook_select.rs`（`body_hook_for`）、`crates/tact_llm/src/models.rs`（`is_models_query_supported`）、`crates/tact/src/config/resolve.rs`（`resolve_provider_kind`、`resolve_llm`、`resolve_subagent`）、Ch 21 §3–§4 |
| 症状 / 动机 | `ProviderKind::from_str` 拒绝 `anthropic | openai | deepseek | kimi` 之外的任何名称，因此 `llm.provider = "moonshot"`（或任何自建 / 网关 provider）即使条目里配好了可用的 OpenAI 兼容 `base_url`，也会报 "unknown provider"。配置层无法表达第三方 OpenAI 兼容端点。 |
| 决策 | 为所有非内建名称新增 `ProviderKind::Custom(String)`。自定义 provider 全链路复用 OpenAI 协议：`build_client` 派发到 OpenAI 兼容适配器（默认 `chat_completions`，可选 `responses`），`body_hook_for` 与 `openai` 使用相同的端点启发式，支持 `/v1/models` 补充，并接受 `reasoning_effort`。它们**没有默认 `base_url`**——条目未设置时 resolve 报 "base_url not configured"。内建门禁不变：`responses` 协议仍限 `openai | deepseek | custom`，`reasoning_effort` 限 OpenAI 兼容 provider（即除 anthropic 外全部）；`resolve_llm` 中的 map key 校验循环已删除（自定义 key 不再报错）。`ProviderKind` 失去 `Copy`（现在持有 `String`），方法接收者改为 `&self`。 |
| 变更后行为 | `llm.provider` / `--provider` 接受任意名称。非内建名称按自定义 OpenAI 兼容 provider 处理，必须在 `[llm.providers.<name>]` 中显式配置 `base_url`。缺失活跃条目仍在 resolve 时报错。 |
| 指向 | `ProviderKind` 位于 `crates/tact_llm/src/types.rs`；测试 `provider_kind_from_str_accepts_unknown_as_custom`（tact_llm）、`custom_provider_resolves_with_openai_protocol` / `custom_provider_without_base_url_errors` / `custom_provider_in_map_resolves`（tact config resolve）；文档 `book/21_chapter_config*.md` §3–§4、`config.example.toml` |

---

## 1. 2026-08-06 — 账户轮询每次故障只提示一次，而非每个退避周期都提示

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 关联 | `crates/tact-ui/src/account.rs`（`poll_loop`、`spawn_poller`）、`crates/tui/src/widgets/state/app/agent.rs`（`handle_account_update` flash）、Ch 22 §9 |
| 症状 / 动机 | `spawn_poller` 把每次失败的余额 / 用量查询都转发为 `AccountUpdate::Error`。在持续故障（如断网）时，TUI 每 10 s → 20 s → … → 5 min 弹一次错误提示，永不停歇——变成通知风暴（「骚扰」），掩盖了应用的真实状态。 |
| 决策 | 把循环抽取为可测试的 `poll_loop(query, tx, next_delay)`，并加入 `error_notified` 标志：连续故障期间只转发**第一条**失败；后续重试保持静默，退避继续。一次成功查询会复位标志，因此恢复后的新一轮故障会再次提示一次。`NotSupported` 仍静默终止循环（不变）；启动查询与 `/balance` 命令保留一次性错误上报（用户主动触发，不算骚扰）。 |
| 行为变化 | 一次故障 = 一条 flash 提示，之后静默退避重试直到恢复；恢复后恢复正常 5–15 s 轮询，下一次故障再提示一次。 |
| 指针 | `crates/tact-ui/src/account.rs` 中的 `poll_loop` / `spawn_poller`；测试 `poller_forwards_error_once_per_outage_then_resumes`、`poller_stops_on_not_supported_without_error_flash`；文档 `book/22_chapter_llm*.md` §9 |

---

## 1. 2026-08-06 — Kimi Code 用量查询仅限官方 `https://api.kimi.com/coding` 端点

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tact_llm/src/account.rs`（`query_kimi_code_usage`、`kimi_usage_url_from_base_url`）、`crates/tact_llm/src/provider.rs`（`is_kimi_usage_supported`、`is_account_query_supported`）、`crates/tact-ui/src/account.rs`（`query_once`）、Ch 22 §3 / §9 |
| 症状 / 动机 | `kimi_usage_url_from_base_url` 从任意配置的 base URL 推导 `{origin}/v1/usages`，导致 `kimi-for-coding` 模型挂在自定义 OpenAI 兼容代理后时，会把代理的 API key 发往猜测出来的用量端点。`is_kimi_usage_supported` 此前等于 `is_kimi_coding(&model)`——任何提供 `kimi-for-coding` 的代理都返回 true，因此 TUI 配额组件也会轮询代理。 |
| 决策 | 与 DeepSeek / Kimi 余额的「凭据边界」对齐：`kimi_usage_url_from_base_url` 仅在 HTTPS 且主机精确为 `api.kimi.com`、路径含 `/coding`（允许 `/v1` 后缀）时返回官方 URL；其余返回 `None`，`query_kimi_code_usage` 报错「Kimi Code usage API is only available for the official endpoint https://api.kimi.com/coding」。`ProviderInfo::is_kimi_usage_supported` 要求同样的官方主机 / 路径，因此代理配置下 `is_account_query_supported` 为 false，TUI 隐藏配额组件。`is_kimi_coding` 本身不变——它仍用于识别 Kimi Code 平台（含代理）以决定 wire shape。 |
| 行为变化 | Kimi Code 用量轮询仅在 `base_url` 指向官方 `https://api.kimi.com/coding` 端点时可用。自定义代理（即使 model 是 `kimi-for-coding`）视为不支持；代理配置的 API key 永远不会发送到 `api.kimi.com`。 |
| 指针 | `crates/tact_llm/src/account.rs` 中的 `kimi_usage_url_from_base_url` + 测试 `kimi_usage_url_derivation`；`crates/tact_llm/src/provider.rs` 中的 `is_kimi_usage_supported` + 测试 `is_kimi_usage_supported_only_for_official_endpoint`；文档 `book/22_chapter_llm*.md` §3 / §9 |

---

## 1. 2026-08-06 — DeepSeek 余额查询仅限官方 `https://api.deepseek.com` 端点

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tact_llm/src/account.rs`（`query_deepseek_balance`、`deepseek_balance_url_from_base_url`）、`crates/tact_llm/src/provider.rs`（`is_deepseek_balance_supported`、`is_account_query_supported`）、`crates/tact-ui/src/account.rs`（`query_once`）、Ch 22 §3 / §9 |
| 症状 / 动机 | `query_deepseek_balance` 从任意配置的 base URL 推导 `{origin}/user/balance`，导致 DeepSeek 模型挂在自定义 OpenAI 兼容代理后时，会把代理的 API key 发往猜测出来的余额端点。DeepSeek 只在官方主机提供 `GET /user/balance`；该回退逻辑错误，可能把凭据泄露到错误主机或产生令人困惑的 404/403。 |
| 决策 | 与 Kimi 的「凭据边界」对齐：`deepseek_balance_url_from_base_url` 仅在 base URL 为空（配置默认）或为 HTTPS 且主机精确为 `api.deepseek.com`（允许 `/v1` 后缀）时返回官方 URL；其余返回 `None`，`query_deepseek_balance` 报错「DeepSeek balance API is only available for the official endpoint https://api.deepseek.com」。`ProviderInfo::is_deepseek_balance_supported` 门控 `is_account_query_supported`，因此代理配置下 TUI 底栏余额组件直接隐藏而非反复报错。 |
| 行为变化 | DeepSeek 余额轮询 / `/balance` 仅在 `base_url` 指向官方端点时可用。自定义代理（即使 model 是 `deepseek-*`）视为不支持；代理配置的 API key 永远不会发送到 `api.deepseek.com`。 |
| 指针 | `crates/tact_llm/src/account.rs` 中的 `deepseek_balance_url_from_base_url` + 测试 `deepseek_balance_url_derivation`；`crates/tact_llm/src/provider.rs` 中的 `is_deepseek_balance_supported` + 测试 `is_deepseek_balance_supported_only_for_official_endpoint`；文档 `book/22_chapter_llm*.md` §3 / §9 |

---

## 1. 2026-08-06 — `/tasks-dag` 弹窗打开期间新建的任务不显示

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tui/src/widgets/state/app/agent.rs`（`on_tasks_changed`）、`crates/tui/src/render/popups/task_dag_popup.rs`、Ch 23（TUI） |
| 症状 / 动机 | `/tasks-dag` 弹窗打开时渲染一次 Mermaid 行。`TasksChanged` 更新只刷新 `task_panel.snapshot`，从不刷新弹窗内容；渲染循环只在弹窗宽度变化时重渲染——因此弹窗打开期间新建的任务（或在打开与渲染之间加入的任务）在关闭重开前永远不会显示。 |
| 决策 | `on_tasks_changed` 现在会刷新已打开的 DAG 弹窗：用最新快照在弹窗当前 `render_width`（首帧宽度感知前回退到 `DEFAULT_DAG_RENDER_WIDTH`）重新执行 `render_task_dag_lines`，原地替换 `lines`/`mermaid_source`，保持滚动偏移。与 `render_task_dag_popup` 中既有的宽度变化重渲染互补，两条路径均幂等。 |
| 行为变化 | 弹窗打开期间新建的任务会在下一渲染帧立即出现，无需关闭弹窗。 |
| 指针 | `on_tasks_changed` 位于 `crates/tui/src/widgets/state/app/agent.rs`；回归测试 `tasks_dag_popup_refreshes_when_new_tasks_arrive` |

---

## 1. 2026-08-06 — `/tasks-dag` 依赖边缺失（任务存储不对称）

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tact/src/task/mod.rs`（`update`、`clear_dependency`）、`crates/tui/src/widgets/state/task_dag.rs`、Ch 23（TUI） |
| 症状 / 动机 | `/tasks-dag` 只显示任务节点，**不显示通过 `task_update` 的 `addBlockedBy` 建立的依赖箭头**：`update` 会把 `addBlocks` 镜像到被阻塞任务的 `blocked_by`，但 `addBlockedBy` 从不镜像 blocker 的 `blocks`（DAG 出边）。`tasks_to_mermaid` 只从 `blocks` 画边，因此这类依赖完全不可见。另外，`clear_dependency`（任务完成时）只把已完成 id 从他人 `blocked_by` 移除，却留下已完成任务自己的 `blocks`，成为幽灵边来源。 |
| 决策 | `update` 现在双向镜像：`add_blocked_by` 也会把当前任务 id 写入每个 blocker 的 `blocks`（去重排序），与既有 `add_blocks` 分支对称。`clear_dependency` 额外清空已完成任务的 `blocks`；由于 `update` 持有的是 `clear_dependency` 之前取的副本，本地副本也同步清空，避免最终写回复活幽灵边。 |
| 行为变化 | 无论通过 `addBlocks` 还是 `addBlockedBy` 建立的依赖，都会在 `/tasks-dag` 中渲染为 `T{blocker} --> T{blocked}`。完成任务后其出边被移除。 |
| 指针 | `crates/tact/src/task/mod.rs`（`update` 的 add_blocked_by 分支、`clear_dependency`）；测试 `update_add_blocked_by_creates_reverse_outgoing_edge`、`completing_task_clears_blocked_by` |

---

## 1. 2026-08-06 — 任务完成后显示统计块（耗时 · 模型 · tokens）

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 关联 | `crates/tui/src/widgets/state/app/messages.rs`（`add_task_stats_block`）、`crates/tui/src/widgets/state/app/agent.rs`（`TaskComplete` 分支）、Ch 23（TUI） |
| 症状 / 动机 | 任务结束后日志区只有任务结束分隔线（耗时标签），token 消耗与模型名仅显示在底部状态栏，且下一个任务开始时会被重置。`TaskComplete` 分支中留有 `// TODO Add task stats block` 标记。 |
| 决策 | `TaskComplete` 分支在 `add_task_end_separator()` 之后调用 `add_task_stats_block()`。统计块直接读取已冻结的状态——`last_prompt_elapsed_secs`（由分隔线设置）、`status_bar.model_name`（来自 `ModelInfo`）与 `status_bar.token_*`（来自 `TokenUsage`）——因此不新增统计结构体或重复收集（YAGNI）。通过现有 `add_system_message` 路径渲染一行 markdown：`📊 任务统计：⏱ mm:ss · 🧠 model · N tokens (prompt X · completion Y · cache Z · reasoning W)`；无模型或零 token 时省略相应片段。 |
| 行为变化 | 每个完成的任务都会在结束分隔线下方留下一行持久统计：耗时、模型名以及可用的 token 明细。取消或失败的任务不显示统计块。 |
| 指针 | `add_task_stats_block` 位于 `crates/tui/src/widgets/state/app/messages.rs`；测试 `task_complete_appends_task_stats_block`、`task_stats_block_skips_empty_parts` 位于 `crates/tui/src/widgets/state/app/agent.rs` |

---

## 1. 2026-08-06 — `/tasks-dag` 改用 ratatui-markdown 渲染 Mermaid（替换 meraid）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 相关 | `crates/tui/src/widgets/state/task_dag.rs`、`crates/tui/src/render/popups/task_dag_popup.rs`、`crates/tui/src/theme.rs`、根 `Cargo.toml`（`ratatui-markdown` git 依赖）、Ch 23（TUI） |
| 现象 / 动机 | DAG 弹窗原先用 `meraid` crate 的 Mono 渲染器（纯文本、无主题、无 markdown 结构）；而 workspace 里已有一个闲置的 `ratatui-markdown` git 依赖，且其分支名 `update-ratatui-0.30` 已失效（真实分支是 `chore/update-ratatui-0.30`），该依赖根本无法解析。 |
| 决策 | `/tasks-dag` 改走 `ratatui-markdown`：`tasks_to_mermaid` 仍生成 `flowchart TD`，`render_task_dag_lines` 将其包装为 `## Tasks DAG` + ` ```mermaid ` 代码块 + `### Legend` 列表（把 `#id` 映射回各任务 subject；节点标签保持窄：状态字形 `○`/`◐`/`✓` + `#id`，因为 fork 的 mermaid 语法遇到第一个 `]` 就结束 `[...]` 文本）。新增 `DagTheme` 适配器把应用 `Theme` 映射为 `RichTextTheme`/`MermaidTheme`（明暗主题由 `MermaidTheme::for_background` 决定），弹窗在首帧按真实宽度重渲染（`render_width` 缓存）。根依赖修正为正确分支并精简为 `default-features = false, features = ["markdown", "mermaid"]`（去掉 `image`/`scroll`/`tree`/`viewer`）。 |
| 改后行为 | 弹窗显示带主题的框线流程图 + 列出任务 subject 的图例；`y` 复制快捷键仍复制原始 Mermaid 源码。`meraid` 不再是 tui crate 的依赖。 |
| 指针 | `crates/tui/src/widgets/state/task_dag.rs`（`tasks_to_mermaid`、`render_task_dag_lines`、`DagTheme`）、`crates/tui/src/render/popups/task_dag_popup.rs`（按宽度重渲染）、测试 `tasks_dag_popup_renders_mermaid_markdown`、`ratatui_markdown_renders_diagram_and_legend` |

---

## 1. 2026-08-06 — 压缩摘要 MaxTokens 截断后自动续写

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 相关 | Ch 5 §3（摘要调用）、Ch 5 §4（校验）、`crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode`）、`crates/tact/src/recovery.rs`（`MAX_CONTINUATION_ATTEMPTS`、`continuation_message`） |
| 症状 / 动机 | 压缩摘要的 LLM 调用返回 `MaxTokens`（输出上限）时，摘要循环把它当作非法 stop reason 直接报错 `compaction summary ended with invalid stop reason: MaxTokens`，压缩整体失败——尽管部分摘要完全可用。对推理模型而言，摘要经常撞上输出预算。 |
| 决策 | 摘要调用现在有两条独立的恢复轴：瞬时传输错误仍按有界退避重试；`MaxTokens` 截断则把已产生的部分摘要作为 assistant 消息、追加一条续写提示（与主循环相同的 `continuation_message` 选择器：第 1 次用直接续写提示，之后用收敛提示）再次调用，最多 `MAX_CONTINUATION_ATTEMPTS`（3）次。每次尝试按增长的消息历史重建请求：`[User(摘要提示), Assistant(部分摘要), User(继续), …]`。续写次数耗尽后，部分摘要被接受为 best-effort 而不是报错（Codex-style 重建反正会保留最近的真实用户消息）；`MaxTokens` 因此不再是"非法 stop reason"。 |
| 变更后行为 | 截断的摘要会发出 `[compact continue n/3] summary truncated, continuing` Info，并把所有部分块合并进最终摘要，压缩成功而不是报错。拒绝 / 其它异常终止原因和空文本仍会失败且不替换旧 context。 |
| 指针 | `crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode` 摘要循环、stop reason 校验），测试 `local_compact_continues_truncated_summary`、`local_compact_continues_through_multiple_truncations`、`local_compact_accepts_partial_summary_when_continuations_exhausted`、`crates/tact-ui/tests/recovery_compaction.rs`（`compact_summary_continues_truncated_response`），Ch 5 §3/§4 |

---

## 1. 2026-08-06 — 首次运行自动生成默认配置 ~/.tact/config.toml

| 字段 | 值 |
|------|-----|
| 类型 | `feature` |
| 相关 | Ch 21 §3（配置来源与优先级）、`config.example.toml`、`crates/tact/src/config/load.rs` |
| 症状/动机 | 安装脚本只装二进制，不带配置。首次启动无配置文件时必然在 resolve 阶段报 "LLM provider not configured" 退出，用户只能靠文档才知道要手动复制 `config.example.toml`——运行时没有任何提示。 |
| 决策 | `load_toml_config` 在所有搜索路径都找不到配置时，向 `~/.tact/config.toml` 写入默认模板并解析返回。模板通过 `include_str!("../../../../config.example.toml")` 在编译期嵌入，首启默认值永远与仓库内 example 同步（当前为 `deepseek` + `protocol = "chat_completions"`）。只有用户全局位置会被自动创建；项目级候选（`./.tact/config.toml`、`./config.toml`）从不写入，避免污染仓库。打印提示：`[config] no config found; wrote default template to ... — edit it to add your API key`。 |
| 改后行为 | 首次运行且任何位置都没有配置：tact-ui 创建 `~/.tact/config.toml`（模板，`api_key` 为占位符）、打印编辑提示，随后仍在占位符 key 处按原样 resolve 报错——用户编辑文件后再次启动即可。已有配置永不被触碰或覆盖。显式 `--config /path` 不存在时仍报错（不会自动创建）。写入失败（HOME 未知/不可写）回退到原先的空默认行为。 |
| 指针 | `crates/tact/src/config/load.rs`（`DEFAULT_CONFIG_TEMPLATE`、`write_default_config`、`load_toml_config`）、`config.example.toml`（头部注释）、Ch 21 §3 |

---

## 1. 2026-08-06 — Session 统计新增 RTK 输出过滤器指标

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 相关 | `docs/token_usage_schema.md`（Session Stats Display）、`crates/tact/src/hook/rtk_filter.rs`、`crates/tact/src/stats.rs` |
| 症状/动机 | 开启 `tools.rtk_filter = true` 后，bash 输出会经 `rtk pipe` 压缩，但没有任何指标说明过滤器是否真的生效、删掉了多少输出、花了多久——用户无法判断 RTK 是在省 token 还是悄悄原样透传。 |
| 决策 | `SessionStats` 新增六个 relaxed 原子计数器（`rtk_calls`、`rtk_success_calls`、`rtk_failure_calls`、`rtk_saved_chars`、`rtk_input_chars`、`rtk_elapsed_ms`），由 post-tool 钩子直接累加。必须用原子量（而非普通 `u64`），因为钩子只能拿到 `&Agent`（不可变）。只有 `rtk pipe` 以 0 退出且 stdout 非空才算一次成功；节省字符数仅在成功时按 `raw_len − filtered_len`（饱和计算，按字符而非字节）累加。`rtk_input_chars` 累计每次尝试的原始长度（成功与失败都计），使会话级节省率能把失败尝试按零节省计入。会话结束摘要新增 `RTK tokens saved` 估算（1 token ≈ 4 字符的长度启发式）与 `RTK savings rate` 行（节省字符 / 输入字符）。 |
| 改后行为 | 只要记录到至少一次 RTK 尝试，会话统计弹窗 / 退出摘要即显示 `RTK calls (s/f)`、`RTK chars saved`、`RTK tokens saved`（chars/4）、`RTK savings rate`（saved/input %）与 `RTK time`。未开启 `rtk_filter` 或没有 bash 输出被过滤时，这些行完全不显示。失败的 bash 执行（`StepStatus::Failed`）既不参与过滤也不计入 RTK 统计——其输出完整进入 LLM 上下文。 |
| 指针 | `crates/tact/src/stats.rs`（`SessionStats::record_rtk`、`summary()` 中的 RTK 行）、`crates/tact/src/hook/rtk_filter.rs`（`pipe_through_rtk` → `(output, succeeded, elapsed_ms)`、`saved_chars`、`should_filter`、`create_rtk_post_tool_hook`）、`crates/tact/src/hook/mod.rs`（`PostToolUseFn` 现接收 `StepStatus`）、`docs/token_usage_schema.md` |

---

## 1. 2026-08-05 — 统一工具族卡片文案（background + team）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 相关 | Ch 7、Ch 26（2026-07-28 条目：工具卡片标签区分） |
| 症状/动机 | 两个工具族文案仍不统一：`background_run`（`⚙️ Background Run`）vs `check_background`（`🔍 Check`）——孤零零的 `Check` 看不出在检查什么；team 协作工具 `send_message` / `broadcast` / `read_inbox` / `plan_approval`（`✉️ Send` / `📢 Broadcast` / `📬 Inbox` / `✅ Approve`）不带 `Team` 前缀，而 `spawn_teammate` / `list_teammates`（`👥 Team Spawn` / `👥 Team List`）带。 |
| 决策 | Background 族：`check_background` 改为 `⚙️ Background Check`，与 `background_run` 共用 `⚙️ Background` 前缀。Team 族：四个协作工具补上 `Team` 族名，保留各自图标（`✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`）。两族的 TUI `tool_display_name` fallback 同步为与 metadata 一致。Task 保持 `format_task_tool_title` 的 `# Task…` 人类标题。 |
| 改后行为 | 每个工具族呈现为同一族：`⚙️ Background Run` / `⚙️ Background Check`；`👥 Team Spawn` / `👥 Team List` / `✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`；`⏰ Cron …`；`🌿 Worktree …`；`🔌 Shutdown …`。 |
| 指针 | `crates/tact/src/tool/background_run.rs`（`CHECK_BACKGROUND_METADATA`）；`crates/tact/src/tool/team.rs`；`crates/tui/src/widgets/tool_widget.rs`（`tool_display_name`） |

---

## 1. 2026-08-05 — `/model` 按 provider 分流 budget/effort + model→档位映射 + effort/model per-agent

| 字段 | 值 |
|------|-----|
| 类型 | `feature` |
| 相关 | `docs/superpowers/specs/2026-08-05-llm-presets-design.md`、Ch 21（配置：`[llm.model_profiles]`、`reasoning_effort` 校验）、Ch 22（§2 ProviderInfo 静态化、§6.3 wire 表） |
| 症状/动机 | `/model` 对任何 provider 都弹同一 5 档 thinking budget，但 openai/deepseek/kimi-k3 实际发的是 `reasoning_effort`；effort 无选择入口、运行时不可改；effort 是进程全局共享（subagent 会污染主 agent）；OpenAI Responses 的 effort 是 client 构建时 snapshot，运行时修改不生效；"模型↔档位"没有静态配置表达。 |
| 决策 | 1) `/model`（及 `/model-subagent`）第二步按 `model_uses_effort` 分流：openai/deepseek/kimi k3/k3-256k → effort 选择器（deepseek 3 档 low/high/max、kimi k3 3 档、openai 6 档 minimal..max，无 none 档）；anthropic/kimi coding 系 → budget 选择器。2) 新增 `[llm.model_profiles."<model>"]`（`thinking_budgets` / `reasoning_efforts` 数组）限定第二步档位，TOML 逐字段覆盖内置 `builtin_model_profiles()`。3) **effort/model per-agent**：`CreateMessageParams.reasoning_effort` + `AgentSettings.model/reasoning_effort`；删除全局 `set_model`、`ProviderInfo.reasoning_effort`、`current_reasoning_effort_from_budget` 及 budget→effort 波段映射（不做存量兼容）；`/model` 改发 `UserCommand::SetModel` / `SetReasoningEffort`（busy 排队）。4) wire 注入全部从 request 读；DeepSeek 纯 effort 驱动（None=不传，默认 ON+high，按官方文档）；Kimi k3 支持 effort（None=默认 high；不提供关闭 thinking——会路由到 K2.6）；OpenAI Responses `create_response` 不再 snapshot effort。5) 持久化：effort 语义写 provider/subagent 的 `reasoning_effort` 字段（`[llm.model_profiles]` 是静态选项集合，不被持久化触碰）；resolve 校验放宽为 openai/deepseek/kimi。 |
| 行为变化 | `/model` 选 openai/deepseek/kimi-k3 模型 → effort 选择器（映射档位或 provider 默认）；选 anthropic/kimi-coding 模型 → budget 选择器（现状）。运行中改 model/effort 只影响当前 agent（主/subagent 独立），wire 立即跟随；Responses 也跟随。持久化后重启生效。Kimi 关闭 thinking 会被路由到 K2.6，本期不提供该 UI 入口。 |
| 指针 | `crates/tui/src/handlers/select.rs`（分流/选择器）、`crates/tact/src/config/{types,resolve,persist,mod}.rs`（model_profiles/校验/持久化）、`crates/tact_llm/src/{provider,deepseek,kimi,openai/*}.rs`（per-request effort/wire）、`crates/tact/src/agent/mod.rs`（SetModel/SetReasoningEffort）、Ch 21/22。 |

---

## 2. 2026-08-04 — `tact upgrade` 自升级命令

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 相关 | README（CLI / 自升级）、第 21 章（配置：`install_without_llm` 路径） |
| 现象 / 动机 | 用户没有就地升级到新版本的方式；升级只能手动重跑 `scripts/install.sh`（或重新源码构建）。 |
| 决策 | 新增 `tact upgrade` CLI 子命令。它会扫描 GitHub release 列表（`GET /repos/{repo}/releases?per_page=100`），找到最新一个非 draft、非 prerelease、且资产里包含当前平台 `tact-ui-v<ver>-<triple>.tar.gz` 的 release——跳过没有构建资产的 tag（例如 `v1.1.1` 最初发布时资产为 0）——下载归档，对照该 release 发布的 `SHA256SUMS` 校验，然后在 Unix 上原子替换正在运行的二进制。参数：`--check`（只打印）、`--yes`（跳过 y/N 确认）、`--repo owner/name` 或 `TACT_UPGRADE_REPO`（跟踪 fork）。Windows 暂不支持就地升级，命令会引导用户重跑 `scripts/install.ps1`。该命令经 `install_without_llm` 解析配置，无需配置任何 LLM provider。 |
| 改后行为 | `tact-ui upgrade --check` 打印当前版本与最新可安装版本；`tact-ui upgrade` 提示确认后（`y` 或 `--yes`）下载、校验 SHA-256 并替换可执行文件，最后提示重启。校验和不匹配会在替换前中止。 |
| 指针 | `crates/tact/src/upgrade.rs`（`run_upgrade`、`find_latest_release_with_asset`、`replace_current_binary`）、`crates/tact/src/config/cli.rs`（`CliCommand::Upgrade`）、`crates/tact-ui/src/main.rs`（分发）、README §3 Run |

---

## 1. 2026-08-04 — Google 语音转写遵循标准代理环境变量

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 21、23 章 |
| 现象 / 动机 | Google Speech-to-Text 客户端通过 `reqwest::ClientBuilder::no_proxy()` 构建。在只能经 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 访问 `speech.googleapis.com` 的网络中，录音虽能完成，转写请求却会绕过已配置代理，最终超时或连接失败。 |
| 决策 | 移除 Google 客户端强制绕过代理的设置，改为使用 reqwest 的标准代理环境解析。继续保持 API key 安全的错误输出：由于 Google API key 目前位于查询参数中，连接错误不会连同可能含完整请求 URL 的底层错误一起展示。 |
| 改后行为 | Google 语音转写会遵循进程代理环境，包括对应的小写变量和 `NO_PROXY`；未配置代理时仍直接连接。子进程回归测试会验证请求抵达已配置的 HTTP 代理，而不是原始主机。 |
| 指针 | `crates/tact/src/voice/transcriber.rs`（`GoogleTranscriber::new`、`google_transcriber_honors_http_proxy`）；第 21 章（语音配置）、第 23 章（TUI 语音流程） |

---

## 1. 2026-08-02 — pre-push 钩子不再把 `GIT_DIR` / `GIT_WORK_TREE` 泄漏给 `cargo test`

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | `scripts/check-rust.sh`、`.githooks/pre-push`、`crates/tact-ui/tests/subsystem_tools.rs` |
| 现象 / 动机 | Git 会把 `GIT_DIR` / `GIT_WORK_TREE` 导出给钩子进程。pre-push 钩子运行 `cargo test -p tact-ui`，其中 `worktree_create_lists_and_shows_status` 测试派生的 `git` 命令继承了这些变量。结果 git 命令没有操作测试的隔离临时仓库，而是指向了真实仓库：测试的 `git worktree add` 注册了一个多余 worktree，其 setup 的 `git init` / `git add` / `git commit` 把破坏性的 `init` 提交追加到当前分支 HEAD——于是 `git push` 可能推送一个刚被自己 pre-push 测试污染过的仓库。 |
| 决策 | 在两个钩子入口、任何子进程运行之前清除钩子注入的变量：在 `.githooks/pre-push` 与 `scripts/check-rust.sh` 顶部 `unset GIT_DIR GIT_WORK_TREE`。同时把 `core.hooksPath` 重新指向 `.githooks`，确保 git 使用受版本控制的钩子（之前安装的 `.git/hooks/pre-push` 是过期的内联副本）。 |
| 改后行为 | `git push` 在干净的 git 环境中运行 fmt/clippy/build/test；集成测试只在 `tact-tool-test-*` 临时目录内创建 worktree 与提交，绝不动真实仓库。 |
| 指针 | `.githooks/pre-push`；`scripts/check-rust.sh`；`scripts/install-git-hooks.sh`；`crates/tact/src/worktree/mod.rs`（仍基于 `current_dir`，钩子不得泄漏环境变量）；`crates/tact-ui/tests/subsystem_tools.rs` |

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
| 改后行为 | 工具卡片标题一眼可区分动作。`background_run` / `check_background` 的 fallback 与 metadata 对齐（`⚙️ Background Run` / `⚙️ Background Check`）。 |
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

**决策：** 可读 tool 标题（`# Task.N · …`）；sticky 默认展开为 `blocks` 树并带 `#id`；`/tasks-dag` 弹窗渲染 Mermaid DAG（节点仅状态+id；2026-08-06 起改用 ratatui-markdown 渲染）。`TaskSnapshot` 携带 `blocks`/`blocked_by`。Log **不再**追加任务系统卡（进度看 sticky + tool 行）。

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
