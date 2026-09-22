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

## 1. 2026-09-22 — 桌面转录渲染切换到 gpui-ai 组件

| 字段 | 值 |
|-------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/transcript.rs`；`crates/tact-gui/src/shell.rs`；`crates/tact-gui/src/pane.rs`；`crates/tact-gui/tests/shell.rs` |

**现象 / 动机：** 桌面转录长期自带 Markdown、thinking、tool、disclosure 和 loading 渲染。它们重复实现了 `gpui-ai` 已经负责的 motion 与状态约定，导致卡片展开、流式输出和加载态容易与上游组件行为漂移。

**决策：** 助手正文改用 `gpui_ai::streaming_text::StreamingText`，reasoning 改用 `gpui_ai::thinking::Thinking`，工具卡片改用 `gpui_ai::tool_call::ToolCall`，模型列表与 subagent transcript 加载改用 `gpui_ai::loading::LoadingState`。Tact 只保留产品级组合：write row 的可点击 diff badge，并使用 gpui-ai 的 `ToolCall::output_max_height` hook，只把展开后的 output body 限制在 190px 滚动窗口内。

**改后行为：** 助手输出、reasoning 和工具活动都由 gpui-ai 的 streaming / disclosure 生命周期驱动。连续工具调用合并为一个 gpui-ai `ToolGroup`，等待中的 permission approval 使用 gpui-ai `ApprovalCard` 直接嵌套在该工具块下方，不再作为独立的尾部卡片漂浮。单选的 `ask_user` 使用 gpui-ai `QuestionFlow`；多选 Ask 保留特殊的 Confirm/Cancel 表单。工具输出格式可选（`Markdown` 或 `Plain`）；Tact 对命令输出选择 `Plain`，换行和缩进都会保留，同时避免 Markdown code block 的额外缩进。工具 header 保持固定，output body 在自己的 190px 窗口内滚动，并带始终可见的滚动条；write row 仍显示可点击 diff badge。Provider 与 subagent 加载态使用 gpui-ai 像素网格 loader。该分支不再保留 Tact 自带的 Markdown block renderer 或 Mermaid fence renderer。

**指针：** `crates/tact-gui/src/transcript.rs`（`StreamingText`、`Thinking`、`ToolCall::output_max_height`、diff badge overlay）；`crates/tact-gui/src/shell.rs`（model picker 中的 `LoadingState`）；`crates/tact-gui/src/pane.rs`（subagent transcript 的 `LoadingState`）；`laohanlinux/gpui-ai` commit `011b585`

## 1. 2026-09-22 — 桌面转录支持 Mermaid 图表渲染

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/Cargo.toml`；`crates/tact-gui/src/transcript.rs`（`mermaid_plain_text`、`render_code_block`） |

**现象 / 动机：** 助手和 reasoning Markdown 中可能包含 ````mermaid` fence，但桌面 GUI 会把它当普通代码块显示，因此 flowchart、state diagram、sequence diagram 都会直接露出 Mermaid 源码；TUI 已经有同一语法的渲染器。

**决策：** 在现有 code-card renderer 中识别 `mermaid` 语言，并用 `mermaid-text` 的 Unicode Mermaid renderer 以 100 列宽度渲染。卡片仍保留原始 source 供 `Copy` 使用；有效图表用 box-drawing 输出替换正文；该 renderer 支持 `<br/>`、edge label、CJK 和 sequence alias。解析失败则回退到源码，避免丢失内容。

**改后行为：** 助手输出、reasoning 以及其他 Markdown 区域的 Mermaid fence 会在桌面客户端渲染为等宽图表。无效 Mermaid 仍按普通代码块显示。

**指针：** `crates/tact-gui/src/transcript.rs`（`mermaid_plain_text`、`render_code_block`）；`crates/tact-gui/Cargo.toml`

## 1. 2026-09-22 — 桌面交互修正：实时预览、菜单、项目与完成态

| 字段 | 值 |
|-------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/session.rs`；`crates/tact-gui/src/transcript.rs`；`crates/tact-gui/src/shell.rs`；`crates/tact-gui/tests/shell.rs` |

**现象 / 动机：** 多处桌面交互仍像原型而不是工具：展开卡片会把视口拉到转录尾部；Thinking 和工具实时输出没有实用的预览上限；Task complete 重复显示助手回答而不是统计信息；已答复授权框继续留在转录里；session 操作只能从标题栏 chip 进入；项目切换也没有 composer 级别、会重新绑定 session 的入口。

**决策：** 把用户主动展开卡片视为阅读，而不是新输出：重测前先关闭 tail-follow。Thinking 和运行中的工具输出显示 3 行实时预览，结束后自动收起；手动展开最多 10 行并可内部滚动。Task complete 改为显示轮次、耗时、context 百分比和紧凑 token 总数。已答复授权从转录中移除而不是补一行记录。每个 session 行自带右键菜单：重命名、复制、置顶、归档、在文件系统中显示；composer 底部新增当前项目行与 `Open project…`，切换后会恢复或新建该项目目录绑定的 session。

**改后行为：** 展开卡片不再跳到转录末尾。Thinking/工具实时内容保持紧凑并自动收起；展开详情在内部滚动而非无限变高。完成行显示 task stats（含耗时），不再重复回答。授权选择后卡片消失。session 行右键即可操作；composer 的项目行提供绑定当前 session 与目录的可见入口。

**指针：** `crates/tact-gui/src/shell.rs`（`toggle_row`、`open_project`、`session_context_menu`、`prompt_composer`、`answer`）；`crates/tact-gui/src/session.rs`（`task_complete_text`、Thinking/tool lifecycle）；`crates/tact-gui/src/transcript.rs`（实时预览上限）；`crates/tact-gui/tests/shell.rs`（`right_clicking_a_session_row_opens_its_context_menu`、`the_model_picker_filters_a_long_list`）

## 1. 2026-09-22 — 暂时隐藏 Thinking budget，把空间让给模型列表

| 字段 | 值 |
|-------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/shell.rs`（`THINKING_BUDGET_UI_ENABLED`、model picker content）；`crates/tact-gui/tests/shell.rs` |

**现象 / 动机：** model picker 同时承担很长的服务端模型列表和五档 Thinking budget。真实模型列表下，budget 区块占用 popover 的固定高度，使模型滚动区域变得过短。

**决策：** 暂时隐藏 Thinking budget 行，把模型列表的滚动视口提高到 21 rem。budget 命令、持久化和渲染分支仍保留在源码中，由 `THINKING_BUDGET_UI_ENABLED = false` 控制；以后恢复控件不需要重新处理协议层。

**改后行为：** Model popover 显示搜索框和更高的可滚动模型列表，不再显示 Thinking budget。长模型列表在专用区域内滚动；把常量打开即可恢复 budget 选择器。

**指针：** `crates/tact-gui/src/shell.rs`（`THINKING_BUDGET_UI_ENABLED`、model picker content）；`crates/tact-gui/tests/shell.rs`（`the_model_picker_filters_a_long_list`、`every_entry_point_answers_a_click`）

## 1. 2026-09-22 — Model picker 在打开期间实时更新，并可搜索完整列表

| 字段 | 值 |
|-------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`model_filter`、`fetch_model_options`、`prompt_composer`） |

**现象 / 动机：** model popover 的 content closure 捕获了打开那帧的 `model_options` 快照。因此第一次点击只会看到拉取前的空状态，即使 HTTP 响应已经回来，面板也可能保持为空。真实服务端返回几十个 id 后，用户还要在很长且没有搜索的列表里滚动，才能找到 DeepSeek 这类靠后的模型。

**决策：** popover content 不再使用单帧快照，而是从 live `TactApp` 读取模型列表、loading 状态、当前模型和 budget。shell 新增 `model_filter` `InputState`；每次打开 popover 时清空并聚焦，输入时按 model id 大小写不敏感过滤。当前模型排在最前，其余保持服务端顺序；面板仍限制为 24 rem 并带滚动条。

**改后行为：** 第一次点击 Model 就立即显示 `Refreshing from provider…`；服务端返回后列表会在已打开的面板内填充。输入 `deepseek` 即可显示位于列表后部的 DeepSeek 系列；清空搜索恢复完整列表；选择模型或按 Escape 会关闭 picker，当前模型保持可见并选中。

**指针：** `crates/tact-gui/src/shell.rs`（`model_filter`、`fetch_model_options`、`prompt_composer`）；`crates/tact-gui/tests/shell.rs`（`the_model_picker_filters_a_long_list`）

## 1. 2026-09-22 — 重新打开桌面应用时恢复最近一次聊天

| 字段 | 值 |
|-------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`TactApp::connect`、`startup_resume_id`） |

**现象 / 动机：** 桌面应用每次启动都调用 `SessionRuntime::start(SessionOptions::new(workdir))`，这会无条件分配一个新的 UUID。即使 store 已有历史会话，侧栏也会在每次启动时多出一个新 session。

**决策：** 启动 runtime 前先读取 workspace 的 recent sessions。如果存在未归档 session，就恢复 `updated_at_unix` 最新的那个；否则才新建 session。这里刻意按活动时间而不是列表顺序选择，因为侧栏的 pinned-first 排序是导航策略，不代表 pinned 行就是最后使用的聊天。归档 session 不作为默认启动目标。

**改后行为：** 在某个 workspace 启动 Tact 会恢复最近使用的未归档 session 及其已存转录。只有 workspace 没有可用历史，或用户主动按 New session 时，才创建新 session。

**指针：** `crates/tact-gui/src/shell.rs`（`TactApp::connect`、`startup_resume_id`）；`crates/tact-session/src/runtime.rs`（`SessionOptions::resume`）

## 1. 2026-09-21 — 配置中的模型成为默认选中，选择也会持久化

| 字段 | 值 |
|-------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-session/src/builder.rs`（`configured_model_params`、`persist_active_model`）；`crates/tact-gui/src/shell.rs`（`TactApp::build`、`set_model`） |

**现象 / 动机：** 桌面客户端只从第一条 `AgentUpdate::ModelInfo` 得知当前模型。第一轮开始前，即使 `~/.tact/config.toml` 已配置了模型和 reasoning effort，chip 仍可能退回 `Tact`。在 GUI 选择模型也只发送 `SetModel`，没有写回配置，因此下次启动选择就丢了。

**决策：** live shell 在第一条 update 之前，用已解析配置预填 `SessionState::model`（`configured_model_params`）；选择模型时除了发送 `SetModel`，还通过 `tact::config::persist_active_provider_model` 持久化。配置写入使用 `toml_edit`，只更新当前 provider 的 `model` 键，保留注释和文件其余结构。

**改后行为：** 已连接窗口打开时就已选中配置里的模型和 effort。选择其他模型会更新运行中的 session，并把当前 provider 的 `model` 写回加载的配置文件；下次启动从保存值开始。打开 picker 仍会刷新服务端列表并标记当前模型。

**指针：** `crates/tact-session/src/builder.rs`（`configured_model_params`、`persist_active_model`）；`crates/tact-gui/src/shell.rs`（`TactApp::build`、`set_model`）；`crates/tact/src/config/persist.rs`

## 1. 2026-09-21 — Model 选择器在打开时刷新，并滚动真实服务端列表

| 字段 | 值 |
|-------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/shell.rs`（`fetch_model_options`、`prompt_composer`）；`crates/tact-gui/src/session.rs`（`model_options_loading`） |

**现象 / 动机：** 已连接的窗口只在启动时拉取一次模型列表。对选择器来说时机不对：服务端之后可能新增模型，而且真实 endpoint 可能返回几十个 id。旧 popover 会随列表一起长到窗口外，大部分选项无法看到。

**决策：** 在 model popover 打开时重新拉取服务端模型列表，而不是在窗口连接时只拉一次。请求进行中显示 `Refreshing from provider…`；每个窗口用 epoch 防止较慢的旧响应覆盖新响应。popover 限制为 24 rem，并带 overlay 滚动条和右侧留白，因此完整模型列表以及 Thinking budget/effort 控件都留在窗口内。

**改后行为：** 打开 Model 就会发起新的服务端请求；当前模型 chip 仍立即显示，列表在服务端返回后填充。真实的长模型列表在受限 popover 内滚动，不再覆盖整个窗口。

**指针：** `crates/tact-gui/src/shell.rs`（`fetch_model_options`、`PromptComposer::model_popover`）；`crates/tact-gui/src/session.rs`（`model_options_loading`）

## 1. 2026-09-21 — 工作面板 tab 保住名称，不再为所有计数让位

| 字段 | 值 |
|-------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/pane.rs`（`WorkPane::label`、`work_tabs`） |

**现象 / 动机：** 工作面板在约 384 px 的条带里放了八个 28 px chip。每个 chip 同时保留文字和计数徽标，于是可压缩的 chip 被挤到只剩 `Pl...`、`Ta...`、`Ag...`、`Fil...`、`Brow...`。计数还在，但主要导航名称读不出来了。

**决策：** 只让当前选中的 chip 保留计数徽标。当前面板的计数最有即时价值；其余七个名称拿回宽度并保持可读。面板正文本来就会解释当前数量，因此用户真正操作时没有信息损失。

**改后行为：** 八个工作面板 tab 都显示完整名称（Plan、Diff、Tasks、Agents、Files、Stats、Term、Browser）；只有 active tab 带数字徽标。

**指针：** `crates/tact-gui/src/pane.rs`（`work_tabs`）；`crates/tact-gui/tests/shell.rs`（`every_work_pane_renders_content_not_just_a_container`、`every_entry_point_answers_a_click`）

## 1. 2026-09-21 — Thinking 与工具卡片按测量高度柔和展开

| 字段 | 值 |
|-------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/transcript.rs`（`CARD_REVEAL`、`card_reveal_policy`、`MotionReveal`、`TranscriptRow::Thinking`、`TranscriptRow::Tool`）；`crates/tact-gui/tests/shell.rs`（`clicking_a_thinking_summary_reveals_it_softly`、`clicking_a_tool_summary_reveals_its_output`） |

**现象 / 动机：** 展开或收起 Thinking 卡与工具卡时，内容会在一帧内从收起高度跳到最终高度。内容本身没错，但行会突兀地跳动，长转录因此显得很机械。

**决策：** 把正文放进 `MotionReveal`，由 180 ms、原型 `cubic-bezier(.23, 1, .32, 1)` 缓动的 transition 驱动。Reveal 先测量一次子元素，再按 `测量高度 × progress` 裁剪；正文同时淡入并上移 2 px。数值 transition 会立即采用第一次目标值，所以启动时就已展开的卡片（历史恢复、预览）不会在挂载时播放动画。收起过程中正文保持挂载，直到 progress 归零才卸载；开启 reduced motion 的用户首帧即看到最终状态。

**改后行为：** 点击 Thinking 或工具摘要时，卡片会按与 chevron 相同的短缓动展开/收起。周围行随测量后的内容一起移动，而不是在它旁边瞬间跳动。

**指针：** `crates/tact-gui/src/transcript.rs`（`CARD_REVEAL`、`card_reveal_policy`、`MotionReveal`）；`crates/tact-gui/tests/shell.rs`（`clicking_a_thinking_summary_reveals_it_softly`、`clicking_a_tool_summary_reveals_its_output`）

## 1. 2026-09-21 — 工具行更矮，滚动条不再压住文字

| 字段 | 值 |
|-------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/transcript.rs`（工具摘要行、工具输出块）；`crates/tact-gui/src/pane.rs`（工作面板主体） |

**现象 / 动机：** 转录里有两个密度问题。工具卡的摘要行沿用原型的 38 px，于是一轮里十几次工具调用的大部分高度都花在读不到的装饰上。另外滚动条是 gpui-component 的覆盖式：它**画在**内容的最后几列之上，而不是给自己留一条槽，于是满宽的一行工具输出会跑到滑块底下。

**决策：** 工具摘要行按 28 px 绘制——比原型矮四分之一——保留 20 px 的图标块以保证这一行仍有明确标记；垂直内边距从 5 px 降到 3 px 与之匹配。滚动条则改为在可滚动内容上加右侧内边距，而不是挪动滚动条：工作面板主体与工具输出块右侧 18 px、左侧 10 px。滚动条本身不变（覆盖式外观是主题预期），变的是文字不再出现在它下面。

**改后行为：** 一串工具调用的转录大约矮四分之一。较长的工具输出与工作面板正文在滚动条之前结束，而不是被压在后面。

**指针：** `crates/tact-gui/src/transcript.rs`（`TranscriptRow::Tool`）；`crates/tact-gui/src/pane.rs`（`view`）

## 1. 2026-09-21 — 切换会话不再中断正在运行的任务

| 字段 | 值 |
|-------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`ParkedSession`、`park_running_session`、`unpark`、`route_agent_update`、`fold_update`、`spawn_pump`） |

**现象 / 动机：** 在一轮任务运行中选择侧栏里的另一个会话，会中断那一轮并丢失部分 stream。根因是结构性的：`adopt` 直接替换 `self.session` 与 `self._pump`，于是旧的 `SessionHandle` 被 drop——driver 的命令通道关闭、任务被取消——旧 pump 也一起被 drop，尚未送达的 stream 数据随之丢失。切回来时又会为同一个 id **再起一个 runtime**，只能重放 store 里已持久化的内容，运行中 stream 出来的部分就永久没了。

**决策：** 把仍在运行的会话**寄存**而不是丢弃。`state.running` 为真的会话会移入 `ParkedSession`——它的 handle、pump、转录、会话状态，以及属于它的排队提示与已选附件——用户切回时原样还原，而不是重新 resume。只寄存运行中的会话：空闲会话的转录已经在 store 里，重新 resume 只是一次历史读取；而给用户点过的每一行都常驻一个 runtime 就是泄漏。基于同样理由，寄存上限为 4 个。

更新按 session id 路由，因为一条更新属于**发出它的**会话，而不是当前显示的那个。`fold_update` 是从 `apply_agent_update` 里抽出的折叠核心，因此也能作用于寄存会话的 `(Conversation, SessionState)`；与视图耦合的后续动作——失效 Diff 缓存、通知滚动器、冲刷 composer 队列——留在调用方，因为它们属于窗口当前显示的内容。

**改后行为：** 从运行中的任务切走，任务继续运行：命令通道保持打开、stream 继续折叠进它自己的转录，切回来看到的是实时会话而不是重建的。寄存的会话保留其排队提示与已选附件。

**已知限制（有意保留）：** 寄存中的会话只有切回去才能 Stop；composer 的草稿文本是窗口级的，不随会话寄存；跨 **workspace** 切换时，旧 workspace 的 runtime 会一直活到四个上限把它淘汰。

**指针：** `crates/tact-gui/src/shell.rs`（`switching_away_keeps_a_running_session_alive`）；`book/05_chapter_compact.md` 不受影响——这是 UI 层的会话归属，不是 agent loop 状态

## 1. 2026-09-21 — 对话区占更大的窗口比例

| 字段 | 值 |
|-------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/shell.rs`（`WORK_PANE_WIDTH`、`TRANSCRIPT_MEASURE`）；`crates/tact-gui/src/layout.rs`（`WORK_PANE_WIDTH_REM`） |

**现象 / 动机：** 原型把工作面板画成 420 px，并把转录限制在 `min(720px, 100% - 48px)`。这在原型所依据的 1440 px 画板上是对的，但在更宽的窗口里，两根固定宽度的列会让对话变成一条 720 px 的细带子，两侧各留一大片死白——转录是这个窗口存在的理由，却成了里面最窄的东西。

**决策：** 两端各让一点。工作面板默认宽度从 420 px 降到 384 px，转录的测量上限从 720 px 提到 896 px。两者都不是硬限制：工作面板本身可拖拽（在其 clamp 范围内），转录仍保留原型的 48 px 边距。原型保持自己的数值——这是壳层的默认值，写在这里是因为原型是 source of truth，而这次偏离是刻意的。

**改后行为：** 在 1440 px 设计宽度下对话列增宽 36 px，且文字填满该列而不是提前停住；在宽窗口下文字上限从 720 px 变为 896 px，两侧留白按比例缩放。既有的布局契约仍然成立：`wide_window_lays_out_three_columns` 的三列算术、抽屉滑入、窄屏断点。

**指针：** `crates/tact-gui/tests/shell.rs`（`wide_window_lays_out_three_columns`、`the_work_pane_drawer_slides_in_and_out_over_the_prototype_duration`）；`docs/design/tact-desktop-design-review.md`

## 1. 2026-09-21 — Projects 按目录切换 workspace

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/src/layout.rs`（`recent_workspaces`、`remember_workspace`）；`crates/tact-gui/src/shell.rs`（`SidebarInputs`、`project_row`、`open_folder_row`、`switch_workspace`、`open_workspace`、`open_project_picker`、`sidebar-scroll`）；`crates/tact-gui/tests/shell.rs`（`the_projects_group_switches_workspace_by_directory`、`the_open_folder_row_does_not_open_a_picker_offline`） |

**现象 / 动机：** 侧栏切换 workspace 的唯一途径是 `Worktrees` 分组，而它执行 `git worktree list`，因此永远只能给出窗口**已经所在**仓库内的目录。完全没有办法打开另一个项目。

**决策：** 把目录当作项目，因为存储本来就是这么做的——会话库位于 `<workspace>/.tact/tact.db`。侧栏在 `Worktrees` 之上新增 `Projects` 分组，按最新在前列出最近八个 workspace 目录，标出当前项，并带一行 `Open folder…`。`Open folder…` 使用 GPUI 自带的 `prompt_for_paths`（`directories: true, files: false`）——即 composer 附件已经在用的同一条平台缝——因此不引入新依赖，也不需要选 GTK 还是 portal。项目用**路径**而不是名称标识：两个 checkout 可能同名。选择器与已记住的行都走 `open_workspace`，它对非目录路径直接拒绝，而不是把窗口重根到空处。列表与布局共用同一个 `~/.tact/gui-layout.json` 文档，`connect` 会记住启动目录，因此首次运行不会是空分组。离线预览不开模态：它回答它本会打开什么。

顺带解决了两件小事。侧栏滚动区必须变得可寻址（`sidebar-scroll`）——在 worktrees 之上新增分组后，那些行在默认 900 px 窗口下已落到折叠线以下，测试巡检无法点击一个它滚不到的行；这要求把该盒子从 gpui-component 的 `overflow_y_scrollbar`（其包装器会用调用点位置替换元素 id）换成 `overflow_y_scroll`。另外 `sidebar`/`sidebar_overlay` 改为接收一个 `SidebarInputs` 结构体而不是五个位置参数借用，这样以后再加分组不会持续突破 clippy 的参数上限。

**改后行为：** 侧栏列出用户打开过的项目，当前项在前并带 `current` 徽标。按已记住的行即切换过去；按 `Open folder…` 会询问目录并切换到所选的那个。两种方式都会重根 workspace、其 git 分支、会话列表、文件树与 diff 面板，并把新目录记入列表（去重、上限八条、最新在前）。打开的是文件而不是目录时会被拒绝并给出提示。

**指针：** `crates/tact-gui/src/layout.rs`（`recent_workspaces_round_trip_newest_first_and_bounded`）；`crates/tact-gui/tests/shell.rs`（`the_projects_group_switches_workspace_by_directory`、`the_open_folder_row_does_not_open_a_picker_offline`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` §6.2

## 1. 2026-09-21 — Files 行整行可点，文件夹可被选中

| 字段 | 值 |
|-------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/pane.rs`（`files_tree`、`file_preview_card`、`FilePreview::load`、`FilesPane::select`）；`crates/tact-gui/tests/shell.rs`（`the_files_pane_row_toggles_and_selects_a_directory`、`the_files_pane_actions_work_on_a_selected_directory`） |

**现象 / 动机：** 目录行只有那个 16 px 的 chevron 会响应点击。行本身画了 hover 底色和选中底色却没有 handler，所以点文件夹**名字**什么都不会发生——整行上最大的目标反而是死的。同一个根因还带来两个后果：文件夹无法被选中，因此面板的 `Reveal` 与 `Mention` 会对用户明明选中的东西回答"请先选择一个文件"；而且一旦文件夹成为选中项就会被当成文件去读——Linux 上 `File::open` 对目录是成功的，随后的 read 才以 `EISDIR` 失败。

**决策：** 让整行成为目标。点目录行任意位置即选中并展开/折叠；点文件行任意位置即选中并打开预览。chevron 保留为精确、可键盘到达的手柄，并且现在会 `stop_propagation()`——stock button 对它**有** handler 的按压并不会阻止冒泡，不加这一句 chevron 的切换会和整行的切换互相抵消，文件夹看起来就像卡死了。选中文件夹也让文件夹成为面板动作的一等对象，因此 `FilePreview` 增加目录分支：文件夹预览为 `Directory · N entries`，而不是读取失败。行同时变成了 tab stop，所以也必须响应 `Enter` 与 `Space`——一个能获得焦点、聚焦后却什么都不做的行，比不能聚焦更糟。

**改后行为：** 点文件夹行（或其 chevron）会展开/折叠并选中；点文件行会预览。`Reveal`、`Mention` 与 footer 的 `Open in editor` 对选中的文件夹都生效：Reveal 在文件管理器中打开它，Mention 把 `@docs `（文件夹的 workspace 相对路径）插入 composer，Open in editor 交给默认应用。选中的文件夹在预览卡里显示 `Directory · N entries`，不再显示读取失败。聚焦行上按 `Enter` 或 `Space` 与点击等效。

**指针：** `crates/tact-gui/src/pane.rs`（`FilesPane::select`、`on_toggle`、`FilePreview::load`）；`crates/tact-gui/tests/shell.rs`（`the_files_pane_row_toggles_and_selects_a_directory`、`the_files_pane_actions_work_on_a_selected_directory`、`the_files_pane_expands_a_directory_through_its_toggle`）

## 1. 2026-09-21 — 工作面板内嵌真实 PTY 终端

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/Cargo.toml`（`portable-pty`、`vt100`）；`crates/tact-gui/src/terminal.rs`；`crates/tact-gui/src/pane.rs`（`WorkPane::Terminal`、`terminal_pane`、`terminal_run`、`terminal_color`）；`crates/tact-gui/src/shell.rs`（`start_terminal`、`restart_terminal`、`terminal_key`、`terminal_input`、`spawn_terminal_pump`、`terminal_size`） |

**现象 / 动机：** 设计把内嵌 Terminal 推迟到 v1 之后，因为壳层没有 PTY 文本面，工作面板也无法在 workspace 里运行任何东西。所有命令都得经过 agent。

**决策：** 用 `portable-pty` 在真实 PTY 里运行用户自己的 `$SHELL`，用 `vt100` 解析输出并绘制网格。关键区分：这是**终端**，不是命令执行器。命令执行器要重新实现作业控制、管道、提示符与 `cd`，而且一定会做错；这里 shell 是子进程，这些行为就是 shell 自己的。读取放在专用阻塞线程里喂 channel，因为 PTY 没有异步句柄；一个 GPUI task 以 16 ms 周期轮询该 channel，render 时也会 drain，因此即使漏掉一次唤醒，单帧仍然自洽。parser 与 PTY 一起 resize，尺寸取自面板真正要绘制的网格。

在用户按下 Start 之前不启动任何东西。打开面板不应启动进程，否则每个遍历面板的测试都会启动一个 shell。`TerminalPane::drop` 会 kill 子进程，所以关窗不会遗留 shell。

**改后行为：** `Term` 面板在被要求之前显示 Start；启动后渲染实时网格，并在 shell 实际放置光标的单元格画块光标。按键按终端期望编码——`Enter` 是 `CR`、`Ctrl-C` 是单个控制字节、方向键是转义序列——面板或窗口缩放时会重新测量 PTY。`Restart` 替换子进程。shell 退出后面板报告退出状态，而不是看起来像死了。

**指针：** `crates/tact-gui/src/terminal.rs`（`runs_from_screen`、`key_bytes`、`control_byte`、`a_pty_childs_output_reaches_the_grid`）；`crates/tact-gui/tests/shell.rs`（`the_terminal_pane_runs_a_shell_in_a_pty`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` §6.5

## 1. 2026-09-21 — 工作面板可停靠到右、左、下边

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/src/layout.rs`（`WorkPaneSide`）；`crates/tact-gui/src/shell.rs`（`cycle_work_pane_side`、`ResizeTarget::WorkPaneLeft`、`work_pane`、`work_pane_bottom`、`resize_handle`）；`crates/tact-gui/src/pane.rs`（`work-pane-dock`、`work_footer`） |

**现象 / 动机：** 原型把工作面板固定在右边，规格把自由 Dock 重排推迟到 v1 之后。宽 diff 或长文件树在用户自选的边上更好读，而底部停靠是终端最常见的形态。

**决策：** 提供三个具名边，而不是 splitter 树。右与左只是同一行 flex 的重排；底部把转录嵌进一个列，使侧栏保持整高。`WorkPaneSide` 属于持久化布局文档，因此停靠位置跨重启保留，可从面板 footer 的 dock 控件或命令面板的 Move work pane 行循环切换。dock 控件放在 footer 而不是 tab 旁边，因为八个 tab chip 已占满面板宽度，放在那一行会把最后一个 chip 裁掉。水平分隔条拖拽面板自身宽度并跟随所在边：停靠左侧时手柄位于面板右缘，宽度从侧栏之后起算。

**改后行为：** 面板出现在所选边上，边框在正确的一侧，转录填满其余空间。停靠底部时是固定高度的横带；其宽度不可拖拽，因为它横跨整列。窄窗抽屉行为不变，仍从右侧滑入。

**指针：** `crates/tact-gui/src/layout.rs`（`the_work_pane_side_round_trips_and_cycles`）；`crates/tact-gui/tests/shell.rs`（`the_dock_control_moves_the_work_pane_around_the_window`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` §6.5

## 1. 2026-09-21 — Browser 面板把地址交给系统浏览器

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-session/src/session_actions.rs`（`open_url`）；`crates/tact-gui/src/session.rs`（`open_url`）；`crates/tact-gui/src/pane.rs`（`WorkPane::Browser`、`browser_pane`）；`crates/tact-gui/src/shell.rs`（`open_browser_url`、`open_browser_history`、`clear_browser_history`、`normalize_url`） |

**现象 / 动机：** 规格把内嵌 Browser 推迟到 v1 之后。GPUI 渲染自己的 GPU 表面，无法在其中承载 `webkit2gtk` 或 `wry` 视图，所以"内嵌浏览器"从来没有可用的实现路径；但工作面板也一直没有打开 agent 产出的链接的办法。

**决策：** 交付诚实版本并如实标注。`Browser` tab 是一个地址栏：按地址栏的方式规范化裸主机名（`example.com` → `https://example.com`），通过与其它打开动作相同的 launcher 链交给桌面默认浏览器，并记住最近十条地址（最新在前、去重）。面板自己的副标题就写明 Tact 没有内嵌 web view。`open_url` 拒绝非 `http`/`https`，且绝不做规范化——对 URL 调 `Path::exists` 会拒绝每一个合法链接。

**改后行为：** 输入地址并按 Open（或在字段里按 `Enter`）会在系统浏览器打开并加入 Recent addresses；按已记住的地址会重新打开；Clear 全部忘记。离线预览下不启动任何东西：动作记录地址并说明它本会打开什么。

**指针：** `crates/tact-gui/tests/shell.rs`（`the_browser_pane_normalizes_and_remembers_addresses`）；`crates/tact-session/src/session_actions.rs`（`open_url`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` §6.5

## 1. 2026-09-21 — 桌面壳持久化布局、暴露预设并支持缩放

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/src/layout.rs`；`crates/tact-gui/src/shell.rs`（`layout_prefs`、`apply_layout`、`set_sidebar_width`、`set_work_pane_width`、`resize_handle`、`zoom_in`、`status_bar`）；`crates/tact-gui/src/commands.rs`（`LayoutSplit`/`Focus`/`Review`/`Zen`、`ZoomIn`/`ZoomOut`/`ZoomReset`） |

**现象 / 动机：** 规格把布局持久化与可拖拽列推迟到 v1 之后，因此窗口关掉后布局就消失：每次启动都回到原型的固定 260 / 420 px 列，侧栏和工作区无论用户上次怎么关都会重新打开，转录细节等级也会重置。设计评审同样把"更大字号"挂起，备注说壳层是 `rem` 基准但没有证据。

**决策：** 新增一个小 JSON store，位于 `~/.tact/gui-layout.json`（`TACT_GUI_LAYOUT_PATH` 可覆盖），保存排列、两列宽度、转录细节等级与基准字号。每次变更即保存——切换、预设行、面板选择、细节循环、拖拽分隔条、缩放——而不是在窗口关闭时保存，因为被强杀的窗口永远等不到关闭事件。读取时用 `#[serde(default)]` 并对每个宽度做 clamp，截断或手改过的文档既不会压塌某一列，也不会让应用拒绝启动。离线与测试壳层使用 disabled store，因此任何测试或预览都不会写入开发者真实布局。

两条分隔条是**画在列边界之上**的 4 px 条带，而不是占据 flex 宽度：原型的两列本身已带边框，占宽的分隔条会挪动所有既有尺寸。预设（`Split`、`Focus`、`Review`、`Zen`）只是两个 open 标志的粗粒度缩写，刻意不重置用户拖出的宽度。缩放就是一个数：壳层从头到尾都是 `rem` 基准，因此 `Window::set_rem_size` 会同时缩放列宽、文字与断点；状态栏只在非 100% 时显示缩放 chip。

**改后行为：** 窗口按上次的排列、宽度、转录细节与缩放重新打开。`Ctrl`+`Alt`+`1..4` 与命令面板四行 Layout 切换排列，状态栏显示当前排列名。拖动任一分隔条会在 clamp 范围内调整列宽，新宽度跨重启保留。`Ctrl`+`=` / `Ctrl`+`-` / `Ctrl`+`0`（面板：Zoom in / Zoom out / Reset zoom）在 12–24 px 之间移动基准字号，非 100% 时状态 chip 报告百分比。会话置顶与列宽是壳层唯一写入的每用户状态。

**指针：** `crates/tact-gui/src/layout.rs`（`LayoutPrefs`、`LayoutStore`、`a_round_trip_preserves_the_arrangement`、`widths_are_clamped_on_load`、`zoom_is_clamped_and_round_trips`）；`crates/tact-gui/src/shell.rs`（`the_layout_store_restores_and_persists_the_column_widths`）；`crates/tact-gui/tests/shell.rs`（`the_layout_palette_rows_rearrange_the_shell`、`zooming_changes_the_rem_size_and_reports_it`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` §14

## 1. 2026-09-21 — Stats 面板把会话自身的数字画出来

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/src/pane.rs`（`WorkPane::Stats`、`stats`、`stat_tile`、`chart_card`、`legend_swatch`）；`crates/tact-gui/tests/shell.rs`（`the_stats_pane_charts_the_session_state`） |

**现象 / 动机：** 计划把"图表密集的仪表盘"推迟到 v1 之后，壳层也没有一个整体读取会话数字的界面。token 用量只出现在 composer 的圆环和状态栏 chip 上，任务进度只在 Tasks 表里，记录的文件变更只是一串列表——想回答"上下文花了多少、改动集中在哪"要读三个界面。

**决策：** 新增第六个工作面板 `Stats`，只从 `SessionState` 取数——也就是其他面板共用的同一份快照。三张图：prompt / completion 堆叠条并列出 cache 与 reasoning 计数；基于任务快照的按状态柱状图；以及改动量最大的五个文件，用增/删成对横条表示。不新增 store 查询，也不缓存副本：这个面板只是对壳层已持有状态的一种读法，因此不可能与旁边的面板产生偏差。六个标签时 tab 条会裁掉最后一个 chip，所以 `Subagent` 改名为 `Agents`——只缩短显示标签，元素 id 不动（现在由稳定的 `WorkPane::slug` 生成）。

**改后行为：** Stats tab 显示四个 tile（tokens、tasks、plan、diff）、token 拆分、任务状态图与按文件变更图。没有活动的会话显示具名空状态，而不是空面板。切走再切回会从实时快照重绘，而不是从陈旧副本。

**指针：** `crates/tact-gui/src/pane.rs`（`stats`）；`crates/tact-gui/tests/shell.rs`（`the_stats_pane_charts_the_session_state`、`every_work_pane_renders_content_not_just_a_container`）；`docs/superpowers/plans/2026-09-19-tact-desktop-client.md`（Deferred）

## 1. 2026-09-21 — 会话可置顶，侧栏搜索改为匹配可见标签

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact/src/store/session_store/{mod,sqlite}.rs`（`pinned_at`、`pin_session`）；`crates/tact-session/src/sessions.rs`（`RecentSession::pinned`、`recent`）；`crates/tact-session/src/session_actions.rs`（`set_pinned`）；`crates/tact-gui/src/shell.rs`（`set_open_session_pinned`、`session-menu-pin`、`session_buckets`） |

**现象 / 动机：** 会话列表有两个缺口。其一只按时间排序，用户反复回到的会话一旦碰过别的就会被压下去。其二搜索框只匹配 `session.id`，而侧栏从不显示 id：输入可见标题得不到任何结果。

**决策：** 置顶使用独立的可空列 `sessions.pinned_at`，形状与归档标志完全一致——只是标记，不是状态。store 仍按 `updated_at` 排序，由 `tact_session::sessions::recent` 做稳定分区把置顶行提到最前，因此取消置顶即恢复常规顺序，SQL 不需要知道这条展示策略。复制会话不复制置顶，正如它不复制归档标志。搜索过滤改为匹配行上打印的每个标签——id、派生标题、用户命名——因为只认 id 的过滤器是在搜索侧栏从不显示的文字。

**改后行为：** 会话菜单为当前会话提供 Pin/Unpin；置顶行排在最前，并在元信息行里显示 `pinned`；该动作可逆，且只落一条转录通知。侧栏搜索同时匹配标题与名称。刻意**没有**添加按项目、按分支分组：列表本就限定在单个 workspace store，且会话没有分支列，两种分组都不会真正分区。

**指针：** `crates/tact/src/store/session_store/sqlite.rs`（`test_pin_session_sets_and_clears_the_flag`、`a_store_from_before_the_title_columns_migrates_in_place`）；`crates/tact-session/src/sessions.rs`（`recent_sorts_pinned_sessions_first`）；`crates/tact-gui/tests/shell.rs`（`pinning_a_session_moves_it_to_the_head_of_the_list`、`the_sidebar_search_filters_the_session_list`）；`book/01_chapter_store.md`

## 1. 2026-09-21 — Diff 与 Files 面板补齐主要动作

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-session/src/session_actions.rs`（`stage_path`、`reveal_path`、`open_path`）；`crates/tact-gui/src/pane.rs`（`DiffPane`、`FilesPane`、`FilePreview`、`diff_card`、`file_preview_card`）；`crates/tact-gui/src/shell.rs`（`stage_diff_path`、`draft_diff_review`、`select_file`、`open_selected_file`、`reveal_selected_file`、`mention_selected_file`） |

**现象 / 动机：** Diff 面板在桌面规格里声明了 review 动作，但实际只渲染 diff 内容和不可用的 `Comment` 按钮。Files 面板虽然能渲染项目树，但文件行不能打开预览，`Reveal` / `Mention` 没有控件，共享的 `Open in editor` 也仍然对每次点击都说不可用。它们是 Work Pane 里最后两块主要动作仍然只是装饰的区域。

**决策：** Diff 暂存通过共享 session action 层执行真实的 `git add`，不创建只在 GUI 内存在的 staged 状态。`Comment` 作为一次批量 review 草稿：它把所有已记录 diff 路径汇总进 composer，让用户补充评论后走普通队列发送，从而保持 session 是唯一真相源。Files 中点击文件行打开有界文本预览（64 KiB / 160 行），`Reveal` 通过文件管理器打开所在目录，`Mention` 向 composer 插入 `@relative/path`，`Open in editor` 用平台默认应用打开选中文件。`Add file` 仍保持不可用，因为 v1 不让 GUI 绕过 agent 工具创建文件。

**改后行为：** Diff 卡片暴露 `Stage`；暂存会在 workspace 中运行 `git add`，并在转录中报告成功/失败。`Comment` 会准备包含所有变更文件的 review 草稿，而不是再提示协议不支持 review comments。Files 行会保留选中状态，在树下方渲染文本预览，明确显示 binary、读取失败、截断等状态，暴露 `Reveal` 与 `Mention`，并让 footer 打开当前选中文件。Mention 会把 workspace 相对路径 `@path` 追加到当前 composer 草稿，保留原有文本。

**指针：** `crates/tact-gui/tests/shell.rs`（`the_diff_pane_stages_and_drafts_a_batch_review`、`the_files_pane_previews_reveals_and_mentions_a_file`）；`crates/tact-gui/src/pane.rs`（`files_preview_reads_the_selected_file_and_refreshes_on_invalidate`）；`crates/tact-session/src/session_actions.rs`（`stage_path_stages_a_repository_root_path_from_a_subdirectory`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` §6.5

## 1. 2026-09-21 — Tasks 面板支持筛选、更新状态并打开所属会话

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/protocol/src/agent.rs`（`UserCommand::TaskUpdate`）；`crates/tact-session/src/driver.rs`；`crates/tact-gui/src/pane.rs`（`TasksPane`、`tasks`、`task_row`）；`crates/tact-gui/src/shell.rs`（`cycle_task_filter`、`cycle_task_sort`、`update_task_status`、`open_task_session`） |

**现象 / 动机：** Tasks 面板虽然渲染了原型里的任务表，但规格中的行级动作仍然缺失。用户无法收窄长任务列表、重排列表、推进任务状态，也无法从任务跳回它所属的会话。唯一可见的控件 `New task` 仍只会回答 v1 暂不可用。

**决策：** 筛选和排序保留为 `TasksPane` 的本地显示偏好：它们只改变面板显示内容，不改会话状态。状态变化通过协议发送 `UserCommand::TaskUpdate`，因为 driver 拥有持久任务管理器，生命周期时间戳与依赖清理由 store 负责。点击任务状态徽标会按 Pending → InProgress → Completed → Pending 循环推进。点击有 session id 的任务 owner 单元格会复用 `resume_session`，因此打开任务所属会话与侧边栏走同一条 store / history 路径。

**改后行为：** Tasks 头部暴露 Filter 与 Sort 循环按钮；行 id 由 task id 稳定生成。状态徽标可点击，并恰好派发一次更新；面板等待刷新后的 `TasksChanged` 快照，而不是乐观改写本地状态。没有 owner session id 的任务仍只显示普通 owner 文本；有归属的任务暴露 Open-session 点击区域。

**指针：** `crates/tact-gui/src/pane.rs`（`task_filter_and_sort_keep_the_expected_rows`）；`crates/tact-gui/src/shell.rs`（`task_filter_and_sort_buttons_change_their_labels`、`updating_a_task_sends_the_status_transition`、`opening_a_task_session_selects_its_session_row`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` §6.5

## 1. 2026-09-21 — Subagent run 可取消并可查看已存储的转录

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/src/session.rs`（`SessionState::subagent_transcript`、`SubagentTranscriptState`）；`crates/tact-gui/src/shell.rs`（`cancel_subagent`、`toggle_subagent_transcript`）；`crates/tact-gui/src/pane.rs`（`subagents`、`subagent_transcript_card`）；`crates/tact-session/src/history.rs`；`crates/protocol/src/agent.rs`（`UserCommand::CancelSubagent`） |

**现象 / 动机：** Subagent 面板只列出了 run 的状态和摘要，规格里为行定义的动作为空缺。桌面端无法取消正在运行的后台 child，也无法直接查看它已存储的转录，只能换到别的界面或从父转录里绕路推断。

**决策：** 取消沿用既有协议路径：Running 行通过 `SessionHandle` 发送 `UserCommand::CancelSubagent { child_id }`，由 driver 翻转 child 的协作式取消标志、更新 run record，并发出权威快照。转录查看复用会话重绘所用的 `tact_session::history::history` 缝，在后台 executor 加载并存入当前 child 的 `SubagentTranscriptState`。面板内联渲染这些消息，同一动作在已打开时切换为 Hide transcript。

**改后行为：** 每个 subagent 行都有 Inspect transcript；Running 行额外有 Cancel。查看 child 会在 run 列表下方加载其已存储对话，保持消息/block 顺序，并如实显示 loading、error 或空状态。取消会发给 driver，面板等待 `SubagentsChanged`，而不是乐观修改快照。

**指针：** `crates/tact-gui/src/shell.rs`（`cancelling_a_subagent_sends_its_child_id_to_the_driver`、`inspecting_a_subagent_loads_its_stored_transcript`）；`crates/tact-gui/src/pane.rs`（`subagents`、`subagent_transcript_card`）；`crates/tact-gui/src/session.rs`（`SubagentTranscriptState`）；`crates/tact-session/src/history.rs`；`docs/design/tact-desktop-design-review.md`

## 1. 2026-09-21 — Plan 步骤可展开、失败可见、跳转转录，并通过 agent 重试

| 字段 | 值 |
|-------|-----|
| **类型** | feature |
| **相关** | `crates/tact-gui/src/session.rs`（`SessionState::plan_expanded`、`plan_failed`、`Conversation::reveal_tool`、`mark_plan_step`）；`crates/tact-gui/src/pane.rs`（`plan_step_row`）；`crates/tact-gui/src/shell.rs`（`toggle_plan_step`、`retry_plan_step`、`open_plan_step_transcript`、`submit_pane_prompt`）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md` |

**现象 / 动机：** Plan 面板虽然画出了原型里的步骤行和进度条，但规格里为行定义的动作仍然只是装饰。用户不能展开步骤查看输入和结果，不能区分失败工具与成功工具，不能跳到产生该步骤的工具卡，也不能重试失败步骤。步骤只保存第一次终态结果、却不记录状态，所以后续成功也无法清掉先前的失败。

**决策：** 把计划状态留在 `SessionState`：`plan_expanded` 记住用户展开过哪些行，`plan_failed` 按步骤下标记录终态失败。`StepFinished` 依据 `result.status` 记录 `result.message`；`StepFailed` 记录错误字符串；任一终态成功都会清掉 failed 位。点击行切换详情块。失败行使用 danger 状态并显示 Retry。Retry 会提交一条新的 `SubmitTask`，其中带上已记录的工具和参数，把执行、历史、权限和 provider 状态继续留给 driver，而不是绕过协议直接调用工具。Open transcript 通过 `Conversation::reveal_tool` 展开拥有该步骤的工具卡、重新测量并滚动到它；如果卡片尚不存在，则明确提示没有可跳转的转录行。

**改后行为：** Plan 行可以展开查看输入、结果/错误和可用动作。失败步骤会明确显示失败并可重试；重试会向 agent 发送一次带记录工具与参数的新指令。Open transcript 会展开并滚动到已有的工具卡。离线且未附着 session 的壳层会拒绝重试并写入系统提示，而不是假装已经发送工作。

**指针：** `crates/tact-gui/src/session.rs`（`plan_step_tracks_failure_and_clears_it_when_the_tool_succeeds`、`step_failed_records_the_error_on_the_plan_step`、`reveal_tool_opens_the_tool_card_and_returns_its_row`）；`crates/tact-gui/src/shell.rs`（`failed_plan_step_expands_to_retry_and_transcript_controls`、`opening_a_plan_step_transcript_expands_the_tool_card`、`retrying_a_failed_plan_step_submits_the_recorded_tool_and_args`）；`crates/tact-gui/src/pane.rs`（`PlanStepRowState`、`plan_step_row`）；`docs/design/tact-desktop-design-review.md`

## 1. 2026-09-21 — 助手正文改用随应用打包的 Lora editorial 字体

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/assets/fonts/`；`crates/tact-gui/src/theme.rs`（`PROSE_FONT_FAMILY`、`bundled_font_data`、`register_bundled_fonts`）；`crates/tact-gui/src/main.rs`；`crates/tact-gui/src/transcript.rs`；`docs/design/tact-desktop-design-review.md` |

**现象 / 动机：** 已批准的 spec 与设计评审都要求助手正文使用 Lora，但桌面端没有注册任何自带字体，所有回答都落在 UI sans 上。直接写 `font_family("Lora")` 在未安装该字体的宿主机上会被 GPUI 静默回退到 sans，等于没有稳定满足这条设计要求。

**决策：** 把 Lora Roman 与 Italic 的可变字体打包进 `crates/tact-gui/assets/fonts`，随文件附上 OFL 许可证，并在启动时通过 `App::text_system().add_fonts` 注册。用同一个 `PROSE_FONT_FAMILY` 常量应用到助手 Markdown 与展开的 reasoning 文本；界面控件继续用 Inter，代码继续用 JetBrains Mono。

**改后行为：** 助手正文和展开的思考正文在任何宿主机上都用 Lora 渲染，不再依赖系统是否安装该字体。注册失败时会记录启动日志并继续用 GPUI 的正常回退，而不是让窗口打不开。字体文件由随包附带的 OFL 许可证覆盖。

**指针：** `crates/tact-gui/assets/fonts/Lora-Regular-Variable.ttf`；`crates/tact-gui/assets/fonts/Lora-Italic-Variable.ttf`；`crates/tact-gui/assets/fonts/OFL.txt`；`crates/tact-gui/src/theme.rs`；`crates/tact-gui/src/transcript.rs`（`TranscriptRow::Assistant`、`TranscriptRow::Thinking`）；`docs/design/tact-desktop-design-review.md`（助手正文字体审计）。

## 1. 2026-09-21 — 会话菜单动作真正落到 store，而不是继续道歉

| 字段 | 值 |
|-------|-------|
| **类型** | feature |
| **相关** | `crates/tact/src/store/session_store/mod.rs`（`SessionSummary`、`rename_session`、`archive_session`、`duplicate_session`）；`crates/tact/src/store/session_store/sqlite.rs`（原地迁移 `title` / `archived_at`）；`crates/tact-session/src/session_actions.rs`；`crates/tact-session/src/sessions.rs`（`RecentSession`）；`crates/tact-gui/src/session.rs`；`crates/tact-gui/src/shell.rs`（会话菜单处理器与重命名对话框）；`book/01_chapter_store_zh.md` |

**现象 / 动机：** 会话 chip 已经画出原型里的下拉菜单，但重命名、复制、归档和「在文件管理器中显示」每一行都只回一条系统消息，说应用暂时做不到。Store 里没有 title 或 archive 列，而归档也不能拿删除来实现——`delete_session` 会级联清掉消息和子会话。

**决策：** 让 store 持有这些持久事实。`sessions` 增加 `title`（空串表示回退到开场消息）和 `archived_at`（可逆的策略标记，不是 tombstone），旧库通过 `PRAGMA` + `ALTER TABLE` 原地补列。`SessionStore` 增加 `rename_session`、`archive_session` 与 `duplicate_session`；前两者对未知 id 报错，而不是静默成功。Duplicate 在一个事务里把源行与 messages 复制到新 id 下，刻意不复制 provider state 与 `token_usages`：副本从消息重新开始，原会话继续持有自己的请求链与用量。`tact-session::session_actions` 暴露四个与展示无关的动作，其中 reveal 会按顺序寻找平台启动器。GUI 对话框和菜单调用这些动作并重绘 `recent`；离线预览只改内存行。

**改后行为：** 重命名保存 trim 后的名称，清空输入则恢复派生标签。归档后会话仍在列表中，切回即可恢复。复制会插入 `<源标签> (copy)` 行并打开它，同时从空 provider 链开始。Reveal 会用第一个可用的启动器（`xdg-open`、GIO、常见 Linux 文件管理器、macOS `open` 或 Windows `explorer`）打开工作区目录；一个启动器都没有时报告尝试过的列表；启动宽限期内非零退出的启动器也会被报告，而不是被当成成功。点击巡检现在断言每一行的可见效果，而不只是「点击没崩」。

**指针：** `crates/tact-session/src/session_actions.rs`；`crates/tact/src/store/session_store/sqlite.rs`（`migrate_sessions_title_and_archive`、`duplicate_session`）；`crates/tact-gui/src/shell.rs`（`open_rename_dialog`、`duplicate_open_session`、`set_open_session_archived`、`reveal_workspace`）；`crates/tact-gui/tests/shell.rs`（`every_entry_point_answers_a_click`）；`book/01_chapter_store_zh.md`（会话动作）；`docs/token_usage_schema.md`（复制不带走用量）。


## 1. 2026-09-21 — 工作面板五个原型专属按钮会响应点击

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-gui/src/pane.rs`（五个 `*_UNAVAILABLE` 理由、`work_footer`、`plan`、`diff`、`tasks`、`files_tree`）；`crates/tact-gui/src/shell.rs`（`push_system_row`、`the_pane_actions_v1_does_not_back_each_answer_a_press`）；`docs/design/tact-desktop-design-review.md`（Parked items、Phase 4–7 跟进） |

**Symptom / motivation:**工作面板上的 `Open in editor`、`Refresh plan`、`Comment`、`New task`、`Add file` 五个控件，按原型的位置与分量画成了按钮，却既没有 handler，也没有快捷键、没有 palette 行。按下去什么都不会发生。覆盖面很广的那次点击巡检抓不到它：`every_entry_point_answers_a_click` 的 `click!` 宏只断言 id 渲染出来了、且这次按压没有 panic，所以一个「渲染出来但什么都不做」的控件照样能通过——而这五个当时正是如此。

**Decision:** 规格的面板章节没有点名这五个中的任何一个，而 `Open in editor` 又与「v1 不取代编辑器」这条 non-goal 直接冲突，因此这五个都没有 v1 行为可接。与其替它们发明一个行为、或删掉像素对齐所依赖的控件，不如让每个都回答「为什么现在不能做」——也就是会话下拉早就用于 rename/duplicate/archive/reveal 的那个形状。五条理由各不相同，能指出去处的就指出：`Refresh plan` 说这个面板本来就跟着 agent 报的每一步走，`Add file` 说这个面板列的是会话自己改过的文件。`TactApp::push_system_row` 提升为 `pub(crate)`，好让面板能往 shell 上落一行——它的关闭按钮早就是这么回调的。

**Behavior after:**按这五个中的任意一个，都会追加恰好一条系统行，写明限制是什么，并在有替代行为时写明用户真正想要的那个行为。按钮保留原型的位置、尺寸与可用外观，视觉对齐没有被动过；变的是工作面板里再没有沉默的控件。五个都由 `the_pane_actions_v1_does_not_back_each_answer_a_press` 覆盖：它依次按下每一个，读回五条互不相同的行，从而补上了巡检「只证存活、不证效果」的那个缺口。

**Pointers:** `crates/tact-gui/src/pane.rs`（五条理由、`work_footer`、`plan`、`diff`、`tasks`、`files_tree`）；`crates/tact-gui/src/shell.rs`（`push_system_row`、`the_pane_actions_v1_does_not_back_each_answer_a_press`）；`docs/design/tact-desktop-design-review.md`（Parked items、Phase 4–7 跟进）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md`（"Non-goals for v1"）

## 1. 2026-09-21 — 已答复的授权卡留在原位置，而不是挂在转录末尾

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-gui/src/session.rs`（`TranscriptRow::Approval`、`Conversation::push_approval`、`to_markdown`）；`crates/tact-gui/src/shell.rs`（`answer`、`transcript_item_count`、`approval` 回调、`answer_panel`）；`crates/tact-gui/src/transcript.rs`（`ApprovalCard`、`RowActions`、`Approval` 分支）；TUI 先例 `crates/agent_tui_kit/src/state/select_popup.rs`（`log_confirm`）与 `book/10_chapter_permission.md` §6 |

**Symptom / motivation:**答复授权框之后，那张已答复的卡片会一直停在转录底部，而刚刚被它放行的这一轮反而把新行追加到卡片**上方**——于是最新的内容往上顶，一张旧的授权卡却始终是屏幕上最后一个东西，直到下一个请求碰巧到来。原因是卡片的两半都活在对话列表**旁边**的槽位里：`SessionState::request` 放待答的、`SessionState::last_request` 放已答的，`transcript_item_count` 把它们渲染成 `1 + rows + extra`。列表里任何一行都不可能排到已答复卡片前面，因为那张卡根本不在列表里。终端端从来不是这个形状：它的 select 弹窗在答复时关闭（`log_confirm = false`），决定则以 `StepResult.permission_label` 的形式落到工具卡的 meta 行上。

**Decision:** 已答复的授权就是一条转录行。`TranscriptRow::Approval { request, result }` 同时携带问题与决定，`Conversation::push_approval` 负责追加，`answer()` 走普通的 `Change::Appended(1)` 路径归档；`RequestAnswer` 与 `last_request` 槽位一并删除。只有**待答**的请求仍然排在行列表之外，因为那正是必须给出答案的位置；已答复的卡片没有理由继续吊在比它更晚的行下面。卡片本身的样子没变：仍是原型的 `.approval.done`，动作换成决定，记录依旧可读，只是跟着这一轮一起滚走。顺带掉出两个小结果：新请求不再需要清掉一张残留的已答复卡（已经没有可清的），`to_markdown` 多了一个分支，复制按钮把已答复卡写成 `> <prompt> -> <decision>`。由于行渲染器现在需要卡片自己的渲染器，shell 以 `ApprovalCard` 回调的形式交给它，并把 `render_row` 的回调打包进 `RowActions`，以留在 clippy 的参数上限之下。

**Behavior after:**答复授权卡或提问卡会清掉待答提示，并把已答复卡片追加到它被问到的那个位置。这一轮之后写下的任何行都落在它下面，卡片随其余对话一起滚走，而不再悬在它们底下。决定仍然读作选项自己的标签（`Allow once` / `Deny` / `Always allow this tool`），卡片仍然丢掉被用来答复的那一行选项，待答请求也仍然渲染成末尾那张没有决定行的卡片。

**Pointers:** `crates/tact-gui/src/session.rs`（`push_approval`、`to_markdown`）；`crates/tact-gui/src/shell.rs`（`answer`、`answer_panel`、`transcript_item_count`）；`crates/tact-gui/src/transcript.rs`（`RowActions`、`Approval` 分支）；`crates/tact-gui/src/shell.rs` 测试（`an_answered_approval_disappears_instead_of_remaining`）；`crates/tact-gui/tests/shell.rs`（`the_permission_card_leads_with_deny`、`the_once_permission_option_answers_the_card`、`the_lasting_permission_option_answers_the_card`）

## 1. 2026-09-21 — 重新打开的会话会重绘它自己存下来的对话

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-session/src/history.rs`（`history`、`HistoryBlock`、`HistoryMessage`、`tool_detail`）；`crates/tact-session/src/lib.rs`（re-export）；`crates/tact-session/src/test_support.rs`（`seed_session_history`）；`crates/tact-gui/src/session.rs`（`session::history`、`Conversation::load_history`、`NO_TIMESTAMP`）；`crates/tact-gui/src/shell.rs`（`replay_history`、`resume_session`）；`crates/tact-gui/src/transcript.rs`（`clock_label`）；`crates/tact/src/store/session_store/sqlite.rs`（`messages` 表） |

**现象 / 动机：** 在侧栏点一个会话时，壳层确实接管了那个会话的 runtime——agent 那边是真的保留了早前的轮次——但窗口给出的是一片空白。`adopt` 会为它接管的会话重置 `Conversation`，之后没有任何东西把它重绘回来，于是用户一离开再回来，对话就消失了，尽管每条消息都还在磁盘上。问题从来不在 store：`messages.content` 存着规范的 `MessageContent` block 向量，`Thinking`、`ToolUse`、`ToolResult` 都是原样落库的。缺的是从 store 到前端的那条路——`tact-session` 没有暴露任何读取入口，而 `tact-gui` 是刻意不依赖 `tact` / `tact_llm` 的。

**决策：** 在 `tact-session` 本来就负责的那条缝上切开。新增的 `tact_session::history::history(workdir, session_id)` 打开工作区的 session store、加载消息，并把它们摊平成与展示无关的 `HistoryMessage`（由 `HistoryBlock` 组成：`Text`、`Thinking`、`ToolUse { id, name, detail }`、`ToolResult { tool_use_id, output }`），于是前端重绘一个会话既不需要 store handle，也不需要 `tact_llm` 类型。`tact-gui` 在 `Conversation::load_history` 里把它们映射成自己的行，并在每次 resume 时由 `replay_history` 调用。这个决策刻意保留 block 顺序而不是按类型重新分组，因为正是这个顺序让重绘出来的对话仍按 `think -> 工具 -> 内容` 读，与那一轮真实的产出顺序一致。保真度严格限定在 store 真实持有的范围内：卡片的耗时、实时输出尾巴、生产者算出的 `arg_summary` 都属于从未持久化的展示态，所以重绘的卡片改从约定的入参键（`command`、`file_path`、`path` ……）取 detail 行、退回通用 `ToolVisualKind`，并在 store 里没有对应结果时保持 `Running`——这也正是实时对话在一轮于工具中途结束时留下的样子。`load_session` 不返回每行的 `created_at`，因此重绘行带 `NO_TIMESTAMP`，`clock_label` 对它不打印任何东西，而不是把一条旧消息标成「会话被重新打开的那一刻」。

**改后行为：** 切换会话会按存储顺序重绘存下来的轮次：重新打开的线程能看到自己的 `think -> 工具 -> 内容` 序列，工具卡片占住它自己那条 `ToolUse` 的位置并填入对应的 `ToolResult`。resume 提示仍落在重绘行之后。store 从未持有的东西回不来——耗时、请求卡片、进度行、模型归属、每条消息的时钟在重绘行上都不存在；压缩依然会删掉被摘要取代的 block，所以那些对**所有**读取方都是消失的，不只是 GUI。

**指针：** `crates/tact-session/src/history.rs`（`a_reopened_session_redraws_thinking_tool_and_answer_in_order`）；`crates/tact-session/src/test_support.rs`（`seed_session_history`）；`crates/tact-gui/src/session.rs`（`a_stored_transcript_redraws_thinking_tool_and_answer_in_order`、`a_redrawn_tool_card_keeps_its_use_slot_and_a_stray_result_is_ignored`）；`crates/tact-gui/src/shell.rs`（`switching_sessions_redraws_the_stored_transcript`）；`book/01_chapter_store_zh.md`（把会话读回来）

## 1. 2026-09-21 — 流式正文就停在工具卡与思考卡给它让出的位置

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/session.rs`（`Conversation::seal_open_assistant`、`ensure_tool`、`apply_thinking`、`end_turn`、`append_stream`、`push_user`）；TUI 先例 `crates/tui/src/widgets/state/app/agent.rs`（`ToolEvent::Started` 之前先 `flush_stream_pending`）、`crates/agent_tui_kit/src/state/log.rs`（`push_placeholder_rows`）、`docs/tool_rendering.md`（「The log block appears on `StepStarted`」） |

**症状 / 动机：** 桌面端的对话里，一轮「先流一段正文、再跑一个工具、然后继续流正文」的回合，把后一段正文显示在了它**跟随的那个工具卡上方**。`Conversation` 整个回合只保留一个 `open_assistant` 下标，而每个 `StreamChunk` 只要那一行还是 `Assistant` 就复用它，于是工具之后到达的文本被追加回了工具之前就打开的那一行。思考也是同一个形状：因为工具生命周期没有清掉 `open_thinking`，迟到的 `Delta` 会写回一条已经位于卡片上方的思考行。终端客户端从来没有这个问题——它在分配工具占位行之前会先把待写正文 flush 掉，之后的正文追加在尾部——于是两个前端对同一串事件给出了不同的排布。

**决策：** 卡片就是转录里的边界。`ensure_tool` 以及两处开启思考行的分支现在都先调用 `seal_open_assistant`，于是下一个 chunk 会在卡片**下方**开自己的新行；而工具卡本身仍然占住它首次出现的位置，之后每一次 `ToolProgress` / `StepFinished` 都写进同一行——这正是让一个长时间运行的卡片不会在读者眼皮底下移动的原因。这个决策刻意比「任何行都封住流」更窄：回合中途排队的 prompt 仍然不封（`push_user`），因为一个尚未派发的插话不该关掉那条仍在流式输出的行——这条规则由壳层既有的契约测试钉住。

**改后行为：** 一轮对话在两个前端上都按产出顺序读作 `think -> 工具 -> 内容`。工具之前到达的正文留在工具上方，之后到达的留在下方，思考块停在它 `Started` 事件所占据的位置。工具卡在首次见到自己的 id 时分配，其后原地更新。

**指针：** `crates/tact-gui/src/session.rs`（`prose_after_a_tool_opens_a_new_row_below_the_card`、`prose_after_a_thought_opens_a_new_row_below_it`、`a_queued_user_row_does_not_seal_the_open_stream`）；`docs/tool_rendering.md`；`book/23_chapter_tui.md`（工具占位行）

## 1. 2026-09-21 — 第三级 ink 补到由组件绘制的控件上

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（composer 的附件 chip）；`crates/tact-gui/src/pane.rs`（`files_tree`）；`crates/tact-gui/src/theme.rs`（`ink3`、`INK3_LIGHT`、`INK3_DARK`）；`docs/design/tact-desktop-prototype.html`（`--ink3` 第 8-9 行、`.chip button` 第 19 行、`.tree .row2` 第 20 行）；`gpui-component-0.6.4/src/button/button.rs`（`Ghost` 前景色第 964 行、绘制期 `.text_color(normal_style.fg)` 第 663 行、`refine_style` 第 690 行、`impl Styled for Button` 第 546 行）；`crates/tact-gui/src/shell.rs`（`prototype_icon_button`，既有的同款写法） |

**现象 / 动机：** 上面那条「第三级 ink」条目为原型的第三级墨色加了 `theme::ink3`，并把约三十个调用点迁了过去 —— 但它只能迁**外壳自己绘制**的那些。原型里有两个第三级控件是由 `Button` 组件画的，而组件的 variant 自带颜色：`ButtonVariant::Ghost` 渲染 `secondary_foreground`，本主题把它映射到原型的 `--ink`。于是附件 chip 的删除按钮（`x`）落在 `--ink`，而 `.chip button{color:var(--ink3)}` 要的是第三级，**整整强了两档**；文件树的展开箭头同样落在 `--ink`，可它所在的行是 `.tree .row2{color:var(--ink2)}` —— 一个附属操作喊得比它所属的标签还响。这两处此前没被抓到，是因为 `ElementSnapshot` 不暴露颜色：自动化只能证明控件被渲染出来且能按，永远证明不了它画成什么颜色，所以这一类缺口只有像素审计才看得见。

**决策：** 在调用点直接告诉组件。`Button` 实现了 `Styled`，它的 render 会把调用方给的 `StyleRefinement` 细化在 variant 算出的颜色之上（第 663 行 `.text_color(normal_style.fg)`，随后第 690 行 `.refine_style(&instance_style)`），所以写在按钮上的 `.text_color(..)` 在静止态是生效的 —— 这正是 `prototype_icon_button` 给标题栏 `.icon` 方框用的既有写法。chip 的 `x` 取 `theme::ink3(cx)`；树的箭头取 `muted_foreground`，与它所在的行保持一致，而不是另发明一个值，这也是此前那条文件树条目给行本身定下的规则。composer 的 placeholder 是唯一够不到的一处，记录在案而不是糊过去：原型要 `.prompt::placeholder{color:var(--ink3)}`，但 `Input` 是用 `InputEditorStyle.muted_foreground`（`--ink2`）画 placeholder 的，且不提供按实例覆盖；改主题角色也不可行，因为 `the_theme_roles_carry_the_prototype_variables` 把 `muted.foreground` 钉在 `--ink2`，而原型真正写作 `--ink2` 的那些面会跟着一起动。

**改后行为：** 附件 chip 的删除按钮在两种主题下都渲染为原型的第三级墨色；文件树的箭头与它自己的行同色，不再压过行标签。两者仍是 ghost 按钮，hover 与 focus 行为不变。

**已知残留：** composer 的 placeholder 渲染在 `--ink2`，而原型要 `--ink3`；要抹平这一档差异，就得在 `Input` 之外自己画 placeholder，这需要一个决定，而不是悄悄绕过去。这个 `.text_color(..)` 覆盖也只在静止态成立：`Ghost` 会在 `hover` / `active` 的样式闭包里重新施加自己的 `secondary_foreground`，这些闭包注册在 `refine_style` 之前、指针悬停时后写生效，所以悬停中的 chip `x` 与树箭头仍会退回到 `--ink`。同一轮还顺手确认了两件与覆盖度有关的事，并都记进了设计评审：宽口径点击巡检抓不住「死了的控件」，因为 `every_entry_point_answers_a_click` 的 `click!` 宏只断言「渲染出来了 + 按下去没崩」；工作面板那五个 ghost 按钮（`Open in editor`、`Refresh plan`、`Comment`、`New task`、`Add file`）仍是唯一一组「没有处理函数、也没有任何已记录理由」的控件。

**指针：** `crates/tact-gui/src/theme.rs`（`ink3`、`the_tertiary_ink_matches_the_prototype`）；`crates/tact-gui/src/shell.rs`（附件 chip、`prototype_icon_button`）；`crates/tact-gui/src/pane.rs`（`files_tree`）；`docs/design/tact-desktop-prototype.html`（第 8-9、19、20 行）；`docs/design/tact-desktop-design-review.md`（Phase 4-7 跟进清单、Parked items）。

## 1. 2026-09-21 — 手绘控件开始响应键盘

| 字段 | 值 |
|-------|-------|
| **类型** | feature |
| **相关** | `crates/tact-gui/src/shell.rs`（`focus_visible_ring`、`session_row`、`sidebar_meta_row`、`chrome_icon_button`、`tool_button`、`mini_chip`、`send_button`、侧栏的 `.new` 行、标题栏的 `.tab` 条）；`crates/tact-gui/tests/shell.rs`（`tab_reaches_the_drawn_controls_and_enter_runs_them`）；`docs/design/tact-desktop-prototype.html`（第 27 行 `button:focus-visible`、第 34-123 行的 `<button>` 行）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md`（第 89-90、124-125、559 行）；上游 `gpui-pre-0.3.5` `src/elements/div.rs`（element state 自带的 handle 第 2246-2264 行、`focus_visible` 第 1292 行、Enter/Space 合成点击第 3013-3060 行、绘制期门槛第 3425-3430 行） |

**症状 / 动机：** 原型把 `.row`、`.tab`、`.icon`、`.cmd`、`.new`、`.mini`、`.wtab`、`.toolbtn`、`.session`、`.send` 都写成 `<button>`，于是浏览器把它们全部放进 Tab 顺序，并给它们第 27 行的 `button:focus-visible` 环。壳层把同样的方块画成带 `on_click` 的普通 `div`，而 `div` 自己不说的话既不是 Tab 停靠点、也没有焦点环——于是指针能按到它们，键盘却只能到达 composer、搜索框和组件按钮。设计指南把这件事当成硬要求而不是点缀：它要求每个交互控件都为 focus-visible 状态做设计，无障碍清单的第一条就是「每个动作都能用键盘到达并操作」，而 spec 又把这份指南定为键盘访问与焦点的规范来源。

**决策：** 在每个手绘控件的构造 helper 里向 GPUI 要齐两半。`tab_index(0)` 把元素同时标成可聚焦和 Tab 停靠点；只声明到这一步、不交 handle 的元素会从自己的 element state 拿到一个 handle，而 GPUI 会为带这个 id 的元素保留它——按 id 稳定，这是渲染函数里临时 `cx.focus_handle()` 做不到的。`Div::focus_visible` 只在元素已聚焦**且** `window.last_input_was_keyboard()` 时应用样式，也就是 `:focus-visible` 而不是 `:focus`，因此鼠标按下永远不会画出这个环。环本身是 `focus_visible_ring`：用纯强调色铺一个 **inset** 的 2px spread `BoxShadow`——GPUI 没有 `outline`，而阴影不参与布局。用 inset 而不是外阴影是**真机截图逼出来的**，不是审美取舍：GPUI 把外阴影画成元素**背后的一块填充**圆角矩形，于是那些自身背景透明或半透明的控件——只在「当前会话」时才有背景的会话行、标题栏的 `.tab` 芯片——会把这块填充透出来，「环」变成盖住整行的实心橙块。inset 阴影画在背景之后、子元素之前，所以无论背景什么样都保持 2px 的环。两处 `:focus-within` 环继续用 `--accentTint`，因为原型那两处是 `box-shadow`。激活那一半是 GPUI 自带的：带 `on_click` 的已聚焦元素会在未修饰的 Enter 或 Space **抬起**时收到合成点击，与原生 `<button>` 一致。这条契约既不能靠快照也不能靠像素：`ElementSnapshot` 读不到颜色，而这些控件用的是 element state 里的 handle、不是交给 `.test_support()` 的 handle；于是测试用 Tab 加 Enter 走到标题栏的侧栏开关——预览窗口里唯一能这样关掉侧栏的控件——并手工派发按键**抬起**，因为测试夹具的 `press` 只发按下。

**改后行为：** Tab 按原型自己的顺序走过手绘的行、页签、图标与芯片，键盘按键会在当前聚焦的那个控件上画出强调色键盘环。Enter 或 Space 执行该控件自己的点击，于是这些控件不只是「可到达」，而是「可操作」。在预览壳层实测：改动前可到达 6 个 Tab 停靠点，改动后是 34 个；环本身也在活的 Wayland 截图里看过——聚焦的会话行读作 2px 强调色环（该行区域内约 700 个强调色像素），而不是外阴影版本画出的实心块（约 16000 个）。

**已知残留：** 原型的 `outline-offset: 2px` 在 GPUI 没有对应物，所以环贴着盒子画——阴影改成 inset 之后是贴在内侧——而不是离开 2px。refine 样式是整体替换阴影列表而不是追加，因此聚焦的活动标题页签会用环替代自己的 `0 1px 2px` 抬升，聚焦的发送方块则用环替代顶部高光。

**指针：** `crates/tact-gui/src/shell.rs`（`focus_visible_ring`、`session_row`、`sidebar_meta_row`、`chrome_icon_button`、`tool_button`、`mini_chip`、`send_button`、侧栏的 `.new` 行、标题栏的 `.tab` 条）；`crates/tact-gui/tests/shell.rs`；`docs/design/tact-desktop-prototype.html`；`docs/design/tact-desktop-design-review.md`。

## 1. 2026-09-21 — 强调按钮的高光、活动页签的抬升、ghost 按钮的描边与主题阶梯的钉住

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`send_button`、`prototype_button`、工作区页签、`request_panel`）；`crates/tact-gui/src/theme.rs`（`TINT_ALPHA_LIGHT`、`TINT_ALPHA_DARK`、`tint_alpha`、`the_tint_ladder_matches_the_prototype`、`the_theme_roles_carry_the_prototype_variables`）；`docs/design/tact-desktop-prototype.html`（`.tab.active` 第 12 行、`.btn.ghost` / `.btn.primary` 第 18 行、`.send` / `.send.run` 第 19 行、`--orange` 第 8-9 行）；`gpui-component-0.6.4/src/button/button.rs`（`Ghost` 边框第 1032 行、`Custom` 边框第 1033-1039 行、custom 背景第 925/943 行）；`gpui-pre-0.3.5/src/elements/div.rs`（hover 是替换，第 848-855 行；绘制期 refine，第 3454-3469 行） |

**症状 / 动机：** 三个凸起表面仍然画得太平。原型的 `.send` 带 `box-shadow: inset 0 1px 0 rgba(255,255,255,.2)`，而 `.send.run` 只换填充和边框，所以运行中的停止状态也应保留这条高光；发布版的方形按钮完全没有阴影。`.btn.primary` 带同样的内阴影，让权限卡的 “Allow always” 和提问卡的 “Confirm” 读起来是凸起的，但按钮组件自带的 `shadow(bool)` 不能接收阴影列表。活动标题页签还缺原型的 `0 1px 2px rgba(20,20,19,.06)` 抬升（它的描边已经由页签边框建模）。另外，主题角色测试只钉了五个映射，三道墨阶、线框角色、强调色的 hover/pressed 档和状态色都可能漂移；tint 透明度还是一对裸字面量；原型未被读取的 `--orange` 也没有与主题的 `base.yellow` 一起说明。此外，ghost 按钮在指针悬停时丢掉了描边：壳层手工恢复了原型的 `1px solid var(--line)`，但组件的 `Ghost` variant 在任何状态都返回 `transparent` 边框，而它的 hover 样式是在绘制期 refine 到调用方样式之上的，于是指针一到描边就消失。

**决策：** 每个值都取自原型。`send_button` 在 running/idle 分支之前无条件加上内阴影 `BoxShadow`，于是强调色发送键和停止键都有同一条高光。`prototype_button` 通过 `<Button as gpui_kit::Styled>::shadow(...)` 给主按钮加同样的内阴影——因为组件自带的 `Button::shadow(bool)` 占用同名 setter；组件会克隆调用方的实例样式，并在最后把它 refine 到 variant 的填充与边框之上，所以调用方的阴影列表能保留。活动工作区页签用原型自己的 `0x141413` 墨色加上 `0 1px 2px` 抬升，而不是用暗色下会翻成近白的 foreground；`0 0 0 1px var(--line)` 仍由边框承担。主题测试现在覆盖 21 个可达映射，并对 `:root` 与 `:root[data-theme=dark]` 逐条断言。`TINT_ALPHA_LIGHT` / `TINT_ALPHA_DARK` 取代裸的 tint 透明度，`the_tint_ladder_matches_the_prototype` 从两个原型块解析 `--accentTint`、`--redTint`、`--greenTint`、`--blueTint` 并比较 alpha。`.btn.ghost` 这种形态从 `Ghost` variant 改到 `ButtonVariant::Custom`：它的边框颜色在各状态间不变，并且可以显式指定 hover 填充。调用点无法自己追加 hover 样式——`hover` 会断言其未被设置并整体替换，而且 `Ghost` 的 hover 样式会在绘制期继续覆盖调用点的边框。静态填充仍由调用点的 `.bg`（`--surface`）承担，因为 `Custom` 自带的常态背景是它颜色的 20% 混合。

**改后行为：** 发送/停止方块与审批/提问的主按钮在两个主题下都带原型顶部高光；选中的工作区页签从轨道上抬起，同时保留线框描边；21 个角色映射与四个 tint alpha 都钉在两个原型块上；`base.yellow` 继续有意不匹配原型未使用的 `--orange`；ghost 按钮在指针悬停时也保留自己的 `--line` 描边，而不是把它丢掉。

**已知残留：** 两个发布主题里的 `base.yellow` 都是 `#D9A441`，而原型的 `--orange` 浅色是 `#9F5D2F`、暗色是 `#D9A441`；原型声明了 `--orange` 却从未读取（`var(--orange)` 没有出现），壳层也没有表面读取 `yellow`，所以浅色值保持不动，不靠猜测重映射。有一处 ghost 细节仍未对齐：悬停时的描边保持在 `--line`，没有跟进到原型的 `--line2`，因为 `Custom` 在各状态间只有一个边框颜色。

**指针：** `crates/tact-gui/src/shell.rs`（`send_button`、`prototype_button`、工作区页签、`request_panel`）；`crates/tact-gui/src/theme.rs`（`TINT_ALPHA_LIGHT`、`TINT_ALPHA_DARK`、`tint_alpha`、`the_tint_ladder_matches_the_prototype`、`the_theme_roles_carry_the_prototype_variables`）；`docs/design/tact-desktop-prototype.html`（`.tab.active` 第 12 行、`.btn.primary` 第 18 行、`.send` / `.send.run` 第 19 行、`--orange` 第 8-9 行）；`gpui-component-0.6.4/src/button/button.rs`；`gpui-pre-0.3.5/src/elements/div.rs`；`docs/design/tact-desktop-design-review.md`。

## 1. 2026-09-21 — 三级墨 `--ink3`、对话框遮罩与 composer 的暗色阴影

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/theme.rs`（`ink3`、`activate`）；`crates/tact-gui/src/shell.rs`（`sidebar_meta_row`、`prompt_composer`、palette 页脚）；`crates/tact-gui/src/pane.rs`；`crates/tact-gui/src/transcript.rs`（`TranscriptRow::Error`）；`docs/design/tact-desktop-prototype.html`（`--ink3` 第 8-9 行、`.composer` 第 19 行、`.overlay` 第 22 行、worktree 行第 65 行） |

**症状 / 动机：** 第三遍像素审计发现壳层根本没有原型三级墨的角色，于是把它悄悄压成了两级。原型在 `--ink` 与 `--ink2` 之外还定义了 `--ink3`（浅色 `#716e65`、深色 `#8c8a80`），而发布的主题把 `muted.foreground` 映射到 `--ink2`，因此凡是复用 `muted_foreground` 充当三级墨的调用点都深了一级——在 palette 分组标题、状态栏元信息、卡片注释这些原型刻意压到 `--ink2` 之下的位置最明显。另外两个取值也落在了主题默认值上，而不是原型自己的值：`.overlay` 两个模式都固定 `rgba(20,20,19,.18)`，组件默认却随主题翻转；`.composer` 的常态阴影是固定墨色，卡片却从 `foreground.opacity(...)` 推导，在暗色下变成近乎白色的光晕。worktree 的中性状态圆点漂得同样远：`.dot` 是 `--line2` 加 `--surface2` 光晕，而不是半透明墨色。

**决策：** 补上缺失的主题角色，而不是再近似一次。`theme::ink3(cx)` 返回 `INK3_LIGHT` / `INK3_DARK`，并用一条单元测试从原型两个 `:root` 块里解析 `--ink3`，让常量无法悄悄漂移；随后把 shell、pane、transcript 的调用点迁到它上面（约三十处），而原型确实写成 `--ink2` 的位置（`.status strong`、`.tree .row2`）继续留在 `muted_foreground`。对话框遮罩在 `Theme::change` **之后**钉成 18% 的 `0x141413`，因为该调用会执行 `apply_config` 重置颜色表，写在它前面的赋值会被覆盖；这条由 `activation_pins_the_dialog_overlay` 锁住。composer 的常态阴影直接取同一个墨色常量，不再用会翻转的 foreground；`sidebar_meta_row` 的中性圆点回到 `--line2` / `--surface2`，它最后一个 `muted` 参数也随之退休。

**改后行为：** 三级标签、注释、图标与状态栏元信息在两个主题下都按原型第三级渲染，而真正的 `--ink2` 表面保持不变；命令面板的遮罩在浅色与深色下重量一致；composer 在暗色下浮在阴影上而不是浅色光晕上；空闲 worktree 圆点读作中性的线与面。transcript 的错误行现在带 `Role::Alert` 和错误文本的无障碍标签，屏幕阅读器会念出这一行已经画出的失败。

**已知残留：** 原型的 `.overlay` 还要求 `backdrop-filter: blur(2px)`，当前 GPUI 表面没有暴露该能力；遮罩只钉了颜色、没有模糊，这一偏差记录在设计评审里。

**指针：** `crates/tact-gui/src/theme.rs`（`ink3`、`INK3_LIGHT`、`INK3_DARK`、`activate`、`activation_pins_the_dialog_overlay`、`the_tertiary_ink_matches_the_prototype`）；`crates/tact-gui/src/shell.rs`（`sidebar_meta_row`、`prompt_composer`、palette 页脚、状态栏）；`crates/tact-gui/src/pane.rs`（work 页脚、面板标题、步骤、计划行、diff 行号、状态徽章）；`crates/tact-gui/src/transcript.rs`（`TranscriptRow::Error`、代码头带、msg meta、thinking 与 tool meta）；`crates/tact-gui/tests/shell.rs`（`the_error_row_reports_its_text_as_an_alert`）；`docs/design/tact-desktop-prototype.html`（`--ink3` 第 8-9 行、`.composer` 第 19 行、`.overlay` 第 22 行、worktree 行第 65 行）；`docs/design/tact-desktop-design-review.md`。


---


## 1. 2026-09-21 — 工具图标、任务表头、worktree 行与两处 focus 环

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/transcript.rs`（运行中的工具图标）；`crates/tact-gui/src/pane.rs`（`task_header_row`、文件树行）；`crates/tact-gui/src/shell.rs`（`sidebar_meta_row`、`worktree_row`、`approval_details`）；`docs/design/tact-desktop-prototype.html`（`.tool.run .toolIcon` L17、`.approval p` L18、`.tasks th` 与 `.tree .row2` L20、`.row` L13、worktree 行 L65） |

**症状 / 动机：** 同一轮像素审计的第二遍又找出六处：桌面壳层落在了组件或主题默认值上，而不是原型自己的取值。运行中的工具把图标画成 `--accent`（`primary`）压在 `--accentTint` 底色上，而 `.tool.run .toolIcon` 要的是 `--accentInk`——浅色下 `#D97757` 应为 `#AB5036`。任务表把 `Task`/`Status`/`Owner` 原样输出，而 `.tasks th` 会做大写转换。审批段落只设了 12 px、没有行高，而 `.approval p` 要求 `1.5`。文件树的行继承了 `--ink`，而 `.tree .row2` 要求 `--ink2`。当前 worktree 行被画成 `.row.active`——`--accentTint` 填充、accent 圆点、主题的 6 px 圆角——而原型把它画成普通 `.row`（7 px），唯一的标记是 `.badge.run`。另外两条 `:focus-within` 规则——`.search` 与 `.composer`——完全没有生效：两个组件都以 `appearance(false).bordered(false)` 创建，所以既没有强调色边框、也没有 2 px `--accentTint` 外圈，composer 卡片还缺了它的常态 `box-shadow`。

**决策：** 组件默认值与原型冲突时一律取原型值。运行中工具的底色保留 `--accentTint`，图标改用 `cx.theme().accent_foreground`。表头写成字面量 `TASK`/`STATUS`/`OWNER`，因为 GPUI 没有 `text-transform`。审批段落补 `line_height(relative(1.5))`。文件树的行以 `muted_foreground`（`--ink2`）起步，再由既有的 `when(is_expanded)`/`hover` 层抬到 `--accentInk`。`worktree_row` 的圆点与活动填充传 `false`，保留 `.badge.run`，并保留 `selected`，让无障碍树仍能报出当前 worktree；侧栏行改用 `px(7.)`，即原型 `.row` 的圆角，而不是主题的 6 px。搜索框与 composer 卡片改为跟踪各自输入框的 focus handle（`track_focus` + `focus`），这是 GPUI 里容器表达 `:focus-within` 的方式：获得焦点时边框转为强调色，搜索框底色抬到 `--surface`，两者都画出 2 px 的 `--accentTint` 外圈。composer 卡片同时补上常态阴影（`--ink` 4% 的 `0 1px 2px` 与 3.5% 的 `0 8px 24px`），聚焦样式会像原型的两条 `box-shadow` 声明那样把它替换掉。

**改后行为：** 运行中的工具是「底色上的深色墨」，而不是强调色压在底色上；任务表表头为大写；审批正文的折行行距是字号的 1.5 倍；文件树的常态行使用次级墨色；当前 worktree 不再看起来像被选中——它的唯一标记是 `1 active`，而 `aria-selected` 仍然指明窗口当前锁定哪个 worktree；搜索框或 composer 获得焦点时边框转为强调色、搜索框底色抬到 `--surface` 并画出外圈，而内部输入框未聚焦时 composer 卡片靠常态阴影浮起。

**已知残留：** 目录行的展开开关是 `gpui-component` 的 ghost `Button`，图标默认用 `secondary_foreground`（`--ink`）、hover 时切到 `accent_foreground`，所以未 hover 时它不跟随行的 `--ink2`。外圈本身是颜色与阴影，`ElementSnapshot` 读不到，因此它们由构造方式保证，而不是由集成断言锁住。

**指针：** `crates/tact-gui/src/transcript.rs`（运行中的工具图标）；`crates/tact-gui/src/pane.rs`（`task_header_row`、文件树行）；`crates/tact-gui/src/shell.rs`（`sidebar_meta_row`、`worktree_row`、`approval_details`）；`docs/design/tact-desktop-design-review.md`。


---

## 1. 2026-09-21 — 侧栏、标题页签与 tint 表面通过像素审计

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（标题页签、`sidebar_row_fills` 与 hover token、`sidebar_meta_row`）；`crates/tact-gui/src/pane.rs`（tint、hover、末行边框）；`crates/tact-gui/src/transcript.rs`（tint、卡片 hover）；`crates/tact-gui/src/theme.rs`（`tint_alpha`、`accent_tint`）；`crates/tact-gui/tests/shell.rs`（`the_title_bar_controls_use_the_prototype_boxes`、`the_sidebar_lists_worktrees_and_background_work`）；`docs/design/tact-desktop-design-review.md` |

**症状 / 动机：** 对 `docs/design/tact-desktop-prototype.html` 的像素审计发现桌面壳层有十处滑回了组件默认值。侧栏的 hover 与活动行共用同一个 token，指向会话和选中会话看起来一样。Chat/Agent/Code 走的是原生 `TabBar::segmented()`——高 32 px、铺 `--line`——而不是原型 `.tabs`（2 px 内边距、1 px 边框、`--surface2`）配 26 px 的 `.tab`。worktree 与 background 行没有使用 `.row` 盒子。tint 底色被硬编码成 `.12`，而浅色主题应为 `.10`。三处强调文字用了 `primary` 而不是 `--accentInk`。若干元素缺少原型已有的 hover 反馈。plan/tasks 的最后一行还留着 `border-bottom`。状态栏项目 chip 用了完整的 `--ink`，而不是 `--ink2`。新增/删除 diff 行比原型的 `color-mix(... 70%)` 更重。侧栏第一组与页眉相距 16 px，而不是 6 px。

**决策：** 所有取值收拢到单一来源。侧栏 hover 取 `--hover`（`accent.background`），活动行取 `--accentTint`，两者统一经过 `crates/tact-gui/src/theme.rs` 的 `tint_alpha`/`accent_tint` 以及 `sidebar_row_fills` 中的行填充。标题页签在 `crates/tact-gui/src/shell.rs` 手工搭建；每个页签保留整数 id，`within("workspace-tabs")` 仍可点击。`sidebar_meta_row` 采用 `.row` 几何并复用 17 px mono `SessionBadge`。diff 行取 tint alpha 的 70%，列表末行去掉 `border-bottom`，状态栏 chip 与侧栏标题分别使用 `--ink2`/`--accentInk`。

**改后行为：** 侧栏 hover 不再与选中行同色；标题页签不高于 26 px，也不再用边框色作填充；worktree/background 行保持 44 px 最小高度；浅色主题拿到原型 `.10` 的 tint；新增/删除 diff 底色更轻。

**保留的折中：** 命令面板继续使用原生 `Command`。它的选中行绘制 `accent.background` + `accent_foreground`（即 `--hover` + `--accentInk`），而原型想要 `--accentTint` + `--ink`。组件没有逐行样式钩子，`accent.background` 也不能改——它同时承载 `.tab`、`.icon`、`.row`、`.cmd`、`.wtab` 的 hover；要消除差异只能替换面板列表，同时保留组件自己的筛选与键盘行为。

**指针：** `crates/tact-gui/src/shell.rs`（标题页签、`sidebar_row_fills` 与 hover token、`sidebar_meta_row`）；`crates/tact-gui/src/pane.rs`（tint、hover、末行边框）；`crates/tact-gui/src/transcript.rs`（tint、卡片 hover）；`crates/tact-gui/src/theme.rs`（`tint_alpha`、`accent_tint`）；`crates/tact-gui/tests/shell.rs`（`the_title_bar_controls_use_the_prototype_boxes`、`the_sidebar_lists_worktrees_and_background_work`）；`docs/design/tact-desktop-design-review.md`。


---

## 1. 2026-09-21 — 卡片圆角、强调文字色与工具输出的滚动

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/pane.rs`（`card`、`card_with_id`）；`crates/tact-gui/src/transcript.rs`（助手 gutter、工具卡的 `.out`、错误行）；`crates/tact-gui/src/shell.rs`（`accent_tint`、侧栏搜索框、`session_row`、`worktree_row`、`background_row`、`status_pill`、`approval_card`、`status_bar`）；`docs/design/tact-desktop-prototype.html`（`.card`、`.tool`、`.code`、`.badge.run`、`.row.active`、`.gutter`、`.out`、`.search input`、`.accent`） |

**症状 / 动机：** 把原型的布局取值逐条与壳层对照后，还剩五处是按组件默认值画、而不是按设计画的。卡片与代码卡圆角是 15 px，而 `.card`、`.tool`、`.code`、`.thinking`、`.approval` 都写 `--r10`（10 px）——因为主题的圆角基数是 6 px，`radius_2xl()` 是它的 2.5 倍。长命令输出按原型的 150 px 截断，但用的是 `overflow:hidden`，超出上限的部分够不到——原型写的是 `overflow:auto`。强调色上的文字用了 `primary`，而原型的 `--accentInk` 在浅色下要暗好几级、深色下更亮。`--accentTint` 被硬编码成 `.12`，而浅色主题是 `.10`。侧栏搜索还在壳层自己的 30 px 凹槽里画了第二个带边框的输入框，而 `.search input{border:0;background:transparent}` 正是为了避免这个。

**决策：** 每个取值都从原型取，而不是从最接近的组件 token 取。卡片圆角在 `card`、`card_with_id` 与错误行上改成显式 `rems(0.625)`——与壳层其他 rem 盒子同样的处理——因为单一主题圆角表达不了原型 6/8/10/14 这一族。`.out` 保留 150 px 上限并加上 `overflow_y_scrollbar()`；它必须是链条的最后一步，因为它会把盒子变成 `Scrollable` 并重新给被包裹元素赋 id，所以测试锚点移到内层节点，滚动包裹层按行取 id，避免各卡片共用一个滚动位置。强调态改用 `accent_foreground`（`.badge.run`、`.row.active strong`、`.gutter`、`.approval .warn` 与状态栏 `.accent` 规则），并新增 `accent_tint()` 按主题读取 `--accentTint`——浅色 `.10`、深色 `.12`——供它们背后的底色使用。搜索框用 `appearance(false).bordered(false)` 绘制。

**改后行为：** 卡片、代码卡、工具卡与审批卡都使用原型的 10 px 圆角；输出超过 150 px 的命令在自己的框内滚动而不是被裁掉；活动会话标题、它的运行徽标、助手 gutter、审批警告块与 `1 running` 状态段都呈现为强调文字色而不是更亮的强调色；强调底色在两个主题下都是原型的透明度；侧栏搜索是一个凹槽加透明输入框。完整套件——70 个集成用例加单元测试——通过，`cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings` 干净。

**指针：** `crates/tact-gui/src/pane.rs`（`card`、`card_with_id`）；`crates/tact-gui/src/transcript.rs`（助手 gutter 与工具卡的 `.out`）；`crates/tact-gui/src/shell.rs`（`accent_tint`、侧栏搜索、`session_row`、`status_pill`、`status_bar`）；`docs/design/tact-desktop-prototype.html`（`.card`、`.out`、`.badge.run`、`.search input`）；`docs/design/tact-desktop-design-review.md`（像素跟进条目）。


---


## 1. 2026-09-21 — 会话标题 chip 打开了 spec 要求的下拉

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`SessionChip`、`title_bar`、`push_system_row`）；`crates/tact-gui/tests/shell.rs`（`every_entry_point_answers_a_click`）；`docs/design/tact-desktop-prototype.html`（`.session`，第 37 行）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md`（第 230-231 行） |

**症状 / 动机：** 原型把会话标题画成真正的 `<button class="session">` 并带一个 chevron；spec 要求「会话标题带下拉：重命名、复制、归档，以及在适用时在文件系统中显示」。v1 只渲染了触发器、后面什么都没有：一个 `h_flex()` 套着列表图标、截断的标题和 `ChevronDown`，没有 id、没有可按区域、也没有 popover——于是标题栏里唯一长得像菜单的控件，成了唯一点了没反应的控件。

**决策：** 菜单用壳层已经验证过的原语搭出来：`Popover` + `Selectable` 触发器 + `Button` 行，与 composer 的附件菜单、用量菜单同栈。触发器改成一个小 `SessionChip` 类型，完整保留原型的盒子——水平 7 px、垂直 4 px 内边距，6 px 圆角，7 px 间距，240 px 上限的 12.5 px 截断标题，列表图标与 chevron——只新增 hover 与展开态配色。四行接到应用今天确实能做的事上。重命名、复制、归档完全没有后端：`SessionHandle` 只暴露 `session_id`/`submit`/`cancel`/`send`，`tact_protocol::UserCommand` 没有对应变体，sessions 表没有标题列也没有归档列，显示标题还是从第一条用户消息推导出来的。Reveal 有工作区路径，但仓库里没有任何平台打开文件的辅助函数。因此每一行都通过壳层既有的 `push_system_row` 通知说明缺的是什么能力，而不是假装执行——尤其没有把归档映射成删除。

**改后行为：** 按下 chip 会打开面板，按下任意一行都会往 transcript 追加一行说明性 system 行，同时面板保持打开，与 composer 自己的菜单行为一致；在外部按下会关闭它。`every_entry_point_answers_a_click` 会走一遍：打开、断言面板存在、依次按下四行并断言 transcript 每次恰好加一行，最后在外部按下并断言面板消失。spec 里 rename/duplicate/archive/reveal 的语义仍需要 store 或协议契约之后才能真正工作；这一点写在设计评审里，而不是留在暗示中。集成套件为 70 个通过用例。

**指针：** `crates/tact-gui/src/shell.rs`（`SessionChip`、`title_bar`、`push_system_row`）；`crates/tact-gui/tests/shell.rs`（`every_entry_point_answers_a_click`）；`docs/design/tact-desktop-prototype.html`（`.session`，第 37 行）；`docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md`（第 230-231 行）；`docs/design/tact-desktop-design-review.md`（session chip 跟进）。


---


## 1. 2026-09-21 — 输入框不再越过原型的上限继续增长

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`prompt_composer`）；`crates/tact-gui/tests/shell.rs`（`the_prompt_grows_between_the_prototype_minimum_and_maximum`、`the_composer_controls_use_the_prototype_boxes`）；`docs/design/tact-desktop-prototype.html`（`.prompt`，第 19、118 行） |

**症状 / 动机：** `rows="2"` 的 textarea 带着 `.prompt{min-height:48px;max-height:150px}`，这是让长草稿留在 composer 里、而不是让输入框吃掉 transcript 的那条规则。v1 两端都没守住：textarea 上的 `min_h(rems(3.))` 加上外层自己的 `10px`/`6px` padding，让空输入框量到 64 px，比原型下限高出 16 px；而 `.auto_grow(1, 8)` 把唯一的上限放在八行，于是 600 词的草稿量到 192 px——比原型上限超出 42 px——中间没有任何东西拦住它。

**决策：** 把上限表达成行数，因为 GPUI 的 textarea 对自动增长框是按整行（`rows * window.line_height()`）布局，而不是按连续像素：textarea 上的 `min_h`/`max_h` 在这套布局里不会生效，而低于行最小值的 `max_h` 还会输给行本身。`.auto_grow(1, 5)` 是仍能落在原型 150 px 上限之内的最大行数——六行已经量到 152 px。textarea 保留原型的 `13px`/`1.5` 文本度量，其 accessibility id 改为 `prompt-composer-field`，这样输入框与 `.prompt` 外框不再共用同一个 id。

**改后行为：** 输入框静止时量到 52 px，比原型下限高 4 px、比 v1 矮 12 px；600 词草稿把它撑到 132 px 后停住：比原型上限低 18 px、比 v1 允许的低 60 px。`the_prompt_grows_between_the_prototype_minimum_and_maximum` 会输入这份草稿并固定两端，`the_composer_controls_use_the_prototype_boxes` 继续固定 48 px 下限。集成套件为 70 个通过用例。

**指针：** `crates/tact-gui/src/shell.rs`（`prompt_composer`）；`crates/tact-gui/tests/shell.rs`（`the_prompt_grows_between_the_prototype_minimum_and_maximum`）；`docs/design/tact-desktop-prototype.html`（`.prompt`，第 19 行；textarea，第 118 行）；`docs/design/tact-desktop-design-review.md`（`.prompt` 有上限）。


---


## 1. 2026-09-21 — 剩余的动效与图标回退现在与设计对齐

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/pane.rs`（`PANEL_ENTRANCE`、`panel_entrance`）；`crates/tact-gui/src/transcript.rs`（`MESSAGE_RISE`、`message_rise_policy`、`CHEVRON_ROTATION`、`chevron_rotation_policy`、`chevron_target`、`render_row`）；`crates/tact-gui/src/shell.rs`（`prompt_composer`、`MessageScroller` 的行闭包）；`crates/tact-gui/tests/shell.rs`（动效与点击套件）；`docs/design/tact-desktop-prototype.html`（`@keyframes panel`、`@keyframes rise`、`.chev`、`.send`、`.workFoot`） |

**症状 / 动机：** 壳层此前已经对齐了原型两个浮层的滑动，但剩下的可见动效仍然是硬切或替身：`.panel.active` 只有淡入、没有那 3px 抬升；新到的 `.msg` 行不会上升；两个可折叠摘要还在 `ChevronRight` 与 `ChevronDown` 之间换图标，而不是让同一个 chevron 用 `160ms` 旋转。另有两个静态图标仍来自组件默认值：运行中的 composer 动作用了暂停图标，而原型画的是方形停止；`Open in editor` 用了终端图标，而原型画的是书。

**决策：** 每个一次性入场都继续使用原型自己的 `--ease` 曲线。`.panel` 保留原有的 `with_animation` 淡入，并补上相对的 `top(3px * (1 - progress))` 替身——这是视觉 inset，不是布局变化，所以不会推动滚动容器。`.msg` 在用户与助手行上使用 `Presence`，首次采样从透明度 `0` 与 `translateY(5px)` 开始，`320ms` 后收敛；位移同样落在相对的 `top` inset 上。虚拟列表只渲染可见行，所以屏幕外的行会在第一次进入渲染窗口时才开始上升，而不是仍处于虚拟化状态时——这是有意接受的、比 CSS `both` 弱的一点。两个摘要现在都渲染同一个 `Icon::new(IconName::ChevronRight)`，并用 `transition((index, channel), chevron_target(open), chevron_rotation_policy(), window, cx)` 驱动，所以一次切换会让真实 SVG 在 executor 时钟上从 `0 -> pi/2` 旋转；中途反向按下时会从当前角度回退。composer 的运行图标改为 `SquareStop`，work footer 的图标改为 `Book`。

**改后行为：** 切换 work pane 标签时，新面板用 `180ms` 淡入并抬升；新到的用户/助手行用 `320ms` 上升；thinking 与 tool 摘要用 `160ms` 旋转同一个 chevron；运行中的 composer 动作是方形停止图标；`Open in editor` 带原型的书本图标。新的 chevron 契约由 `the_chevron_rotates_a_quarter_turn_over_the_prototype_duration` 单测固定，现有集成套件继续覆盖每条入场与点击路径。减弱动态效果时，`Presence` 与 `transition` 的采样仍然直接解析到终态，无需逐个调用点加判断。

**指针：** `crates/tact-gui/src/pane.rs`（`panel_entrance`、`view`）；`crates/tact-gui/src/transcript.rs`（`MESSAGE_RISE`、`message_rise_policy`、`CHEVRON_ROTATION`、`chevron_rotation_policy`、`chevron_target`、`render_row`）；`crates/tact-gui/src/shell.rs`（`prompt_composer`、`MessageScroller::new`）；`docs/design/tact-desktop-prototype.html`（`@keyframes panel`、`@keyframes rise`、`.chev`、`.send`、`.workFoot`）；`docs/design/tact-desktop-design-review.md`（Phase 4-7 跟进）。


---


## 1. 2026-09-20 — 抽屉与浮起的侧栏现在两个方向都会滑动

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`OVERLAY_SLIDE`、`WORK_PANE_DRAWER_OFFSET`、`SIDEBAR_OVERLAY_OFFSET`、`WORK_PANE_DRAWER_PRESENCE`、`SIDEBAR_OVERLAY_PRESENCE`、`overlay_slide`、`overlay_inset`、`work_pane_drawer`、`sidebar_overlay`、`TactApp::render`）；`crates/tact-gui/tests/shell.rs`（`settle_motion`、`the_work_pane_drawer_slides_in_and_out_over_the_prototype_duration`）；`docs/design/tact-desktop-prototype.html`（第 25-27 行） |

**症状 / 动机：** 原型里会动的只有这两个浮层，而且每次切换都动：`@media(max-width:1120px)` 给 `.work` 的是 `transform:translateX(105%)` 配 `transition:transform 180ms var(--ease)`，它那条 `<=880px` 的同族规则给浮起 `.sidebar` 的是镜像的 `translateX(-105%)`；而关闭路径只翻转 `body.workOpen`——面板本身从不被移除，是「过渡回画面之外」。v1 在标志翻转的那一帧直接挂载/卸载两者，于是抽屉和侧栏都是硬切出场，关闭方向更是完全没有原型那条退出过渡的对应物。

**决策：** 把两个问题分开问，因为答案不同。某个浮层「能不能」是抽屉形态，只取决于宽度（`work_pane_drawer_form`）；它「在不在画面上」交给 `Presence`——退出期间保留节点，过渡结束的那一帧才丢弃。180ms 与 `cubic-bezier(.23,1,.32,1)` 取自原型自己的令牌，收在一个 `overlay_slide()` 里，两个浮层共用同一套时序。这个 GPUI 没有绘制层的 transform，所以 `overlay_inset` 把 `translateX(±105%)` 落在浮层自身的锚定 inset 上——同一份几何，抽屉写 `right`、侧栏写 `left`。scrim 被刻意排除在滑动之外：原型把它写成自己那条窄规则里的 `display` 开关，所以它与面板同帧出现/消失，抽屉是「不被压暗地」滑出去的。

**改后行为：** 在「面板没有列位」的宽度上按预设，抽屉从 960 + 441px 处用 180ms 滑入；关闭时节点仍然挂着并往回滑，滑完才卸载——`the_work_pane_drawer_slides_in_and_out_over_the_prototype_duration` 把时钟推到 90ms 与 290ms 并检查两端，且做过两次变异验证（把 inset 变成空操作、以及关闭即卸载，各自都会让它失败）。减弱动态效果无需逐浮层判断即已生效：`gpui-base` 的 motion 层在 `App::reduce_motion()` 为真时直接解析到终态；契约测试正是靠打开它（`settle_motion`）来断言稳定几何，所以「读取浮层最终位置」的那些测试不会与过渡抢时间。集成测试为 69 个通过。

**指针：** `crates/tact-gui/src/shell.rs`（`OVERLAY_SLIDE`、`overlay_slide`、`overlay_inset`，以及 `TactApp::render` 里两处 `Presence` 采样）；`crates/tact-gui/tests/shell.rs`（`settle_motion`、`the_work_pane_drawer_slides_in_and_out_over_the_prototype_duration`）；`docs/design/tact-desktop-design-review.md`（动效条目）；`docs/design/tact-desktop-prototype.html`（`@media(max-width:1120px)` 及其 `<=880px` 同族规则，第 25-27 行）。

---

## 1. 2026-09-20 — 桌面三列在 1320px 以下一起收缩

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`Columns`、`Columns::for_width`、`Columns::NARROW_UNDER`、`TranscriptFrame`、`title_bar`、`sidebar`、`work_pane`、`transcript`）；`crates/tact-gui/tests/shell.rs`（`the_narrow_breakpoint_shrinks_the_three_columns`）；`docs/design/tact-desktop-prototype.html`（第 24 行） |

**症状 / 动机：** 原型的第一条媒体查询让三列一起收缩——`@media(max-width:1320px){:root{--sidebar:244px;--work:374px}.thread{width:min(680px,calc(100% - 36px))}}`——而 v1 在任何宽度都保持 260 / 420 / 720。受影响的并不只是列宽：`.thread` 的规则把转录限制在「列宽减去两侧留白」，所以原型在 1320px 以上始终留 48px（每侧 24px）、以下留 36px；而 v1 的 `w_full().max_w(720px)` 让正文吃满整列，转录列一旦窄于 720px 就完全丢掉了留白。

**决策：** 每帧用 `Columns::for_width(width, rem_size)` 解析出这三个宽度，再交给所有按列绘制的界面：标题栏左右两段、流内工作面板、侧栏列，以及转录的 text measure。留白改由转录列的内边距承担——同一条 `min(680px, 100% - 36px)` 规则在 flex 里的写法：内边距之内的 `w_full()` 就是 `100% - 留白` 那一项，`max_w` 是上限。只有列会收缩：工作面板在抽屉形态下仍是 420px，因为原型自己的 `@media(max-width:1120px)` 块在它变成浮层时把 `.work` 重置为 `min(420px,88vw)`；浮起的侧栏也仍取列宽。

**改后行为：** 1300px 窗口下侧栏量到 244px、面板 374px、转录 646px（682px 的列减去 36px 留白）；1440px 下壳层与原来完全一致——260 / 420 / 720 加每侧 24px 留白，宽布局不受影响。`the_narrow_breakpoint_shrinks_the_three_columns` 在 1300px 钉住这三个数值，并做过变异验证（把解析器强制走宽分支即失败）。集成测试为 68 个通过。

**指针：** `crates/tact-gui/src/shell.rs`（`Columns`、`Columns::for_width`、`Columns::NARROW_UNDER`、`TranscriptFrame`、`title_bar`、`sidebar`、`work_pane`、`transcript`、`TactApp::render`）；`crates/tact-gui/tests/shell.rs`（`the_narrow_breakpoint_shrinks_the_three_columns`、`wide_window_lays_out_three_columns`）；`docs/design/tact-desktop-design-review.md`（列宽相关条目）；`docs/design/tact-desktop-prototype.html`（`@media(max-width:1320px)`，第 24 行）。

---

## 1. 2026-09-20 — 工作区抽屉响应 Escape，窄窗口按预设会把它带回来

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`select_work_pane`、`work_pane_is_drawer`、`on_stop_task`、`TactApp::render`）；`crates/tact-gui/src/pane.rs`（工作面板标签）；`crates/tact-gui/tests/shell.rs`（`a_preset_press_opens_the_drawer_only_where_the_pane_has_no_column`、`escape_closes_the_work_drawer_but_leaves_the_column_alone`）；`docs/design/tact-desktop-prototype.html`（`pane()`，第 148 行；全局 keydown，第 159 行） |

**症状 / 动机：** 抽屉又漏了原型的两条规则，两条都源于「低于壳层 1280px 阈值时，抽屉是面板唯一的容身之处」。原型 `pane(name)` 结尾是 `if(innerWidth<=1120) work(true)`——窄窗口上选中一个面板同时也是要求看见它；而 v1 的 `set_work_pane` 只赋值，于是在 1100px 按下预设，面板被换到了已关闭的抽屉背后，屏幕上什么也没变：标签亮了、正文没出现，正是上一轮浮起侧栏那种「按了等于没按」。另一处是原型的全局 keydown 写 `if(overlay.open) closePalette(); else if(body.workOpen) work(false)`，而 v1 把 `escape` 绑给了 `StopTask`，抽屉没有任何 Escape 通路：抽屉开着且没有回合在跑时 `stop()` 直接早退，这个键什么也不做。

**决策：** 当面板没地方可放时，「选中面板」与「让面板可见」是同一个动作：`select_work_pane(pane, window)` 赋值之后再判宽度，低于 `WORK_PANE_IN_FLOW_FROM`（即「打开的面板只能是抽屉」的那个宽度）就把抽屉打开。高于该宽度时 `innerWidth` 守卫的语义保持不变——按预设不会把已关闭的列打开。`work_pane_is_drawer(window)` 成为抽屉形态的唯一判定，由 `TactApp::render` 与按键通路共用，`on_stop_task` 在触达任务之前先解散抽屉。Escape 只有抽屉这一半需要写代码：获得焦点的对话框自带 `Dialog` 键上下文并压过 `TactApp`，所以调色板早已把 Escape 当作自己的 `Cancel` 消费掉，轮不到壳层。

**改后行为：** 1100px 下先关掉抽屉再按预设，抽屉会带着该预设的面板回来；1440px 下同样一按仍然保持关闭，于是宽度条件本身就是契约而不是测试的布置细节。抽屉浮在转录之上时 Escape 会解散它；面板自己拥有列宽时，Escape 保持 v1 语义去停任务。`a_preset_press_opens_the_drawer_only_where_the_pane_has_no_column` 与 `escape_closes_the_work_drawer_but_leaves_the_column_alone` 都在两种宽度上跑并断言结果不同；两条都做过变异验证（把所钉的分支删掉即失败）。集成测试为 67 个通过。

**指针：** `crates/tact-gui/src/shell.rs`（`select_work_pane`、`work_pane_is_drawer`、`on_stop_task`、`TactApp::render`）；`crates/tact-gui/src/pane.rs`（工作面板标签）；`crates/tact-gui/tests/shell.rs`（`a_preset_press_opens_the_drawer_only_where_the_pane_has_no_column`、`escape_closes_the_work_drawer_but_leaves_the_column_alone`、`the_workspace_tabs_pair_each_preset_with_its_pane`）；`docs/design/tact-desktop-design-review.md`（Escape 与 `pane()` 两条）；`docs/design/tact-desktop-prototype.html`（`pane()`，第 148 行；全局 keydown，第 159 行）。

---

## 1. 2026-09-20 — 工作区抽屉补上原型的 scrim，浮起的侧栏保住自己的点击

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`work_pane_scrim`、`work_pane_drawer`、`sidebar_overlay`、`TactApp::render`）；`crates/tact-gui/src/transcript.rs`（`render_code_block`）；`crates/tact-gui/tests/shell.rs`（`the_work_pane_drawer_swallows_presses_behind_it`、`the_sidebar_overlay_keeps_its_presses_above_the_scrim`、`the_floating_sidebar_stays_above_the_work_pane_where_they_overlap`、`the_code_block_copy_chip_writes_its_fence_to_the_clipboard`）；`docs/design/tact-desktop-prototype.html`（`.scrim`、`body.workOpen`、`#scrim` 的点击处理、`.codeHead`） |

**症状 / 动机：** 两个窄窗口缺陷，此前每一个测试都从旁边走了过去。原型用自己的 `.scrim` 兜住窄布局：`body.workOpen .scrim{position:absolute;inset:0;z-index:25;display:block;background:rgba(20,20,19,.14)}`，配 `q('#scrim').onclick=()=>work(false)`——按下抽屉之外的地方只会关掉抽屉，而不是落到抽屉正好盖住的那个控件上。壳层只画了抽屉，于是在 1100px（面板已浮成抽屉、侧栏仍是列）按一下侧栏的会话行，会**既选中那一行、又关掉抽屉**：这一下按到了用户根本没瞄的控件。另一处是 `sidebar_overlay` 拿到的是 `self.session`，而列形态拿到的是 `open_session_id()`；离线壳层的「当前行」走 `preview_current`，所以低于 960px 时浮起的侧栏压根没有活动行，按会话在画面上什么也不会发生。

**决策：** 画出 scrim——`.absolute().inset_0()` 加 `rgba(20,20,19,.14)` 的原型底纹——作为覆盖整个工作区的那层；用 mouse-down 上的 `stop_propagation` 吞掉它该吞的那一下，保证这一下不会同时落到被盖住的控件上；再由它的 click 关掉抽屉。GPUI 没有 `z-index`，所以原型 `25 < 30 < 40` 的层叠改由渲染顺序下的「点击归属」表达：抽屉与浮起的侧栏各自在冒泡路上认领自己的那一下，这正是一次按下不会同时被算成「控件点击」和「scrim 点击」的原因。浮起的侧栏改用 `open_session_id()`，与列形态拿到同一个 `current`。

**改后行为：** 面板作为抽屉时，按在它外面会关掉面板且底下的东西一律不响应，按在抽屉自己的页签上仍然切换面板。低于侧栏断点后，按浮起的会话行会选中它并让工作区面板保持打开，对应原型里 `.sidebar{z-index:40}` 压在 `.scrim{z-index:25}` 之上。`ElementSnapshot::visible()` 表达不了这些——它只是与视口求交——所以两条契约都是行为断言：先按下，再读回这一下本该改变的状态。这一轮顺手关掉了点击巡检里最后一个够不到的控件：转录里围栏代码卡的 `Copy` 标签既没有元素 id 也没有可访问名字，任何测试都按不到它。它现在用框架给这个块打的那个锚点做 id——`code-block-copy-<anchor>`，即该围栏在消息里的字节偏移，markdown 渲染器就是这么给自定义块打点的——带上 `aria_label("Copy code")`，并在 click 时把围栏正文写进剪贴板；`the_code_block_copy_chip_writes_its_fence_to_the_clipboard` 会按下它并读回剪贴板。集成测试为 65 个通过。

**指针：** `crates/tact-gui/src/shell.rs`（`work_pane_scrim`、`work_pane_drawer`、`sidebar_overlay`、`TactApp::render`）；`crates/tact-gui/src/transcript.rs`（`render_code_block`）；`crates/tact-gui/tests/shell.rs`（`the_work_pane_drawer_swallows_presses_behind_it`、`the_sidebar_overlay_keeps_its_presses_above_the_scrim`、`the_floating_sidebar_stays_above_the_work_pane_where_they_overlap`、`the_code_block_copy_chip_writes_its_fence_to_the_clipboard`）；`docs/design/tact-desktop-design-review.md`（Phase 4–7 跟进）；`docs/design/tact-desktop-prototype.html`（`.scrim`，第 25 行；`#scrim` 处理，第 147 行；`.codeHead`，第 90 行）。

---

## 1. 2026-09-20 — 状态栏每个分段都有可读名字并钉住取值，标题栏预设也逐个按下

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`status_bar`、`status_item`、`Workspace::work_pane`、`title_bar`）；`crates/tact-gui/tests/shell.rs`（`the_status_bar_renders_its_segmented_chips`、`the_workspace_tabs_pair_each_preset_with_its_pane`）；`docs/design/tact-desktop-prototype.html`（`footer.status`，以及 `.tab` 的点击处理） |

**症状 / 动机：** 原型页脚是一排分段——项目、分支、权限、diff 增减、checks、上下文、余额、running——每段读的是会话状态里不同的一项。壳层画出了同样一排，但没有任何断言读它的文本：`the_status_bar_renders_its_segmented_chips` 只问 `status-bar` 挂没挂载，于是某段读错字段、格式写错、甚至整段消失，测试都照样全绿。这些分段同时没有可访问名字，辅助技术在肉眼看到 `42% context` 的位置只能碰到一排没有标签的盒子。

同一轮里还翻出第二个空洞：标题栏的三个预设页签（`Chat`、`Agent`、`Code`）从来没有被任何测试按过——点击巡检一路长满了窗口主体，唯独漏掉这三个顶层切换；于是壳层「预设对应哪个工作区面板」这条映射（原型里是 `pane(n==='agent'?'tasks':n==='code'?'diff':'plan')`）只靠读源码成立。

**决策：** 标题栏预设改由 `the_workspace_tabs_pair_each_preset_with_its_pane` 逐个按下，每按一个就读回它打开的面板，`TabBar` 与壳层的配对映射都被覆盖。`status_item` 接收该段的 id，并用与画面同一个字符串设置可访问名字——这既让分段可被朗读，也让测试能读回它；两段不是「图标 + 文本」的普通组合（`status-permission`、`status-diff`）手工补上同样的一对。测试改为读回每段文本并与预览种入的会话比对——`Ask permission`、`+676 −152`、`42% context`、`$18.42`、`1 running`——而不是断言「状态栏存在」。回合计数段反过来断言 *不存在*，因为预览没有 `TurnStats` 可显示。原型里的 `checks passing` 段保持不实现：协议不上报 check 状态，硬画就是一段永远不会变的死分段；该偏差记在设计评审里，而不是用占位内容填上。

**改后行为：** 状态栏挂载的每一段都把文本暴露给无障碍树和 `try_find`，且每段取值都钉在它背后的状态上：diff 增减段是已记录改动之和，上下文段与 composer 的 `.ring` 读同一份 usage 快照，余额段按 provider 上报的币种排版。删掉某段、改写措辞、或把某段接到错误字段，都会让 `the_status_bar_renders_its_segmented_chips` 失败；按下某个预设会把它对应的工作区面板切到前台，另外两个面板不再挂载。集成测试为 63 个通过。

**指针：** `crates/tact-gui/src/shell.rs`（`status_bar`、`status_item`、`balance_label`、`Workspace::work_pane`、`title_bar`）；`crates/tact-gui/tests/shell.rs`（`the_status_bar_renders_its_segmented_chips`、`the_workspace_tabs_pair_each_preset_with_its_pane`）；`docs/design/tact-desktop-design-review.md`（Phase 4–7 跟进）；`docs/design/tact-desktop-prototype.html`（`footer.status`，第 132 行）。

---

## 1. 2026-09-20 — 预览壳层画出原型的上下文圆环，点击巡检覆盖请求卡的按钮与 composer 自己的入口

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`seed_preview`、`with_question`、`answer_panel`、`request_panel`）；`crates/tact-gui/tests/shell.rs`（`every_entry_point_answers_a_click`、`the_composer_controls_use_the_prototype_boxes`、`the_permission_card_reports_the_choice_it_was_given`、`the_question_card_confirms_the_toggled_choices`、`the_question_card_cancels_without_choosing`、`a_press_in_the_prompt_box_sends_the_draft`）；`docs/design/tact-desktop-prototype.html`（`.ring`、`.send`） |

**症状 / 动机：** 点击巡检有两处它看不见的空洞，以及这两处量出来的第三个问题。

1. *预览壳层不画上下文圆环。* 原型的 composer 带一个读数 `42` 的 `.ring`，状态栏也重复 `42% context`；而预览从未种入 usage 快照，圆环因此根本不挂载。巡检里那段圆环分支虽然还写着，却是一个预览永远走 `else` 的 `if`——一个围着「设计评审面已经丢掉的控件」的死检查。
2. *请求卡自己的按钮从来没被按过。* 巡检有意跳过 `request-option-*`，因为按下就会结算它后面断言要读的那个请求；而 `request-confirm` / `request-cancel` 它也够不着：这两个按钮只在多选 `ask_user` 形态下渲染，预览种入的却是权限卡。上一轮那种只 diff `.id("...")` 字面量的覆盖审计完全漏掉它们，因为两个 id 都是经 `prototype_button` 传进去的。
3. *圆环一旦挂载，用的是组件自己的盒子而不是原型的。* 它的触发器是标准 `Button`，而 `gpui-component` 对「带子元素」的按钮——这里是画百分比的圆——按 `size * 0.2` 加左右内边距，再套 32 px 的 compact 高度。于是圆环量到 34x32，而原型 `.ring` 是 `width:24px;height:24px`。它又是 `.bar` 里剩下的最高控件，composer 工具条的高度就被它一个人顶起来了。

**决策：** 预览种入两处读数共用的 usage——`4200 / 10000` tokens，圆环画 `42`、状态栏写 `42% context`——于是设计评审面上有原型画的那个控件，而不是留一个洞。请求卡的两种形态保持分开：`request_panel` 的 `Confirm` / `Cancel` 只由 agent 会话产生的请求渲染，因此新增构造器 `TactApp::with_question`，与 `preview`、`with_sessions` 并列，用来在离线壳层上把该形态摆到屏幕里。已答复的卡片新增 `request-decision`：一行带无障碍标签的决策行，测试因此能读出「按下产生了哪个决定」，而不只是「按钮消失了」。巡检改为按下 composer 的完整选项表（5 个模型、5 个思考预算、6 档 effort、3 种权限模式），而不是各取一个样本；圆环也不再是条件分支。圆环触发器改用 `.with_size(px(24.)).px(px(0.))` 取代 `.compact()`：`with_size` 只对图标按钮生效，内容为子元素的按钮仍会保留内边距，所以要拿到原型自己的盒子必须两个都写。

**改后行为：** `--preview` 在 composer 旁显示 42% 的上下文圆环、状态栏显示 `42% context`，与原型的两处读数一致；按下圆环会打开它自己的计数弹层。在权限卡上按 `Deny`，三行选项会被 `Deny` 取代；在提问卡上勾选后按 `Confirm` 报 `Confirmed N choice(s)`，按 `Cancel` 报 `Dismissed` 并丢弃已勾选的选项。在提示框里按下会把光标落在随后输入的草稿上，`.send` 的发送那一半把草稿变成 transcript 的第一行、让空状态占位消失、并清空输入框，于是再按一次不会再发出任何东西。圆环量到 24x24，与 `.ring` 一致，所以挂上圆环后 composer 工具条仍是原型的高度；`the_composer_controls_use_the_prototype_boxes` 钉住 `.mini` 的 25 px、`.send` 的 28x28，现在再加上 `.ring` 的 24x24。新增覆盖：`the_permission_card_reports_the_choice_it_was_given`、`the_question_card_confirms_the_toggled_choices`、`the_question_card_cancels_without_choosing`、`a_press_in_the_prompt_box_sends_the_draft`，以及巡检里扩充后的选项表。壳层注册的控件现在都有测试按过，例外是侧栏头像（原型里同样是无 handler 的标签）与附件走的原生文件对话框。集成测试套件现为 59 个通过用例。

**指针：** `crates/tact-gui/src/shell.rs`（`seed_preview`、`with_question`、`answer_panel`、`request_panel`、`composer_bar`）；`crates/tact-gui/tests/shell.rs`（`every_entry_point_answers_a_click`、`the_permission_card_reports_the_choice_it_was_given`、`the_question_card_confirms_the_toggled_choices`、`the_question_card_cancels_without_choosing`、`a_press_in_the_prompt_box_sends_the_draft`、`the_composer_controls_use_the_prototype_boxes`）；`docs/design/tact-desktop-prototype.html`（`.ring`、`footer.status`）。

---

## 1. 2026-09-20 — 列不存在时标题栏也不再为它留宽，worktree 行按下后离线壳层的会话列表保留

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`sidebar_in_flow`、`TitleBarState::sidebar_float`、`switch_worktree`、`resume_session`）；`crates/tact-gui/tests/shell.rs`（`the_title_bar_gives_up_the_sidebar_column_when_the_sidebar_closes`、`clicking_between_sessions_and_worktrees_keeps_one_open_row`、`the_title_bar_toggles_survive_every_width_around_the_overlay_breakpoint`、`the_chrome_stays_inside_the_viewport_at_every_width`）；`docs/design/tact-desktop-prototype.html`（`@media(max-width:880px)`） |

**症状 / 动机：** 壳层有两处仍在为已经不存在的列排版。

1. *关掉侧栏后标题栏还占着它的宽度。* 侧栏关闭时主体已经不画会话列，但标题栏只在 `SIDEBAR_OVERLAY_UNDER` 以下才停止预留 `SIDEBAR_WIDTH`。于是在宽窗口下 transcript 从窗口边缘开始，而标签页与会话 chip 仍往里缩了一整列，浮在主体已经让出的空隙上。
2. *按下 worktree 行会把离线壳层的侧栏清空。* `switch_worktree` 每次重新定根都重读会话存储：对连上 agent 的窗口这是对的，对 `--preview` 与每个测试窗口所用的离线壳层则是错的。这类壳层没有存储，它的行是构造时交给它的，重读会把这些行换成本地 `.tact` 数据库里碰巧有的东西，演示列表再也回不来。

**决策：** 侧栏是否为列，这条规则现在只有一个出处。`sidebar_in_flow` 等于 `!sidebar_is_overlay && self.sidebar_open`，主体按同一个条件画出这一列，标题栏则取 `!sidebar_in_flow` 作为浮起标志——所以侧栏一关，标题栏在任何宽度下都会随主体的列一起交出预留宽度，而不只是断点以下才交。`switch_worktree` 只在壳层连上时重读会话列表：这正是 `resume_session` 早就做的离线豁免——离线壳层的点击只改变窗口显示什么，绝不查询存储。

**改后行为：** 在 1440px 关闭侧栏后，transcript 的起点左移，标题栏左段从一整列收缩到只包住自己的控件，而重新打开侧栏的开关仍然点得到。在预览或离线壳层里按下 worktree 行仍会重新定根——分支行、Files 面板、被标记为当前的 worktree 行都跟着走——而侧栏保留它构造时拿到的那些行，被标记为打开的会话也留在原处。`the_title_bar_gives_up_the_sidebar_column_when_the_sidebar_closes` 把标题栏的收缩与 transcript 的左移钉在一起；`clicking_between_sessions_and_worktrees_keeps_one_open_row` 在两张列表之间切换三轮，断言每一次按下后被点的那张列表恰好只有一行打开，且按下 worktree 行不会挪动侧栏标记的打开会话。`the_chrome_stays_inside_the_viewport_at_every_width` 把这张网从「存在」扩到「真正可达」：它扫 1440、1320、1280、1279、960、959、880 与 700px，断言壳层在每个宽度都会挂载的每个控件都*完整*落在视口内——`visible()` 只表示有交集——随后把两个面板开关各关一次再开一次。把标题栏改回两段都预留列宽，它就会以当年的形态失败：`toggle-work-pane` 的 `origin.x = 1053px`，只不过这次是在 960px 窗口里。

**指针：** `crates/tact-gui/src/shell.rs`（`sidebar_in_flow`、`TitleBarState::sidebar_float`、`switch_worktree`、`resume_session`）；`crates/tact-gui/tests/shell.rs`（`the_title_bar_gives_up_the_sidebar_column_when_the_sidebar_closes`、`clicking_between_sessions_and_worktrees_keeps_one_open_row`、`the_chrome_stays_inside_the_viewport_at_every_width`）；`docs/design/tact-desktop-prototype.html`（`@media(max-width:880px)`）。

---

## 1. 2026-09-20 — 侧栏搜索框按下即接管光标，窗口变窄时标题栏的开关仍留在屏幕内

| 字段 | 值 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`sidebar_top`、`title_bar`、`TitleBarState`、`SIDEBAR_OVERLAY_UNDER`、`WORK_PANE_IN_FLOW_FROM`、`WORK_PANE_WIDTH`，以及 `TactApp::render` 里的 `sidebar_is_overlay` / `work_pane_in_flow` 判定）；`crates/tact-gui/tests/shell.rs`（`the_sidebar_search_filters_the_session_list`、`the_side_columns_overlay_when_the_window_narrows`、`clicking_between_sessions_and_worktrees_keeps_one_open_row`）；`docs/design/tact-desktop-prototype.html`（`.search`、`.search:focus-within`、`@media(max-width:1120px)`、`@media(max-width:880px)`） |

**症状 / 动机：** 两个点击巡检够不到的控件，成因相同：壳层复刻了原型的**外观**，却丢掉了浏览器免费提供的行为。

1. *侧栏会话过滤框点不到。* 原型的输入框是 `<label class="search">` 里的原生 `<input>`，浏览器在 label 内任意位置按下时都会聚焦它——随后 `:focus-within` 画出强调色描边。壳层搭了一样的 wrapper + `Input` 外形，但 wrapper 没有挂按下处理，而 `gpui-component` 的 `Input` 不会自己响应点击取得焦点，于是这个画出来的输入框永远拿不到光标，往里敲的字也全都落空。
2. *窗口变窄时标题栏的 work pane 开关跑到屏幕外。* 标题栏由固定的 `SIDEBAR_WIDTH` 段、可伸缩的中段和固定的 `WORK_PANE_WIDTH` 段组成。原型在两侧面板不再作为列存在时会丢掉对应的网格列——`@media(max-width:1120px)` 把 `.work` 变成绝对定位抽屉，`@media(max-width:880px)` 则丢掉 `.tl`/`.tr` 两列——但壳层一直保留着这两个固定宽度。于是在 `SIDEBAR_OVERLAY_UNDER`（60rem，默认 rem 下 960px）以下，整行排不下，`toggle-work-pane` 被挤出视口：900px 窗口里量到 `bounds.origin.x = 1053px`，即 `visible() == false`，点不到。

**决策：** 标题栏改为接收一个小的 `TitleBarState` 结构体，`sidebar_float` 与 `work_pane_float` 和它本来就需要的字段放在一起；对应面板浮起后，两侧的段不再预留列宽——`sidebar_float` 时左段收缩到只包住自己的控件，不再占着 `SIDEBAR_WIDTH`，`work_pane_float` 时右段对 `WORK_PANE_WIDTH` 做同样的事。两个标志都来自决定布局的同一组判定，因此标题栏和壳层不可能对"还有哪些列存在"各执一词。阈值是严格的——`SIDEBAR_OVERLAY_UNDER` 为 60rem，恰好 960px 时侧栏仍然保留自己的列，只有 `width < 60rem` 才让它浮起。过滤框那边，`session-search` wrapper 挂上 `on_mouse_down(MouseButton::Left)` 并把焦点交给 `Input` 的 focus handle，这正是浏览器为原型做的 label 按下行为。

**改后行为：** 在搜索框任意位置按下都会聚焦它，于是打字就能过滤侧栏会话列表；清空查询后每一行都会回来，因为过滤只是会话之上的一层视图，而不是破坏性编辑。900x640 时侧栏浮在 transcript 之上、work pane 是右侧抽屉，标题栏的两个开关仍然能把它们挂载 / 卸载。同一处测量此前在 900px 窗口里把开关放在 `bounds.origin.x = 1053px`，现在读数是 `828px`，落在视口内。`the_sidebar_search_filters_the_session_list` 点击输入框、输入 `d464`，断言唯一匹配的预览会话留下来、另外两条消失，再用 `ctrl-a`、`backspace` 清空查询把它们带回；`the_side_columns_overlay_when_the_window_narrows` 钉住 900x640 的形态和两个开关。`clicking_between_sessions_and_worktrees_keeps_one_open_row` 在会话列表与 worktree 列表之间来回切换三轮，断言每一次按下后被点的那张列表恰好只有一行打开，且按下 worktree 行不会挪动侧栏标记的打开会话。集成测试套件现为 59 个通过用例。

**指针：** `crates/tact-gui/src/shell.rs`（`sidebar_top`、`title_bar`、`TitleBarState`、`SIDEBAR_OVERLAY_UNDER`、`WORK_PANE_IN_FLOW_FROM`、`WORK_PANE_WIDTH`、`sidebar_is_overlay`、`work_pane_in_flow`）；`crates/tact-gui/tests/shell.rs`（`the_sidebar_search_filters_the_session_list`、`the_side_columns_overlay_when_the_window_narrows`、`clicking_between_sessions_and_worktrees_keeps_one_open_row`）；`docs/design/tact-desktop-prototype.html`（`.search`、`.search:focus-within`、`@media(max-width:1120px)`、`@media(max-width:880px)`）。

---

## 1. 2026-09-20 — 命令面板每一行只执行一次命令，并落到与快捷键相同的状态

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`open_palette`、`run_palette_command`、`focus_composer`、`open_settings`、`PaletteCommand`、各个 `on_*` action 处理函数）；`crates/tact-gui/src/commands.rs`（`command`、`groups`、`init`）；`crates/tact-gui/tests/shell.rs`（`the_command_palette_rows_run_their_commands`、`the_command_palette_rows_past_the_fold_run_their_commands`、`the_palette_new_session_row_answers_like_the_chord`、`the_command_palette_session_rows_answer_without_an_agent`）；`Root::close_dialog`（LIFO 对话框栈）；上游 `gpui-component-0.6.4` `src/command/state.rs`（`CommandState::confirm`） |

**症状 / 动机：** `CommandState::confirm` 会把选中行跑两遍——它先派发该行自己的 GPUI action，*再* defer `Command` 的 `on_confirm` 回调。而 tact 把两端都接上了：`commands::command()` 给每个 `CommandItem` 挂了真实 action（这也正是面板画出 `Kbd` 快捷键列的依据），`open_palette` 的 `on_confirm` 又通过 `PaletteCommand::from_index` 把行号解成命令并调 `run_palette_command`。于是面板每一行都把自己的命令执行了两次。修掉双重派发后，又暴露出两个更具体的行级时序问题：

1. *开关类命令互相抵消。* Toggle sidebar、Toggle work pane、Toggle theme 都是先翻转标志，紧接着又翻回去。按这三行中的任何一行都完全没有反应——这是这个 bug 最有欺骗性的形态，因为行看起来是"死"的，而不是"翻倍"的。
2. *非幂等命令做了两遍活。* New session 追加两条提示，Cycle sessions 跳两个会话，Compact session 发两次压缩。面板行与它自己的键盘快捷键行为不一致。
3. *幂等命令把所有痕迹都盖住了。* Open diff、Open tasks、Focus composer 两种走法都落到同一个状态，所以这个 bug 恰恰在点击巡检最先会跑到的地方完全不可见。
4. *Focus composer 的焦点被覆盖。* 该行在面板仍打开时先聚焦 composer；随后关闭对话框又把焦点还给面板打开前的持有者，覆盖了该行刚设好的焦点。点击结束时看不出任何效果。
5. *Open settings 被连同面板一起弹掉。* 该行同步打开 Settings，但面板的 `on_confirm` 在该行 action 之后才关闭面板；`Root::close_dialog` 弹出 LIFO 对话框栈，于是新打开的 Settings 位于栈顶，和面板一起被弹出。

**决策：** 以行自身的 action 作为唯一的派发路径。`on_confirm` 现在通过 `std::mem::take` 查询 `palette_live`，只有面板仍是其栈顶对话框时才关闭；`on_cancel` 会清掉这个标志，避免 Escape 关闭的面板被误认成某一行替换后的对话框。`PaletteCommand::from_index`——`commands::groups()` 顺序的第二份手工副本——直接删除，而不是继续维护同步。每个 `on_*` action 处理函数统一汇入 `run_palette_command`，于是面板行与键盘快捷键派发同一个 action、落到同一张表的同一个分支。不能简单地"删掉索引表、只留 action"：一旦摘掉行上的 action，面板就会丢掉 `Kbd` 快捷键提示；而如果放任处理函数继续各自抄一份分支逻辑，这张表就仍是两条路径唯一对账的地方。需要活过面板关闭的行会在同一次同步派发中自己弹出面板：Focus composer 先关面板再聚焦 composer，于是对话框恢复焦点发生在移交焦点之前；Open settings 先关面板再同步打开 Settings。vendored `Command::on_confirm` 虽然一帧后才到，但 `palette_live` 已被清掉，因此不会再动对话框栈。在键盘快捷键路径上没有面板，同一个分支只会执行一次聚焦或一次 `open_dialog`。

**改后行为：** 面板行与它的快捷键做同一件事，且只做一次。Toggle sidebar 与 Toggle work pane 落到相反状态，而不是读回出发时的状态；Toggle theme 只切一次；New session 只追加一条提示；循环会话只移动一个会话。`the_command_palette_rows_run_their_commands` 把两种形态都钉住——必须落到相反状态的开关，以及必须落到指定主体的面板切换；`the_palette_new_session_row_answers_like_the_chord` 钉住那个"跑两遍就会多出一行"的行——它多跑一次就会多出第二条 transcript 行。两条都做过变异验证：对着未修复的源码，它们恰好在这些断言上失败。Focus composer 现在会在同一次派发中先关闭面板，再把焦点交给 composer，因此对话框恢复焦点时不会再覆盖它；Open settings 同样先关闭面板、再同步打开 Settings，由 `palette_live` 告诉面板迟到的确认不要弹掉替换后的对话框。覆盖补充：面板测试现在会点击全部 16 行；低于折叠区的行先把 `command` 列表滚动进视野，再断言每一行各自的可观察效果。

**指针：** `crates/tact-gui/src/shell.rs`（`open_palette`、`run_palette_command`、`focus_composer`、`open_settings`、`PaletteCommand`、`on_*`）；`crates/tact-gui/src/commands.rs`（`command`、`groups`、`init`）；`crates/tact-gui/tests/shell.rs`（`the_command_palette_rows_run_their_commands`、`the_command_palette_rows_past_the_fold_run_their_commands`、`the_palette_new_session_row_answers_like_the_chord`、`the_command_palette_session_rows_answer_without_an_agent`）；`Root::close_dialog`；上游 `gpui-component-0.6.4` `src/command/state.rs` 的 `CommandState::confirm`。

---

## 1. 2026-09-20 — 桌面端每个入口都能点，worktree 行会切换窗口的工作区

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/shell.rs`（`offline`、`switch_worktree`、`resume_session`、`new_session`、`sidebar_meta_row`、`worktree_rows`、`WorktreeRow::path`、`files_listed`）；`crates/tact-gui/tests/shell.rs`（`every_entry_point_answers_a_click`、`the_shipped_toast_placement_keeps_the_work_pane_tabs_clickable`、`rapid_switching_between_surfaces_stays_responsive`）；`docs/design/tact-desktop-prototype.html`（`.row`、`.toast`） |

**症状 / 动机：** 壳层从未被从头点到尾，而测试套件看不见这件事掩盖了什么。

1. *worktree 行是一个死控件。* 原型把 worktree 组的每一行都渲染成 `<button class="row">`，`worktree_row` 也复刻了整套外形——hover 底色、圆角、状态点、徽标——却没有挂任何点击处理。按下这一行只会高亮，什么也不会发生，而这恰恰是侧栏里唯一选择窗口工作区的地方。
2. *离线壳层里的会话行会真的启动一个 agent。* `TactApp::new`、`with_sessions`、`preview` 列出的是它们并不持有运行时的会话，但行的处理函数直接走了 `resume_session` → `session::resume` → `SessionRuntime::start`：tokio 运行时、一次 sqlite 写入、一次 skill registry 扫描、一个 agent 线程，每点一次一整遍。把八个预览行点完就起了八个运行时。
3. *在巡检跑在用户实际拿到的主题上之前，它的结论不可信。* 测试此前只调 `gpui_kit::init`，于是留在 gpui-component 的出厂主题上；该主题把 toast 锚在右上角，正好把一张不透明卡片压在 work 面板的标签行上方，于是一次主题切换会让 Diff 标签在 toast 存活期间完全点不到。已发布主题（`theme::activate`）早就把 `.toast` 钉在原型的下右角——只是测试没在跑它。

**决策：** 交付一条集成巡检 `every_entry_point_answers_a_click`，把每个渲染出来的控件都按一遍，再问按完发生了什么：标题栏开关、会话搜索、transcript 工具栏（detail 循环、复制）、由代码手工滚入视口的每一条可展开行与每个 diff 徽标（transcript 滚动器是虚拟的）、侧栏全部八个会话行、新建会话动作、每一个 worktree 行、work 面板全部五个标签以及每个标签自己的控件、每个 composer 芯片以及它打开的每个弹层里的每一项、主按钮，以及每个对话框——先是命令面板，再是设置（真的滚动一次才够到它的最后一行）。标题栏的主题开关刻意放在最后：它弹出的 toast 活得比巡检更久，压在后面会吞掉下一次按压。

两处发现都在源头修掉，而不是改测试。`TactApp::offline` 标出三个不持有运行时的壳层，它们的会话控件改为就地应答：`resume_session` 只移动 `preview_current`（即侧栏画成打开状态的那一行），`new_session` 推一条系统行说明本壳层没有 agent。`connect` 即使启动失败也把这个标志留在 `false`，因此再点一次仍可重试。`WorktreeRow` 自带 `path`，`switch_worktree` 据此把窗口重新扎根到该目录——分支行、`state.workdir`、侧栏的会话列表，以及两个按工作区缓存的窗格；而已挂载的 agent 会话保留自己的根：窗口只是换掉它显示的范围，不重启会话，于是下一次新建会话会落在用户选中的 worktree 里。

原型只用画笔表达的行状态（`.row.active`）必须活到无障碍树里，巡检才能对它断言，因此 `session_row` 与 `worktree_row` 带上 `.aria_selected`，共用的 `sidebar_meta_row` 增加 `selected: Option<bool>` 与 `on_click: Option<SidebarRowClick>`——什么都不做的行不再拿到处理函数，而这正是让"死控件"现形的机制。渲染壳层的测试现在都先调 `activate_shipped_theme`；`the_shipped_toast_placement_keeps_the_work_pane_tabs_clickable` 把 toast 契约的两半都钉住（下右栈存在、右上栈为空，且 toast 在屏时 Diff 标签仍能应答点击）。

**改后行为：** 壳层画出的每个控件按下都有反应，而且巡检会断言是什么反应。八个会话行各自成为打开行，且任意时刻只有一个打开，同时离线的"新建会话"动作会应答但不抢走这一行；每个 worktree 行都会成为窗口扎根的那一行，巡检最后回到出发的那一行，于是后面的窗格读到的仍是窗口打开时的那个检出；每个 work 面板标签都会切换可见主体。在预览里点会话行不再启动 agent——没有 tokio 运行时、没有 sqlite 写入、没有 agent 线程。worktree 行现在会切换窗口的工作区。`rapid_switching_between_surfaces_stays_responsive` 在 60 秒看门狗下泡 440 次交互（五个标签 × 40 轮，加上窗格开关、一次主题切换与 `Ctrl+O`），于是"一次点击做无界工作"的回归在这里失败，而不是在用户桌面上失败。刻意排除在巡检之外：`sidebar-avatar`（没有处理函数）、`composer-attachment-*`（会打开原生文件对话框）、以及 request-option 那些行（点下去会结掉种入的请求）。

**指针：** `crates/tact-gui/tests/shell.rs`（`every_entry_point_answers_a_click`、`the_shipped_toast_placement_keeps_the_work_pane_tabs_clickable`、`rapid_switching_between_surfaces_stays_responsive`、`activate_shipped_theme`、`worktree_row_ids`）；`crates/tact-gui/src/shell.rs`（`offline`、`switch_worktree`、`resume_session`、`new_session`、`WorktreeRow`、`worktree_rows`、`sidebar_meta_row`）；原型 `.row` / `.toast`，见 `docs/design/tact-desktop-prototype.html`。

---

## 1. 2026-09-20 — 桌面端一次点击的代价是一帧，而不是三帧

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | `Cargo.toml`（`[profile.dev]`、`[profile.dev.package."*"]`）；`crates/tact-gui/src/pane.rs`（`FilesPane::cache`、`revision`、`invalidate`、`FILE_TREE_SKIPPED_DIRS`）；`crates/tact-gui/src/composer.rs`（`collect_files`）；`crates/tact-gui/src/shell.rs`（`files_listed`） |

**症状 / 动机：** 壳层的输入本身都不贵，贵的是它被构建与被重建的方式。在 1536x842 的窗口里，一次会重渲染壳层的点击要花 31–33 ms——在任何产品逻辑跑起来之前就吃掉一整个帧预算——而这些时间几乎全在 gpui、taffy 与文本整形这些 crate 里，因为 dev profile 让每个依赖都不优化。Files 面板每帧重走一次工作区（`read_dir` 加每条目一次 `stat`，展开时还会走进 `target/`），一次展开点击就变成一次长达一帧的卡顿，`node_modules/` 与 `target/` 是常见的元凶。`collect_files` 里的 mention 补全遍历形状相同，但它跑在每个带 `@` 的按键上：在本仓库根走一趟 19.6 ms，基本全花在 `target/` 上。

**决策：** 优化占大头的那些 crate，然后不再索要产物目录，最后不再重复同一次遍历。`[profile.dev] opt-level = 1` 配合 `[profile.dev.package."*"] opt-level = 2`，在把依赖编成优化的同时让工作区自身的构建与单步调试仍然可用；取 `2` 而不是 `3`，因为在这里多出来的内联并不值那些编译分钟。`FILE_TREE_SKIPPED_DIRS = ["target", "node_modules"]` 放在 `pane.rs`，面板的 `collect` 与 composer 的 `collect_files` **都**要查它——它们是 gitignore 掉的构建产物、动辄几千层，而这两个界面都不是用来浏览产物缓存的，所以两者不能走偏。没人该重复的遍历被记住：`FilesPane` 把摊平后的行与 `expanded` 集合放在一起缓存，只有显式 `invalidate()`——一次开关、一次面板重新进入、一次 worktree 切换——才允许重读磁盘。`TactApp::files_listed` 记录上一帧 Files 面板是否画过，从而在重新进入该标签时丢掉缓存；没有它，一份在 agent 写文件之前取的清单会活到进程结束。

**改后行为：** 壳层完整重渲染实测从 31–33 ms 降到约 11–12 ms，其中 `Ctrl+B` 约 11 ms、`Ctrl+O` 约 12 ms、空按键约 4 ms；40 轮切换泡测（440 次交互）留在看门狗之内而不是卡住。Files 面板改成每次展开 / 重新进入走一次，而不是每帧一次，并且从不走进 `target/` 与 `node_modules/`；在本仓库里，一次 mention 按键的遍历从 19.6 ms 降到 0.67 ms。`files_rows_are_cached_until_the_pane_is_invalidated` 钉住缓存契约（遍历之后写入的文件在 invalidate 之前不可见），`files_rows_skip_build_output_directories` / `file_suggestions_skip_build_output_directories` 钉住跳过规则。接受的代价：即使 `target/` 或 `node_modules/` 被 git 跟踪，这两个界面也够不到它。

**指针：** `Cargo.toml` 的 profile 段；`crates/tact-gui/src/pane.rs` 的 `FilesPane::rows`、`revision`、`cache`、`invalidate`、`FILE_TREE_SKIPPED_DIRS` 与 `files_tree`；`crates/tact-gui/src/composer.rs` 的 `collect_files`；`crates/tact-gui/src/shell.rs` 的 `files_listed`；测试 `files_rows_are_cached_until_the_pane_is_invalidated`、`files_rows_skip_build_output_directories`、`file_suggestions_skip_build_output_directories`。

---


## 1. 2026-09-20 — 工作面板标签行、transcript 元信息与复制入口对齐原型

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/transcript.rs`（`msg_meta`、`clock_label`、`diff_line_counts`、`code_block_style`）；`crates/tact-gui/src/pane.rs`（`work_tabs`、`step_age`）；`crates/tact-gui/src/shell.rs`（`copy_transcript`、`close_work_pane`、`scroll_transcript_to`、`seed_preview`）；`crates/tact-gui/src/session.rs`（`Conversation::to_markdown`、`now_unix`、`mark_plan_step`）；`docs/design/tact-desktop-prototype.html` |

**症状 / 动机：** 有五个原型元素在壳层里没有对应物。`.msgMeta`——每条消息上方的作者、时间与模型行——完全缺失，连续两轮对话无从区分。工具卡片只打印状态、不呈现自己造成的改动，而原型会给一次写入打上 `+142 −0` 徽标。围栏代码块只是一个空框；原型的 `.code` 带一条含复制动作的 `.codeHead` 横带。工作面板没有关闭按钮，而加上关闭按钮后 `.wtabs` 标签溢出到按钮下方——最后一个标签（Files）根本点不到，并且标签行仍在穿组件默认的 32px 标签，而不是 `.wtab` 的 28px 芯片加它的 `.count` 徽标。Plan 面板的问题比外观更严重：`StepFinished`/`StepFailed` 丢掉了步骤的输出，于是每一步永远显示"待执行"，尾列也从不显示原型的相对时间（`2m ago` / `now` / `next` / `later`）。此外没有办法把 transcript 作为文本带走，尽管原型的 `.mainTop` 带一个复制图标。

**决策：** 每个元素都按原型自己的数值来搭，而不是沿用组件默认值。`.msgMeta` 变成 `msg_meta`（11px 半粗作者、10.5px 弱化时间与模型、7px 间距、7px 下边距），数据来自 push 时记下的 `sent_at`，由 `clock_label` 渲染成本地 `HH:MM`；助手行的模型取自运行中的 `ModelParams`。工具行挂上 `diff_stats`，由 `diff_line_counts` 计算（跳过 `+++`/`---` 文件头，且宁可返回 `None` 也不谎报 0），`tool_meta` 对写入与编辑渲染成 `+新增 −删除`。代码块获得 `code_block_style` 与 `code_block_actions`——一个真正可用的复制按钮，把围栏里的文本写进剪贴板。工作面板标签行按 `.wtabs` 手搭：`flex_1`/`min_w_0`/`overflow_hidden` 的弹性子项与 `flex_shrink_0` 的关闭按钮并列，28px 芯片、2px 间距、16px `.count` 徽标在所在标签激活时换成强调色底——裁剪标签行是对的，让标签盖住关闭按钮是错的。Plan 的汇报在源头修好：`mark_plan_step` 记录 `StepFinished`/`StepFailed` 的输出，并以步骤下标为键记下 `plan_done_at` 时间戳，于是尾列能给已完成的步骤算相对时间，又不必维护一个会与列表脱节的并行向量。`Conversation::to_markdown` 与 `TactApp::copy_transcript` 支撑工具栏上的复制按钮。

**改后行为：** 每条 transcript 消息都以作者、本地时间、（助手行）模型开头；用户气泡在多了这一行之后仍保持 72% 上限与右对齐。写入/编辑卡片只要工具输出里带 hunk，就显示 `+n −m` 徽标。围栏代码块是一张带可用复制动作的卡片。工作面板有一条 28px 的 `.wtab` 标签行，以及独占一列的关闭按钮；每个标签都可点击，面板可用 ✕ 或 Ctrl+\ 关闭并用同一快捷键打开。Plan 步骤在完成后显示相对时间，而不是永远"待执行"；transcript 工具栏可把整段对话复制为 Markdown。两处缺口是刻意保留的：`.codeHead` 的文件名横带需要自定义 markdown block renderer；侧栏会话行仍没有 diffstat 芯片与状态词，因为其背后的 store 查询尚不存在。

**指针：** `crates/tact-gui/src/transcript.rs`（`msg_meta`、`clock_label`、`diff_line_counts`、`tool_meta`、`code_block_style`、`render_row`）；`crates/tact-gui/src/pane.rs`（`work_tabs`、`view`、plan 行）；`crates/tact-gui/src/shell.rs`（`copy_transcript`、`close_work_pane`、`scroll_transcript_to`、`seed_preview`）；`crates/tact-gui/src/session.rs`（`to_markdown`、`now_unix`、`mark_plan_step`）；测试 `the_transcript_serializes_to_markdown`、`the_transcript_toolbar_carries_the_prototype_copy_button`、`work_pane_tabs_switch_the_visible_body`、`every_work_pane_renders_content_not_just_a_container`、`the_user_bubble_is_capped_and_right_aligned`；原型 `docs/design/tact-desktop-prototype.html`。

---

## 1. 2026-09-20 — 桌面端壳层与 transcript 改用原型自己的数值

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/shell.rs`（`TRANSCRIPT_MEASURE`、`session_intro`、`transcript_toolbar`、`status_pill`、`status_bar`、`transcript`）；`crates/tact-gui/src/transcript.rs`（`render_row`、`RowToggle`）；`crates/tact-gui/src/session.rs`（`Conversation::push_row`、`toggle_expanded`、`apply_tool_progress`、`retain_output_tail`）；`crates/tact-gui/src/pane.rs`（`view`，`.workTop` 标签行）；`docs/design/tact-desktop-prototype.html` |

**症状 / 动机：** 桌面端沿用了原型的形状，但间距和尺寸是照感觉调的，于是每块都差几个像素：transcript 文本栏是 760px 而原型是 720px；会话头部没有发丝线也没有 detail 控件；用户消息铺满整栏而不是靠右悬挂；工具与推理活动渲染成扁平行而不是卡片；滚动器自带的 12px 行内缩加 32px 行距让每张卡片恒定比所在栏窄 24px。此外工具的输出流被直接丢弃——卡片只能显示一行摘要，想在 GUI 里看命令打印了什么必须回到 TUI。

**决策：** 数值一律从原型的 CSS 里取，而不是目测微调；原型需要而模型里没有的数据就改行模型。`.thread` 对应 `TRANSCRIPT_MEASURE = 45rem`；`.head` 变成 `justify_between` 的头部——19px 标题、`session-intro-detail-cycle` 芯片、一条发丝线；`.msg.user .body` 改成手搭而不是组件（72% 上限、10/12 内边距、三个 12px 圆角加一个 4px 尾角）；工具行与推理行变成 10px 圆角的卡片并带色调图标芯片；`MessageScroller` 自带的内缩与行距通过 `with_row_style` 覆盖，让 `.thread` 的 18px 节奏成为唯一生效的间距。工具输出现在留在行上：`apply_tool_progress` 把 `ToolOutputChunk` 累加进 `TranscriptRow::Tool::output`，按字符边界只保留尾部 8 KiB；卡片通过点击摘要行展开它，`TranscriptDetail::Verbose` 则展开所有卡片。

**改后行为：** transcript 文本栏、会话头部、工具栏、状态胶囊、状态栏与工作面板标签行都带上原型的数值；刻意保留的偏差仍然刻意（默认暗色主题、不引入 Lora 正文、不伪造 `checks passing`、多出的第五个标签、只有在真实用量存在时才出现的 `42% context` 芯片、账户余额放状态栏）。点击工具卡片的摘要行可以开合输出，Verbose 会打开全部；每行 8 KiB 的尾部上限约束了话痨命令的代价，且不会在字符中间截断。

**指针：** `crates/tact-gui/src/shell.rs`（`TRANSCRIPT_MEASURE`、`session_intro`、`transcript_toolbar`、`status_pill`、`status_bar`、`toggle_row`、`transcript`）；`crates/tact-gui/src/transcript.rs`（`render_row`、`RowToggle`）；`crates/tact-gui/src/session.rs`（`push_row`、`toggle_expanded`、`apply_tool_progress`、`TOOL_OUTPUT_LIMIT`、`retain_output_tail`）；测试 `the_user_bubble_is_capped_and_right_aligned`、`clicking_a_tool_summary_reveals_its_output`、`tool_progress_accumulates_into_the_row_output`、`tool_output_keeps_its_tail_without_splitting_a_character`；原型 `docs/design/tact-desktop-prototype.html`。

---

## 1. 2026-09-20 — 桌面端 Diff 面板按仓库解析记录下来的路径

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact-gui/src/pane.rs`（`git_diff`）；[Ch 26](./26_chapter_issue_zh.md) §1（2026-09-19，读工作区的那次改动） |

**症状 / 动机：** Diff 面板把记录下来的路径原样交给 `git diff --no-color -- <path>`，而 git 是在会话工作区里跑的。当工具调用按仓库根写路径、而会话工作区位于子目录时，pathspec 命中不了任何东西：`git diff` 依然以 0 退出且输出为空，面板便静默回退到工具 detail 字符串。此时"路径写错"和"文件没变"完全无法区分。

**决策：** 交给 git 之前先归一化路径。绝对路径原样透传（git 完全可能被问及一个磁盘上不存在的路径）；相对路径若在工作区下存在就拼到工作区上；其余情况用 `:(top)<path>` 魔数 pathspec 锚定到仓库根。

**改后行为：** 按仓库根写的路径和按工作区写的同一路径落到同一个文件，面板展示真实 diff 而不是回退到 detail。

**指针：** `crates/tact-gui/src/pane.rs`（`git_diff`）；测试 `git_diff_resolves_a_repository_root_path_from_a_subdirectory`。

---

## 1. 2026-09-19 — 桌面端 Diff 面板展示工作区真实差异，而不是只展示工具摘要

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | `crates/tact-gui/src/pane.rs`（`DiffPane`、`git_diff`、`diff_lines`、`diff_body`）；`crates/tact-gui/src/shell.rs`（`TactApp::diffs`、`adopt`、`apply_agent_update`）；`crates/agent_tui_kit/src/render/popups/diff_popup.rs`（同样的懒加载 git diff 策略） |

**症状 / 动机：** GPUI 的 Diff 面板原先只列出变更路径和工具结果里的 detail 字符串，但 `write_file` / `edit_file` 的 detail 是完整新文件内容，`apply_patch` 只有摘要。面板因此无法展示真正的增删行，也没有可用的行号槽。

**决策：** 不改协议，直接在会话工作区读取 `git diff --no-color -- <path>`，与 TUI 的懒加载 diff 弹窗保持一致。每个路径在会话存续期间或下一次记录变更之前只缓存一次结果；clean、untracked 或不在仓库中的路径回退到记录下来的 detail。

**改后行为：** Diff 面板按主题色渲染新增、删除、上下文和 hunk 标记行，并使用区分新旧文件侧的行号。缓存避免每帧重复启动进程；切换会话或追加变更时使其失效，从而拾取外部修改。

**指针：** `crates/tact-gui/src/pane.rs`（`DiffPane`、`git_diff`、`diff_lines`、`DiffNumbering`、`diff_body`）；`crates/tact-gui/src/shell.rs`（`TactApp::diffs`、`adopt`、`apply_agent_update`）；测试 `diff_lines_drop_the_preamble_and_start_at_the_first_hunk`、`diff_numbering_uses_the_old_side_for_removed_lines`、`git_diff_reads_the_working_tree_change_for_a_tracked_path`。

---

## 1. 2026-09-18 — 取消能打到"leader 已退出的进程树"，且记录保留输出流的结尾

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | `crates/tact/src/background.rs`（`terminate_tree`、`run_background_process` 的 spawn、`OutputAccumulator`、`output_tail`）；`crates/tact/src/tool/subagent.rs`（子进程的 toolset）；[Ch 13](./13_chapter_background_zh.md) §1；[Ch 07](./07_chapter_tool_zh.md) §7.1；[Ch 27](./27_chapter_sandbox_zh.md) §3 |

**症状 / 动机：** 合并前 review 的三项发现，都是实测而非推断。

1. *取消打不到"没有 leader 的树"*。`terminate_tree` 在**杀的时刻**读 `child.id()`，但 tokio 的 `Child::id()` 在子进程被 poll 到完成后返回 `None`——而 `sh -c 'server &'` 恰好就是这个状态：shell 退出并被收割，后台化的孙进程仍持有 stdout/stderr 管道，于是运行循环停在 `closed_pipes < 2` 而 `exit_status = Some(...)`。此时取消不会发出任何信号，却把记录写成 `Error: Cancelled by the user`，孤儿继续运行——正是当年移除 120 秒超时要消灭的那个"记录谎报自己做了什么"的失败（2026-09-16 条目）。本机实测：leader 已消失，孤儿的 `ppid` 为 1、`pgid` 仍是已死 leader 的 pid，而 `kill(-pgid)` **确实能打到它**——进程组是可信号的，只是 id 取晚了。既有测试用的是 `'sleep 371 & wait'`，其中的 `wait` 让 leader 存活，因此从未进入该状态。
2. *记录里的输出既不是开头也不是结尾*。`OutputAccumulator` 保留**前** `MAX_OUTPUT_CHARS`（50k）并丢弃其后全部内容，于是 `output_tail`——其命名与文档都声称是"输出的尾部"，也是所有面向模型的读取（`check_background`、`wait_background`、`background_run(wait_ms)`）所报告的内容——返回的是更长输出流中约第 46k–50k 个字符，却打印着"truncated to the last 4000 chars"。构建 / 测试日志真正有用的结尾从未进入上下文。既有测试只注入 12k 字符（未达上限），因此从未触发截断。
3. *子 agent 的 `bash` 在沙箱里跑，描述却说是非沙箱*。子 agent 继承父进程的 `ToolContext`（因此 `ctx.sandbox` 是 `Some`，它的 shell 确实运行在 bubblewrap 下），但只有 `tact-ui` 的两个入口应用了 `SANDBOXED_BASH_DESCRIPTION`；`spawn_subagent` 构造 router 时没有应用。在 worktree 场景下更糟：子 agent 的 system prompt 给出一个宿主路径，而它自己的 `/workspace` shell 看不到该路径。

**决策：**（1）进程组 id 在 **spawn 时**捕获、绝不在 kill 时才取，并传入 `terminate_tree`——与 `tool::bash` 早已采用的顺序一致。只要进程组还有成员，它就比 leader 活得更久，因此捕获的 id 仍能命中幸存者。（2）让 `OutputAccumulator` 保留**末** `MAX_OUTPUT_CHARS`，并以 8,192 字符的 slack 触发裁剪，使话痨命令很少付出那次 `memmove` 代价，从而让 `output_tail` 名副其实。（3）当 `ctx.sandbox.is_some()` 时，在 `spawn_subagent` 中应用沙箱版 bash 描述。

**改后行为：** 取消 `background_run` 任务会杀掉整棵树，即使它的 shell leader 已经退出，因此记录里的 `Error: Cancelled by the user` 是真的。输出超过 50k 字符的任务记录其**最后** 50k，`check_background` / `wait_background` 显示日志真正的结尾；完整输出流仍在 `<workdir>/.tact/background/<id>.log`，而它现在是更早那部分唯一留存的地方。处于 `tools.sandbox = true` 的子 agent 会看到沙箱版 `bash` 描述，因此被告知的是 `/workspace` 而不是宿主路径。同一轮修正的文档：Ch 07 §7.1 与 Ch 10 §1 曾称沙箱"禁用网络"/"连不上网络"，自 `--share-net`（2026-09-16 条目）起都不成立；`config.example.toml` 曾称 `cargo fetch` / `npm install` 仍可用（工具链目录是只读的）以及沙箱读不到凭据（只读挂载的 `~/.cargo` **是可读的**），其 subagent `reasoning_effort` 注释还描述了一个并不存在的第三层回退。刻意未修、仍然开放的两项：`sleep` 的时长在生产路径不可达（`ArgumentSummaryPolicy::Json` 永远不会产出裸数字，因此标题无法格式化成 `💤 Sleep · 1m 30s`），以及弹窗正文按宽度截断而非折行。

**指针：** `crates/tact/src/background.rs` 的 `terminate_tree` 与 `process_group_id` 捕获；`OutputAccumulator` + `output_tail`；`crates/tact/src/tool/subagent.rs`（`subagent_tools`）；测试 `cancelling_reaches_a_tree_whose_leader_already_exited`（已验证：对"kill 时刻取值"的写法会失败，幸存者为 `['sleep 372']`）、`the_output_buffer_keeps_the_newest_characters`、以及 `run_writes_full_output_to_log_file_and_truncates_db_record`（断言改为 `ends_with`）。

---

## 1. 2026-09-17 — 卡片标题的标签来自工具自己的 presentation，不再由绘制它的分支硬编码

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | `crates/agent_tui_kit/src/widgets/tool_widget.rs`（`title_text` 的 `Sleep` 分支、`display_name_from_presentation`、`is_written_argument`）、`crates/tact/src/tool/metadata.rs`（`ArgumentSummaryPolicy::Id`）、`crates/tact/src/tool/background_run.rs`（`CHECK_BACKGROUND_METADATA`、`WAIT_BACKGROUND_METADATA`）、`crates/tact/src/agent/tool_dispatch.rs`（`tool_arg_full`）；[第 13 章](./13_chapter_background_zh.md) §1；[第 23 章](./23_chapter_tui_zh.md) §6.16 |

**现象 / 动机：** `wait_background` 复用了 `ToolVisualKind::Sleep`，而那个分支把标签写死成 `⏳ Sleep · {}`，`{}` 里是序列化后的输入。一行里两个 bug。标签完全无视工具自己的 `display_name`（`⏳ Wait Background` 成了构造上就不可能被读到的死字段），同一个硬编码还意味着 `sleep` **从来没有**画出过它元数据里声明的 `💤 Sleep`。参数同样错位：两个后台工具都用 `ArgumentSummaryPolicy::Json`，于是标题里带的是 `{"task_id":"abc123"}` 这种 dump——可选的 id 没传时就是一个光秃秃的 `{}`——而这里本该是给人读的 id。

**决策：** visual kind 只决定标题的**形状**，绝不决定**名字**。`Sleep` 分支的标签改为取 `display_name_from_presentation(&self.presentation, &self.tool_name)`（为空或等于工具名时回退 `tool_display_name`），这样一个共享 kind 可以服务多个工具——`Command` 与 `Task` 本来就是这个规则。时长 mini-language 保留；序列化参数不进标题，判据与弹窗一致（`is_written_argument`，即 `argument_line` / `popup_detail` 用的那个守卫）。参数本身在源头修：新增 `ArgumentSummaryPolicy::Id { field }`，只暴露一个字段，且当调用省略了这个可选 id 时返回 `""`——刻意不回退到 JSON dump，因此没传 id 的调用画出来就是光标签。`check_background` 与 `wait_background` 由 `Json` 改为 `Id { field: "task_id" }`，唯一消费点是 `tool_dispatch::tool_arg_full`。

**改后行为：** `⏳ Wait Background`、`⏳ Wait Background · abc123`、`💤 Sleep · 1m 30s`；`check_background` 读作 `⚙️ Background Check  abc123`（`Generic` 分支两空格拼接的形态不变）。其余元数据为 `Json` 的工具——`team_*`、`worktree_*`、`save_memory`、`load_skill`、`compact`，以及所有走 `tool_arg_full` 的 `_ => Json` 回退的 MCP / 插件工具——标题里仍然会打出 dump：那是逐工具的决定，还没动，不能一次全局改掉（`_` 分支正是 MCP 工具依赖的）。之后若需要，`subagent_check` / `subagent_wait` 是下一批 `Id { field: "child_id" }` 的候选。顺手记一个 rustfmt 坑：新变体上用的是 `//` 行注释而不是 `///`——在枚举变体上加文档注释会把整个 enum 展开成每变体多行。

**指针：** `crates/agent_tui_kit/src/widgets/tool_widget.rs` 的 `title_text` / `display_name_from_presentation` / `is_written_argument`；测试 `wait_background_title_reads_its_own_label`、`check_background_title_shows_the_task_id`、`sleep_title_keeps_the_duration_with_a_presentation`（5 条既有的 `sleep` 标题断言由 `⏳` 改为 `💤`）；`crates/tact/src/agent/tool_dispatch.rs` 的 `id_policy_reads_the_field_and_tolerates_absence`、`background_tools_title_shows_the_id_not_the_input_dump`。

---

## 1. 2026-09-16 — 每个工具的弹窗都以它发起的那次调用开头

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | `crates/agent_tui_kit/src/widgets/tool_widget.rs`（`with_command_detail`、`popup_detail`、`argument_line`、`is_written_argument`、`command_detail`）、`crates/agent_tui_kit/src/components/tool.rs`（`on_background_task_finished`）、`crates/agent_tui_kit/src/render/popups/subagent_popup.rs`（既有的 `Prompt:` 前置）；[第 13 章](./13_chapter_background_zh.md) §1；[第 23 章](./23_chapter_tui_zh.md) §6.16 |

**症状 / 动机：** 弹窗是读取已折叠块内容的唯一途径，所以它既要显示结果、也要显示调用本身。已完成的 `bash` 块与**运行中**的 `background_run` 卡片都以 `$ <命令>` 开头；有三处比用户刚刚在读的那张卡片显示得更少。(1) **收尾后**的 `background_run` 卡片：`BackgroundTaskFinished` 不携带 `StepResult`，该路径只用进程输出拼 detail。(2) **失败**的命令（`bash`、`worktree_run`、`web_search`、`background_run`）：`from_step_result` 对失败刻意不加前缀，好让报错占住卡片前几行预览——而失败弹窗的标题是通用的错误卡片标题，于是失败命令的参数在弹窗里彻底消失。(3) **Task** 类工具（`task_create/get/list/update`）与 `ask_user`：弹窗正文只有结果，任务标题、以及用户正在回答的那个问题，都不在弹窗里。

**决策：** 一条规则，按视觉种类分派，统一在 `ToolWidget::popup_detail` 里拼装（`detail_full` 的唯一产出点，而只有弹窗会读它）：弹窗以调用开头。`Command` → 所有阶段都以 `$ <命令>` 开头（keep-live 收尾路径由 `with_command_detail` 提供，失败时它原样存下 detail，再由 `argument_line` 补上该行，因此卡片仍然报错优先）；`FileRead`/`FileWrite`/`FileEdit` → 不再额外加，正文**就是**这次调用（文件、写入内容、差异），路径本来就在弹窗标题里；`Subagent` → 这里也不加，它自己的弹窗会用同一个 `arg_full` 前置 `Prompt:`；`Task`/`Generic`/`Sleep` → 加参数行，**除非它是工具的序列化输入对象**（`is_written_argument`）——JSON dump 是一条超长转义行，与日志参数行重复，还会把弹窗存在的理由（结果）往下挤。`detail_preview` 不变；折叠块的提示现在按 `detail_full` 计数，所以提示与弹窗仍然打印同一个数字。

**改后行为：** 已完成或失败的 `background_run` 弹窗、失败的 `bash`/`worktree_run`/`web_search` 弹窗、折叠后的 `task_*` 弹窗、以及 `ask_user` 弹窗，都以那次调用开头（`$ <命令>`／任务标题／问题），而所有卡片形态不变——失败命令的卡片依旧报错优先。JSON 入参的工具（`save_memory`、`load_skill`、`wait_background`、所有 MCP/插件工具）刻意保持原样：它们的参数是 dump，不是给人读的文字。两条需要留档的审查更正：第一轮审查把 `spawn_subagent` 报成"prompt 不在弹窗里"，这是错的——双击子代理块打开的是专用的 subagent 弹窗（永远不会走 diff 弹窗），而那个弹窗从写下起就会前置 `Prompt:\n<arg_full>`，由 `live_layout_prepends_prompt_to_transcript` / `completed_layout_prepends_prompt_to_summary` 钉住。刻意留待决定、已上报的缺口：`apply_patch` 把 patch 预览当文件路径去 `git diff`（所以它的弹窗只显示执行结果，patch 全文看不到）；`read_file` 的 `offset`/`limit` 任何地方都不显示；命令类弹窗标题仍硬编码 `bash (…)`，其他类型则用原始工具名（`save_memory output`）；弹窗正文行仍是横向截断而非换行。

**指向：** `crates/agent_tui_kit/src/widgets/tool_widget.rs` 的 `popup_detail` / `argument_line` / `is_written_argument` / `with_command_detail` / `command_detail`；`crates/agent_tui_kit/src/components/tool.rs` 的 `on_background_task_finished`；测试 `failed_command_card_stays_error_first_but_its_popup_opens_with_the_command`、`failed_command_detail_is_not_double_prefixed`、`failed_non_command_keeps_its_raw_detail_in_the_popup`、`task_popup_opens_with_the_task_title`、`ask_user_popup_opens_with_the_question`、`json_argument_is_not_repeated_in_the_popup`、`kinds_whose_body_is_the_call_do_not_repeat_it`、`failed_task_popup_shows_the_title_before_the_error`、`background_run_popup_opens_with_the_command_like_bash`、`failed_command_popup_opens_with_the_command`、`collapsed_task_popup_opens_with_the_task_title`、`ask_user_popup_opens_with_the_question`、`json_input_tool_popup_does_not_repeat_its_argument`。

---

## 1. 2026-09-16 — 后台任务不再有时间上限，取消则终止整棵进程树

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | `crates/tact/src/background.rs`（删除 `COMMAND_TIMEOUT`，新增 `configure_process_group` / `terminate_tree`，`start`/`run` 接收会话取消标志）；`crates/tact/src/tool/background_run.rs`（`MAX_RUN_WAIT_MS`、提示词）；[第 13 章](./13_chapter_background_zh.md) §1/§3/§8；`docs/agent_guidelines.md` |

**症状 / 动机：** `background_run` 自称是慢命令（构建、测试套件、安装）的去处，但每个任务在 **120 秒**后被 `SIGKILL`，记录被改写成 `Error: Timeout (120s)`。同一次会话实测：`cargo test --workspace`（约 110 秒）被杀了两次，而两次模型都把这个 `Error` 读成"测试失败"。这个 kill 还谎报了它做了什么：它只对 `sh -c` 领头进程发信号，于是 `sleep` 孙进程继续跑（实测：在"超时" 35 秒后仍活着），而记录却说任务已结束。另外，`background_run(wait_ms: 300000)` 能把一整个回合卡住最多五分钟——一个**启动**原语却在**慢**命令上阻塞。

**决策：** 后台任务**没有时间上限**：它要么在命令退出时结束，要么在会话取消标志被置位时结束。这使得取消成为唯一的提前终止途径，因此它必须变成真正的 kill：任务现在 spawn 进自己的进程组（`configure_process_group`，与 `bash` 工具同一手法），取消时向取负的 pid 发 `SIGKILL`，把 `cargo`/`npm` 等孙进程一起带走。标志复用已有的进度 tick（约 50 ms）检查，没有引入新的定时器。`background_run(wait_ms:)` 明确降级为**短任务**捷径，上限 **10 秒**；`wait_background` 保留 5 分钟上限，因为阻塞就是它的全部职责。等待到期只回报"仍在运行"，绝不碰任务本身。

**改后行为：** 用 `background_run` 启动的命令想跑多久就跑多久——30 分钟的构建也没问题。取消一个回合（Esc）会终止该会话所有运行中的任务，状态变为 `Error`、内容为 `Cancelled by the user`；进程树里不会留下任何东西。`background_run` 即使传了很大的 `wait_ms`，也会在 10 秒后带着任务 id 和"仍在运行"一行返回，而不是卡住回合。`wait_background` 行为不变：任务一结束就返回，否则如实说仍在运行。

**指向：** `crates/tact/src/background.rs`（`run_background_process` 循环、`terminate_tree`、`configure_process_group`）；`crates/tact/src/tool/background_run.rs`（`MAX_RUN_WAIT_MS`、`capped_run_wait_ms`）；测试 `background::tests::cancelling_terminates_the_task_and_its_children`、`tool::background_run::tests::the_run_wait_is_capped_to_a_short_task`；[第 13 章](./13_chapter_background_zh.md) §1/§3/§8。

---

## 1. 2026-09-16 — bash 沙箱重新共享宿主网络

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | `crates/tact/src/sandbox/bwrap.rs`（`--share-net`、`RESOLVER_PATHS`、`PROXY_ENV_VARS`、`bwrap_args_with(work_dir, exists, env)`）；`crates/tact/src/tool/bash.rs`（`SANDBOXED_BASH_DESCRIPTION`）；spec `docs/superpowers/specs/2026-09-15-bwrap-sandbox-design.md` §8/§17/§Tests 7；[第 27 章](./27_chapter_sandbox_zh.md) §3 |

**症状 / 动机：** 打开 `[tools] sandbox = true` 后 shell 是个"网络死人"：没有 DNS、没有默认路由，宿主的代理 `127.0.0.1:7890` 甚至都不可达——连接 0 ms 就被拒，看起来像"代理坏了"而不是"没有网络"。`curl` / `git fetch` / `npm install` / `cargo fetch` 全都跑不通。文档里给的绕行方案（`background_run`，未沙箱化的宿主 shell）对 subagent 不成立——它们的受限工具集里没有 `background_run`——所以 subagent 什么都抓不到，本次实测三个 research lane 全部报 `curl` exit 6/7。用户要求把网络还给沙箱，并选择同时透传代理变量。

**决策：** 共享宿主网络命名空间（`--share-net`，bwrap 默认行为的显式写法）；并且，因为只共享命名空间还不够，额外把解析器目录只读挂载进来。`--clearenv` 白名单只开一个例外：宿主设置了 `http_proxy` / `https_proxy` / `all_proxy` / `no_proxy`（两种大小写拼写）时原样透传，因为宿主 loopback 上的代理在沙箱内**确实**可达。这个能力依旧**不可配置**：不加开关，与策略其余部分保持一致。v1 当初刻意禁网，此决定现被推翻——沙箱约束的是文件系统，不是连通性。

**改后行为：** 沙箱内域名解析可用，出网行为与宿主完全一致——包括宿主自身直连不了的目标，这正是本开发机的现状（`example.com` 通；`api.binance.com`、`api.coinbase.com`、`1.1.1.1:443` 全部超时；走代理则一切正常）。文件系统边界不变：工作区仍是唯一可写的宿主目录，宿主 home 仍未挂载，白名单之外的变量仍进不来。`--unshare-pid`、`--die-with-parent`、工作区守卫与被禁止的 `--new-session` 都未改动，`bash` 的工具描述也不再声称网络被禁用。

**两个坑，均为实测：** 只共享命名空间**不足以**解析 DNS——宿主的 `/etc/resolv.conf` 是指向 `/run/systemd/resolve` 的符号链接，而 `/run` 不在挂载列表里，于是链接悬空，每次解析都报 "Temporary failure in name resolution"；而直接绑定 `/etc/resolv.conf` 会被 bwrap 拒绝（`Can't mount on symlink destination /etc/resolv.conf`），因此必须挂目录。回归测试因此刻意不碰外网：它连接由测试自己在宿主 loopback 上开的监听端口，只有命名空间共享时才连得通。

**指向：** `crates/tact/src/sandbox/bwrap.rs`（`RESOLVER_PATHS`、`PROXY_ENV_VARS`、flag 列表、可注入的环境查询）；测试 `sandbox::bwrap::tests::shares_the_host_network_and_binds_the_resolver`、`sandbox::bwrap::tests::carries_proxy_variables_and_nothing_else`、`tool::bash::sandbox_tests::shares_the_host_network_namespace`、`tool::bash::sandbox_tests::system_files_are_readable_and_proxies_follow_the_host`；[第 27 章](./27_chapter_sandbox_zh.md) §3。

---

## 1. 2026-09-16 — 后台三条工具提示词与实现重新对齐，状态读取也改为有界尾部

| Field | Value |
|-------|-------|
| **Type** | docs |
| **Related** | `crates/tact/src/tool/background_run.rs`（三条元数据描述 + 输入字段）；`crates/tact/src/tool/sleep.rs`；`crates/tact/src/background.rs`（`check`、`OUTPUT_TAIL_CHARS`、`output_tail`）；[Ch 13](./13_chapter_background_zh.md) §1/§6 |

**症状 / 动机：** 等待工具与会话作用域落地后，工具描述已与代码不符：`check_background` 仍描述成不加范围的状态查询（列表现在只列本会话，`No background tasks.` 也因此含义不明）；`wait_background` 从未写明默认 5 分钟、以及不给 id 即等本会话全部任务；`background_run` 既没说何时该优先于 `bash`，也没提一条会坑到模型的事实——它是普通宿主 shell，`bash` 描述的沙箱 `/workspace` 路径空间对它不适用。另外 `check_background <id>` 会把整条记录倒出来，其中 `output` 可达 50,000 字符（约 1.2 万 token）直接进 context，而等待路径只内联最后 4,000 字符。

**决策：** 让三条描述与实际行为重新对齐，并把"有界尾部"收敛成面向模型读取的唯一实现：`OUTPUT_TAIL_CHARS` / `output_tail` 移入 `background.rs`，`check_background <id>` 与 `wait_background`、`background_run(wait_ms:)` 走同一份。列表为空时若调用方有会话，文案改为 "No background tasks in this session."。

**行为变化：** 提示词写明了作用域、等待默认值、不给 id 的会话级等待，以及宿主 shell / 宿主路径这一注意点。单任务状态读取返回同样的有界尾部加 `output_path`，全量流仍在磁盘上。工具的权限、调度与结果状态均未改动。

**指针：** `crates/tact/src/background.rs`（`check`、`output_tail`、`OUTPUT_TAIL_CHARS`）；`crates/tact/src/tool/background_run.rs`（`BACKGROUND_RUN_METADATA`、`CHECK_BACKGROUND_METADATA`、`WAIT_BACKGROUND_METADATA`、`report_waited`）；测试 `background::tests::check_bounds_the_output_of_a_single_task`、`output_tail_keeps_the_end_and_marks_truncation`；[Ch 13](./13_chapter_background_zh.md) §1/§6。

---

## 1. 2026-09-16 — 后台任务的检索按会话收窄

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/background.rs`（`check`、`record_in_session`）；`crates/tact/src/tool/background_run.rs`；`crates/tact-ui/src/driver.rs`（`QueryBackground`）；[Ch 13](./13_chapter_background_zh.md) §1 |

**症状 / 动机：** 任务 store 在 `<workdir>/.tact/tact.db`，同一项目的每个会话共用它——而 `check_background`（不带 `task_id`）与 TUI 的 `/background` 会列出其中**每一条**记录，包括别的会话启动的任务。agent 自己的列表里混进了它从未启动过的活，人看的列表也一样。

**决策：** 把列表按调用方会话收窄，复用等待逻辑里已有的判定。显式给出 `task_id` 仍然不限会话（调用方点名要它），而缺失/空的 session id 仍旧表示"全部"——没有会话的上下文没有可过滤的依据。本身不带 session id 的记录不属于任何会话，因此在过滤后的列表中不可见。

**行为变化：** 不带 `task_id` 的 `check_background`、不带 id 的 `wait_background`、以及 `/background` 都只报告当前会话的任务；`check_background <id>` / `/background <id>` 不变。子 agent 根本无法启动后台任务（其受限 toolset 没有 `background_run`）；若将来有了，记录会挂在子会话 id 下，届时把范围扩展到子会话即可。

**指针：** `crates/tact/src/background.rs`（`check(task_id, session_id)`、`record_in_session`、`has_running`）；`crates/tact/src/tool/background_run.rs`（传 `ctx.session_id`）；`crates/tact-ui/src/driver.rs`（传 agent runtime 的 session id）；测试 `background::tests::check_lists_only_the_requested_session`、`tool::background_run::tests::check_background_lists_only_this_session`。

---

## 1. 2026-09-16 — 等后台任务不必再猜一个 sleep 时长

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/background.rs`（`wait`、`start`、`record`）；`crates/tact/src/tool/background_run.rs`（`wait_background`、`background_run(wait_ms:)`）；`crates/tact/src/tool/registry.rs`；[Ch 13](./13_chapter_background_zh.md) §1/§6；[Ch 11](./11_chapter_task_zh.md)；`docs/agent_guidelines.md` |

**症状 / 动机：** `background_run` 立即返回，而**模型**侧没有完成推送——`AgentUpdate::BackgroundTaskFinished` 只进 TUI 卡片。于是实际模式变成 `background_run` → `sleep` → `check_background`：时长全靠猜，猜长了是纯空等，猜短了又多花一整轮 LLM 往返；更糟的是单个 `sleep 300000` 完全无法打断——in-flight 工具不会被取消（取消只在 wave 边界生效），而 `sleep` 的 future 根本不读 `cancel_flag`。本仓库 `tact.db` 实测：18 次 `sleep` 调用时长落在 30–300 秒，`check_background` 77 次。

**决策：** 把"等它"做成精确的一等原语，而不是一个要猜的时长。`BackgroundManager::wait(task_id, session_id, timeout, cancel)` 阻塞到该任务（或本会话所有任务）进入终态；判定在**第一次 sleep 之前**先做一次，所以早已完成的任务立刻返回；实现是对现有 store 的 150 ms 读循环（完成状态由分离的任务写入，目前没有可订阅的事件源），并在下一次轮询观察取消标志。两个工具架在它上面：`wait_background { task_id?, timeout_ms? }` 与 `background_run { command, wait_ms? }`，后者在窗口内跑完时直接把输出返回。这**不是**事件驱动的方案（完成即唤醒回合，subagent 走的那条路）——那需要协议与 driver 改动，仍然开放。

**行为变化：** 单独调用 `background_run` 行为不变。带 `wait_ms` 时，及时结束的命令返回的是任务结果（状态、耗时、输出尾部、日志路径）而不是一个 id；否则返回启动行并附"仍在运行"的说明。`wait_background` 不带 `task_id` 时等待本会话的全部任务；`timeout_ms` / `wait_ms` 默认 5 分钟并以此为上限，与 `sleep` 一致。内联输出被限制为最后 4,000 字符并给出完整日志路径，避免大日志淹没 context。`sleep` 的描述现在写明它不用于等待后台任务，`check_background` 也指向 `wait_background`。

**指针：** `crates/tact/src/background.rs`（`WaitOutcome`、`WAIT_POLL_INTERVAL`、`wait`/`has_running`、从 `run` 中拆出的 `start`、`record`/`records`）；`crates/tact/src/tool/background_run.rs`（`WaitBackgroundTool`、`report_waited`、`session_report`、`output_tail`）；测试 `background::tests::wait_*` 与 `tool::background_run::tests::wait_background_*`；`crates/agent_tui_kit/src/widgets/tool_widget.rs`（回退视觉类型 `Sleep` + 显示名）。

---

## 1. 2026-09-15 — 只收到 reasoning 的 Responses 流会自报身份，不再读起来像空流

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/openai/responses/stream.rs`（`ResponsesStreamState`、`finish()`）；[Ch 22](./22_chapter_llm_zh.md) §6.2 |

**症状 / 动机：** 在一个兼容端点上（`protocol = "responses"` + `https://opencode.ai/zen/go/v1`，模型 `deepseek-v4.1-flash`，`reasoning_effort = "low"`），只产出 reasoning 的一轮以 `unsupported response state: OpenAI Responses stream ended without a terminal event` 结束。这条文案描述的是**空流**，排查方向也就被带偏了——实际上 reasoning delta 是到达过的（TUI 上显示为 thinking）。它们只被转发给 UI、哪里都没存，于是在 `finish()` 内部，"只收到 reasoning 的流"和"什么都没收到的流"完全无法区分。

**决策：** 保持硬失败不变——既没有可见文本、也没有已完成 output item 的回合不是完整回合；而"恢复"它只会产出一条仅含 thinking block 的 assistant 消息，正是 `sanitize_assistant_messages` 专门在打补丁的形态（[Ch 22](./22_chapter_llm_zh.md) §6.1）——但让状态可观测：在流状态里统计 reasoning delta 数，把判定所依据的三个量（reasoning delta 数、已完成 output item 数、已 announce 但未完成数）都报出来，并按"是否收到过 reasoning"分流文案。该端点上的根因是网关未发送终态事件就关闭了流；把该条目改成 `protocol = "chat_completions"` 可以完全绕开。

**行为变化：** "只有 reasoning 的流"与"真正的空流"现在给出不同的句子，前者会同时点明端点行为，以及为什么这是协议失败而不是空回答。除此之外没有变化：完整 `output_item.done` 序列与已流式可见文本这两条恢复路径未被触碰，判定逻辑本身也未改。

**指针：** `crates/tact_llm/src/openai/responses/stream.rs`（`reasoning_deltas`、`thinking_delta`、`finish()` 的终态事件分支）；测试 `no_terminal_event_after_reasoning_only_names_the_reasoning` 与 `no_terminal_event_empty_stream_is_error`；[Ch 22](./22_chapter_llm_zh.md) §6.2。

---

## 1. 2026-09-15 — `bash` 可以在可选开启的 bubblewrap 沙箱中运行

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/sandbox/{mod,bwrap}.rs`；`crates/tact/src/tool/bash.rs`；`crates/tact/src/config/types.rs`（`tools.sandbox` 开关）；[Ch 7](./07_chapter_tool_zh.md) §7.1；[Ch 10](./10_chapter_permission_zh.md)；[Ch 21](./21_chapter_config_zh.md)；[Ch 27](./27_chapter_sandbox_zh.md)；[设计](../docs/superpowers/specs/2026-09-15-bwrap-sandbox-design.md)；[实施计划](../docs/superpowers/plans/2026-09-15-bwrap-sandbox.md) |

**症状 / 动机：** 被批准的 `bash` 命令此前以普通宿主进程运行：可以读取用户 home（SSH 密钥、云凭证）、任何无关仓库，以及宿主网络。权限回答的是*这条命令能不能运行*，而不是*它能触达什么*；真正要紧的失败面也不是 agent 自己写的命令，而是它引入的第三方代码——`cargo` 构建脚本、`npm` 生命周期脚本、测试二进制、`make` 配方。

**决策：** 不动权限模型，另加一层可选开关。`[tools] sandbox` 布尔开关（默认 `false`）把现有的 `sh -c` 进程包进平台的沙箱——Linux 选 bubblewrap，其他平台尚无实现，开关在那里是空操作（会被告知，绝不静默）。开关做成布尔值而非后端名，是因为后端不是用户的选择：把 `"bwrap"` 暴露成配置取值，等于允许写出一份在读取它的机器上根本无法生效的配置；沙箱只负责构造调用，spawn、流式输出、超时、取消与进程组清理都不变。策略是：把 `work_dir` 以读写绑定到 `/workspace`；`/usr` `/bin` `/lib` `/lib64` `/etc` 只读；再加固定的工具链白名单（`~/.rustup`、`~/.cargo`、`~/.config/git`、`~/.npm`，通过 `RUSTUP_HOME` / `CARGO_HOME` / `GIT_CONFIG_GLOBAL` / `NPM_CONFIG_CACHE` 接线）只读；挂载新的 `/proc` 与 `/dev`、tmpfs 的 `/tmp`；`--clearenv` + 显式变量白名单；`--unshare-net`、`--unshare-pid`、`--die-with-parent`。**禁止** `--new-session`：实测它会把命令分离到自己的进程组，于是现有的 `killpg` 清理只杀掉 `bwrap`，孙进程仍持有管道写端，工具调用永不返回。任何导致沙箱无法启动的情况都降级为不沙箱，并在启动时告警（fail-open，但绝不静默），而不是让工具失败。

**行为变化：** 默认 `false` 下行为完全不变；在无实现的平台上设为 `true` 同样不变（但会告警）。真正启动沙箱时：`pwd` 为 `/workspace`；工作区之外的宿主路径与宿主 home 不可达；`curl`/`git fetch`/`npm install` 没有网络；挂载的 `/proc` 只显示沙箱内进程（拿不到宿主进程表，也无法给同 uid 的宿主进程发信号）。由于只读工具链白名单，`cargo`/`git`/`npm` 仍可工作。`bash` 的工具描述会在启动时按**实际解析结果**重写，写明 `/workspace` 与网络禁用，从而把路径空间分裂（进程内工具仍报宿主绝对路径）暴露给模型。文档同时写明范围：沙箱约束的是被批准命令所引入的第三方代码，**不是** agent——`background_run` 与 `worktree_run` 仍启动未沙箱的宿主 shell（[Ch 13](./13_chapter_background_zh.md)、[Ch 15](./15_chapter_worktree_zh.md)）。

**指针：** `crates/tact/src/sandbox/mod.rs`（`Sandbox`、`resolve`、`SandboxDegradation`）；`crates/tact/src/sandbox/bwrap.rs`（纯函数 `bwrap_args`、工作区守卫、probe）；`crates/tact/src/tool/bash.rs`（`SANDBOXED_BASH_DESCRIPTION`、`match &ctx.sandbox` 起点、一次性降级提示）；`crates/tact-ui/src/{interactive,headless}.rs`（启动解析 + 提示 + 描述覆盖）；`config.example.toml` 的 `[tools] sandbox`。

---

## 1. 2026-09-15 — 原生 MCP 配置改名 `.mcp.json`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/consts.rs`（`MCP_CONFIG_FILE`）；`crates/tact-ui/src/mcp_cli.rs`；[第 8 章](./08_chapter_mcp_zh.md)；[第 21 章](./21_chapter_config_zh.md) |

**现象 / 动机：** Tact 的两个原生配置文件叫 `mcp.json`（`~/.tact/mcp.json`、`<workdir>/.tact/mcp.json`），而同一份东西在整个生态里都叫 `.mcp.json`：Claude Code 的项目文件、插件包 `mcpServers` 指向的文件、VS Code 的 `.vscode/mcp.json`。同一个概念挂两个名字，唯一的效果是让人在"我这份该叫哪个"上多犹豫一次。

**决策：** 两个作用域统一改名为 `.mcp.json`（`~/.tact/.mcp.json`、`<workdir>/.tact/.mcp.json`），路径集中在 `MCP_CONFIG_FILE` 一个常量上，改名因此只有一处。**旧名不再读取，也不做任何兼容处理** —— 残留的 `mcp.json` 就是一个普通的不相关文件，既不是来源也不会被上报。

**改后行为：** 来源顺序不变（`<workdir>/.mcp.json` → `~/.tact/.mcp.json` → `<workdir>/.tact/.mcp.json` → 已安装插件），只是两个原生文件的名字带上了点。`mcp add` / `remove` 写入的是新路径；旧路径上的文件不再产生任何影响，也不会被提及。

**指针：** `crates/tact/src/consts.rs`（`MCP_CONFIG_FILE = ".mcp.json"`）；`crates/tact-ui/src/mcp_cli.rs`（`scope_hint` 改为按完整路径比对 Claude 项目文件）；[第 8 章](./08_chapter_mcp_zh.md) Step 1 来源表。

---

## 1. 2026-09-15 — 工作目录的 `.mcp.json` 会被读取；`enabled: false` 与未建模的键都不再无声

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/mcp/mod.rs`（`collect_sourced_servers`、`resolve_servers`、`McpProjectConfig`、`report_unmodelled_keys`）；`crates/tact-ui/src/mcp_cli.rs`（`scope_hint`、`disabled_text`）；[第 8 章](./08_chapter_mcp_zh.md)；[第 21 章](./21_chapter_config_zh.md) |

**现象 / 动机：** 与 Codex / Claude Code 生态有两处不一致。其一，工作目录下的 `.mcp.json`——Claude Code 的项目作用域文件，也就是团队随仓库提交的那一份——被刻意忽略，于是检出别人的仓库后，随仓库共享的 server 必须手工抄进 `.tact/mcp.json` 才能用。其二，Codex 的条目级字段被静默丢弃：`McpProjectConfig` 没有 `deny_unknown_fields`，于是 `"enabled": false`（OpenAI 自带的 `unified-computer-use` 正是这么写的）被解析成一个普通条目并**照常连接**——一个声明"关掉"的 server 反被拉起；`enabled_tools`、`omit_tools_from`、`startup_timeout_sec`、`tools.<name>.output_token_limit` 更是连痕迹都不留。

**决策：** 把 cwd 的 `.mcp.json` 接纳为**最低优先级**来源（`<workdir>/.mcp.json` → `~/.tact/mcp.json` → `<workdir>/.tact/mcp.json` → 已安装插件），让仓库文件开箱可用，同时永远不会静默顶掉用户自己声明的 server（被顶掉的声明照旧以 `MCP server X overrides <file>` 上报）。它属于项目而不属于用户，因此解析失败只记一条 warning 并跳过，而不像 `mcp.json` 那样硬报错——否则 clone 到一个坏文件就能让 Tact 在那个目录里根本起不来。`McpProjectConfig` 新增两个字段：`enabled`（缺省为真）与收容未知键的 `#[serde(flatten)] extra`。`enabled: false` 的声明照常参与解析（能顶掉更低优先级的启用声明，也能被更高优先级的启用声明顶掉），但**绝不连接**，`mcp list` 把它显示为 `disabled (enabled: false)` 而不是 `unknown`；未知键不再丢弃，而是在 `mcp list` 与日志文件里逐条点名。**刻意未实现**：`enabled_tools`、`omit_tools_from`、`startup_timeout_sec`、`tools.<name>.output_token_limit` 只告警、不生效——Tact 的 MCP 层目前没有工具白名单、输出上限或 per-server 启动超时，`omit_tools_from` 更是 Codex 内部概念（`code_mode` / `deferred`），没有对应物。

**改后行为：** `mcp list` 多出一块「Entry keys Tact does not model」，点名哪个 server、被忽略的键、以及来源于哪个文件。检出仓库后，仓库 `.mcp.json` 里的 server 直接可用，工具名与原生 `mcp.json` 一样是 `mcp__<key>__<tool>`（不带前缀）。同名冲突时赢的永远是 Tact 自己的文件，且胜负会在启动提示与 `mcp list` 的 "Overridden declarations" 里点名。被 `enabled: false` 关掉的 server 不占启动时间、不出现在工具列表里，但仍在 `mcp list` / `/mcp list` / `mcp get` 里可见并标为 disabled。Codex 专有字段会在日志里逐条点名，配置不再无声消失。解析失败的外来 `.mcp.json` 只产生一条 warning。

**指针：** `crates/tact/src/mcp/mod.rs`（`collect_sourced_servers` 的来源顺序与前缀；`resolve_servers` 的 `Resolution` / `disabled`；`unmodelled_keys` 与 `McpLoadReport::unmodelled`）；`crates/tact-ui/src/mcp_cli.rs`（`disabled_text`、`scope_hint` 的 Claude 项目文件分支）；[第 8 章](./08_chapter_mcp_zh.md) Step 1 的来源表；[第 21 章](./21_chapter_config_zh.md)。

---

## 1. 2026-09-14 — 点号分隔的 DeepSeek V4 id 保住 1M 窗口

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/resolve.rs`（`model_context_window_for_model` 的 `deepseek-v4-` 前缀臂；`resolve_model_context_window_maps_deepseek_v4_variants`）；`config.example.toml`；[第 21 章](./21_chapter_config_zh.md)；[第 5 章](./05_chapter_compact_zh.md) |

**现象 / 动机：** 使用 `deepseek-v4.1-flash` 的会话窗口报成了 200K。V4 家族那条臂只匹配**连字符**前缀 `deepseek-v4-`，而这个 id 在小版本号前用的是点号（`v4` `.` `1`），于是没有任何一条臂命中，解析落到 `200_000` 默认值。这个回退是静默的，代价也不止于显示：底栏 `ctx` 显示 `/200K`，而在 `protocol = "responses"` 下推导出的 `responses_compact_threshold` 是 `窗口 − max_tokens − 10% 余量`——200K 且 `max_tokens = 65536` 时为 114,464，1M 时为 834,464——也就是说 1M 上下文的模型提前约 7 倍触发压缩。

**决策：** 把家族前缀放宽为接受两种分隔符——`starts_with("deepseek-v4-") || starts_with("deepseek-v4.")`——而不是干脆去掉连字符（裸 `starts_with("deepseek-v4")` 会连 `deepseek-v44` 之类一起吞掉）。两个显式 1M 别名（`deepseek-flash`、`deepseek-reasoner`）不变，解析顺序也不变：CLI > `[agent]` > 该映射 > 默认 200,000。顺手修正了该函数的文档注释——它仍声称映射优先级**最高**、会压过 CLI/TOML，而代码自从顺序翻转后就不是这样了。

**改后行为：** `deepseek-v4.1-flash`——以及任何 `deepseek-v4.*` 兄弟 id——无需改配置即解析为 `1_000_000`，`ctx` 用量条与推导出的 Responses 压缩阈值随真实窗口走。用户显式配置过的模型不受影响；其它未知 id 仍回退到 200,000。

**指针：** `crates/tact/src/config/resolve.rs`（`model_context_window_for_model`）；`config.example.toml`（模型→窗口清单）；[第 21 章](./21_chapter_config_zh.md)「模型 → 窗口映射」；[第 5 章](./05_chapter_compact_zh.md)（窗口 → 自动压缩阈值）。

---


## 1. 2026-09-14 — 压缩日志：单位修正，且每次尝试都打印它的请求信封

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode`、`think_block_bytes`、`[compact summary …]` / `[compact continue …]` 消息）；[第 5 章](./05_chapter_compact_zh.md) |

**现象 / 动机：** 压缩日志一错一缺。（a）续写提示写作 `summary truncated(8861 think tokens, 2000 max tokens)`，但前一个数字其实是 `thinking.len() + signature.len()`——思考块的**字节**长度加上它那段不透明的签名——却用了同一行另一个数字的单位；把它当 token 读，会引导出恰好错误的对比：拿 8861 去对 2000 的正文预算，或去对同一响应报出的 `reasoning_tokens: 2173`。（b）请求信封只在重试时才可见：`[compact usage: …]` 位于截断分支内部，因此一次成功（0 次续写）什么都不打印，而真正发给 provider 的 `max_tokens` 在成功路径上也从不显示。

**决策：** 修正单位，并让阶梯在**每一次**尝试上都自述，包括第 1 次。`think_len` 更名为 `think_block_bytes`；续写提示打印 `{think_block_bytes} think bytes`，并改为点名下一次的 `max_tokens`，而不是复述刚刚用掉的那个值。每次尝试现在都在截断分支之外发两行：调用前 `[compact summary {stage}/{total}] request model=… max_tokens=N (text T + reasoning R), reasoning_effort=…, input C chars`，调用后 `[compact summary {stage}/{total}] response stop=… usage=…`。请求行里的 `max_tokens` 就是实际发出的值（`attempt_max_tokens = summary_text_max_tokens + attempt_reserve`），并按两部分拆开；原先独立的 `[compact usage: …]` 已删除，因为响应行本身就带 usage。预算逻辑没有任何改动。

**改后行为：** 共六级（`continuation_attempt + 1` / `MAX_COMPACT_SUMMARY_ATTEMPTS + 1`），不论是否截断，每级都各打一行请求、一行响应，例如 `[compact summary 1/6] request model=deepseek-v4.1-flash max_tokens=2000 (text 2000 + reasoning 0), reasoning_effort=low, input 12044 chars` → `[compact summary 1/6] response stop=MaxTokens usage=TokenUsageInfo { … reasoning_tokens: 2173 … }` → `[compact continue 1/5] summary truncated (8861 think bytes), next attempt max_tokens=2000`。截断阶梯、预留升级与实际发出的 `max_tokens` 均不变。

**指针：** `crates/tact/src/agent/mod.rs`（`think_block_bytes`、`[compact summary …]` / `[compact continue …]` 消息）；[第 5 章](./05_chapter_compact_zh.md)。

---


## 1. 2026-09-14 — 空闲状态栏把聚焦面板还回它自己的槽位

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/agent_tui_kit/src/i18n.rs`（`status_idle_tmpl`，中英各一处）；`crates/agent_tui_kit/src/render/bar.rs`（`render_status_bar` 的 `Status::Idle` 分支）；`crates/tui/src/render/bar.rs`（`status_bar_idle_keeps_focus_theme_and_language_in_their_own_slots`）；[第 23 章](./23_chapter_tui_zh.md) §6.6 |

**现象 / 动机：** 空闲时顶栏一次搞错了两个读数，还悄悄丢掉了第三个。`status_idle_tmpl`——`"{} │ ⌨H Hist │ 🎨 {} │ 🌐 {} │ ? Help │ ✕ Quit"`——只有**三个**占位符，而 `render_status_bar` 的 `Status::Idle` 分支按固定顺序替换**四个**值：模式、聚焦面板、主题、语言。`str::replacen("{}", …, 1)` 完全按位置替换，于是每个槽位整体左移一格：聚焦标签落到了 🎨 主题的字符下，主题标签落到了 🌐 语言的字符下，而第四次替换已经找不到 `{}`——`replacen` 匹配不到时原样返回字符串，**不会**追加——语言标签就此彻底消失。空闲时因此渲染成 `◇ 插入 │ ⌨H Hist │ 🎨 Log │ 🌐 Dark │ ? Help │ ✕ Quit`：主题的位置上放着面板名，语言的位置上放着主题，而语言本身没了。鼠标点击判定以及其他 `render_status_bar` 分支都不受影响——Planning、Executing、Done 都用显式的 `format!("{} {} │ …", mode_str, focus_str, …)` 拼行，根本不碰这个模板。

**决策：** 模板补上它本来就该接的那一槽——`"{} {} │ ⌨H Hist │ 🎨 {} │ 🌐 {} │ ? Help │ ✕ Quit"` / `"{} {} │ H 历史 │ 🎨 {} │ 🌐 {} │ ? 帮助 │ ✕ 退出"`——四个占位符与四个实参（模式、聚焦面板、主题、语言）一一对应，也与另外三个分支开头的 `{mode} {focus} │ …` 一致。该分支的替换列表一字未改，只有模板补上了缺失的槽位。

**改后行为：** 空闲时英文渲染 `◇ 插入 Log │ ⌨H Hist │ 🎨 Dark │ 🌐 English │ ? Help │ ✕ Quit`，中文为对应镜像，聚焦面板、主题、语言各就其位。由 `status_bar_idle_keeps_focus_theme_and_language_in_their_own_slots` 钉住：它断言聚焦标签被画出、且**不在** 🎨 槽里，同时断言 `🌐 EN` 存在。（测试里带了一条测试桩说明：`buffer_text` 会把宽字符（`🎨`、`🌐`）的续格读成空格，所以匹配前要先折叠连续空白——否则即使槽位真的错位，`🎨  Log` 也会满足 `!contains("🎨 Log")` 这类守卫。）

**指针：** `crates/agent_tui_kit/src/i18n.rs`（`status_idle_tmpl`，中英）；`crates/agent_tui_kit/src/render/bar.rs`（`render_status_bar`、`Status::Idle`）；`crates/tui/src/render/bar.rs`（上述测试）；[第 23 章](./23_chapter_tui_zh.md) §6.6（顶栏）。

---


## 1. 2026-09-14 — 步骤标签去掉分母

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/i18n.rs`（`status_executing_tmpl`，中英各一处）；`crates/agent_tui_kit/src/render/bar.rs`（`Status::Executing` 分支）；`crates/tui/src/render/bar.rs`（`status_bar_executing_shows_the_step_label_without_a_gauge`）；[Ch 23](./23_chapter_tui_zh.md) §6.6 |

**现象 / 动机：** 顶栏渲染 `⠋ 正在执行步骤 4/10` / `⠋ Executing step 4/10`。分母描述的是**计划**而不是这次运行：`total` 是计划的步骤数，而分子是由「已完成 + 进行中」推导出来的，并不来自计划的顺序——于是有并行工具时，`n/total` 其实是两个不同的测量被印成一个分数。

**决策：** 模板只保留一个占位符——`Executing step {}` / `正在执行步骤 {}`——由该分支只填入推导出的步骤号。对该数字的 `total` 钳制保留——有工具在跑时是 `(completed + 1).min(*total)`，否则是 `completed.max(1).min(*total)`——因此 `total` 仍被读取，只是不再渲染。

**改后行为：** `Executing` 渲染 `◇ 插入 ◆ Log │ ⠋ 正在执行步骤 4 │ 并行中 1`；除步骤数与并行工具数之外，顶栏依旧不渲染任何自有数字。由 `status_bar_executing_shows_the_step_label_without_a_gauge` 钉住——它现在同时断言标签存在、且 `1/4` 与 `step 1/` 都不会被画出。

**指针：** `crates/agent_tui_kit/src/i18n.rs`（`status_executing_tmpl`）；`crates/agent_tui_kit/src/render/bar.rs`（`Status::Executing`）；`crates/tui/src/render/bar.rs`（上述测试）；[Ch 23](./23_chapter_tui_zh.md) §6.6（顶栏）。

---


## 1. 2026-09-14 — 符号链接的 skill 能加载了，Assembled prompt 也显示它携带的 MCP skills

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/skill/mod.rs`（`load_skills_from_dir_with_namespace`、`load_direct_plugin_skills` + `symlinked_skill_dir_is_loaded`、`symlinked_plugin_skill_dir_is_loaded`）；`crates/tui/src/system_prompt.rs`（`extract_mcp_skill_paths`、`assemble_prompt_view`）；`crates/tui/src/handlers/select.rs`（`SelectKind::ViewSystemPrompt`）；[Ch 2](./02_chapter_skill_zh.md) §2·§6；Ch 26 2026-09-10（skill 根收敛） |

**现象 / 动机：** 已安装的 skill 有两条路会在 `/view-system-prompt` 里消失，用户都是在 "Assembled current prompt" 弹窗上发现的。(1) skill 根目录是靠**符号链接**组装出来的——Omarchy 提供 `~/.agents/skills/omarchy -> /usr/share/omarchy/default/agents/skills/omarchy`——而 `load_skills_from_dir_with_namespace` 用 `WalkDir` 的默认 `follow_links(false)` 遍历：符号链接的**目录**既不会被下降进入、也不满足 `is_file()`，于是 `omarchy` 与 `diagnose-crash` 从 `# Available skills` 里静默消失（`~/.agents/skills` 下 32 个条目只出现 30 个）。(2) MCP server 是在**工具描述**里宣传自己 skill 的——Figma：`prefer the /figma-use skill if available, otherwise read skill://figma/figma-use/SKILL.md`——而 Tact 原样转发 MCP 描述，所以一个普通请求确实携带 `skill://figma/{figma-use,figma-shaders,figma-design-to-code,figma-generative-plugins}/SKILL.md`。弹窗只渲染 system prompt，于是这些路径除了翻原始请求体之外无处可读。

**决策：** 独立 skill 根改用 `.follow_links(true)` 遍历：被链接的 skill 目录与复制一份完全等价。插件根在它那套扁平扫描里遵守同一条规则——判断子项用 `Path::is_dir()`（stat，跟随链接）而不是 `DirEntry::file_type()`（lstat），因此符号链接形式的插件 skill 目录同样能加载；「只扫一层、只认直接子项」的契约不变。弹窗侧，"Assembled current prompt" 末尾新增 `## MCP skills` 段，列出持久化请求里 **仅工具定义** 中出现的 `skill://…` 路径（对话里引用的 URI 不算请求在宣传 skill），去重并排序。被提取的 prompt 本身一字未改、仍是视图开头：`# Available skills` 依旧只来自磁盘，因为没有任何 MCP server 向它贡献过内容。

**改后行为：** 符号链接形式的 skill 条目——无论是独立根还是插件的 `skills/` 子项——都会出现在 `# Available skills` 中，并可通过 `load_skill` / `/skill-name` 加载，与复制目录一致。`/view-system-prompt` → "Assembled current prompt" 只在该请求确实引用了 MCP skill 时才追加 `## MCP skills`（本仓库的 Figma server 为 4 条路径）；没有时视图与被提取的 prompt 逐字节相同。由 `symlinked_skill_dir_is_loaded` 与 `symlinked_plugin_skill_dir_is_loaded`（两者对修复前的判断方式都会失败）、`plugin_skills_only_load_direct_skill_children`（深度不变）、`mcp_skill_paths_are_deduped_and_sorted`、`mcp_skill_paths_ignore_non_tool_text`、`assembled_view_keeps_the_prompt_and_appends_mcp_skills` 钉住。

**指针：** `crates/tact/src/skill/mod.rs`（`load_skills_from_dir_with_namespace`、`load_direct_plugin_skills`、`symlinked_skill_dir_is_loaded`、`symlinked_plugin_skill_dir_is_loaded`）；`crates/tui/src/system_prompt.rs`（`SKILL_PATH`、`extract_mcp_skill_paths`、`assemble_prompt_view` 及其 4 个测试）；`crates/tui/src/handlers/select.rs`（`SelectKind::ViewSystemPrompt`）；[Ch 2](./02_chapter_skill_zh.md) §2（发现根目录）· §6（系统提示词集成）；Ch 26 2026-09-10（skill 根收敛）。

---


## 1. 2026-09-14 — 实时耗时搬到底栏第 1 行，状态栏的步骤进度条一并删除

| Field | Value |
|-------|-------|
| **类型** | optimization |
| **相关** | `crates/agent_tui_kit/src/render/bar.rs`（删除 `render_progress_bar` 与 `PROGRESS_BAR_WIDTH`、`Status::Executing` / `Status::Planning` 分支、第 1 行 group 推入顺序）；`crates/tui/src/render/bar.rs`（测试）；[第 23 章](./23_chapter_tui_zh.md) §6.6；`docs/token_usage_schema.md` |

**症状 / 动机：** 任务运行期间，顶栏自己带着两个数字：步骤标签后面的 `[██████░░░░░] 88%` 进度条，以及行尾的实时任务耗时——`◇ 插入 ◆ Log │ ⠋ 正在执行步骤 4/10 │ 并行中 1 [██████░░░░░] 88%  ⏱ 耗时 00:12`。进度条只是把步骤数（`4/10`）用字符重画了一遍；而那只时钟放错了地方——它是这套 bar 拥有的第三只墙钟，而它的两个同类（进程运行 `⊙ 运行`、冻结合耗时 `⏱ 02:05 均 01:45`）都在底栏、却各占一行。

**决策：** 进度条直接删除；实时时钟并入底栏**第 1 行**、紧跟运行之后：第 1 行承载描述*本次运行*的时钟（进程运行、任务耗时）以及权限模式、cwd、分支；第 2 行继续承载 token/ctx 读数与冻结合耗时。`Status::Planning` 与 `Status::Executing` 同时去掉行尾时钟，于是状态栏不再渲染任何自有数字——它回答*正在发生什么*（阶段、步骤数、并行工具数），底栏回答*已经过去多久*。该段保留标签（`⏱ 耗时 00:12` / `⏱ Elapsed 00:12`）：旁边的运行是裸的 `⊙ 运行 00:03`，一个无标签的 `⏱ 00:12` 会被读成第二个运行时间。

**之后的行为：** `Executing` 渲染 `◇ 插入 ◆ Log │ ⠋ 正在执行步骤 4/10 │ 并行中 1`；第 1 行在任务运行时渲染 `… │ ⊙ 运行 00:03 │ ⏱ 耗时 00:12 │ ⎇ main`，无任务在跑时该段直接**不渲染**（不是渲染成空）——`task_start_time` 为 `None`，`format_task_elapsed` 返回 `""`。它作为第 1 行最后一个可丢弃段推入，因此丢弃顺序为 `实时耗时 > 运行 > cwd`：先丢临时的任务时钟，再丢会话运行，最后才是路径（权限模式、分支与账户永不丢弃）。第 2 行恢复原样：所有段填充时 85–86 列；第 1 行五段在 100 列内仍全部保留，由 `bottom_bar_fits_the_task_elapsed_on_row_1_in_100_columns` 与 `bottom_bar_drops_the_task_elapsed_before_uptime_and_path` 钉住。位置由 `bottom_bar_puts_live_elapsed_next_to_uptime_on_row_1`（断言第 1 行顺序，并断言第 2 行**不带**该时钟）与 `bottom_bar_omits_live_elapsed_without_a_task` 钉住；状态栏一侧由 `status_bar_executing_shows_the_step_label_without_a_gauge` 与 `status_bar_planning_has_no_elapsed` 钉住。第 2 行的预算测试 `bottom_bar_fits_every_segment_in_100_columns` 回到 2026-09-14 之前的形式，因为第 2 行不再有时钟。*（同日稍后被取代：步骤标签去掉了分母，`Executing` 渲染 `正在执行步骤 4` / `Executing step 4`——见最新条目。）*

**指针：** `crates/agent_tui_kit/src/render/bar.rs`（`format_task_elapsed` 文档、`render_bottom_bar` 第 1 行 groups、`Status::Executing` 分支）；`crates/tui/src/render/bar.rs`（上面六个测试）；[第 23 章](./23_chapter_tui_zh.md) §6.6（顶栏、第 1 行、第 2 行、瘦身与回合耗时各段）；`docs/token_usage_schema.md` §"Session Stats Display"。

---

## 1. 2026-09-14 — 卡片的行数前缀随它引导的那句标签一起本地化

| Field | Value |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/agent_tui_kit/src/i18n.rs`（`tool_card_progress_tmpl`、`code_card_progress_tmpl`）；`crates/agent_tui_kit/src/render/cells/tool.rs`（`card_bottom_text`）；`crates/agent_tui_kit/src/render/cells/code.rs`；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5 |

**症状 / 动机：** 只预览了部分行的卡片会在底栏标签前标出省略量——` 3/10 lines |  Double-click for full code `。这个前缀是硬编码英文 `" {}/{} lines | {} "`，于是中文界面读到 ` 3/10 lines |  双击查看完整代码 `。代码卡片有同样的前缀（` +{} lines | {}`），问题相同。

**决策：** 两个前缀都变成消息模板（`tool_card_progress_tmpl`、`code_card_progress_tmpl`），由绘制它们的 cell 填充——这个计数是它引导的那句标签的装饰，不是工具读数的回显，因此属于同一份字符串集。英文模板渲染出的字节与原先的 `format!` 完全一致。

**之后的行为：** 中文显示 ` 3/10 行 |  双击查看完整代码 `；英文不变。由 `overflow_prefix_is_localized` 钉住（断言中文前缀，并断言底栏里不再残留 `lines`）。

**指针：** 测试 `overflow_prefix_is_localized`、`overflow_is_merged_into_bottom_hint`；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5。

---

## 1. 2026-09-14 — 折叠输出的点击窗口跟随**画出来的**那一行，而不是块构建时的语言

| Field | Value |
|-------|-------|
| **类型** | bugfix |
| **相关** | `crates/agent_tui_kit/src/widgets/tool_widget.rs`（`ToolRenderOutput::meta_text`、`collapsed_action_cols`、`hits_collapsed_action`、`card_title`、`card_bottom`、`card_title_text`）；`crates/agent_tui_kit/src/render/cells/tool.rs`（`from_output`、`title_line`）；`crates/tui/src/widgets/state/app/popups.rs`（`open_diff_popup_at`、`popup_from_tool_output`）；`crates/tui/src/widgets/state/app/config.rs`（`toggle_language`）；`crates/tui/src/widgets/state/app/construct.rs`；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5 |

**症状 / 动机：** `/lang`（或 Ctrl-L）切到中文后，已完成折叠命令的 meta 行确实用新语言绘制——`✓ 成功 · 1us · 3 行 · 双击查看结果`——但提示只在**旧语言**会放它的位置可点。在 100 列的画面上实测：字形落在 x=31..43，而真实可点的列是 x=37..55，也就是这个块构建时那行英文（`✓ Success · 1us · 3 lines · double-click-result`）的尾部。于是看得见的提示前几个字点了没反应，反而提示右侧的空白能弹出弹窗。同一份冻结还让卡片外壳停在构建时的语言：中文界面里的失败卡片在中文 meta 行上方画出 `┌ Error ─` / `└ Double-click for full error ─┘`。

这**不是** CJK 宽度问题：两侧本来就都用 `UnicodeWidthStr::width` 测量，只要构建与绘制共用同一份 `Messages`，数字完全一致（中文动作词画在 34..46，测得 34..46）。偏差只来自两侧读了不同的语言。

**决策：** `ToolRenderOutput` 不再存任何语言相关文本——它是一份「事实」规格（阶段、计数、耗时、kind）。meta 行改为在**绘制时**按当前语言现推（`meta_text(&msgs)`），点击目标随之现算（`collapsed_action_cols(&msgs)`、`hits_collapsed_action(row, col, &msgs)`），于是它描述的永远只可能是屏幕上那一行。卡片外壳同理现推（`card_title(&msgs)`、`card_bottom(&msgs)`，它们依赖的事实由新的 `live_detail` 字段承载），标题行改由 cell 自己上色（删掉 `title_line`），顺带终结了标题行上的**主题**冻结。根因是 `Messages` 曾在构造时被快照进组件——`App::new` 写死 `Language::English`，而 `/lang` 只翻转渲染路径读取的 `App::language`——因此 `App::toggle_language` 现在同时把新的 `Messages` 推给持有它的组件（tool / thinking / stream），让语言变更只有这一个入口。

**之后的行为：** 提示恰好在自己被画出来的位置可点，两种语言都如此，**切换之前就已存在的块**也一样——切换是重绘已有行，不是重建它们。中文界面下，切换之前建的卡片也显示中文外壳。工具卡片在切主题后颜色随主题走（含标题行）。builder 也不再接收主题与语言（`ToolWidget::new()`、`ToolWidget::from_step_result(&result)`）：规格里已经没有语言相关内容可让它们决定，而一个拿不到语言的构造函数也就无法冻结语言。把标题行的上色搬进 cell 是唯一会碰到渲染的改动，已逐格比对过上一版：100×30 帧（折叠命令、截断的失败卡片、实时卡片、代码卡片）的 fg/bg/modifier 完全一致，只有运行中卡片的走动耗时不同；新行为由 `theme_change_repaints_existing_tool_title_rows` 钉住。两条新增回归测试均已对旧行为验证过、在旧实现上失败：`collapsed_hint_click_window_matches_the_drawn_glyphs`（渲染整帧、从 buffer 量出动作词的字形列，断言这些列——且仅这些列——能打开弹窗，覆盖两种语言以及「切换前建好的块」）与 `language_toggle_repaints_tool_card_chrome`。

**指针：** 测试 `collapsed_hint_click_window_matches_the_drawn_glyphs`、`language_toggle_repaints_tool_card_chrome`、`widget_meta_text_matches_the_rendered_meta_row`、`completed_command_renders_header_rows_only`、`double_click_collapsed_command_hint_opens_diff_popup`、`finished_block_meta_row_matches_its_hit_range`；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5「Collapsed output」。

---

## 1. 2026-09-14 — 折叠输出提示改为点名它打开的结果

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/i18n.rs`（`tool_collapsed_output_action`、`tool_collapsed_output_hint`、`tool_collapsed_output_hint_one`）；`crates/agent_tui_kit/src/widgets/tool_widget.rs`（`collapsed_output_hint`、`collapsed_action_cols`）；`crates/tui/src/widgets/state/app/popups.rs`（`open_diff_popup_at`）；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5 |

**症状 / 动机：** 无卡片 block 的 meta 行只说出了手势，没说出手势的效果：`… · 4 lines · double-click` 与 `… · 4 行 · 双击查看`。同样的字也出现在**仍然画卡片**的弹窗卡片底栏上，于是这一行里唯一可点的字符串始终没有说明：它打开的是**这次工具的结果**。

**决策：** 动作词点名它打开的东西——`double-click-result` / `双击查看结果`——两个提示模板随之跟进，因为 `collapsed_action_cols` 正是从行尾往回、把提示末尾的动作词当作点击目标来测量的，而 `collapsed_output_hint_ends_with_its_action` 对每种语言都钉住了这个尾部。只动折叠输出这一条提示：卡片底栏那些串（`Double-click for full code`、`双击查看完整代码` 等）不动，因为它们坐在已经画出内容的卡片上。

**之后的行为：** 无卡片的已完成 block 显示为 `✓ Success · 21ms · 4 lines · double-click-result` / `✓ 成功 · 21ms · 4 行 · 双击查看结果`。可点范围恰好是这几个字形、其余都不响应——行数与同一行前面的文字照旧无响应，与上面 2026-09-13 那条一致。

**Pointers:** 测试 `collapsed_output_hint_ends_with_its_action`、`collapsed_command_meta_row_reports_hidden_output`、`double_click_collapsed_command_hint_opens_diff_popup`、`collapsed_command_ignores_clicks_off_the_hint`、`edit_file_collapses_its_detail_card`、`read_file_collapses_its_detail_card`、`write_file_collapses_its_detail_card`、`multiline_result_of_a_cardless_kind_becomes_expandable`；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5「Collapsed output」。

---

---


## 1. 2026-09-13 — 已完成工具的输出：卡片收起为两行，无卡片的结果变得可打开

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/widgets/tool_widget.rs`（`ToolLayout::detail_collapsed`、`ToolWidget::collapses_detail`）；`crates/agent_tui_kit/src/render/cells/tool.rs`（meta 行提示）；`crates/agent_tui_kit/src/i18n.rs`（`tool_collapsed_output_hint`、`tool_collapsed_output_action`）；`crates/tui/src/widgets/state/app/popups.rs`（`popup_from_tool_output`、`open_diff_popup_at`）；`crates/tact/src/tool/read_file.rs`（`DetailPolicy::Result`）；`crates/tact/src/tool/write_file.rs`（`DetailPolicy::InputField("content")`）；`crates/tact/src/tool/edit_file.rs`（`DetailPolicy::InputField("new_text")`）；`crates/tact/src/agent/tool_dispatch.rs`（MCP / 插件工具都以 `Generic` 到达）；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5/§8 |

**症状 / 动机：** 每个跑完的 `bash` 都会留一张内联卡片：两行 header 加上下边框再加 1 行预览。一次会话里几十条短命令下来，日志大部分是卡片边框，而留下的那一行是输出的**尾部**（`1/27 lines | Double-click for full code`）——通常不是想看的那行，而且想读到任何内容都得先打开卡片。不管输出多少，预览开销都是固定的：一行输出的命令和 27 行输出的命令付一样多的卡片行。跑完的 `read_file` / `write_file` / `edit_file` 也在为「本来就要打开弹窗去读」的内容付同一笔开销——卡片里只留了整份文件的一行。

**决策：** 折叠类工具完成后不再画卡片。规则按 visual kind 判定，绝不按工具名，条件是 `Success` + 非 live 卡片。`Command | FileRead | FileWrite | FileEdit` 一律折叠：它们本来在画卡片，折叠**省下**行数，与正文多长无关。`Subagent` 永不折叠——它是 transcript 弹窗的入口，而且它保留的那一行就是子代理的结果摘要。其余 kind（`Task` / `Sleep` / `Generic`）从来就没有卡片，所以它们的结果不是被折叠而是**根本读不到**：`detail_full` 一直是 `None`，连带 popup 和点击目标也一起没有。折叠这些不增加任何行成本，于是多行结果（`task_list`、`read_inbox`、`worktree_status`、`load_skill`、`check_background`，以及所有以 `Generic` 到达的 MCP / 插件工具）变成双击一次即可读到；只有一行的结果仍然不给入口，`ask_user` 也不给——它的答案本来就在 meta 行上（`compact_result_to_meta`），一个打开后没有可读内容的入口只是噪音。对外表现为新的 `ToolLayout.detail_collapsed` 标志，而不是按工具名开关（同一个渲染器里那种 per-tool flag 曾试过又被删掉）。两处连带影响被显式处理：`build()` 对折叠卡片照旧填 `detail_full` / `detail_total_lines`，所以弹窗路径不需要新的内容管道；`popup_from_tool_output` 原来遇到 `!has_detail_card` 直接返回 `None`，现在也接受折叠卡片。由于没有卡片可点，点击目标收窄为**说出这个手势的那几个字**：`open_diff_popup_at` 接收点击列并与 `ToolRenderOutput.collapsed_action_cols` 比较，即 meta 行末尾 `double-click-result` 动作词所占的列（`hits_collapsed_action`）。最初那版把整行 header 文字都当目标，两个方向都太宽：上面的参数行、以及 meta 行自身靠前的文字（成功标记、耗时、行数）都会弹出弹窗，而它们没有任何「可点」的暗示。要测量它就得知道该行的确切文本，于是 widget 把已完成块的 meta 行存进 `ToolRenderOutput.meta_text`（运行中为 `None`，因为 cell 会重新推导走动的耗时）；widget 与 cell 通过同一组 `build_meta_text` + `meta_suffixes` 组装，并有测试钉住二者相等。动作词范围是从行尾往回量的，成立的依据是提示本身位于行尾、且各语言的提示串都以自己的动作词结尾。仍然画卡片的工具，其 header 行照旧无响应——2026-07-15 的 `tool_card_double_click_detail_area_only` 规则（[Ch 4](./04_chapter_prompt_zh.md)）对所有**存在卡片**的情形仍然成立，只是对无卡片的情形做了补充。

**之后的行为：** 运行中不变（`Live output` 卡片，1→3 行 tail）。成功时只打印两行，并在 meta 行追加 `… · {n} 行 · 双击查看结果`（`tool_collapsed_output_hint`，另有单数形 `tool_collapsed_output_hint_one`，对应后台任务收尾路径可能出现的单行文本）——没有这条提示，无卡片的 block 就等于悄悄藏起了输出。`n` 取 `detail_total_lines`，与弹窗报告的行数完全一致（含 `$ <command>` 前缀行），因此提示与弹窗永远对得上。失败仍保留五行预览的 `Error` 卡片，`background_run` 在 `BackgroundTaskFinished` 收尾时折叠。每条完成的命令从 5 行降到 2 行；剩下的可点区域只有 `双击查看结果` 这几个字——参数行与其前面的 meta 文字都不响应。文件读取（`read_file`，以及共用 `FileRead` kind 的 `read_image`）、文件写入（`write_file`，`FileWrite`）与文件编辑（`edit_file`、`apply_patch`，`FileEdit`）适用同一条规则：正文卡片 / diff 卡片同样消失，内容仍可从弹窗读到（读取读文件正文，写入优先从磁盘读回该文件、失败时回落到工具返回的内容，编辑读 git diff），meta 提示带上它的行数。已完成 subagent 保留摘要卡片。从来没有卡片的 kind 是镜像情形：多行结果现在折叠，行数成本与从前一样是两行但变得可打开，单行结果则不动。划分始终按 kind 而非按工具名，MCP / 插件那条路径因为走 `Generic` 而自动受益。（动作词本身在 2026-09-14 由 `double-click` / `双击查看` 改名为 `double-click-result` / `双击查看结果`——见上面最新那条。）

**已被取代（2026-09-14）：** 已完成块的 meta 行不再**存**进 `ToolRenderOutput`，而是按绘制时所用语言现推，这样 `/lang` 切换不会把点击窗口落在后面（见上面那条）。卡片标题与底栏同理。

**Pointers:** `crates/agent_tui_kit/src/widgets/tool_widget.rs`（`collapses_detail`、`layout`、`build`、`collapsed_action_cols`、`hits_collapsed_action`、`meta_suffixes`、`collapsed_output_hint`）；`crates/agent_tui_kit/src/i18n.rs`（`tool_collapsed_output_action`）；`crates/agent_tui_kit/src/render/cells/tool.rs`；`crates/tui/src/widgets/state/app/popups.rs`（`open_diff_popup_at`）；`crates/tui/src/handlers/mouse.rs`（点击列）；测试 `completed_command_renders_header_rows_only`、`double_click_collapsed_command_hint_opens_diff_popup`、`collapsed_command_ignores_clicks_off_the_hint`、`collapsed_output_hint_ends_with_its_action`、`double_click_collapsed_edit_hint_opens_diff_popup`、`double_click_collapsed_read_hint_opens_diff_popup`、`double_click_cardless_tool_hint_opens_result_popup`、`edit_file_collapses_its_detail_card`、`read_file_collapses_its_detail_card`、`write_file_collapses_its_detail_card`、`read_file_has_plain_gutter`、`read_image_collapses_with_the_file_read_kind`、`multiline_result_of_a_cardless_kind_becomes_expandable`、`multiline_mcp_result_becomes_expandable`、`one_line_result_of_a_cardless_kind_stays_plain`、`result_already_on_the_meta_row_is_not_collapsed`、`full_frame_edit_file_tool_shows_in_log`、`full_frame_read_file_tool_shows_in_log`、`full_frame_write_file_tool_shows_in_log`、`full_frame_cardless_tool_result_is_openable`、`double_click_subagent_header_does_not_open_diff_popup`、`failed_command_keeps_its_error_card`、`collapse_spares_running_commands_and_subagents`、`collapsed_command_meta_row_reports_hidden_output`、`widget_meta_text_matches_the_rendered_meta_row`、`finished_block_meta_row_matches_its_hit_range`；[Ch 23](./23_chapter_tui_zh.md) §6.16；`docs/tool_rendering.md` §5「Collapsed output」。

---

---


## 1. 2026-09-13 — `[agent]` 拒绝未知键：写错位置的 thinking 设置会报错，而不是凭空消失

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/types.rs`（`AgentTomlConfig`、`SubagentTomlConfig`）；`crates/tact/src/config/resolve.rs`（`resolve_config`）；`config.example.toml`；[Ch 21](./21_chapter_config_zh.md) §4 |

**症状 / 动机：** `AgentTomlConfig` 是 `#[serde(default)]` 且没有 `deny_unknown_fields`，因此任何非 agent 字段的键都会被丢弃——不报错、不警告、无效果。危险的不是拼写错误，而是那些**在别处确实存在、写在这里看起来合理**的键：`thinking_budget` 与 `reasoning_effort` 是**运行时** agent 设置（`AgentSettings`）的字段（subagent 段亦然），但作为 TOML 键它们属于 `[llm]`（全局）或 `[llm.providers.<name>]` 条目；`model` 属于 provider 条目。修复前实测：`[agent] max_tokens = "abc"`（已知键、类型错）解析即失败，而 `[agent] thinking_budget = "abc"` 加 `[agent] reasoning_effort = 123` 却正常启动、两个值全丢——与同日移除的 `[llm].max_tokens` 属同一"配了却被忽略"类别。

**决策：** 给 `AgentTomlConfig` 与 `SubagentTomlConfig` 加上 `#[serde(deny_unknown_fields)]`。这两个键仍然不在 schema 里，因此 serde 的 `unknown field` 报错只会列出真正的 agent 字段——这正是把读者引向 `[llm]` 的线索。曾尝试为每个错位键单独加一个守卫字段，后来**撤掉**：把 `thinking_budget` 留在结构体里会让 serde 在它自己的 "expected one of …" 列表里把它列为合法字段，于是再补上该键的配置会先被告知"这个键有效"、然后在下一层被拒。把无效键说成有效的报错，比通用报错更糟。其他顶层段保持原样——`[llm]` 仍需保留其 `max_tokens` 字段以承载"已移除"守卫。

**之后的行为：** `[agent]` 或 `[agent.subagent]` 下任何非真实字段的键在解析阶段即失败，报 `unknown field \`thinking_budget\`, expected one of \`max_tokens\`, \`model_context_window\`, …`。只使用合法键的配置不受影响（随包发布的 `config.example.toml`、进程自身的 `persist` 写入方、以及测试套件中的全部夹具均照常解析）。

**Pointers:** `AgentTomlConfig` / `SubagentTomlConfig` in `crates/tact/src/config/types.rs`; tests `agent_thinking_keys_are_rejected`, `agent_unknown_key_is_rejected`, `subagent_unknown_key_is_rejected` in `crates/tact/src/config/resolve.rs`; [Ch 21](./21_chapter_config_zh.md) §4「未知键会被拒绝」; `config.example.toml`.

---

---


## 1. 2026-09-13 — 移除 `[llm].max_tokens`，残留该键将直接报错

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | `crates/tact/src/config/types.rs`（`LlmTomlConfig`、`AgentTomlConfig`）；`crates/tact/src/config/resolve.rs`（`resolve_config`）；`config.example.toml`；[Ch 21](./21_chapter_config_zh.md) §3/§4 |

**症状 / 动机：** 输出预算解析链有五个层级（`--max-tokens` > provider 条目 > `[agent].max_tokens` > `[llm].max_tokens` > 内置默认），而 `[llm]` 全局是唯一一个**永远不会被观察到**的层级：它排在 `[agent]` **之下**，只对"没在 `[agent]` 里设过"的用户生效。两级都设的人，其中一个值被静默忽略——正是当初新增 `[agent]` 一级要消灭的"配了却被忽略"类别，只是下沉了一层。一个对最可能设置它的用户都不生效的全局，比没有全局更糟：它诱使人写下一个看起来生效的键。

**决策：** 去掉这一级。解析链变为 `--max-tokens` > `[llm.providers.<active>].max_tokens` > `[agent].max_tokens` > 默认（8000；Kimi K2.x 为 32000），`[llm]` 只保留 `provider`、`thinking_budget`、`providers`、`model_profiles`。该键仍留在 `LlmTomlConfig` 中，**仅作为守卫**：`resolve_config` 发现它存在即报错，并指明替代键 `[agent].max_tokens`。没有选择静默忽略，因为那样请求会悄悄回落到内置默认值、输出里没有任何提示——正是本次移除要终结的失败模式。这与同日 `[agent.subagent]` 缺 `provider` 的处理先例一致。

**之后的行为：** 设置 `[llm] max_tokens` 的配置启动失败，报 `[llm].max_tokens was removed. Set [agent].max_tokens instead (or [llm.providers.<name>].max_tokens for a per-provider value), or delete the key.`，随后给出解析顺序。从未设置该键的配置不受影响；`thinking_budget` 保留其 `[llm]` 全局（它没有 `[agent]` 对应键，因此全局是唯一的非 provider 开关）。`responses_compact_threshold` 的校验文案不再写死 `llm.max_tokens`，改为 `max_tokens`——该值可能来自 CLI、provider 条目或 `[agent]`。

**Pointers:** `resolve_config` in `crates/tact/src/config/resolve.rs`; tests `llm_max_tokens_is_rejected`, `absent_llm_max_tokens_still_resolves`, `agent_max_tokens_overrides_default`, `per_provider_max_tokens_overrides_default`, `cli_max_tokens_overrides_entry`, `parse_removed_llm_max_tokens_is_captured`; [Ch 21](./21_chapter_config_zh.md) §3 优先级表 + §4 schema; `config.example.toml`.

---

---


## 1. 2026-09-13 — 底栏 `out` 显示请求参数本身，而不是推算出来的 reasoning 份额

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/agent_tui_kit/src/render/bar.rs`（`format_max_out_tokens`）；`crates/tact_llm/src/openai/responses/convert.rs`、`crates/tact_llm/src/convert.rs`、`crates/tact_llm/src/anthropic/mod.rs`；[Ch 23](./23_chapter_tui_zh.md) §6.6；`docs/token_usage_schema.md` |

**症状 / 动机：** 底栏 `out` 段对 effort 语义模型（openai / deepseek / kimi k3）渲染 `max_tokens × 100/(100+pct)`，按 effort 分档表扣掉 reasoning 份额（`high` → 75%）。这让读数与线上请求不一致：请求发出的是**完整**的 `max_output_tokens` / `max_tokens`，信封内推理与正文的切分由服务端按次请求决定。固定的 75% 只是猜测——于是在 `[agent] max_tokens = 65536` + `high` 的配置下，底栏显示 `37.4K`，而接口实际收到的是 65536。该扣减约定原本借自压缩预留：那里必须在调用前**先定下一个尺寸**，与"把已知数字读出来"是两回事。

**决策：** `format_max_out_tokens` 现在只接收 `(label, max_tokens)` 并原样渲染；`thinking_budget` / `reasoning_effort` 不再是入参。原先固定扣减行为的三个测试（`..._subtracts_effort_share`、`..._budget_keeps_full_envelope`、`..._zero_budget_subtracts_effort_share`）合并为 `format_max_out_tokens_is_the_wire_value`。这也顺带退掉了本区域上一次的修复——那个用来决定"是否扣减"的 `None` vs `Some(0)`「thinking 关闭」判据再也无法影响该段，因为该段已完全不依赖 thinking 设置。reasoning 预留的估算仍留在必须选尺寸的地方：压缩摘要预算与 `should_auto_compact` 的 incoming-turn 预留。

**之后的行为：** 在 `[agent] max_tokens = 65536` 配置下，无论 effort 为何，底栏都显示 `out 65.5K`——即真正发出去的数字；按构造与 `ModelInfo.max_tokens` 及请求体（`max_output_tokens` / `max_tokens`）一致。该段在 `/model` 切换 effort 时、跨会话边界时都保持稳定。

**Pointers:** `format_max_out_tokens` in `crates/agent_tui_kit/src/render/bar.rs`; test `format_max_out_tokens_is_the_wire_value`; [Ch 23](./23_chapter_tui_zh.md) §6.6; `docs/token_usage_schema.md`.

---

---


## 1. 2026-09-13 — 显式配置压过内置模型→窗口映射，subagent 段不再被静默丢弃

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/resolve.rs`（`resolve_config`、`resolve_subagent`）；`config.example.toml`；[Ch 21](./21_chapter_config_zh.md) §3 |

**症状 / 动机：** 同一类 bug 的两个实例——配置**看起来**生效，却从未到达请求。

1. `model_context_window` 原解析顺序为 映射 > CLI > 文件，即内置模型→窗口表**同时压过** CLI 标志与 `[agent].model_context_window`。用户为存在内置映射的模型刻意设置的窗口会被忽略。当时的官方理由是安全（过时的手工窗口不会低估已知模型），但实际效果违背了项目"配置优先"的规则，且同一文件里 `max_tokens`（生效）与 `model_context_window`（被忽略）可以并排存在。
2. `resolve_subagent` 一旦缺少 `provider` 就返回 `Ok(None)`，于是 `provider` 被注释掉的 `[agent.subagent] max_tokens = 64000` 静默失效。这里 `tracing::warn!` 也无济于事：日志仅在设置 `RUST_LOG`/tokio-console 时安装，而配置解析更早发生（`crates/tact-ui/src/main.rs` 的 `init()`），警告根本不会打印。

**决策：** (1) `model_context_window` 改为 CLI > `[agent]` > 映射 > 默认 `200_000`：内置表是**未配置模型时的回退**，而非覆盖。安全代价改为文档说明而非强制——过时的手工窗口会低估长上下文模型并触发过早自动压缩，因此应删除该键而不是留着过期值。显式 `0` 仍表示"禁用/未知窗口"，**不会**回退到映射。(2) `[agent.subagent]` 若设置了 `model` / `max_tokens` / `thinking_budget` / `reasoning_effort` 却没有 `provider`，现在会在 resolve 阶段**硬报错**并给出修法——因为 `provider` 文档上即为必填，另一条路就是静默忽略这些覆盖项。完全没有任何覆盖项的段（键全被注释掉的遗留表头）仍是静默 no-op，因此该守卫不会误伤无害模板。

**之后的行为：** 为 `deepseek-v4-pro`（内置 1M）设置 `[agent] model_context_window = 128000` 现在得到 128,000，且 `--model-context-window` 优先级高于两者。带有 subagent 覆盖项却缺 `provider` 的配置会启动失败并报 `[agent.subagent] sets overrides but \`provider\` is missing, so the whole section is ignored. … Set \`provider\` to a key from [llm.providers.*], or remove the section.`——这类错误此前需要读 resolve 源码才能发现。

**指针：** `crates/tact/src/config/resolve.rs` 的 `resolve_config` + `resolve_subagent`；测试 `resolve_model_context_window_toml_overrides_mapping`、`resolve_model_context_window_cli_overrides_toml_and_mapping`、`resolve_model_context_window_mapping_is_the_fallback`、`subagent_overrides_without_provider_errors`、`subagent_empty_section_without_provider_is_ignored`；[Ch 21](./21_chapter_config_zh.md) §3。

---

---


## 1. 2026-09-13 — 底栏 `out` 额度不再在会话首个 prompt 后跳变

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/agent_tui_kit/src/render/bar.rs`（`format_max_out_tokens`）；`crates/tact/src/agent/mod.rs`（`emit_model_status`、回合内 `ModelInfo`）；[Ch 23](./23_chapter_tui_zh.md) §6.6；`docs/token_usage_schema.md` |

**症状 / 动机：** 配置未变的情况下，底栏 `out` 段在启动时显示一个值、发出首个 prompt 后变成另一个更大的值。`out` 是**有效**文本输出额度：effort 语义模型（openai / deepseek / kimi k3）的 reasoning 与文本共享 `max_tokens` 信封，因此扣除 reasoning 份额（`max_tokens × 100/(100+pct)`）；budget 语义模型（Anthropic 式 `thinking_budget`）的 thinking 走独立信封，显示完整值。原判定条件是 `thinking_budget.is_some()`，但两个发送方对"thinking 关闭"的编码不同：`/model` 路径发 `None`（`(budget > 0).then_some(..)`），而回合内请求路径发 `Some(0)`——它映射的是 `with_thinking` 里那个恒存在的 `Thinking` 结构。于是会话首个 prompt 就把渲染端从"共享信封"翻成"独立信封"，`out` 从扣减后的值跳到完整 `max_tokens`，例如 64000 信封 + `high` 下 `36.6K` → `64K`。

**决策：** 判定条件改为**非零**预算——`thinking_budget.is_some_and(|b| b > 0)`。`Some(0)` 与 `None` 都表示"thinking 关闭"，即共享信封语义，两者都扣减、渲染结果一致。在渲染端修（而不是把回合内发送方对齐 `emit_model_status`）还能覆盖压缩摘要路径——那处发 `thinking_budget: None` + 小 `max_tokens`，否则仍可能在会话中途翻转该段。`think` 段本就带 `> 0` 过滤，这就是只有 `out` 会跳的原因。

**之后的行为：** 配置不变时 `out` 跨会话边界保持稳定：64000 信封 + `high` effort 在首个 prompt 前后都显示 `36.6K`，不再出现 `36.6K` → `64K`。真正的 budget 语义模型（`thinking_budget > 0`）仍显示完整 `max_tokens`。

**指针：** `crates/agent_tui_kit/src/render/bar.rs` 的 `format_max_out_tokens`；测试 `format_max_out_tokens_zero_budget_subtracts_effort_share`；[Ch 23](./23_chapter_tui_zh.md) §6.6；`docs/token_usage_schema.md`。

---

---


## 1. 2026-09-13 — `[agent].max_tokens` 成为输出预算链上的真实一级

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/types.rs`（`AgentTomlConfig::max_tokens`）；`crates/tact/src/config/resolve.rs`（`resolve_config`）；`config.example.toml`；[Ch 21](./21_chapter_config_zh.md) §3 |

**症状 / 动机：** `[agent] max_tokens = 64000` 完全没有效果。`AgentTomlConfig` 没有这个字段，且该结构是 `#[serde(default)]` **没有** `deny_unknown_fields`，于是 serde 静默丢弃该键——不报错、不警告。原解析链是 `--max-tokens` > `[llm.providers.<active>].max_tokens` > `[llm].max_tokens` > 默认 8000，因此只配 `[agent] max_tokens` 的配置实际跑在 8000，**看起来却是配好的**。实测：配置文件带 `[agent] max_tokens = 64000`（mtime 14:32），14:34 发出的请求仍是 `"max_tokens": 8000`，且项目 DB 的 `token_usages.request_body` 里从未出现过 64000。同一文件里的 `[agent.subagent] max_tokens = 64000` 还因第二个原因失效——该段没有 `provider` 时 `resolve_subagent` 直接返回 `Ok(None)`。

**决策：** `[agent].max_tokens` 成为受支持的键，占据**provider 条目与 `[llm]` 全局之间**的位置：`--max-tokens` > `[llm.providers.<active>].max_tokens` > `[agent].max_tokens` > `[llm].max_tokens` > 默认值（8000；Kimi K2.x 为 32000）。它排在 `[llm]` 全局**之上**，这样同时设了全局的用户也不会让它失效——反过来排会重现本次要修的"配了但被忽略"陷阱。`[llm]` 全局保留，既有配置继续可解析。`model_context_window` 校验的错误文案不再写死 `llm.max_tokens`，因为该值现在有三个来源。

**之后的行为：** 只配 `[agent] max_tokens = 64000` 时，凡 provider 条目未设 `max_tokens` 的 provider 都跑在 64000；provider 条目仍然优先，`--max-tokens` 优先级最高。未设 `[agent.subagent].max_tokens` 的子 agent 继承已解析的主值（含 `[agent]` 这一级）。窗口校验报错改为 `invalid token limits: max_tokens (N) must be less than agent.model_context_window (M)`。

**指针：** `crates/tact/src/config/resolve.rs` 的 `resolve_config`；测试 `agent_max_tokens_overrides_global`、`per_provider_max_tokens_overrides_agent`、`cli_max_tokens_overrides_agent`、`subagent_inherits_agent_max_tokens`；[Ch 21](./21_chapter_config_zh.md) §3 优先级表 + §4 schema。

---

---


## 1. 2026-09-13 — 压缩摘要改用 effort 桶 + 分档阶梯，取代固定预留

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 相关 | `crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode`、`compact_effort_reserve_tokens`、`compact_summary_server_default_effort`、`compact_summary_effort`、`next_compaction_reserve`）；`crates/tact/src/recovery.rs`（`MAX_COMPACT_SUMMARY_ATTEMPTS`、`MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS`） |

**现象 / 动机：** 摘要器的 reasoning 预留是摘要**文本**预算（封顶 2,000 token）的百分比，于是 `high` effort 只预留 1,500 token，而 effort 档位本身表达的是一个绝对思考额度。截断恢复也只是固定 3 次续写，在推理密集的 provider 上无法收敛：DeepSeek 不回放历史 `reasoning_content`，每次调用都会从头重新思考，于是每次续写都重复同样的溢出，直到接受部分摘要——或者因空文本而报错。Codex 自身的摘要器是单发、用与采样相同的 effort、并容忍被截断的摘要，这决定了下面的改法。

**决策：** (1) 初始预留改为**有效 effort 的绝对 token 桶**——`none` 0 / `minimal|low` 2,000 / `medium` 4,000 / `high` 8,000 / `xhigh|max` 16,000——不再按文本预算的百分比；未配置 effort 时，DeepSeek / Kimi K3（服务端默认 effort high）取 `high` 桶，其余 provider 取 0。(2) 单一续写循环改为**分档阶梯**：阶段 0 继承会话 effort；阶段 1 降档（DeepSeek / Kimi K3 发 `low`，OpenAI 推理模型发 `none`，其余省略）；阶段 2+ 依据上一次的 `usage.reasoning_tokens` 设定预留 `clamp(observed × 1.25, floor, cap)`，其中 `floor = max(上次预留, effort 桶, 文本/4)`、`cap = 2 × floor`，并受窗口上限约束。(3) 新增 `MAX_COMPACT_SUMMARY_ATTEMPTS`（5），独立于主循环的 `MAX_CONTINUATION_ATTEMPTS`（3）；传输重试 `MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS` 从 3 提到 5。空摘要文本仍会失败。

**改后行为：** `max_tokens` = 2,000 文本 + effort 桶——`high` 为 10,000，DeepSeek / Kimi K3 未显式配置 effort 时同为 10,000（服务端默认 high）。截断的摘要会发出 `[compact continue n/5]` 并携带递增的预算；阶梯耗尽后发出 `[compact fallback]`，接受部分摘要而不是失败——压缩不再因为推理模型吃掉了上一次信封就报错。

**设计说明（与 Codex 对比）：** 摘要调用是**合成**一次无 tools 的 `create_message`——指令 + 可选 focus + 最近文件清单 + 序列化成 JSON 文本的近期消息切片——而不是回放真实历史。Codex 则回放：把 `SUMMARIZATION_PROMPT` 追加到原生 items 上发送，输入超限时通过 `ContextWindowExceeded` 删除最旧项重试。合成带来的好处是请求必然放得进输入预算、且结构永远合法（不会出现孤立的 `tool_use`/`tool_result`，wire 上也不带 tools），并且 `focus`、最近文件、超大媒体降级都在这里注入；代价是工具往来的语义只以 JSON 形式保留。重组环节两者都是「近期真实用户消息 + 一条摘要单元」、上限 20k 估算 token；但 Tact 会在保留用户消息前剥掉 `ToolResult` 块（Tact 的工具结果挂在用户消息里，而 Codex 是独立的 `FunctionCallOutput` item），并有一个外层 fit-loop，按 `system prompt + tool specs + rebuilt + max_tokens + headroom` 收缩保留预算直到放得下；Codex 保持固定 20k，交由调用方处理。另外 Tact 在摘要为空时报错，Codex 则接受仅有 `SUMMARY_PREFIX` 的单元。同一改动还收紧了提示词：明确对话以 JSON 消息数组附在后面、摘要须仅基于该内容，并要求输出紧凑、结构化、不转述标识符的手交。

**指针：** `crates/tact/src/agent/mod.rs` 中的 `compact_history_local_with_mode` 与各辅助函数；`crates/tact/src/recovery.rs` 中的常量；测试 `compact_effort_reserve_bucket_tiers`、`compact_summary_server_default_effort_tiers`、`compact_summary_effort_ladder_per_provider`、`next_compaction_reserve_*`、`local_compact_inherits_session_effort`、`local_compact_keeps_server_default_reasoning_reserve`、`local_compact_accepts_partial_summary_when_continuations_exhausted`；[Ch 5](./05_chapter_compact_zh.md) §5 步骤 3。

---

## 1. 2026-09-12 — `/model` 流程合二为一，TUI 不再在 UI 线程上 fork git

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tui/src/widgets/state/mod.rs`（`ModelTarget`）；`crates/tui/src/handlers/select.rs`；`crates/tui/src/widgets/state/app/background.rs`（新增） |

**症状 / 动机：** 两个彼此独立的问题。

(1) `SelectKind` 把 `/model` 流程承载了两遍：五个 `Subagent*` 变体镜像主 agent 的变体（`SubagentModelProfileEffortPick` 对 `ModelProfileEffortPick` 等等），并由 `handlers/select.rs` 里三个大型并行 match 重复处理。其中一个 match 以一个 12 变体的 OR-pattern 收尾，而该 match 同时还有通配分支——于是新增变体时若忘记处理，会静默落入通配分支而不是编译失败。

(2) 两个阻塞操作跑在事件循环上：`App::maybe_refresh_git_branch` 会 fork `git branch --show-current`（虽被节流到 5 秒，但仍在 UI 线程上启动进程）；`refresh_skills` 取 `std::sync::Mutex` 后持锁遍历文件系统。两者都没有被持有句柄，因此关闭时无法中止。

**决策：**(1) 用 `ModelTarget { Main, Subagent }` 参数化 model/effort/budget 流程，五个重复变体通过 `target` 字段折叠进共享变体——`SelectKind` 从 13 个减到 8 个变体，五个重复的 `*_subagent_*` 辅助函数删除，三个并行 match 合并为一个分发。剩余的 OR-pattern 现在覆盖所在 match 的全部变体且该 match 无通配分支，因此新增变体会编译失败而非静默穿透。

(2) 两个操作迁到新文件 `app/background.rs` 中用 `tokio::task::spawn_blocking` 执行，各自把 `JoinHandle` 与 `oneshot::Receiver` 存入 `App`（`git_branch_task` / `skills_task`）。`poll_background_tasks` 每轮循环应用结果，`abort_background_tasks` 在关闭时执行。git 刷新保留 5 秒节流且带 in-flight 门控（绝不每帧新建任务）。`reload_skills` 是同步且锁作用域受限的，因此不存在跨 `.await` 持锁。无 tokio runtime 时两者回退为内联执行，从而保持既有测试可用。

**改后行为：** `/model` 与 `/model-subagent` 行为完全不变——相同弹窗、顺序、文案与结果状态；两个测试锁定了重复变体族原本承载的按 target 差异（subagent 预算流程使用 subagent 持久化模板；subagent effort 选择写入 `agent.subagent.reasoning_effort`）。状态栏 git 分支与技能列表现在都在 UI 线程之外刷新。已知且有意保留的不对称（现已加注释说明）：subagent 的 **effort** 流程与主 agent 共用持久化/仅会话模板，而预算流程有专门的 subagent 文案。

**Pointers：** `crates/tui/src/widgets/state/mod.rs`；`crates/tui/src/handlers/select.rs`；`crates/tui/src/widgets/state/app/background.rs`。

---

## 1. 2026-09-12 — Responses 流事件改由 SDK 枚举分类，不再手写清单

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact_llm/src/openai/responses/mod.rs`（`sdk_event_types`、`parse_stream_event_with_raw`） |

**症状 / 动机：** `parse_stream_event_with_raw` 在反序列化*之前*，用事件 `type` 与一份 23 条 `"response.*"` 字面量白名单做字符串匹配，来决定是否消费该 SSE 事件。而 vendored SDK 的 `ResponseStreamEvent` 枚举实际建模了 48 种类型，因此那份清单只是手工维护的子集，还得同时和另外两处保持一致（`stream.rs` 的 match 分支、`wire.rs` 的输出项表）。凡是 SDK 后续版本才认识的类型、或当初写清单时遗漏的类型，都会在状态机看到它之前被静默丢弃——既无日志也无报错。

**决策：** 直接问 serde。对于内部标记（internally tagged）枚举，unknown-variant 错误会枚举出全部合法 tag，因此 `sdk_event_types()` 在运行时从枚举自身派生出完整且权威的集合（`LazyLock`，用假 tag 做一次探测反序列化）。它会随 SDK 升级自动跟进，无需镜像清单。白名单已删除，判断改为 `sdk_knows_event(type)`。

该派生**仅**用于区分两种情况："本构建未建模的类型"（丢弃——对更新版服务器的前向兼容）与"SDK *确实*建模的类型的畸形载荷"（仍为硬错误）。它**不**决定 Tact 关心哪些事件：那仍是 `stream.rs` 的 `ResponsesStreamState::apply`，即流语义的唯一真相来源。

归一化（`normalize_stream_event_json`）仍在反序列化之前执行，因为它在修复类型化解析器本会拒绝的线格式；现在它只按真正需要修复的两类事件触发（`output_item.{added,done}` 与终态 `completed`/`incomplete`/`failed`）。

**改后行为：** SDK 建模的每个事件都会到达状态机；`stream.rs` 照旧忽略 Tact 不使用的那些。SDK 枚举之外的事件类型仍被丢弃而非让整条流失败。已建模事件的畸形载荷仍报错。由于派生集合是从错误信息里解析出来的，`sdk_event_types_are_derived_from_the_enum` 锁定了派生仍然有效——否则 serde 措辞一变就会让集合静默变空，从而杀死所有流。

**Pointers：** `crates/tact_llm/src/openai/responses/mod.rs`；`crates/tact_llm/src/openai/responses/stream.rs`；`crates/tact_llm/src/openai/responses/wire.rs`。

---

## 1. 2026-09-12 — 配置只剩一个编排器，权限设置只剩一处加载

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/resolve.rs`（`resolve_non_llm`、`NonLlmSettings`）；`crates/tact/src/permission/settings.rs`（`PermissionSettings::load`） |

**症状 / 动机：** 两处重复，其中一处还会崩溃。(1) `resolve_non_llm_settings` 与 `resolve_config` 各自解析同一批约 45 行的非 LLM 设置（通知、快照、微压缩、技能目录、指令来源、主题、视觉、bash 超时/nice、RTK 过滤、权限模式），且各持一份优先级链——于是改动优先级必须改两遍，否则两条路径会静默分歧。(2) `PermissionSettings::load` 与 `load_from` 的合并块逐字节相同。

此外非 LLM 路径用 `.expect("invalid instruction_sources in config")` 解析 `[agent].instruction_sources`。那是从 `main` 可达的库代码，于是配置键写错会让进程 panic 中止，而不是指出出错的键。

**决策：** 抽出 `NonLlmSettings` + `resolve_non_llm(args, toml_cfg) -> Result<NonLlmSettings>`，两条路径共用，并在解析处一次性写明优先级顺序。`.expect` 改为 `Err`，`resolve_non_llm_settings` 随之返回 `Result`，`config/mod.rs` 中的调用方用 `?` 传播。`PermissionSettings::load` 变为一行委托给 `load_from`。

有一处差异被**有意保留**未合并：畸形 `[voice]` 在完整路径上是致命的（`resolve_voice(...)?`），在非 LLM 路径上则告警降级（该路径服务的是从不录音的子命令）。合并它会改变行为，因此 `voice` 仍在各调用点解析，并附注说明原因。

**改后行为：** 优先级不变（`CLI 参数 > TOML > 内置默认`；`--no-notifications` / `--no-micro-compact` 是绝对的，不再读取 TOML 值）。两条路径现在共用同一实现，不会漂移。错误的 `[agent].instruction_sources` 会报 `invalid [agent].instruction_sources: …` 而非 panic。权限规则合并语义不变，并由 `load_from_unions_global_then_project_deduplicating` 锁定：全局规则在前、项目规则追加、去重——不存在按层覆盖，因为优先级是在匹配时决定的（`deny > ask > allow`）。

**Pointers：** `crates/tact/src/config/resolve.rs`；`crates/tact/src/permission/settings.rs`；`crates/tact/src/config/mod.rs`。

---

## 1. 2026-09-12 — MCP 握手与工具调用都有超时上界

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/mod.rs`；`crates/tact/src/mcp/remote.rs` |

**症状 / 动机：** MCP 加载器在返回前会等待*每一个*服务器连接，而 initialize 握手、`tools/list`、`tools/call` 三者都没有截止时间。因此只要有一个第三方服务器卡住，`tact` 启动就会永久挂起——没有超时、没有报错，也看不出是哪个服务器的问题；远端服务器更糟，只有 OAuth 那两段（`OAUTH_CALLBACK_TIMEOUT`、`OAUTH_TOKEN_EXCHANGE_TIMEOUT`）有上界。

**决策：** 新增三个具名截止时间，施加在阻塞式 await 上：`MCP_INIT_TIMEOUT`（60 秒）用于 stdio 与远端两种传输的 initialize 握手，`MCP_LIST_TOOLS_TIMEOUT`（30 秒）用于 `tools/list`，`MCP_CALL_TOOL_TIMEOUT`（600 秒）用于 `tools/call`。工具调用给得宽松而非紧凑：服务器侧的长任务本来是合理工作，无界等待才不是。

**改后行为：** 握手永远完不成的服务器会以超时失败上报并从 router 中移除，而不是阻塞启动；每次调用同理。每个超时错误都会在消息里标明自己的时长。

**Pointers：** `crates/tact/src/mcp/mod.rs`（`MCP_INIT_TIMEOUT`、`MCP_LIST_TOOLS_TIMEOUT`、`MCP_CALL_TOOL_TIMEOUT`）；`crates/tact/src/mcp/remote.rs`（`REMOTE_INIT_TIMEOUT`）。

---

## 1. 2026-09-12 — 钩子子进程超时后被真正杀死

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/plugin/hooks.rs` |

**症状 / 动机：** 插件命令钩子以 60 秒超时 spawn `sh -c`。超时后 `wait_with_output` future 被 drop，这只会让子进程脱离而非杀死它：钩子被上报为超时，进程却继续运行并继续占用父进程早已不再读取的 stdout/stderr 管道。泄漏的钩子进程会随会话累积，而一个超时后仍存活的钩子还能在 Tact 判定其失败之后继续改动工作区。

**决策：** 在 spawn 的命令上设置 `.kill_on_drop(true)`，使 future 被 drop 时终止子进程。`tool/bash.rs` 与 `tool/background.rs` 早已如此，钩子路径是唯一的例外。

**改后行为：** 超过超时的钩子被终止而非变成孤儿。不会有进程比上报它的那条工具结果活得更久。

**Pointers：** `crates/tact/src/plugin/hooks.rs`。

---

## 1. 2026-09-12 — 锁中毒不再中止进程

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/utils/lock.rs`；`crates/tact_llm/src/lock.rs` |

**症状 / 动机：** 有 42 处生产代码用 `.lock().expect("… lock poisoned")` 或 `.write().unwrap()` 获取锁。这些锁守护的无非是缓存、计数器、注册表或配置快照，没有一个守护的是"panic 后可能被撕裂"的不变量。于是别处的 panic 恰好在持锁时发生，就把一次可恢复的状态抖动升级成进程中止；在 TUI 里这等于拆掉整个会话并丢掉进行中的回合，而不是只降级出问题的那个子系统。

**决策：** 新增 `LockExt::lock_recover` 与 `RwLockExt::{read_recover, write_recover}`（`utils/lock.rs`；并以 `tact_llm::lock` 镜像一份，因为两个 crate 无法共享模块），用 `PoisonError::into_inner` 取代 panic。安全性论证：Rust 保证中毒 panic 之后被守护的数据仍是内存安全的；最坏情况只是逻辑上陈旧，而对计数器 / 缓存 / 注册表来说，那恰好是下一次读取本就要处理的状况。42 处全部迁移：`agent/mod.rs`、`agent/tool_dispatch.rs`、`config/mod.rs`、`ui_responder.rs`、`store/sqlite.rs`、`voice/recorder.rs`、`prompt/mod.rs`、`tact_llm/provider.rs`、`tact_llm/models.rs`、`tact-ui/driver.rs`。

**改后行为：** 锁中毒退化为"被守护的值可能少更新一次"，永不退化为中止。保留的少量 panic 属于全局未初始化不变量（`LLM provider not initialized; call tact_llm::init_provider first`），那不是锁状态，故不动。

**Pointers：** `crates/tact/src/utils/lock.rs`；`crates/tact_llm/src/lock.rs`。

---

## 1. 2026-09-12 — 子代理继承其 provider 的压缩路由

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/agent/mod.rs`（`Agent::provider_kind`、`Agent::new`）；`crates/tact_llm/src/provider.rs`（`current_provider_kind`） |

**症状 / 动机：** `Agent::provider_kind` 初始化为硬编码的 `ProviderKind::OpenAi`，只有当调用方链上 `.with_provider_kind(…)` 时才被纠正。`tact-ui` 对顶层 agent 会这么做，`tool/subagent.rs` 不会。于是子代理无论真实 provider 是什么都自认 OpenAI；而由于 Responses 的工具集会剔除本地 `compact` 工具，一个走 Responses 协议的 DeepSeek 子代理会把压缩路由到 `POST /responses/compact`——一个 DeepSeek 并未实现的端点。

**决策：** 字段改为 `Option<ProviderKind>`：`None`（默认）表示"继承当前 provider"，由 `Agent::provider_kind()` 惰性解析，仅在尚未安装 provider 时回退到 OpenAI。为此新增 `tact_llm::current_provider_kind()`——`read_provider` 的非 panic 版本，否则它在 `init_provider` 之前的路径上会直接中止。它把指向 DeepSeek 的通用 OpenAI 兼容端点报告为 `DeepSeek`，与 `is_deepseek` 一致。

**改后行为：** 路由跟随实际配置的 provider，包括子代理。显式 `.with_provider_kind(…)` 仍然优先。未安装 provider 时（测试）行为不变。

**Pointers：** `crates/tact/src/agent/mod.rs`；`crates/tact_llm/src/provider.rs`。

---

## 1. 2026-09-12 — 重试由 HTTP 状态码决定，而非错误文案

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/recovery.rs`（`FailureKind`、`classify_error`、`classify_llm_error`）；`crates/tact/src/agent/mod.rs`（`stream_message`、`retry_compaction_call`） |

**症状 / 动机：** 两个缺陷叠加。(1) 重试判定基于 `error.to_string()` 与英文子串（`timeout`、`rate limit`…）匹配，而唯一携带状态码的变体 `LlmError::HttpError { status, .. }` 被忽略。429 与 400 无法区分，于是一个永久畸形的请求也能一路退避重试到预算耗尽。(2) `Agent::stream_message` 用 `anyhow::anyhow!("{e}")` 包装类型化错误，在恢复逻辑看到它*之前*就字符串化了；退出路径又用 `anyhow::anyhow!(error)` 再包一层。类型在做决定时已不可恢复，且对上层所有调用方而言错误链被压平了。

**决策：** 新增 `is_transient_http_status`（408/429/5xx 可重试，其余 4xx 不可）、`FailureKind { PromptTooLong, Transient, Permanent }`，以及两个分类器：`classify_llm_error(&LlmError)` 与 `classify_error(&anyhow::Error)`，后者在类型化原因仍存活时向下转型到 `LlmError`。`stream_message` 改用 `anyhow::Error::from` 保留错误，循环原样向上传播。非瞬时的状态码仍会检查是否为超长 prompt——那通常以 400 上报且*确实*可恢复，但恢复方式是压缩而非原样重试。两条压缩路径里重复的重试 / 退避 / 上报块合并为 `Agent::retry_compaction_call`，且它现在走分类而非子串匹配。

**改后行为：** 429/408/5xx 退避重试；400/401/403/404 立即失败、不消耗配额；上报超长上下文的 400 触发压缩。到达调用方的错误保留其类型与错误链。

**Pointers：** `crates/tact/src/recovery.rs`；`crates/tact/src/agent/mod.rs`。

---

## 1. 2026-09-12 — worktree 名称在变成路径与 ref 之前先校验

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/worktree/mod.rs`（`validate_worktree_name`） |

**症状 / 动机：** worktree 名称被用两次：拼接到 `<repo>/.worktrees` 之下，以及插入 `wt/<name>` 分支 ref。此前没有任何校验。唯一阻止逃逸的是 `git worktree add` 恰巧会拒绝非法 ref 名——那是 git 的规则而非 Tact 的，而且并不作用于路径。

**决策：** 新增 `validate_worktree_name`，在 `WorktreeManager::create` 里、触碰 store 之前调用：ASCII 字母数字加 `-`、`_`、`.`、`/`；不超过 100 字符；必须以字母数字开头；不允许 `..`、`//`、`.lock` 路径分量、结尾的 `/` 或 `.`。"必须以字母数字开头"这条来自测试而非推想：初版允许了 `/abs`，而 `Path::join` 遇到绝对路径会*替换*基路径，名称将直接逃出 `.worktrees`。

**改后行为：** 危险名称在构造任何路径或 ref 之前就被拒绝，错误信息点明违规字符或模式。`subagent-<id>` 与 `feat/thing` 这类名称不受影响。

**Pointers：** `crates/tact/src/worktree/mod.rs`。

---

## 1. 2026-09-12 — SQLite 启用 WAL，单条消息写入原子化

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/store/sqlite.rs`（`connect_with_pragmas`）；`crates/tact/src/store/session_store/sqlite.rs`（`append_message`） |

**症状 / 动机：** 所有领域 store（sessions、tasks、background、team、worktrees）共用同一个 `<workdir>/.tact/tact.db`，并以默认的 rollback journal 打开。读写因此互相阻塞，而 TUI 直接受此影响：它在 agent 追加的同时读取会话历史。另外 `append_message` 把 `messages` 插入与 `sessions.updated_at` 更新作为两条独立语句发出，二者之间失败就会留下一条"已落库但会话看起来仍是旧的"消息。

**决策：** 改用 `SqliteConnectOptions` 配置连接池，而非一次性 `PRAGMA`：`journal_mode = WAL`、`synchronous = NORMAL`（在 WAL 下不会损坏，且免去每次提交的 fsync；rollback journal 下仍保持 `FULL`）、`busy_timeout = 5 秒`。选择回退而非失败：把数据库切换*进* WAL 需要 `busy_timeout` 无法等待的排他锁，因此并发打开者确实可能切换失败——这种情况记录告警后用默认 journal 重试，而不是拒绝启动。`append_message` 现在把两条语句放进同一事务。

**改后行为：** 写者持锁时读者仍可继续。WAL 持久化在数据库文件中，已有数据库会在下次打开时转换。消息写入失败不留部分状态。

**Pointers：** `crates/tact/src/store/sqlite.rs`；`crates/tact/src/store/session_store/sqlite.rs`。

---

## 1. 2026-09-12 — Anthropic token 计数饱和而非截断

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/anthropic/mod.rs`（`usage_from_json`、`clamp_token_count`） |

**症状 / 动机：** 有 8 处用 `as u32` 转换线上 token 计数，会回绕。provider（或代理、或损坏的响应体）上报超过 `u32::MAX` 的 prompt token 时，会静默变成一个小而看似合理的数字，使底栏展示与持久化记账都少报用量。`total: prompt + completion` 还可能溢出并在 debug 构建下 panic。Chat Completions 路径本就饱和处理、Responses 路径本就用 `try_from` 校验，Anthropic 是异类。

**决策：** 新增 `clamp_token_count`（饱和）以及 `usage_field` / `usage_reasoning_tokens` / `usage_from_json` 辅助函数，替换全部 8 处转换，并合并流式与非流式两路中重复的内联提取。

**改后行为：** 越界计数饱和到 `u32::MAX`；`total` 饱和而非溢出。三个适配器在越界行为上现已一致。

**Pointers：** `crates/tact_llm/src/anthropic/mod.rs`。

---

## 1. 2026-09-12 — ctx 百分比前置到底栏，进度条移除

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/render/bar.rs`；`crates/tui/src/render/bar.rs`；`docs/token_usage_schema.md`；第 23 章 §6.6 |

**症状 / 动机：** 同日的第 2 行瘦身（见下一条）删掉了 ctx 的 `pct%` 却保留了 `■`/`·` 进度条，于是显示为 `ctx [▍···] 45K/1M`。要回答这个段存在的唯一问题——"离自动压缩还有多远"——读者得自己心算除法，而进度条不过把这个百分比用字符又画了一遍。

**决策：** 反转那次瘦身中 ctx 的部分：仍然遵守**一个值只留一种渲染**，但选择另一种编码：
1. **彻底删除进度条**——`render_usage_bar`、`partial_block_char`、`■`/`·`/半格字符常量与 `USAGE_BAR_WIDTH` 全部删除。顶栏的步骤进度（`render_progress_bar`，`█`/`░`）是另一个 widget，未受影响。*（该进度条也已在 2026-09-14 删除——见最新一条。）*
2. **恢复百分比并前置**——`format_context_meter` 现渲染 `ctx 4% 45K/1M`。绝对 `used/window` 保留：比率无法替代它来自的两个计数；它也是小数值唯一的区分方式（590/200K 渲染为 `0%`）。
3. **把 `▣` 缓存段移到紧接 `ctx` 之后**、回合计数之前——两者都是会话级比率，现在挨着读。push 顺序变为 `model → out → think → ctx → cache → turns → timing`；`fit_row_spans` 从末尾开始丢弃，因此存活顺序变为 `ctx > cache > 回合 > 耗时`（缓存与回合计数对调）。

**Behavior after：** 第 2 行渲染为 `deepseek-v4  out 73.1K  think high  ctx 4% 45K/1M  ▣ 30%  ⟳ 12  ⇅ 3  ⏱ 02:05 avg 01:45`——**86 列**（原 90）。顺序与 100 列预算分别由 `bottom_bar_orders_cache_before_turn_counters` 与 `bottom_bar_fits_every_segment_in_100_columns` 锁定；`format_context_meter_leads_with_the_percentage` 断言进度条字符不会回归。

**Pointers：** `crates/agent_tui_kit/src/render/bar.rs`（`format_context_meter`、`context_usage_pct`、第 2 行 `DropGroup` push 顺序）；`crates/tui/src/render/bar.rs`；`docs/token_usage_schema.md`；第 23 章 §6.6。

---

## 1. 2026-09-12 — 已 drain 的子代理结果不再触发空的唤醒轮

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-ui/src/driver.rs`（`spawn_wakeup_task`）；`crates/tact/src/agent/mod.rs`（`Agent::has_pending_subagent_results`）；Ch 12 |

**Symptom:** 后台子代理的 `SubagentFinishedNotification` 到达时，若其结果**已被**进行中的轮次 drain 并注入，driver 仍会提交一个唤醒轮。此时队列为空，没有任何内容被注入，父级只收到那句光秃秃的 prompt `A background subagent finished. Review its result below.` —— 而「below」之下什么都没有。用户看到多出来的一轮，其全部内容是没有任何载荷的通知；模型则必须回答一个承诺了内容、却从未收到内容的 prompt。

**Decision:** 用队列作为唤醒的门控。`Agent::has_pending_subagent_results()` 暴露 `pending_subagent_results` 是否非空，`spawn_wakeup_task` 在为空时提前返回 —— 唤醒轮的唯一职责就是把已入队的结果投递进父级上下文；队列为空即意味着上一轮已经投递过该 summary。同时把通知 prompt 指向 `check_subagent` 作为兜底取回路径，而不再承诺「below」。driver 的保留逻辑不变：轮次进行中到达的通知仍会保留到该轮 `JoinHandle` 完成；在最后一次 drain 之后才入队的结果仍会唤醒父级。

**Behavior after:** summary 仍在队列中的完成事件会唤醒父级并在该轮投递；summary 已被 drain 的完成事件成为 no-op（不产生额外轮次、不浪费 LLM 请求）；队列锁 poisoned 时回退到此前「总是唤醒」的行为，而不是吞掉一次完成事件。

**Pointers:** `crates/tact-ui/src/driver.rs`（`spawn_wakeup_task`、`run_command_loop_with_account`）；`crates/tact/src/agent/mod.rs`（`has_pending_subagent_results`、`agent_loop` drain）；driver 测试 `subagent_finished_notification_is_not_lost_when_parent_finishes`、`subagent_notification_with_empty_queue_does_not_wake_parent`；Ch 12。

---

## 1. 2026-09-12 — 底栏第 2 行瘦身到 90 列预算

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/render/bar.rs`；`crates/agent_tui_kit/src/i18n.rs`；`crates/tui/src/render/bar.rs`；`docs/token_usage_schema.md`；Ch 23 §6.6 |

**Symptom / motivation：** 新增回合计数与回合耗时后，第 2 行涨到约 138 列，普通终端上 `fit_row_spans` 开始静默丢段——这一行被塞满了。审计内容后发现同一个比例或数值被渲染了两三遍：`∑ₜₒₖ {total}` 与 `ctx` 进度条的 `used` 都读 `StatusBarState.token_total`；而 ctx 段自身又把比例同时编码成进度条、`pct%` 和 `used/window` 三种形式。

**Decision：** 按**一个值只留一种渲染**的规则，把第 2 行压到 **90 列且不丢失任何独立信息**：
1. **删除 `∑ₜₒₖ {total}` 段**——与 `ctx` 进度条的 `used` 重复。精确整数仍可在每轮后的任务 stats 块与 `/stats` 中看到，因此丢掉的只是重复渲染。`ICON_TOKENS` / `format_token_total` 一并删除。
2. **删除 ctx 段的 `pct%`**（同一规则）——它是紧邻的 `used/window` 的纯函数——并把**进度条从 10 格压到 6 格**，因为进度条的精确值同样已被给出两次，它只需传达直观感受。该段由 **24 列降到 17 列（−29%）**，同时仍能区分接近阈值的用量（85% 时 `[■■■▍]` vs 4% 时 `[▍···]`）。`format_context_meter` 当时渲染为 `ctx [▍···] 45K/1M`——**同日即被取代**（见最新一条）：进度条删除、百分比恢复，改为 `ctx 4% 45K/1M`。
3. **`max_out_token` → `out`**（中文 `输出`），与相邻 `ctx` / `think` 的简写程度一致。
4. **`▣ cache% 30%` → `▣ 30%`**——图标加 `%` 已足够标识该数字。
5. **`⟳ 12 turns ⇅ 3 turns` → `⟳ 12 ⇅ 3`**——两个计数都去掉 `turns` 文字；相邻的图标对读作一个"回合"数值。
6. **`⏱ 02:05 · avg 01:45` → `⏱ 02:05 avg 01:45`**——去掉分隔点。

随之失效的 i18n 字段（`bottom_cache_pct`、`bottom_turns`、`bottom_llm_turns`）被移除，而非留作陈旧字段。`render_usage_bar` 的单元测试改为从 `USAGE_BAR_WIDTH` 推导期望宽度，不再硬编码内宽 8，因此下次改宽度不会再连带打断它们。

**Behavior after：** 所有段都填充时，整行在 90 列内渲染完毕（`deepseek-v4  out 73.1K  think high  ctx [▍···] 45K/1M  ⟳ 12  ⇅ 3  ▣ 30%  ⏱ 02:05 avg 01:45`），普通终端不再丢段。丢弃顺序为 `ctx > 回合 > 缓存 > 耗时`。宽度预算测试（`bottom_bar_fits_every_segment_in_100_columns`）会在将来某段把行推回约 100 列以上时失败——正是本次修掉的那种失效模式。（行宽、丢弃顺序以及下文提到的 ctx 进度条辅助函数均在同日稍后被改动，见最新一条。）

**Pointers：** `crates/agent_tui_kit/src/render/bar.rs`（`USAGE_BAR_WIDTH`、`format_context_meter`、`format_cache_pct`、`format_turn_user`、`format_turn_llm`、`format_turn_timing`、第 2 行 group 推入顺序；已删除 `ICON_TOKENS`/`format_token_total`）；`crates/agent_tui_kit/src/i18n.rs`（`bottom_out`、`bottom_avg`；移除三个字段）；`crates/tui/src/render/bar.rs`（`bottom_bar_fits_every_segment_in_100_columns`）；`docs/token_usage_schema.md`；Ch 23 §6.6。

---

## 1. 2026-09-12 — TUI 底栏显示回合计数与回合耗时

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/protocol/src/agent.rs`（`AgentUpdate::TurnStats`）；`crates/tact/src/agent/mod.rs`（`agent_loop`）；`crates/agent_tui_kit/src/state/status_bar_state.rs`；`crates/agent_tui_kit/src/components/status_bar.rs`；`crates/agent_tui_kit/src/render/bar.rs`；`crates/tui/src/handlers/skills.rs`；`crates/tui/src/widgets/state/app/popups.rs`；`docs/token_usage_schema.md`；Ch 23 §6.6 |

**Symptom / motivation：** 底栏此前只报告 token、缓存命中率与上下文占用，对**回合**一无所知。唯一的耗时显示是 task-end 分隔线上冻结的 `⏱ mm:ss` 以及任务结束 stats 块——都是历史日志行，不是实时计数。会话跑了多少回合、当前任务占用了多少次 agent-loop 迭代、平均每回合耗时多久，全都看不到。

**Decision：** 在底栏第 2 行新增三个计数，全部来自内存中已有的数据（无新持久化、无 schema 变更）：
1. `AgentUpdate::TurnStats { turns_taken, max_turns }` 由 `agent_loop` 在 `self.turns_taken += 1` 之后每次循环发一次——与 `TokenUsage` 同频，不增加事件量。kit 的 `StatusBarComponent` 认领它；shell 在派发时重置 `turn_llm`。
2. 会话用户回合（`⟳`）在唯一派发入口 `handlers/skills.rs::dispatch_user_task` 计数（同时服务排队刷新与 skill 派发）；断点续传时由 `load_history` 统计已持久化的 user 消息播种。
3. 回合耗时在 `add_task_end_separator` 累计——这是**唯一**真正冻结 `task_start_time` 的位置（`freeze_last_prompt_cost` 在其后运行、永远看到 `None`），因此不会重复计数。被取消的回合计入（其墙钟时间是真实的）；合成分隔线（无 start time）不计入。
4. `TurnStats` 在 `coordinator_prepass` 中登记为**按次调用的元数据**，与 `TokenUsage`/`ModelInfo` 同级。它在回合计间隙、loading 转圈仍在时到达，若不登记就会走内容门、让转圈在循环刚开始时消失（回归测试：`turn_stats_is_metadata_and_keeps_the_loading_placeholder`）。
5. `max_turns` 接入 `StatusBarState.turn_llm_cap` 但**刻意不渲染**：只有 `spawn_subagent` 会设置 cap，主 agent 也没有 CLI flag 或 TUI 接线，底栏永远不可能显示 `/cap`。因此渲染裸 `⇅ {n}`；字段保留，以便将来为主 agent 加 cap 时立即可用。

**Behavior after：** 第 2 行显示 `⟳ 12 轮 ⇅ 3 轮次  ∑ₜₒₖ …  ▣ 缓存% 5%  ⏱ 02:05 · 均 01:45`（英文对应 `⟳ 12 turns ⇅ 3 turns … ⏱ 02:05 · avg 01:45`）。任务的首次 LLM 调用前隐藏 `⇅` 段；回合完成前隐藏 `avg`；尚无回合完成时隐藏整个耗时组。运行中的实时耗时仍只在顶栏——底栏只显示冻结值。*（2026-09-14 起已取代：实时耗时现在是底栏第 1 行紧挨运行的那一段，顶栏不再渲染时钟；见最新一条。）* 窄终端下新段可被丢弃，存活顺序 `ctx > 回合 > ∑ₜₒₖ > 缓存 > 耗时`，原有 `ctx > ∑ > cache` 优先级不变。*（同日稍后已被取代——本条记录的是回合统计刚落地时的状态；当前 90 列的行见上方"第 2 行瘦身"条目。）*

**Pointers：** `crates/protocol/src/agent.rs`（`AgentUpdate::TurnStats`）；`crates/tact/src/agent/mod.rs`（`agent_loop` 发送）；`crates/agent_tui_kit/src/state/status_bar_state.rs`（`turn_user`、`turn_llm`、`turn_llm_cap`、`turn_last_secs`、`turn_done`、`turn_total_secs`）；`crates/agent_tui_kit/src/render/bar.rs`（`ICON_TURNS`/`ICON_LLM_TURNS`/`ICON_ELAPSED`、`format_turn_user`、`format_turn_llm`、`format_turn_timing`）；`crates/tui/src/widgets/state/app/popups.rs`（`add_task_end_separator`）；`crates/tui/src/widgets/state/app/messages.rs`（`load_history` 播种）；spec `docs/superpowers/specs/2026-09-12-turn-stats-bottom-bar-design.md`；Ch 23 §6.6；`docs/token_usage_schema.md`。

---

## 1. 2026-09-11 — `bash` 支持按次传入 `timeout`

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/tool/bash.rs`（`BashInput::timeout`、`resolve_timeout_secs`）；Ch 7 §8 |

**Symptom / motivation：** bash 的墙钟时限此前只能由配置决定（`[tools].bash_timeout_secs`，默认 1,800 秒）。一次长时间构建无法延长它，想要**收紧**上限的 agent 也无从表达；而且未知 JSON 字段会在反序列化时被丢弃，模型自行编造的 `timeout` 只会被静默忽略而非生效。

**Decision：** 在 `BashInput` 上新增可选字段 `timeout`（秒；serde 别名 `timeout_secs`），由 `resolve_timeout_secs(input_timeout, ctx.bash_timeout_secs)` 解析。按次传入的值优先：`Some(0)` 表示本次调用禁用时限，`None` 继承配置值（包括配置中的 `0` 禁用）。

**Behavior after：** `{"command": "cargo build", "timeout": 600}` 无论配置如何都把该次调用限制在 10 分钟；`"timeout": 0` 表示本次调用不设墙钟上限（用户取消仍然生效）。失败信息报告实际生效的时限：`Timeout (<n>s)`。

**Pointers：** `crates/tact/src/tool/bash.rs`（`BashInput`、`resolve_timeout_secs`、`bash`）；测试 `tool::bash::tests::{resolve_timeout_prefers_input_then_config,bash_input_timeout_overrides_configured,bash_input_timeout_zero_disables_configured_timeout}`；Ch 7 §8。

---


## 1. 2026-09-11 — `/mcp list` 在 TUI 内提供实时的 MCP server 视图，且不重连

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/protocol/src/agent.rs`（`UserCommand::McpList`）；`crates/tact/src/mcp/mod.rs`（`McpLiveStatus`、`McpServerView`、`describe_servers`）；`crates/tact-ui/src/mcp_cli.rs`（`render_live_listing`）；`crates/tact-ui/src/driver.rs`；`crates/tui/src/handlers/mcp.rs`；Ch 8 §Step 1c |

**Symptom / motivation：** 此前只能在 shell 里列出 MCP server——`tact-ui mcp list`——而该命令会逐个连接所有已配置的 server。在运行中的 TUI 里无法看到 agent 实际持有哪些 server，因此启动时连接失败的 server、或因等待 OAuth 而挂起的远程 server，除了重启就没有按需查看的途径。

**Decision：** 在既有 `/mcp` slash 命令下新增第二个子命令 `/mcp list`，由 driver 回答。
1. `UserCommand::McpList` 承载请求。`tui::handlers::mcp` **仅在空闲时**发送；任务处于 `Planning`/`Executing` 时只 flash 忙碌提示——driver 会把普通命令排到进行中的轮次之后，若入队则要等该轮结束才显示表格。
2. `tact::mcp::describe_servers(connected)` 在不发起连接的前提下，把每个已配置 server 与**实时**连接集合对照分类：router 持有则为 `Connected { tools }`，远程 OAuth 且无可用凭据为 `NeedsAuthorization`，其余为 `NotConnected`。connected 判断优先，因此可用的 server 绝不会被凭据启发式误标。
3. driver 通过 `render_live_listing` 将结果渲染为 `AgentUpdate::MdInfo`，日志中因此显示一张 Markdown 表格（server / transport / source / status），与 `/skills` 走同一个 `MarkdownCell`。单元格会转义 `|` 与换行，因为 source 路径由用户控制。

**Behavior after：** `/mcp list` 打印每个已配置 server 的实时状态与工具数。它**绝不**发起连接，因此不会重复远程连接、也不会与运行中的 stdio 子进程争用——这与仍会连接并报告最新状态的 `tact-ui mcp list` 不同。配置为空时会说明应当在哪里声明 server。

**Pointers：** `crates/protocol/src/agent.rs`（`UserCommand::McpList`）；`crates/tact/src/mcp/mod.rs`（`transport_kind`、`McpLiveStatus`、`McpServerView`、`describe_servers`、`describe_resolved`）；`crates/tact-ui/src/mcp_cli.rs`（`render_live_listing`）；`crates/tact-ui/src/driver.rs`（`UserCommand::McpList` 分支）；`crates/tui/src/handlers/mcp.rs`；`crates/agent_tui_kit/src/bridge.rs`（`TryFrom<UserCommand>`）。测试：`mcp::tests::{describe_resolved_classifies_against_the_live_connection_set,describe_resolved_lists_a_connected_oauth_server_as_connected}`；`mcp_cli::tests::{live_listing_has_a_row_per_server_with_its_status,live_listing_explains_how_to_configure_when_empty,live_listing_escapes_pipes_so_a_source_path_cannot_break_the_table}`；`driver::tests::mcp_list_emits_the_live_listing_without_reconnecting`；`handlers::mcp::tests::{mcp_list_queues_a_listing_request_when_idle,mcp_list_flashes_busy_instead_of_queueing_while_a_task_runs}`。

---


## 1. 2026-09-11 — Mermaid 弹窗显示渲染后的图，无法渲染时也会说明原因

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/render/popups/mermaid_popup.rs`；`crates/agent_tui_kit/src/state/ui_types.rs`（`MermaidPopupView`、`MermaidPopup::new`）；`crates/tui/src/handlers/overlay.rs`；Ch 23 §6.7 |

**Symptom / motivation:** 日志面板虽然会渲染 Mermaid 图，但只能在日志栏的宽度下渲染；而双击弹窗原本**只用来复制源码**，从不显示图。密集的 flowchart（agent 自己画的图通常如此）在主区域被挤压得难以阅读，弹窗又没有提供更好的视图。更糟的是，使用 Mermaid `style` / `classDef` / `linkStyle` 的 fence 会静默失败：上游 `ratatui-markdown` 的 grammar 只接受 `chain` / `nodedef` / `comment` 语句，于是整个 block 回退为原始代码，且没有任何提示说明原因。

**Decision:** 把弹窗定位成图的「宽视图」，而不是源码查看器。
1. 弹窗状态新增 `MermaidPopupView { Diagram, Source }`，默认 `Diagram`；复用已有的 `scroll` 字段。
2. 弹窗改用 `render_mermaid_block` 以弹窗自身宽度（`centered_popup_area`，约占 frame 的 80%）重新渲染 fence 正文，而不是显示原始行——这个宽度（而非日志面板宽度）正是弹窗的价值所在。
3. `Tab` 在两个视图间切换（overlay 弹窗内此前未绑定该键），并把 `scroll` 重置为 0，因为两个视图高度不同。两种视图下 `y` 均复制源码。
4. 当图视图无法渲染时，弹窗会降级到源码视图，**并**明确打印一行提示，使语法不支持的 fence 可被诊断，而不是看起来像一张空图。

**Behavior after:** 双击图会在约 80% frame 宽度下以渲染后的形式打开；`Tab` 显示源码；`y` 复制；`Esc` 关闭。无法渲染的 Mermaid 会显示其源码，并附上 `⚠ this diagram does not render (unsupported syntax) — showing source`。主区域渲染行为不变。

**Pointers:** `crates/agent_tui_kit/src/render/popups/mermaid_popup.rs`；`crates/agent_tui_kit/src/state/ui_types.rs`（`MermaidPopupView`、`MermaidPopup::new`）；`crates/tui/src/widgets/state/app/popups.rs`（`open_mermaid_popup`、`toggle_mermaid_popup_view`）；`crates/tui/src/handlers/overlay.rs`（`Tab`）。测试：`render_gap_tests::mermaid_popup_opens_on_rendered_diagram_not_source`、`mermaid_popup_tab_switches_to_source_and_back`、`mermaid_popup_falls_back_to_source_and_labels_unsupported_syntax`、`mermaid_popup_renders_diagram_at_wider_width_than_log`、`mermaid_popup_paints_theme_bg_across_its_area`；`handlers::overlay::mermaid_view_tests::*`。

---


## 1. 2026-09-11 — 压缩不再产生孤立的 `role: tool` 消息

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/compact/mod.rs`（`build_compacted_history`、`without_tool_results`）；`crates/tact_llm/src/convert.rs`（`drop_orphaned_tool_messages`）；Ch 5 |

**Symptom / motivation:** 自动压缩之后，下一个 OpenAI 兼容（chat-completions）请求被 provider 以 400 拒绝：`Messages with role 'tool' must be a response to a preceding message with 'tool_calls'`。Harness 风格的 user turn 会把 tool result 和 image 混在同一个消息里（`[ToolResult, Image]`，例如读取 PNG 的截图工具）。`is_real_user_message` 因为其中的 image 而把它判定为真实用户消息，于是 Codex 风格重建会**原样保留**它，却同时丢掉了产出该结果的 assistant `tool_use` turn。被保留的 block 随后被转换成一条没有父级 `tool_calls` 的 `role: tool` 线上消息，OpenAI 兼容 provider 会因此拒绝整个请求。把触发问题的 transcript 用旧的重建逻辑回放，正好产生 6 条这样的孤立消息；在重启 session 之前请求一直失败。

**Decision:** 两层防护，与已有的正向防护（`sanitize_assistant_messages`，用于剔除结果缺失的 tool call）对称：
1. **重建时剥离 tool result** — `build_compacted_history` 让每个保留的 user 消息经过 `without_tool_results`，移除 `ToolResult` block、保留其余内容（text/image）。如果一条消息只有 tool result，则整条跳过，而不是变成空 turn。
2. **线上转换作为最后防线** — `drop_orphaned_tool_messages` 删除所有父级 assistant `tool_calls` 不存在的 `role: tool` 消息；它会先向前跨越一段连续的 tool 消息再查找父级，因此并行 tool call（一个 assistant turn → N 个结果）不会被误删。

**Behavior after:** 压缩后的上下文再也不会出现没有对应 assistant turn 的 tool result。保留的 harness turn 保留其 image 与 text；纯 tool result 的 turn 随其父级一起消失。若将来仍有其他路径产生孤立消息，请求仍会发出（并打印一条带 `tool_call_id` 的 `tracing::warn`），而不是以 400 失败。

**Pointers:** `crates/tact/src/compact/mod.rs`（`without_tool_results`、`build_compacted_history`）；`crates/tact_llm/src/convert.rs`（`drop_orphaned_tool_messages`）。测试：`compact::tests::build_compacted_history_drops_tool_results_from_retained_harness_turns`、`compact::tests::build_compacted_history_skips_pure_tool_result_turns`、`convert::tests::orphan_tool_messages_without_preceding_tool_calls_are_dropped`、`convert::tests::tool_message_with_preceding_tool_calls_is_kept`。

---


## 1. 2026-09-11 — `deepseek-v4-*` 实验变体使用 1M 窗口，不再落到 200K 默认值

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/resolve.rs`（`model_context_window_for_model`）；`config.example.toml`；Ch 21 §5、Ch 5 设置表 |

**现象 / 动机：** 模型→上下文窗口映射按精确 id 匹配 DeepSeek V4（`deepseek-v4-pro`、`deepseek-v4-flash`、`deepseek-reasoner`）。任何带后缀的变体——`deepseek-v4-flash-version-exp`、`deepseek-v4-flash-vision-exp`——都匹配不到，落到 `200_000` 默认值。底栏 `ctx` 计量因此对真实 1M 的模型显示 `…/200K`，派生的自动压缩阈值（窗口的 80%）在 ~160k 而非 ~800k 处触发，压缩掉本还有大量余量的会话。

**决策：** 改为按前缀匹配 DeepSeek V4 家族（`model.starts_with("deepseek-v4-")`），取代固定 id 列表，使实验 / 视觉 / 重发后缀都继承家族窗口。无版本号的网关别名 `deepseek-flash` 与 `deepseek-reasoner` 保留显式 1M 分支。该映射仍保持对 CLI/TOML 的最高优先级。

**改后行为：** 所有 `deepseek-v4-*` id 均解析为 `1_000_000` token 窗口，包括 `deepseek-v4-flash-version-exp` 与 `deepseek-v4-flash-vision-exp`；`deepseek-flash`（OpenAI 兼容网关别名）与 `deepseek-reasoner` 同样解析为 1M。`ctx` 计量与 80% 自动压缩阈值随之生效。手工 `model_context_window` 仍只对无内置映射的模型生效。

**指针：** `crates/tact/src/config/resolve.rs`（`model_context_window_for_model`）。测试：`config::resolve::tests::resolve_model_context_window_maps_deepseek_v4_variants`。

---


## 1. 2026-09-11 — `/mcp auth` 可容忍杂散回环请求，且 token 交换有超时上限

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/remote.rs`（`await_oauth_callback`、`handle_callback_request`、`read_request_head`、`percent_decode`、`hex_nibble`、`redact_query_value`、`OAUTH_TOKEN_EXCHANGE_TIMEOUT`、`OAUTH_CALLBACK_PATH`、`MAX_REQUEST_BYTES`）；Ch 8 §Step 1c |

**现象 / 动机：** 回环回调监听只 `accept()` 一次，且把任何不带 `code` + `state` 的请求都当成致命错误。于是一次浏览器预取、一个 `favicon.ico` 探测、手动访问 `http://127.0.0.1:<port>/`，甚至本地端口扫描，都能抢先占掉这一次 `accept()`；流程随即以 "authorization callback did not carry a code and state" 退出，稍后真正到达的重定向再也读不到——用户只好把锅甩给 provider，而实际上是本地流程坏了。同一条路径上还有三个缺陷：请求头只用一次 `read()` 读取，请求头若被拆进多个 TCP 分片就会解析出空查询，得到同一个假错误；`percent_decode` 按字节偏移切 `&str`，当畸形转义后紧跟多字节 UTF-8 字符（`?code=%aé`）时，会在 TUI 轮询的 future 内 panic（"byte index N is not a char boundary"），直接终止 driver 任务；而 `session.handle_callback`（token POST）完全裸 await、没有超时，provider 接受连接却不回应时 `/mcp auth` 会永久挂住。此外，授权链接以 `info` 级别连查询串一起记入日志，把一次性 CSRF `state` 持久化了下来。

**决策：** 让回调监听既宽容又有界，并且不再记录链接中的秘密部分。`await_oauth_callback` 改为循环 `accept()`：只有命中 `OAUTH_CALLBACK_PATH` 且带 `code` + `state` 的请求才结束流程；带 `error` 的请求才算失败（并把 `error_description` 一并带出）；其余请求回一段简短响应后忽略，同时在整体 300 秒 `OAUTH_CALLBACK_TIMEOUT` 内继续等待。请求头用 `read_request_head` 重组，直到遇到结束空行、EOF 或 8 KiB 的 `MAX_REQUEST_BYTES` 上限。`percent_decode` 改为经 `hex_nibble` 按字节处理，畸形转义退化为字面字符而不再 panic。token 交换外层包上 `tokio::time::timeout(OAUTH_TOKEN_EXCHANGE_TIMEOUT, …)`（新增的本地 60 秒上限），token 端点卡住时会以可处理错误返回。`redact_query_value(&url, "state")` 只把 `state` 的值替换为 `[redacted]`，URL 其余部分（端点、`client_id`）保持可读，便于排查。

**行为变化：** 杂散或畸形的 localhost 请求不再中止 `/mcp auth`；监听会在同一个 300 秒窗口内继续等待真正的重定向。拒绝时会报告 provider 的 `error_description`，而不只是 `error=access_denied`。畸形百分号转义不会再 panic 掉 driver。token 交换卡住会在 60 秒后失败而不是永久挂起。`RUST_LOG=tact=info` 中授权链接显示为 `state=[redacted]`。

**指针：** `crates/tact/src/mcp/remote.rs`。测试：`mcp::remote::tests::{a_stray_connection_before_the_callback_is_ignored,a_fragmented_request_head_is_reassembled,callback_listener_surfaces_denied_authorization,a_denial_reports_the_error_description,a_malformed_escape_on_the_callback_path_is_not_fatal,malformed_percent_escapes_degrade_instead_of_panicking,the_csrf_state_is_redacted_before_logging,callback_listener_times_out_without_a_request}`。

---

## 1. 2026-09-11 — 改写 `mcp.json` 保留文件权限、临时文件名唯一，并容忍 BOM

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/edit.rs`（`read_document`、`write_document`）；`crates/tact/src/mcp/remote.rs`（`write_private`、`create_dir_private`）；`crates/tact-ui/src/mcp_cli.rs`（`add`、`remove`） |

**现象 / 动机：** `mcp.json` 合法地保存机密（`headers` —— 常见的是 `Authorization: Bearer …` —— 以及 `env`），所以用户可能用 `chmod 600` 加固它。改写时用 `fs::write` 写同目录临时文件，这会按 umask 默认值（通常 0644）创建，然后 `rename` 覆盖原文件——把一个 0600 文件静默放宽成 0644，把这些 header/env 值暴露给本机其他用户。临时文件名固定为 `mcp.json.tmp`，因此同一作用域内两个并发 `mcp add` 会互相交错、截断对方的字节，其中一个新增会丢失。带 UTF-8 BOM（Windows 上常见）保存的 `mcp.json` 解析失败，导致每次编辑都被 "fix or remove it" 挡住。OAuth 凭据存储有同一类问题：token 文件先写、之后再 `chmod 0600`，中间存在 0644 窗口；其父目录按 umask 默认值创建，暴露了已授权 server 的集合。

**决策：** 要么经私有句柄写入，要么在文件可见之前恢复权限；同时让临时文件名不会撞车。`write_document` 生成唯一同目录临时文件（`mcp.json.<pid>.<nanos>.tmp`），在 `rename` 之前用 `set_permissions` 把原文件的 `Permissions` 复制过去；rename 失败时删除临时文件。`read_document` 解析前剥离开头的 `\u{feff}`。`remote.rs` 中 `create_dir_private` 用 `DirBuilder::mode(0o700)` 建凭据目录，`write_private` 用 `OpenOptions::mode(0o600)` 打开 token 文件，因此它从未以更宽松的模式存在过。

**行为变化：** `chmod 600 ~/.tact/mcp.json` 在执行 `tact-ui mcp add …`/`remove` 后仍是 0600。同一作用域的并发编辑不再互相覆盖临时文件，失败时也不会留下 `.tmp` 残留。带 BOM 的配置可以编辑，而不再被报为不可解析。`~/.tact/mcp/oauth` 为 0700，其 token 文件创建即 0600。

**指针：** `crates/tact/src/mcp/edit.rs`（`read_document`、`write_document`），`crates/tact/src/mcp/remote.rs`（`create_dir_private`、`write_private`）。测试：`mcp::edit::tests::{rewriting_preserves_the_file_mode,the_written_file_has_no_leftover_temp_sibling,a_leading_bom_does_not_block_editing}`；`mcp::remote::tests::file_credential_store_round_trips_and_clears`。

---

## 1. 2026-09-11 — `mcp add`/`mcp list` 遵循作用域优先级，重复的凭据 flag 被拒绝

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/mod.rs`（`resolve_servers`）；`crates/tact-ui/src/mcp_cli.rs`（`add`、`parse_pairs`、`scope_hint`）；Ch 8 §`tact-ui mcp` |

**现象 / 动机：** 三个作用域与入参相关的 bug。(a) 项目文件已声明 `foo` 时，`tact-ui mcp add foo --user` 仍打印 "Added MCP server 'foo' in ~/.tact/mcp.json"——但加载器是项目覆盖用户，实际生效的 server 没有变化，用户收到的是一个空操作的成功提示。(b) 某个 server 名在一个作用域里没有可用的 `command`/`url`、在另一个作用域里声明有效时，它既被推入 `skipped_remote` 又被解析进 `order`，于是 `mcp list` 会列出两次、计数两次（一个 server 却显示 "2 MCP server(s) configured"）。(c) `--header A:1 --header A:2`（`--env` 同理）静默只保留最后一个值——正是凭据类 flag 最不该有的那种无声意外。

**决策：** 如实报告遮蔽而不是假装成功；把跳过列表与实际解析出的 server 去重；重复的 flag 名直接报错。`add` 把写入路径与 `mcp::resolved_server_for(name)` 比较，若另一来源胜出，就打印警告、指名胜出的文件，并说明该改动在那条声明移除前不会生效。`resolve_servers` 在排序/去重前执行 `skipped_remote.retain(|name| !index_of.contains_key(name))`，因此一个名字要么被跳过、要么被配置，不会两者兼具。`parse_pairs` 在名字被重复插入时报错，且不回显任何值。

**行为变化：** `mcp add` 不再为加载器到不了的声明谎报成功；它会指名真正生效的文件。`mcp list` 每个 server 只列出、计数一次。重复的 `--header`/`--env` 名会以 `--header NAME was given more than once` 失败，而不是丢掉某个值。

**指针：** `crates/tact/src/mcp/mod.rs`（`resolve_servers`），`crates/tact-ui/src/mcp_cli.rs`（`add`、`parse_pairs`）。测试：`mcp::tests::a_name_configured_in_another_scope_is_not_also_reported_as_skipped`；`mcp_cli::tests::{a_repeated_pair_name_is_rejected,malformed_pairs_fail_without_echoing_the_value,pair_parsing_trims_the_name_and_value}`。

---

## 1. 2026-09-11 — `/mcp auth` 现在会在等待浏览器期间就显示 OAuth 链接

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-ui/src/driver.rs`（`stream_auth_progress`、`UserCommand::McpAuth` 分支）、`crates/tact/src/mcp/remote.rs`（`authorize_remote_server`、`OAUTH_CALLBACK_TIMEOUT`）；Ch 8 §Step 1c |

**现象 / 动机：** `/mcp auth figma` 只打印了 `Starting MCP authorization for figma (watch for the URL below)...`，之后就再无输出——提示里承诺的链接始终不出现，用户没有可点击的东西，命令看起来像卡死。流程本身并不慢：`authorize_remote_server` 是在阻塞等待回环回调**之前**就通过 `notify` 上报链接的，但 driver 把这些行收进 `Vec`，直到 future 结束才一次性刷出。而该 future 最早也要等用户完成授权后才结束（此时链接已毫无用处），最晚则要等满 300 秒的 `OAUTH_CALLBACK_TIMEOUT`——也就是说，唯一重要的那一行在结构上根本无法及时到达。CLI 路径（`tact-ui mcp login`）不受影响，只是因为它在自己的 `notify` 里直接打印。

**决策：** 进度行实时流式送往 UI，不再缓冲。`authorize_server` 要求 notify 闭包为 `Send`，因此闭包内不能捕获 `&Agent`；用一个无界 channel 解耦两者——闭包负责发送每一行，`stream_auth_progress` 在授权 future 与接收端之间 `select`，一旦有行到达就立刻以 `AgentUpdate::Info` 发出。流程结束时仍留在队列里的行会在 `select!` 之后补收，因此在「同一次 poll 里既产生链接又结束流程」的情况下链接不会被竞态丢掉。

**行为变化：** 只要 `authorize_server` 产出授权链接，就会立刻以 `Info` 更新发出——此时流程仍阻塞在等待重定向，用户无需等命令结束就有链接可点。授权语义不变：回调监听仍保持 300 秒，期间该命令会占用 driver 的用户命令循环。由于链接走的是普通的 `Info` 更新，它会出现在会话记录里，而不是一次性的浮层。

**指针：** `crates/tact-ui/src/driver.rs`（`stream_auth_progress`、`UserCommand::McpAuth`）。测试：`crates/tact-ui/src/driver.rs` 单元测试 `driver::tests::{auth_progress_reaches_the_user_before_the_flow_finishes,auth_progress_drains_lines_sent_at_completion}`；端到端回归 `crates/tact-ui/tests/mcp_auth_url_progress.rs`——用 `wiremock` 起一个模拟 OAuth provider 驱动 `handle_user_command`，在旧的缓冲实现下会失败。相关：Ch 8 §Step 1c（本次改动恢复的正是该处已记录的 `/mcp auth` 行为）。

---


## 1. 2026-09-11 — `mcp.oauth_client_name`：OAuth 注册身份，默认 `Codex`

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/config/{types,resolve}.rs`（`McpTomlConfig`、`McpSettings`、`resolve_mcp`）、`crates/tact/src/mcp/remote.rs`（`OauthRequestParameters`、`oauth_parameters`、`registration_message`、`authorize_remote_server`）、`config.example.toml`；Ch 8 §Step 1c + FAQ（双语） |

**症状 / 动机：** 原因查明后（Figma 按精确 `client_name` 对动态客户端注册放行，接受 `"Codex"` 而拒绝 `"Tact"`），`tact-ui mcp login figma` 依然不可用：该名称是硬编码常量，无法修改，因此知道答案的用户也无从下手。Figma 的远程 server（官方推荐、功能最全）无法从 Tact 使用。

**决策：** 把注册名称做成配置——`config.toml` 中的 `[mcp] oauth_client_name`，默认 `"Codex"`；并为「只有某个 provider 需要不同答案」这一常见情形提供 `mcp.json` per-server 的 `auth.clientName` 覆盖。默认值的选择使 OAuth 对只接受已知客户端的 provider 开箱可用；`McpSettings::TACT_OAUTH_CLIENT_NAME` 记录如实的替代值（`"Tact"`），且 `config.example.toml` 与书中都明确写出代价：provider 与展示给用户的授权同意页看到的是 `"Codex"` 而非 `"Tact"`。Tact 不隐藏实际使用的名称——它会与其他 OAuth 进度日志一起以 `info` 级别记录（`client_name=…`），并出现在任何注册失败提示中，同时给出两个覆盖点。`oauth_parameters` 原本返回 3 元组，增至 4 元后改为具名的 `OauthRequestParameters`，并提供 `effective_client_name()`：按 per-server 覆盖 → 配置默认 → 内置默认解析，且把空白覆盖视为未设置（任何 provider 都会拒绝空的 `client_name`）。`McpSettings` 手写 `Default` 而非派生，因为空名称是错误的兜底值。`resolve_mcp` 会去除配置值的首尾空白，纯空白则回退默认。

**行为变化：** 无需任何配置，`tact-ui mcp login figma` 即可到达授权 URL（已对线上 Figma 端点验证）。任何接受已知客户端名称的 provider 都可以通过设置该名称打通。`mcp.oauth_client_name = "Tact"` 恢复如实标识，白名单类 provider 随后会以可操作的提示拒绝。per-server 的 `clientName` 优先于全局默认（已验证：全局 `"Tact"` + per-server `"Codex"` 成功）。实际使用的名称可在 `RUST_LOG=tact=debug`/`info` 日志中看到。

**指针：** `crates/tact/src/config/types.rs`（`McpTomlConfig`、`McpSettings::{DEFAULT_OAUTH_CLIENT_NAME,TACT_OAUTH_CLIENT_NAME,Default}`）、`crates/tact/src/config/resolve.rs`（`resolve_mcp`）、`crates/tact/src/mcp/remote.rs`（`OauthRequestParameters::effective_client_name`、`oauth_parameters`、`registration_message`、`authorize_remote_server`）；`crates/tact-ui/src/test_support.rs`、`crates/tui/src/handlers/select.rs`、`crates/tact/src/{agent/mod.rs,tool/read_image.rs}`、`crates/tact-ui/tests/recovery_compaction.rs`（测试配置字面量新增必填 `mcp` 字段）。测试：`config::resolve::tests::resolve_mcp_oauth_client_name_defaults_to_codex_and_is_overridable`、`mcp::remote::tests::{oauth_parameters_default_when_auth_is_not_declared,the_registration_name_falls_back_to_the_configured_default,refused_registration_explains_the_options_not_just_the_status}`。文档：Ch 8 §Step 1c + FAQ 对照表 + `config.example.toml` 新增 `[mcp]` 段；`README.md`。

---

## 1. 2026-09-11 — Figma 的 OAuth 拦截是按客户端名称白名单，不是 bug

| Field | Value |
|-------|-------|
| **Type** | docs + bugfix（报错文案） |
| **Related** | `crates/tact/src/mcp/remote.rs`（`OAUTH_CLIENT_NAME`、`registration_message`）；Ch 8 §Step 1c + FAQ（双语） |

**症状 / 动机：** `codex mcp add figma --url https://mcp.figma.com/mcp` 能成功认证，而 `tact-ui mcp login figma` 在动态客户端注册处以 `HTTP 403 Forbidden` 失败。这个反差用上一条记录里的措辞（「provider 只接受已知客户端」）无法解释——它没说 provider 究竟按什么判定，而 Codex 明显能通过。

**排查：** Codex 内置 `figma@codex-marketplace-global` 插件，其 `.mcp.json` 是普通远程条目（没有 `client_id`），且每次 `codex mcp add` 得到的 `client_id` 都不同——说明 Codex 与 Tact 一样在做动态注册。差别在请求体：`AuthorizationSession::new(..., Some("Tact"), None)` 发送 `client_name: "Tact"`，Codex 发送 `"Codex"`。向 `https://api.figma.com/v1/oauth/mcp/register` 发送其余部分完全一致的注册请求体，得到干净且可复现的分界：`Codex` → `200`（连续三次，每次新的 `client_id` + `client_secret`，`token_endpoint_auth_method: "none"`），`Claude Code` → `200`；而 `Tact`、`Cursor`、`Visual Studio Code` 与小写 `codex` → `403`。因此 Figma 是按**精确的客户端名称字符串**对动态注册放行，与其文档中的 MCP catalog 一致。发现阶段与流程其余部分都正常。

**决策：** Tact 坚持以自己的名称注册并如实报告被拒。发送 `"Codex"` 确实能让注册通过，但那等于向 provider 谎报客户端身份，且依赖另一个产品的授权资格，因此不做——即便藏在配置开关后也不做。替代做法是：常量 `OAUTH_CLIENT_NAME` 统一命名客户端；报错现在明确指出注册常按客户端名称放行、Tact 以 `"Tact"` 注册；并给出三条而非两条路线（自行注册的 `auth.clientId`、`headers` 中的静态 token，或 provider 自带的本地 server——Figma 是 `http://127.0.0.1:3845/mcp`，无需 OAuth）。实测对照表记入 Ch 8 FAQ，避免日后重复排查。

**行为变化：** 对接受 Tact 注册的 provider 无任何变化；此后 `mcp login` 失败时会给出指明「客户端名称」这一关卡的说明，用户可以据此行动（自行注册、改用 token、或改用本地 server）而不是反复重试。代码中不存在任何冒充路径。另有已知缺口保持不变并已写在原因旁：机密客户端的 `client_secret` 仍无法提供，因为 rmcp 的 `StoredCredentials` 只持久化 `client_id`。

**指针：** `crates/tact/src/mcp/remote.rs`（`OAUTH_CLIENT_NAME`、`registration_message`、`authorize_remote_server`）。测试：`mcp::remote::tests::{refused_registration_explains_the_options_not_just_the_status,a_missing_registration_endpoint_does_not_blame_the_client_name}`。文档：Ch 8 §Step 1c 条目 + FAQ 对照表（双语）。方法：对比 `codex mcp` 与线上 Figma 端点，并通过受控注册请求定位到 `client_name` 这一分界。

---

## 1. 2026-09-11 — 回环 MCP server 不再被送进环境代理

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/remote.rs`（`http_client_for`、`is_loopback_url`、`serve_remote`）、`crates/tact/Cargo.toml`（`reqwest13`）；Ch 8 §Step 1c + 缺口（双语） |

**症状 / 动机：** 当导出了 `http_proxy` / `all_proxy` 时，Tact 会把**回环** MCP 请求也发进代理。除非另行指定，reqwest 对所有 host 都遵循这些环境变量，于是 `http://127.0.0.1:3845/mcp`（Figma 桌面 server）永远到不了本地 server：响应来自代理，而代理错误页没有 `Content-Type`，因此失败表现为极具误导性的 `Unexpected content type: None`，而不是「connection refused」。用同一 server 在「导出代理 / 不导出代理」下对比即可确认（`Unexpected content type: None` 对 `error sending request`）；再起一个本地监听器，报错变成 `Some("text/plain")`——即该监听器自己的 content type——证明请求终于到达本地。这个问题重要，是因为对拒绝 OAuth 注册的 provider（Figma）所给出的官方替代方案正是一个回环 URL，而本仓库的开发环境恰好导出了这些变量。

**决策：** 显式构建传输层 HTTP 客户端，并在端点为回环时禁用代理。通过 `StreamableHttpClientTransport::with_client` 传入 `reqwest13::Client::builder().no_proxy().build()`，得到的类型与原先 `from_config` 完全相同（`StreamableHttpClientTransport<reqwest::Client>`），其余逻辑不变。非回环端点仍用 `Client::default()`，因此保留环境代理——在受限网络里，代理正是远程 MCP server 可达的原因。回环判定刻意基于字符串（`127.0.0.0/8`、`localhost`、`::1`，并处理端口、userinfo、路径与查询串）：URL 来自用户配置，为了识别 `localhost` 而引入解析器或 DNS 只会凭空增加失败模式。`localhost.evil.com` 与 `127.0.0.1.evil.com` 会被正确判定为**非**回环。构建无代理客户端失败时回退到默认客户端并打警告，使连接尝试不中断，同时在日志中说明此后可能失败的原因。另有一处限制如实写明而非隐藏：OAuth 管理器会自建客户端，因此针对回环 server 的发现/注册仍会走环境代理——对无需 OAuth 的 Figma 桌面 server 无影响。

**行为变化：** 导出代理时本地 MCP server 可用；而真正不存在的 server 会报告连接失败而不是代理状态码。远程 server 不受影响，仍走代理。`crates/tact` 新增依赖 `reqwest13`（rmcp 0.17 使用的同一版本；它原有的 0.12 crate 不满足 `StreamableHttpClient for reqwest::Client` 的实现）。

**指针：** `crates/tact/src/mcp/remote.rs`（`http_client_for`、`is_loopback_url`、`serve_remote`）。测试：`mcp::remote::tests::{loopback_urls_are_recognized_so_they_can_bypass_a_proxy,remote_urls_keep_using_the_environment_proxy}`。文档：Ch 8 §Step 1c 条目、缺口表（双语）、`README.md`。实网：导出代理的情况下 `cargo test -p tact --test live_remote_mcp -- --ignored` 仍通过。

---

## 1. 2026-09-11 — OAuth 客户端注册被拒时，现在会说明该怎么办

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/remote.rs`（`registration_error`、`registration_message`、`registration_reason`、`authorize_remote_server`）；Ch 8 §Step 1c + FAQ（双语） |

**症状 / 动机：** 对拒绝动态客户端注册的 provider 授权时，报错既不说原因也不给出路：

```
Error: MCP authorization failed for figma: OAuth client registration failed for figma:
Registration failed: Dynamic registration failed: Registration failed: HTTP 403 Forbidden: Forbidden
```

这里的 provider 是 Figma：其远程 server（`https://mcp.figma.com/mcp`）只接受其 MCP catalog 中的客户端（VS Code、Cursor、Claude Code），因此 `https://api.figma.com/v1/oauth/mcp/register` 会对其他任何客户端返回 `403`。发现阶段是成功的——授权服务器为 `https://api.figma.com`，其元数据声明了 `client_secret_basic`/`client_secret_post`，但**没有**公共客户端的 `none`——所以流程是在最后一步失败，而这个状态码看起来像临时网络问题。已针对线上端点验证：带代理与不带代理、加 `Authorization: Bearer`、表单编码 body、空 body，结果一律 `403`，即服务端策略而非请求形状问题。`mcp list` 也正确地把该 server 显示为 `needs authorization`，所以死路只存在于报错文案里。

**决策：** 在流程仍持有 metadata 的那一处识别 `AuthError::RegistrationFailed`，用「原因 + 两条真正出路」取代原始错误链。提示会区分「声明了注册端点但拒绝了我们」与「从未声明注册端点」（后者 rmcp 报 `Dynamic client registration not supported`），并打印具体的 redirect URI——设置了 `callbackPort` 时即为固定的 `http://127.0.0.1:<callbackPort>/callback`，因为这正是 provider 在接受手工注册客户端时要你填写的东西。rmcp 会把自己的错误包两层，因此 `registration_reason` 会剥掉重复的 `Registration failed:` / `Dynamic registration failed:` 前缀，只留信息量最大的尾部；若 rmcp 改了措辞，前缀不再匹配就会原样展示完整文本，属于降级而非误报。失败会连同 server 名、provider URL 以及是否声明了注册端点一起写入日志（绝不记录 token），符合该子系统既有的排障约定。此改动**刻意不声称**能修好 Figma：任何客户端侧改动都做不到，文档现在也如实说明，并指向无需 OAuth 的桌面 server（`http://127.0.0.1:3845/mcp`）。

**行为变化：** `tact-ui mcp login figma`（以及共用同一代码路径的 `/mcp auth figma`）会打印 `OAuth client registration failed for figma: HTTP 403 Forbidden: Forbidden`，随后是原因、重试无用的说明，以及可选方案：自行注册 OAuth 客户端并设置 `auth.clientId` 与固定的 `callbackPort`，或通过 `headers` 提供 provider 签发的静态 token。支持 DCR 的 provider 不受影响——新分支只在 `RegistrationFailed` 时进入。机密客户端的 **client secret** 仍无法提供（rmcp 的 `StoredCredentials` 只带 `client_id`，刷新时会丢失），这一点现已作为已知缺口写明，而不是静默限制。

**指针：** `crates/tact/src/mcp/remote.rs`（`registration_error`、`registration_message`、`registration_reason`、`authorize_remote_server` 中对 `has_registration_endpoint` 的捕获、OAuth metadata 的 `debug` 日志）。测试：`mcp::remote::tests::{registration_reason_unwraps_rmcps_nested_wrapping,refused_registration_explains_the_options_not_just_the_status,missing_registration_endpoint_reads_differently_from_a_refusal}`。文档：Ch 8 §Step 1c 条目 + FAQ 条目 + 缺口表行（双语）。实网检查：`cargo test -p tact --test live_remote_mcp -- --ignored` 仍通过（DCR provider 不受影响）。

---

## 1. 2026-09-11 — `tact-ui mcp`：完整的 CLI MCP server 管理

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/mcp/edit.rs`（新增）、`crates/tact/src/mcp/{mod,remote}.rs`、`crates/tact/src/config/cli.rs`（`McpSubcommand`）、`crates/tact-ui/src/mcp_cli.rs`、`crates/tui/src/handlers/mcp.rs`；计划 `docs/superpowers/plans/2026-09-11-remote-mcp.md` |

**症状 / 动机：** CLI 能查看（`mcp list`）与授权（`mcp auth`）server，却无法创建、删除或取消授权——声明 server 仍只能手写 `mcp.json`，而该格式容易写错（远程与 stdio 的键不同、`auth` 拼写、引号），headless 用户也没有任何界面可以帮忙。凭据更是完全没有删除路径：一旦授权就永久授权，`~/.tact/mcp/oauth/<server>.json` 只能手工删除。此外 `mcp list` 会连接**全部** server，查看单个条目也会启动并拨号其余配置。

**决策：** 按「各自负责的副作用」拆分为六个子命令，任何命令都不会悄悄做两件事：`list`（全部 server，连接）、`get <name>`（单个 server，只连接它）、`add`（写配置，绝不连接）、`remove`（写配置，保留凭据）、`login`（OAuth 流程，写凭据；保留 `auth` 作为可见别名）、`logout`（删除凭据，绝不连接）。`tact-ui mcp add <name> --url <URL> [--oauth] [--header N:V]…` / `--command <CMD> [--arg A]… [--env N=V]…` 覆盖两种传输；clap 强制 `--url` 与 `--command` 互斥，且 `--arg`/`--env`/`--header`/`--oauth` 各自依赖对应传输。`--user` 选择 home 文件而非项目文件；`add --force` 覆盖，而 `remove` 一个不存在的名字是报错而非无声空操作——消息会指出该 server **究竟**声明在哪个文件（`retry with --user`），或说明它由插件提供。校验（名称字符集、URL scheme、HTTP header 名**与值**）在任何写入之前完成。配置写入**直接编辑原始 JSON 文档**而不经 `McpConfigFile` 往返，因此未知键与其他 server 都会保留；写入是原子的（临时文件 + rename），无法解析的文件会报错而不是被覆盖。删除最后一个 server 会留下空的 `mcpServers` 对象（可预测的编辑，读回来仍是「无 server」）。server 名还会额外拒绝空白、控制字符与路径分隔符：名称同时是 `mcp__<server>__<tool>` 的 `<server>` 段与凭据文件名，因此 `mcp logout <name>` 绝不能指向任意文件——`oauth_credential_path` 现在直接拒绝不安全的名称，这也顺带加固了 `login`/`auth` 面对恶意 `mcp.json` 键的情形。TUI 接受 `/mcp login <server>` 作为 `/mcp auth <server>` 的别名；两个视图通过抽出的 `connected_text`/`needs_auth_text`/`failed_text` 共用同一套状态措辞。`mcp list` 还会打印 **Overridden declarations** 段，指明被覆盖与最终生效的文件——覆盖关系决定了「删掉一条声明」究竟改变了什么。header 与 env 的**值**绝不回显或写日志。

**行为变化：** `tact-ui mcp add figma --url https://mcp.figma.com/mcp` 会创建或扩充 `.tact/mcp.json`，并打印路径与新条目的传输方式；`--oauth` 还会打印后续的 `tact-ui mcp login figma` 提示。`mcp get figma` 打印传输方式、来源、状态，以及 agent 实际必须调用的工具名（`mcp__figma__<tool>`），且只连接该 server。`mcp remove` 只删除一条声明；名字在别处时它会指出该去哪里找。`mcp logout` 删除凭据文件，无凭据时幂等成功，且声明已被删除后依然可用。重复添加同名条目会报错，除非给出 `--force`。Tact 未建模的键在往返后保留；对象键以排序后的 pretty JSON 重新序列化，因此手写格式的文件会在首次写入时被重新格式化一次。

**指针：** `crates/tact/src/mcp/edit.rs`（`McpConfigScope`、`McpDraftTransport`、`McpServerDraft::{new,transport_kind}`、`add_mcp_server`、`RemovedMcpServer`、`remove_mcp_server`、`read_document`、`write_document`、`validate_remote_url`）；`crates/tact/src/mcp/mod.rs`（`is_safe_server_name`、`validate_server_name`、`McpServerStatus`、`McpServerInspection`、`ConnectOutcome`、`connect_server`、`resolved_server_for`、`inspect_server`、重构后的 `load_mcp_router_with_report_inner`）；`crates/tact/src/mcp/remote.rs`（`oauth_credential_path` 加固、`forget_credentials`）；`crates/tact/src/config/cli.rs`（`McpSubcommand::{List,Get,Add,Remove,Login,Logout}`）；`crates/tact-ui/src/mcp_cli.rs`（`get_server`、`render_server_detail`、`status_text`、`remove`、`scope_hint`、`scope_of`、`add`、`draft_from_args`、`parse_pairs`、`logout`）；`crates/tui/src/handlers/mcp.rs`。测试：`mcp::edit::tests::{creates_a_project_file_for_a_remote_server,writes_oauth_declaration_for_remote_server,writes_stdio_entry_with_args_and_env,adding_a_second_server_keeps_unknown_keys_and_the_first_server,refuses_to_replace_without_force_and_replaces_with_it,an_unparseable_file_is_never_overwritten,a_flat_mcp_servers_value_is_rejected,written_entries_load_back_through_the_reader,the_written_file_has_no_leftover_temp_sibling,invalid_names_and_transports_are_rejected_before_writing,project_scope_targets_the_workdir_and_user_scope_the_home_dir,remove_deletes_only_that_entry_and_keeps_everything_else,removing_the_last_server_leaves_an_empty_but_valid_config,remove_reports_an_absent_name_instead_of_succeeding_silently,an_unsafe_server_name_is_rejected_by_both_add_and_remove}`、`mcp::remote::tests::{an_unsafe_server_name_never_derives_a_credential_path,forgetting_credentials_rejects_an_unsafe_name_before_touching_disk,forgetting_a_missing_credential_is_not_an_error}`、`mcp_cli::tests::{overridden_declarations_name_the_file_that_wins,a_report_without_overrides_has_no_override_section,detail_view_shows_transport_source_status_and_qualified_tool_names,detail_view_reuses_the_list_wording_for_pending_and_failed_servers,detail_view_never_prints_an_empty_tool_list_as_success,scope_of_maps_the_user_flag,a_url_becomes_a_remote_draft_with_headers_and_oauth,a_command_becomes_a_stdio_draft_with_args_and_env,neither_or_both_transports_are_rejected,malformed_pairs_fail_without_echoing_the_value,pair_parsing_trims_the_name_and_value}`、`tui::handlers::mcp::tests::mcp_login_is_an_alias_for_auth`。文档：Ch 8 Step 1 + Step 1c（双语）、Ch 21 插件段（双语）、`README.md`。

---

## 1. 2026-09-11 — 远程 MCP server：Streamable HTTP + OAuth 2.0

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/mcp/{mod,remote}.rs`、`crates/tact/src/consts.rs`、`crates/tact/src/agent/mod.rs`（`reload_mcp_router`）、`crates/protocol/src/agent.rs`（`UserCommand::McpAuth`）、`crates/tact-ui/src/driver.rs`、`crates/tui/src/handlers/mcp.rs`、`crates/agent_tui_kit/src/i18n.rs`；设计 `docs/superpowers/specs/2026-09-11-remote-mcp-design.md`；计划 `docs/superpowers/plans/2026-09-11-remote-mcp.md` |

**症状 / 动机：** Tact 的 MCP 客户端只会说 stdio。`McpProjectConfig` 早已解析 `type: "http" | "sse"` 与 `url`，但 `resolve_servers` 把每个远程条目塞进 `skipped_remote` 后丢弃，因此 `{ "url": … }` server 既不连接，也只留一句简短的「已跳过」。2026-09-10 接入的 `openai-curated` 目录把这个缺口具体化：MCP server 为远程的插件可以安装却永远用不了。远程 MCP 端点通常还要求 OAuth（MCP 2025-06-18 / SEP-985），所以只做传输也不足以让其可用。

**决策：** 基于 `rmcp` 的 Streamable HTTP 客户端与其 OAuth 支持，打通远程条目的全链路，并保留既有 `McpService` 抽象，使路由、命名与权限完全不变。条目要么是 `command`（stdio），要么是 `url`（远程），可选 `headers`（静态认证）与 `auth: { "type": "oauth", … }`。两者同时存在时 `command` 优先；两者都没有的条目按已跳过上报。OAuth 采用授权码 + PKCE 流程：元数据发现、动态客户端注册（除非提供 `clientId`）、`127.0.0.1` 回环重定向；token 按 server 持久化在 `~/.tact/mcp/oauth/<server>.json`（`0600`）并自动刷新。启动绝不等待浏览器：声明了 OAuth 但没有可用凭据的 server 记为 `pending_auth`（`MCP server <name> needs authorization — run /mcp auth <name>`），既不连接也不算失败。`/mcp auth <server>` 执行流程、打印授权 URL，随后热重载 MCP router（`Agent::reload_mcp_router`），无需重启。各关键步骤仅记录 server 名、URL 与 header **名**——token 值从不写日志。

即使服务器需要 OAuth，也**不要求**声明 `auth`。实网测试发现：对 Linear（需要 OAuth）只写 `url` 时，暴露出来的是一句晦涩的 `Auth required` 连接失败，因此现在会识别 401 并升级为 `pending_auth`。rmcp 用 `StreamableHttpError::AuthRequired` 表达它，但该变体承载的类型既未实现 `Display` 也未实现 `Error`，且 `ClientInitializeError::TransportError` 没有用 `#[source]` 串接它，因此无法 downcast；可命中的是 `ClientInitializeError` 本身（已针对 rmcp 0.17 验证），所以 `is_auth_required_error` 匹配该传输变体并检查 rmcp 的 `"Auth required"` 文案——无法识别时退化为普通失败，绝不误报。为避免提示指向死路，另一半同样必要：`/mcp auth` 在未声明 `auth` 时也能工作（默认动态注册、无 scope、临时端口），且无论是否声明过 `auth`，已存凭据都会被采用。

交互式 TUI 通过 `/mcp auth <server>` 使用这套能力；headless 用户没有 TUI，因此同一对能力以 CLI 子命令形式暴露：`tact-ui mcp list` 与启动完全一致地解析并连接，然后打印每个 server 的传输方式、来源与状态（connected / needs authorization / failed / skipped）；`tact-ui mcp auth <server>` 执行流程、打印 URL，结束后重新列出以便立即看到结果。两者都注册为不调用 LLM 的命令，因此不需要 provider 配置或 API key。`mcp list` 也覆盖了本工作原先推迟的 `/mcp status` 面；TUI 内部状态仍来自启动时的提示。

**行为变化：** `{ "url": … }` / `type: "http"` 的 server 会被连接而非跳过；`skipped_remote` 现在只表示传输不受支持或不完整。已安装插件也可提供远程 server。OAuth server 只出现一条待授权提示——无论是声明了 `auth` 还是被 401 识别——`/mcp auth <server>` 之后其工具在同一会话内即可用，后续会话无需再授权。过期且无法刷新的 token 回到 `pending_auth` 而不是表现为连接失败。连接失败依旧绝不致命。

**指针：** `crates/tact/src/mcp/remote.rs`（`McpRemoteConfig`、`McpAuthConfig`、`serve_remote`、`resolve_remote_auth`、`stored_access_token_at`、`oauth_parameters`、`is_auth_required_error`、`authorize_remote_server`、`FileCredentialStore`、`await_oauth_callback`、`percent_decode`）；`crates/tact/src/mcp/mod.rs`（`McpTransportConfig`、`McpTransportKind`、`ConfiguredServer`、`to_transport`、`resolve_servers`、`ResolvedServers::configured`、`load_mcp_router_with_report`、`remote_config_for`、`authorize_server`、`McpLoadReport::{configured,pending_auth,notice_lines}`）；`crates/tact-ui/src/mcp_cli.rs`（`run_mcp_cli`、`render_report`、`status_for`）；`crates/tact/src/config/cli.rs`（`McpSubcommand`）；`crates/tact/src/agent/mod.rs`（`rebuild_cached_tool_specs`、`reload_mcp_router`）；`crates/tact/src/consts.rs`（`home_mcp_oauth_dir`）。测试：`remote_config_parses_url_headers_and_oauth`、`invalid_header_names_are_dropped_from_the_transport_config`、`oauth_token_becomes_the_bearer_auth_header`、`file_credential_store_round_trips_and_clears`、`callback_listener_{extracts_code_and_state,surfaces_denied_authorization,times_out_without_a_request}`、`commandless_entries_are_skipped_not_fatal_while_remote_entries_connect`、`remote_entry_with_oauth_needs_authorization_without_credentials`、`load_report_renders_pending_authorization`、`auth_required_detection_ignores_unrelated_errors`、`undeclared_auth_still_uses_a_stored_credential`、`oauth_parameters_default_when_auth_is_not_declared`、`mcp_cli::tests::{empty_report_explains_how_to_configure,renders_each_server_with_its_status,skipped_servers_are_listed_even_though_they_are_not_configured,a_server_with_no_recorded_outcome_is_not_reported_as_healthy}`。公网远程 server 的实网（可选）端到端检查：`crates/tact/tests/live_remote_mcp.rs`（`cargo test -p tact --test live_remote_mcp -- --ignored --nocapture`），覆盖 DeepWiki + Cloudflare Docs 连接并暴露 `mcp__<key>__*` 工具、Linear 的 401 被升级为 `pending_auth`、以及声明与不声明 `auth` 两种情况下都能产出授权 URL（对真实 provider 验证发现、动态注册与 PKCE S256）。文档：Ch 8 §3.2/Step 1b/1c/FAQ/缺口（双语）、Ch 21 插件段（双语）。

---


## 1. 2026-09-10 — `tracing-subscriber` 的 `default-features = false` 终于生效

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `Cargo.toml`、`crates/tact-ui/Cargo.toml`、`crates/tact-ui/src/main.rs`（`init_logging`）；取代 `24150b08` 的一部分 |

**症状 / 动机:** 2026-09-08 那次「把 MCP 日志导入 `tracing`、不让它进 TUI」的修复，同时也想关掉 `tracing-subscriber` 的默认 feature，于是在 `crates/tact-ui/Cargo.toml` 里写成 `tracing-subscriber = { workspace = true, default-features = false }`。但**依赖从 workspace 继承时，Cargo 会忽略 `default-features`** —— 只会给一条 warning —— 因此 `ansi` 与 `tracing-log` 实际一直开着。`tracing-log` 开着会让 `registry().init()` 安装全局 `log` bridge，于是依赖里每一条 `log::` 记录（rmcp 的 MCP 握手、reqwest 等）都会被转发进 Tact 的每日日志文件，成为没有字段的行：正是上一次修复想消掉的噪声。`ansi` 在这里同样无用，因为 `init_logging` 写文件时用的是 `.with_ansi(false)`。

**决策:** 把 feature 选择放到 Cargo 唯一会尊重的位置 —— workspace 依赖 —— 让成员原样继承：根 `Cargo.toml` 写 `tracing-subscriber = { version = "0.3", default-features = false, features = ["fmt", "env-filter"] }`，`crates/tact-ui/Cargo.toml` 简化为 `{ workspace = true }`。`fmt` 与 `env-filter` 是 `init_logging` 真正用到的全部 feature；`ansi` 与 `tracing-log` 已从解析后的依赖图中消失。

**改后行为:** 依赖的 `log` 记录不再进入 Tact 的日志文件 —— `.tact/logs/tact-<date>.log` 只包含 Tact 各 crate 通过 `tracing` 发出的事件。`RUST_LOG` 过滤行为不变（`EnvFilter` 仍从环境读取），交互式 TUI 也仍然不会安装终端 fmt layer。已用 `cargo tree -e features -i tracing-subscriber` 验证（结果中不再有 `ansi` 或 `tracing-log`），并跑过 `cargo test -p tact --lib`（751 通过）与 `cargo check -p tact-ui --all-targets`。

**指针:** `Cargo.toml`（`[workspace.dependencies] tracing-subscriber`）；`crates/tact-ui/Cargo.toml`；`crates/tact-ui/src/main.rs`（`init_logging`）。被取代的提交：`24150b08`。

---

## 1. 2026-09-10 — MCP 有了原生配置文件；插件状态与技能根目录收敛

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/mcp/mod.rs`、`crates/tact/src/consts.rs`、`crates/tact/src/plugin/store.rs`、`crates/tact-ui/src/{interactive,headless}.rs`；设计见 `docs/superpowers/specs/2026-09-10-path-convergence-design.md`；计划见 `docs/superpowers/plans/2026-09-10-path-convergence.md` |

**症状 / 动机:** MCP 配置散落在三套生态里，没有 Tact 自己的位置。项目的 server 只能来自 cwd 级的 `.codex-plugin/plugin.json`，全局作用域则要走一遍 marketplace → install → cache；既没有 `~/.tact/mcp.json`，也完全没有项目级的 MCP 文件。另外三个缺陷叠加：cwd manifest 使用**另一套**命名（`{plugin}__{server}`），与插件提供的 server 命名不一致，同一个概念有了两个答案；插件根的 `.mcp.json` 会被读取，而工作目录下的同名文件却被静默忽略；server 连接失败只记 `tracing::debug!`，`command` 写错完全没有用户可见信号；`~/.tact/plugins/` 把几 KB 的状态和几百 MB 的 cache 混在一起。此外 `skill_search_dirs` 返回 `[workdir/.tact/skills, ~/.tact/skills, ~/.agents/skills]` 且后者覆盖前者，导致 Codex 兼容根反而压过 Tact 自己的根。

**决策:** 给 MCP 一个原生 `mcp.json`，两个作用域各一个 —— `~/.tact/mcp.json`（用户）与 `<workdir>/.tact/mcp.json`（项目）；沿用所有 MCP 客户端都接受的 `mcpServers` 结构，按 server 名让项目覆盖用户。在这里声明的 server 直接以 map key 命名，工具名因此恰好是 `mcp__<key>__<tool>`，不带 manifest 前缀。项目作用域**只有一个**文件名：Tact 现在既不读 cwd 的 `.mcp.json`，也不读 cwd 的 `.codex-plugin/plugin.json`，因此「这个项目的 server 声明在哪」只有一个答案，也不再有「某个目录算不算 plugin」的解释问题。`PluginLoader` 随该来源一并删除——它没有其他调用方。只有已安装的 marketplace 插件仍提供 server，并保留 `plugin__<plugin>__<server>` 命名，因为插件是可分发的包而非配置约定。解析优先级：`~/.tact/mcp.json` → `<workdir>/.tact/mcp.json` → 已安装插件。解析过程不再丢弃失败，而是返回 `McpLoadReport`（connected / failures / shadowed / skipped_remote）。插件状态迁到 `~/.tact/plugins/state/`，保留旧路径读取与一次性尽力迁移。技能根重排为 `[~/.agents/skills, ~/.tact/skills, <workdir>/.tact/skills]`，项目始终优先、Tact 始终压过 Codex。`PluginHome` 改为显式保存 `home` 与 `state`，不再用 `root.parent().parent()` 推导 `$HOME`。

**改后行为:** 添加 MCP server 只需一个文件（项目级或用户级），不必再走 marketplace 往返，也不存在「该用哪个项目文件」的歧义。server 丢失时只需检查两处：`~/.tact/mcp.json` 与 `.tact/mcp.json`。server 出问题会在启动时给出可见提示（TUI 走 `AgentUpdate::Info`，headless 走 stderr），指明 server 名与错误；一切正常时保持安静。连接失败仍不致命，单个坏 server 不会阻止 agent 启动。来源之间的覆盖会被上报，不再静默。远程（`http`/`sse`）与缺少 `command` 的条目按「已跳过」上报，不再中断解析。cwd manifest 解析失败不再可能中断启动，因为它已不被读取。插件状态写入 `state/`，旧文件只读保留，使共享同一 home 的旧版二进制仍可用。同名用户技能同时存在于 `~/.tact/skills` 与 `~/.agents/skills` 时，现在解析到 `~/.tact/skills` 的内容 —— 这是有意的优先级反转。

**指针:** `crates/tact/src/mcp/mod.rs`（`McpConfigFile`、`McpLoadReport`、`collect_sourced_servers`、`resolve_servers`、`load_mcp_router_with_report`）；`crates/tact/src/consts.rs`（`TactPath::{mcp_config_path, home_mcp_config_path}`、`skill_search_dirs`、`PluginHome::{home, state}`）；`crates/tact/src/plugin/store.rs`（`state_file`、`read_state`）；`crates/tact-ui/src/{interactive,headless}.rs`。测试：`mcp_config_file_reads_servers_and_missing_is_none`、`mcp_config_file_parse_error_names_the_path`、`later_source_overrides_earlier_by_server_name`、`non_conflicting_sources_merge`、`remote_and_commandless_entries_are_skipped_not_fatal`、`native_config_key_is_the_server_name_without_a_prefix`、`project_mcp_json_is_read_and_a_cwd_dot_mcp_json_is_not`、`load_report_*`、`plugin_home_exposes_explicit_home_state_and_cache_paths`、`new_state_location_wins_when_both_exist`、`legacy_state_is_read_and_migrated_to_state_dir`、`state_is_written_to_the_state_directory`、`tact_skill_root_outranks_the_agents_compatibility_root`。

---

## 1. 2026-09-10 — 权限提示改为从共享 pending-UI broker reconcile

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/ui_responder.rs`、`crates/tui/src/widgets/state/app/agent.rs`、`crates/tui/src/handlers/select.rs`、`crates/tact-ui/src/interactive.rs`；Ch 25 §4.3；`docs/state_machines.md` §2/§8 |

**症状 / 动机:** 权限提示可能永久停在 `Running`：唯一的 `AgentUpdate::RequestSelect` 事件丢失，或在另一条 update 重置 `input_mode` 时被处理。此前的 `restore_pending_select_mode` 只修了已知的 `SessionStats` 失同步；事件本身仍是唯一真相来源，一旦它丢失或 TUI 没处理到，`UiResponder` 的等待者就没有任何东西能回答。交互式 `edit_file` 提示也命中同类问题；实际观察到某一步停在 Running 约 30 分钟，文件未写入，也没有产生 `tool_result`。

**决策:** 不改 protocol 类型，改为让 in-process `UiResponder` 成为权威的 pending-request registry。`register_select` / `register_multi` 在发出既有 `RequestSelect` hint 之前记录 `PendingUiRequest` 元数据；`snapshot()` 返回有序 pending 集合；`respond()` 原子移除并唤醒等待者；`withdraw()` 处理取消/丢弃。TUI 在每次 agent update 与 poll tick 后从 `snapshot()` reconcile `InputMode::Select`，把 `RequestSelect` 降级为 wake-up hint，并在交互模式下直接经 broker 回答。没有 broker 的 headless/tests 仍以 `RequestSelect` 为权威。若等待 future 被丢弃，`PendingRequestGuard` 会 withdraw，避免被中止的工具留下幽灵弹窗。

**变更后行为:** 丢失或重复的 `RequestSelect` 不再让工具卡在 Running：TUI 会拉取 pending snapshot 并显示弹窗。多个提示（并发 subagent / `ask_user`）按 request id 排队在 broker 中，而不是单独的 `VecDeque`。Enter/Esc 直接回答 broker；`/cancel` 会先用 `None` 回答当前提示，再发送 `UserCommand::Cancel`；等待者被 abort 会移除 pending entry。`tact_protocol` enum 与 wire shape 未变；这是 in-process reconciliation 层，后续做 server transport 前应提升为带版本的 snapshot protocol。

**指针:** `crates/tact/src/ui_responder.rs`（`PendingUiRequest`、`snapshot`、`respond`、`withdraw`、`PendingRequestGuard`）；`crates/tui/src/widgets/state/app/agent.rs`（`reconcile_pending_ui`）；`crates/tui/src/handlers/select.rs`；`crates/tact-ui/src/interactive.rs`；`crates/tui/src/lib.rs`；Ch 25 §4.3；`docs/state_machines.md` §2/§8。测试：`ui_responder::tests::*`、`broker_snapshot_*`、`broker_mode_enter_wakes_registered_waiter`、`broker_mode_cancel_answers_pending_select_with_none`。

---

## 1. 2026-09-10 — 发现 Codex 本地 marketplace，支持 plugin install/list

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/plugin/{model,store,marketplace,install,mod}.rs`、`crates/tact-ui/src/plugin_cli.rs`；设计见 `docs/superpowers/specs/2026-09-10-codex-marketplace-design.md`；计划见 `docs/superpowers/plans/2026-09-10-codex-marketplace.md` |

**症状 / 动机:** Tact 已采用 Codex 的 plugin manifest/hooks/MCP 布局，但 marketplace 发现仍只暴露硬编码的 Claude 官方 Git marketplace。Codex personal marketplace（`~/.agents/plugins/marketplace.json`）不会出现在 `tact-ui plugin marketplace list`；Codex 的 `source: "local"` catalog entry 无法解析；不带 `@marketplace` 的 `plugin install <name>` 也仍默认到 `claude-plugins-official`。

**决策:** 发现 `$HOME/.agents/plugins/marketplace.json` 与 最近的祖先目录 `.agents/plugins/marketplace.json`；把它们以非持久化的 `MarketplaceSource::LocalPath` 记录加入 marketplace state；支持 Codex `source: "local"`，并将插件路径按 marketplace root 解析。裸安装现在优先扫描已发现的 Codex marketplace，找不到才回退到 `claude-plugins-official`；本地 marketplace 的 update 只重新读取 catalog，不做网络刷新。

**改后行为:** `tact-ui plugin marketplace list` 会先显示 Codex 本地 marketplace，再显示旧官方 marketplace；`tact-ui plugin install build-ios-apps` 可不带 `@marketplace` 直接从用户 Codex marketplace 安装；`plugin marketplace update <codex-local-name>` 会从磁盘重读 catalog。Claude 官方 marketplace 仍作为兼容 fallback 保留。

**指针:** `crates/tact/src/plugin/model.rs`（`LocalPath`、discovered state）、`crates/tact/src/plugin/store.rs`（Codex marketplace 发现）、`crates/tact/src/plugin/marketplace.rs`（`source: "local"`、catalog path）、`crates/tact/src/plugin/install.rs`（本地 source root）、`crates/tact/src/plugin/mod.rs`（默认安装解析）；测试 `parses_codex_local_plugin_source`、`load_marketplaces_discovers_codex_personal_marketplace`、`install_from_codex_local_marketplace_resolves_relative_to_home`、`install_without_marketplace_prefers_discovered_codex_marketplace`。

---

## 1. 2026-09-10 — Seed the OpenAI Codex marketplace as a built-in

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/plugin/{model,install,hooks}.rs`；测试 `codex_manifest_accepts_string_mcp_servers_and_inline_hooks`、`parses_inline_manifest_hooks`；Ch 21 §plugin |

**症状 / 动机:** Tact 只有 `claude-plugins-official` 一个内置 marketplace。OpenAI 的 Codex 官方目录 `github.com/openai/plugins`（catalog `openai-curated`）无法开箱即用；而且其 manifest 并不内联 `mcpServers`/`hooks`——`mcpServers` 是相对文件路径 `"./.mcp.json"`，`hooks` 可能是内联对象——导致安装阶段解析失败。

**决策:** 将 `openai-curated`（源码 `https://github.com/openai/plugins.git`）注册为第二个内置 marketplace，与 `claude-plugins-official` 一样受保护、不可 replace/remove，并在 load/deserialize 时恢复。安装器与 hooks 的 manifest 解析改为同时接受“内联值”与“相对文件路径”两种 Codex 形状，缺失声明的文件不算 feature。`mcp` 特征只用于安装校验；运行期仍只连接 stdio server，远程（http/url）MCP 与 `apps` 连接器由 Tact 之外的东西消费。

**改后行为:** `plugin marketplace list` 默认显示 `claude-plugins-official` 与 `openai-curated`；`plugin install linear@openai-curated`（及不带后缀时回退到已发现 Codex marketplace）可安装 OpenAI 目录中 57 个插件（剩余为纯 `apps` 连接器——`mcpServers`/`skills` 为空）。`openai/plugins` 中的远程 HTTP MCP 插件能安装但运行期跳过其 MCP。

**指针:** `crates/tact/src/plugin/model.rs`（`OPENAI_MARKETPLACE`、`BUILTIN_MARKETPLACES`、`is_builtin_marketplace`、`builtin_record`）、`crates/tact/src/plugin/install.rs`（`PluginManifest`、`manifest_declares_file_or_inline`）、`crates/tact/src/plugin/hooks.rs`（`inline_hooks`、`load_installed_hooks`）。

---

## 1. 2026-09-09 — 最后 subagent 完成后，subagent sticky 不再残留展开

| 欄位 | 值 |
|------|-----|
| **類型** | bugfix |
| **相關** | `crates/agent_tui_kit/src/state/subagent_panel.rs`（`SubagentPanelState::apply_snapshot`）；宿主渲染 `crates/tui/src/render/task_panel.rs` |

**症狀 / 動機:** 最后一个运行中的 subagent 完成后，sticky subagent 条仍保持**展开**（`[Subagent] 0/1` + 一条 `— Completed —` 摘要行）而不是自动收起；只有用户先手动收合才消失。而 Tasks sticky 只要没有打开项就自动隐藏。

**决策:** 对齐 Tasks 规则 —— 只有当至少一个 subagent 处于 **Running** 时 subagent sticky 才可见。最后一个完成后整条隐藏（`visible = false`、`expanded = false`）。完成的运行摘要/详情不会丢失：它保留在父级 `spawn_subagent` 工具卡片 / subagent 弹窗上，而非 sticky。

**变更后行为:** 运行中的 subagent 会弹出（展开）该条；最后一个进入终态后自动收起。若多个并发运行，则直到最后一个完成才收起。sticky 的显示/时机不再依赖用户的展开/收合状态。

**指针:** `crates/agent_tui_kit/src/state/subagent_panel.rs`；对应规则 `crates/agent_tui_kit/src/state/task_panel.rs::apply_snapshot`；Ch 9 hook / agent-loop 章节提及 subagent 概览。测试：`agent_tui_kit` 状态 `subagent_panel`（`hides_when_all_done`、`stays_visible_while_other_runs_are_running`）与 `crates/tui` `sticky_host` 渲染测试。

---

## 1. 2026-09-09 — 采用 Codex 插件/生态；移除 Claude 目录兼容

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | `crates/tact/src/plugin/{install,hooks,marketplace,store,model}.rs`、`crates/tact/src/mcp/mod.rs`、`crates/tact/src/consts.rs`、`crates/tact/src/skill/mod.rs`、`crates/tact/src/config/instruction_sources.rs`、`crates/tact/src/prompt/{mod.rs,system_prompt_template.md,responses_system_prompt_template.md}`、`crates/tact/src/agent/mod.rs`；设计见 `docs/superpowers/plans/2026-09-09-codex-plugin-compat-and-memory.md`；Ch 2、3、4、8、9、12、18 |

**症状 / 动机:** Tact 长期维护两套插件生态（Claude 的 `.claude-plugin` 与 Codex 家族）。两者共享同一命令-hook 内核（子进程 + stdin JSON + `CLAUDE_PLUGIN_ROOT`），但维护两套 manifest/发现系统并不划算；Codex 布局是更干净、仍在积极维护的规范，且 agentmemory 自带 `.codex-plugin`，因此仅 Codex 也能接入它。Claude 目录兼容（`.claude-plugin`、`.claude/`、`CLAUDE.md`、`.claude/skills`）属遗留表面。

**决策:** 以 Codex 插件系统为规范，并**彻底移除**（而非 gated）Claude 目录兼容：插件 manifest 目录改为 `.codex-plugin/plugin.json`；技能根为 `.tact/skills` → `~/.tact/skills` → `~/.agents/skills`（删除 `.claude/skills`）；删除 `home_claude_dir()` / `claude_dir()`；移除 `CLAUDE.md` 指令注入，`[agent].instruction_sources` 仅接受 `agents_md`；系统提示 `# Additional context` 不再携带 claude_md 分支。**保留** `CLAUDE_PLUGIN_ROOT` 环境变量名（Codex 自己的 hook 引擎也注入它）；Claude/Anthropic 作为 **LLM provider 与 `claude-*` 模型名不受影响**；保留 `~/.agents`。

**改后行为:** 插件仅通过 `.codex-plugin/plugin.json`（+ 默认 `hooks/hooks.json`）发现。项目技能从 `.tact/skills` 加载（另含 `~/.tact/skills`、`~/.agents/skills`）；工作目录下的 `.claude/skills` 会被忽略。仅注入 `AGENTS.md` 作为指令文件；`instruction_sources` 中出现 `claude_md*` 会被拒绝。从 `.claude-plugin` 迁移的用户需改用 codex 布局。

---

## 1. 2026-09-08 — 移除权限弹窗超时；改为修复「弹窗被错过」的失同步

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tui/src/widgets/state/app/agent.rs`（`restore_pending_select_mode`）、`crates/tact/src/agent/tool_dispatch.rs`（Ask 分支的 `request_select` 等待） |

**症状 / 动机:** 中间的兜底修复（`PERMISSION_PROMPT_TIMEOUT_SECS`，300 s）两处都错了。它只限制了挂起时长，隐藏弹窗的原始失同步仍然存在。而且其副作用过大：任何搁置超过 300 s 的询问都会被静默自动 `deny`，把用户可能只是暂时离开的工具直接拒绝，且无法区分「用户说了不」与「用户不在场」。权限决定必须来自用户本人，绝不该来自时钟。

**根因:** 待处理的 `RequestSelect` 必须保持 `input_mode == Select` 才会渲染弹窗，但 `AgentUpdate::SessionStats`（以及未来任何更新）都会无条件地把 `input_mode` 重置为 `Normal`。当它在权限提示尚未应答时发生，弹窗消失但 `select.request_id` 仍被占用，等待者便永久阻塞（最初的 14 分钟 `save_memory` 挂起）。

**决策:** 两部分修复。(1) 彻底移除超时——Ask 分支重新变为无界的 `request_select().await`，其唯一终止条件是真实用户应答或 UI 关闭（两者都经 `UiResponder`：Esc → `choice: None`，UI 关闭 / channel 死亡 → `Err(Closed)`，均按 Deny）。(2) 消除使弹窗可能被错过的失同步：每次 `handle_agent_update` 之后，若 `select.request_id` 已设置但 `input_mode` 不再是 `Select`，则恢复为 `Select`（`restore_pending_select_mode`）。待处理的请求绝不会被渲染成不可见，用户始终有弹窗可应答，ACK 路径即可生效。

**改后行为:** 权限弹窗不再有任何超时。弹窗会一直显示并等待用户作答（Allow once / Always allow / Deny）或 UI 关闭——绝不会因经过时间而被自动拒绝。造成最初 14 分钟挂起的「弹窗被错过」失同步已被根除。

---

## 1. 2026-09-08 — 补齐三个 hook 以完成 agentmemory 接入（PostToolUseFailure · Notification · TaskCompleted）

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/tact/src/hook/mod.rs`、`crates/tact/src/agent/{mod,tool_dispatch}.rs`、`crates/tact/src/plugin/hooks.rs`、`crates/tact-ui/src/driver.rs` |

**症状 / 动机:** agentmemory 的自动采集插件声明了十二个 Claude Code hook 事件（`plugin/hooks/hooks.json`），但 Tact 仍缺 `PostToolUseFailure`、`Notification`、`TaskCompleted` —— 工具失败、权限提示与任务完成对记忆采集不可见。

**决策:** 按 Claude Code 语义补齐这三个事件。`PostToolUseFailure` 在工具*失败*后触发（在面向成功的 `PostToolUse` 之外），携带 `tool_name` / `tool_input` / `tool_use_id` / `error`。`Notification` 在 agent 呈现用户通知时触发——目前仅 `permission_prompt`——携带 `notification_type` / `title` / `message`。`TaskCompleted` 在每个用户任务完成时于 driver 的 `SubmitTask` 边界触发一次，携带 `task_description`（最后一条 assistant 消息）。三者均为观测性（`Block` 仅记日志并忽略），并经 `apply_plugin_hooks` 接入。

**改后行为:** Tact 现覆盖 agentmemory 的全部十二个 hook 事件（另含 agentmemory 不消费的 `PostCompact`）。工具失败、权限提示与已完成任务都会流入插件命令 hook。

---

## 1. 2026-09-08 — 新增五个生命周期 hook（SubagentStop · Stop · SessionEnd · PreCompact · PostCompact）

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/tact/src/hook/mod.rs`、`crates/tact/src/agent/mod.rs`、`crates/tact/src/compact/mod.rs`、`crates/tact/src/tool/{mod,subagent}.rs`、`crates/tact/src/plugin/hooks.rs`、`crates/tact-ui/src/{interactive,headless,driver}.rs` |

**症状 / 动机:** Tact 只映射了五个 Claude Code 风格的 hook 事件（`SessionStart`、`UserPromptSubmit`、`SubagentStart`、`PreToolUse`、`PostToolUse`），而 Codex 暴露了十二个。移植依赖 `SubagentStop`、`Stop`、`SessionEnd`、`PreCompact`、`PostCompact` 的 Codex/Claude 插件时，找不到可挂接的循环点。

**决策:** 补齐 Codex 有而 Tact 缺的五个事件，语义对齐 Codex（来源：`codex-rs/hooks/src/events/{stop,compact,session_end}.rs`）。`Stop` 在外层回合边界触发一次，`Block(reason)` 以 `reason` 作为下一条 prompt *继续*该回合（Codex continuation fragment）——是唯一一个 `Block` 反转为「继续」的事件。`PreCompact` 在压缩前触发、`Block` 否决压缩；`PostCompact` 在成功后触发；两者都按 `CompactTrigger { Auto|Manual|Recovery|Command }` 字符串做 matcher。`SessionEnd` 在拆除时触发（仅观测）。`SubagentStop` 是独立 `ToolContext` trait（与 `SubagentStart` 类似），可改写子代理 summary；所有事件都接入了插件命令层（`apply_plugin_hooks`）与 `Hook` 枚举。

**改后行为:** 插件可声明这十个事件；`Stop` 的 block 会让 agent 多跑一个回合（每任务最多 4 次续跑）；`PreCompact` 的 block 跳过压缩；其余三个仅观测。压缩调用点传入显式 `CompactTrigger`，让插件 matcher 区分 auto/manual/recovery/command。

---

## 1. 2026-09-08 — 推理模型在 thinking 模式下强制在兼容 base URL 上回放 reasoning

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/openai/responses/mod.rs`（`reasoning_replay_required`、`build_wire_request` / compact 的 reasoning 回放策略）；DeepSeek（OpenCode）provider 支持 |

**症状 / 动机:** 当 base URL 是兼容 OpenAI 但**非** `api.openai.com` 的端点（如 OpenCode Go 端点 `opencode.ai/zen/go/v1` 或 `api.deepseek.com`）时，适配器的 base-URL 启发式默认会*丢弃*历史 `reasoning` 回放以节省输入 token。但 DeepSeek 风格的推理模型在 **thinking 模式**（设置了 `thinking` 或 `reasoning_effort`）下要求下一轮把之前的 `reasoning_text` 传回；丢弃它会令 provider 返回 HTTP 400（`reasoning_text in the thinking mode must be passed back to the API`）。

**决策:** `reasoning_replay_required(request)` 在请求处于 thinking 模式**且**模型名看起来具备推理能力时（目前为小写后的模型 id 含 `deepseek` 子串）为 true。实际生效策略变为 `replay_prior_reasoning || reasoning_replay_required(...)`，因此显式的 `with_replay_prior_reasoning` 覆盖仍然生效。对普通（非 thinking）请求，仅凭 base-URL 的启发式仍会丢弃回放以省 token。

**改后行为:** 兼容 base URL 上、DeepSeek 风格模型处于 thinking 模式时，会回放历史 `reasoning` 条目，provider 不再返回 400。非 thinking 请求、以及启发式未识别的模型上的 thinking 模式，仍保持原有省 token 的默认行为。

---

## 1. 2026-09-08 — 权限弹窗加超时，工具不再无限卡在 "Running"（已被最新条目取代）

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/agent/tool_dispatch.rs`（Ask 分支的 `request_select` 等待） |

**症状 / 动机:** 一次 `save_memory` 调用在 TUI 中停留为 `Running · 829s`（14 分钟），且没有可见的授权弹窗。`save_memory` 是 `PermissionPolicy::Write`，在 Default 模式下会触发交互式 `PermissionBehavior::Ask`，向 TUI 派发 `RequestSelect` 后等待 `UiResponder` 的 oneshot。若弹窗被错过、关闭或在 UI 忙时被丢弃，工具 future 会永久阻塞，卡片的实时计时只会一直增加。权限弹窗本身是有意保留的、必须存在——缺的是用户一直不答复时的无界等待。

**决策（已被取代）:** 首次尝试用 `PERMISSION_PROMPT_TIMEOUT_SECS`（300 秒）限制每次交互式权限 `request_select` 的等待，超时自动 Deny。**同日已回退**：超时会自动拒绝用户从未应答的询问（把可能只是暂时离开的工具直接拒绝），且掩盖了而非修复了隐藏弹窗的失同步。最终根因修复见最新条目（「移除权限弹窗超时；改为修复『弹窗被错过』的失同步」）。

**改后行为:** 仅中间状态；未在任何发布中包含该超时。Ask 分支的等待重新变为无界（以真实用户应答或 UI 关闭终止），且隐藏弹窗的失同步已在源头修复。

---

## 1. 2026-09-07 — 插件 hook 的 stdin 管道断开不再吞掉 hook 的 stdout

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/plugin/hooks.rs`（`run_process` 的 stdin 载荷投递；回归测试 `run_command_hook_stdin_broken_pipe_still_honors_stdout`）；Claude Code 插件 hook 兼容（v1.1.26） |

**症状 / 动机:** 在负载较高的机器或并行测试负载下，命令 hook 若在父进程把 JSON 载荷写入 stdin 之前就结束——例如不读 stdin 的快速 `printf` 式 hook——会先关闭管道，载荷写入随即以 `Broken pipe (os error 32)` 失败。`run_process` 把*任何* stdin 写入失败都视为致命错误，返回默认 `Continue`，静默丢弃 hook 的 stdout：`block` 决策、`suppressOutput` 或 `additionalContext` 都会丢失（hook 变成 fail-open）。这表现为 `plugin::hooks` 测试（`run_command_hook_expands_plugin_root_env`、`run_command_hook_suppress_output_new_format` 等）约每十次运行失败一次。

**决策:** stdin 载荷只是尽力投递；管道断开（hook 已退出或已关闭 stdin）不算运行失败——hook 的退出码与 stdout 仍是权威结果。只有真正的非 `BrokenPipe` I/O 错误才让 hook 失败。确定性回归测试把载荷撑大到超过 OS 管道缓冲区（64 KiB），确保遇到不读 stdin 的 hook 退出时写入必然断开，与调度时序无关。

**改后行为:** 不读 stdin 就退出的 hook，其 stdout 仍会被解析（`decision` / `reason` / `additionalContext` / `suppressOutput` / 旧格式）；只有 spawn 失败、超时、非零退出、非法 JSON 以及真实 I/O 错误才回退为 `Continue` 并告警。

---

## 1. 2026-09-07 — Subagent sticky tab：Log 下方子代理总览条

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/protocol/src/agent.rs`（`SubagentRunSnapshot`、`SubagentStatusSnapshot`、`AgentUpdate::SubagentsChanged`）、`crates/tact/src/subagent.rs`（`SubagentManager.known`、`note_started`、`ui_snapshot`、`MAX_SUBAGENT_SNAPSHOT`、`emit_subagents_changed`）、`crates/tact/src/tool/subagent.rs` + `crates/tact-ui/src/driver.rs`（发射点）、`crates/agent_tui_kit/src/{state,components,render}/subagent_panel.rs`、`crates/agent_tui_kit/src/render/sticky_host.rs`（双域 host）、`crates/tui/src/render/task_panel.rs` + `handlers/{mouse,normal}.rs`；设计 `docs/superpowers/specs/2026-09-07-subagent-sticky-tab-design.md`、计划 `docs/superpowers/plans/2026-09-07-subagent-sticky-tab.md`；Ch 12、23 |

**症状 / 动机:** 后台 `run_in_background` 子代理 fan-out 缺少常驻状态总览：每个子代理的流式内容渲染在自己的父 `spawn_subagent` 工具卡里，Log 只显示已返回调用的那张卡，于是「哪些子代理还在跑 / 刚完成 / 各自说了什么」散落在多张工具卡与 `check_subagent` 中。2026-07-26 的移除提交（`98a133f`）删掉了旧 Subagent sticky pane，留下一个**状态级**表面的缺口；本次在当前组件化架构上重新实现，**不**恢复旧的 `AgentUpdate::Subagent` 包裹、也不把明细流路由进 sticky。

**决策:** 镜像 Tasks 模式，新增 `AgentUpdate::SubagentsChanged { runs }` 全量快照事件。`SubagentManager` 维护一个**进程内** `known` 集合（只记录本进程 spawn 过的 child，而非会跨会话累积并混入 orphan-repair 噪音的整张 `subagent_runs` 表），并在 spawn start / sync+async finish / `cancel_subagent` 工具 / driver `CancelSubagent` 之后发射。TUI 新增 `SubagentPanelComponent`/`SubagentPanelState`（kit），Log 下方 sticky 升级为双域 host `[Tasks] [Subagent]`；每个域独立维护 visible/expanded/scroll。Subagent body 分组 Running → Completed → Failed → Cancelled，行格式 `{marker} {短id} {摘要首行} ⏱ {耗时}`；数量封顶（`MAX_SUBAGENT_SNAPSHOT = 20` 总数，Running 全保留）。明细仍留在工具卡 / SubagentPopup。

**改后行为:** 当前进程内任何子代理启动/结束时都会刷新 sticky（无可见域则整条隐藏；首次出现默认展开；收起且无 Running 后折叠为一行并最终隐藏）。点击可见的非活动 tab 会切换活动域并展开；滚轮/`jk` 滚动活动域。子代理永不进入主 Log（一行 = 工具卡），也绝不混入 Tasks。

---

## 1. 2026-09-07 — 重新接入 OpenCode Go `x-opencode-session` 头（绑定会话）

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/opencode.rs`（重建；`endpoint_headers(base_url, session)`）、`crates/tact_llm/src/openai/responses/mod.rs`（`OpenAiResponsesAdapter::set_session_id`、`ResponsesCompatConfig.opencode_session`、compact POST）、`crates/tact_llm/src/openai/compatible/mod.rs`（`OpenAiAdapter::set_session_id`、`request_headers`、`CompatibleConfig::headers`）、`crates/tact_llm/src/openai/compatible/multi_model.rs`（`ChatCompletionsAdapter::set_user_id` 转发会话）、`crates/tact_llm/src/models.rs`（`fetch_model_ids`）、`crates/tact_llm/src/client.rs`（`LlmProvider::set_user_id`）；Ch 21 |

**症状 / 动机:** OpenCode 发布的 OpenCode Go（`https://opencode.ai/zen/go/v1`）客户端要求再次要求各类 coding-agent 客户端自报身份，并在每个请求里发送**稳定的会话 id**（`x-opencode-session`，用于路由与 prompt 缓存）；会话支持不全的客户端会被列入 "Known Problematic"。09-05 的 `revert(llm)`（ed480ec）在该头还属可选时把整套机制（绑定会话的头 + `tact/<version>` User-Agent）删掉了，于是 OpenCode Go 请求又变成裸奔。

**决策:** 重建 `opencode` 辅助模块，并重新挂载这些头：Responses SDK config（普通 `/responses` 的 `create_byot` / `create_stream_byot`）、直连 `/responses/compact` POST、**Chat Completions 传输层**（`OpenAiAdapter::request_headers`，现与会话感知）、以及 `/v1/models` 选择器拉取。取值**只由 Tact session id 填充**：`Agent::with_session` → `LlmProvider::set_user_id` → 各 adapter 保存之（Responses 走 `OpenAiResponsesAdapter::set_session_id`；Chat Completions 走 `ChatCompletionsAdapter::set_user_id` 转发到 `OpenAiAdapter::set_session_id`），因此一个 Tact 会话（含 resume 复用的会话、以及每个子代理自己的 child session）在任一协议下都恰对应一个 OpenCode 会话/缓存。与 09-02 的设计不同，这里**没有按 `base_url` 的 fallback token，也没有 `TACT_OPENCODE_SESSION` 环境变量覆盖**——头值要么是 `session_id` 原文、要么不发送。无会话的请求（如早于 `with_session` 的 `/v1/models` 选择器拉取）省略 `x-opencode-session`，只发送标识用的 `tact/<version>` `User-Agent`。端点识别与删除前一致（`opencode.ai` 或其子域）。

**改后行为:** 发往 OpenCode Go 端点的会话请求，无论走 Responses 还是 Chat Completions 协议，都带 `x-opencode-session`（等于 Tact session id）与 `tact/<version>` `User-Agent`；无会话的请求（模型选择器拉取）省略会话头但仍以 User-Agent 自报身份；非 OpenCode 端点不加额外头。这是对 09-05 删除设计的重新接入——下方条目保留作历史记录。

---

## 1. 2026-09-06 — 子代理技能卡：经 `skill` 注入隔离角色

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/tact/src/tool/subagent.rs`（`SubagentInput.skill`、`parse_skill_card_frontmatter`、`read_skill_card`、`list_skill_cards`、`format_skill_card_line`、`apply_skill_card`、`annotate_spawn_subagent_skill_catalog`）、`crates/tact/src/tool/mod.rs`（`ToolRouter::set_tool_description` description override）、`crates/tact-ui/src/{interactive,headless}.rs`；设计 `docs/superpowers/specs/2026-09-06-subagent-skill-cards-design.md`、计划 `docs/superpowers/plans/2026-09-06-subagent-skill-cards.md`；Ch 12 §2.1 |

**症状 / 动机:** 移除声明式 agent definitions（`8c74f4e`）后，子代理缺少复用角色/工作方法的途径——每种专业 worker 身份都只能在 `prompt` 里临时手写。复用主 agent 的 `SkillRegistry` 被否决：会污染主 agent 可见的 skill 列表并使两套系统纠缠。

**决策:** 给 `spawn_subagent` 增加一个隔离、可选的 `skill: <name>` 字段。handler 读取 `~/.tact/subagent/<name>.md`（key = 文件 stem；frontmatter 的 `description` 用于报错列表与发现清单展示），将其正文以 `<skill>` 块追加到子代理静态 system prompt（在 `SubagentStart` hooks 之前）。名字必须是纯文件 stem（无分隔符 / `.`/`..` / NUL），因此 `skill` 值无法读到技能卡目录之外。未知名使 spawn 失败并列出可用卡。不新增 `ToolContext` 状态、不触及 `SkillRegistry`、不引入 tools/model/permission frontmatter 语义。

**改动后行为:** `spawn_subagent { prompt, skill: "reviewer" }` 让子代理获得写入 system prompt 的稳定角色（每轮都在、压缩后不丢）；不传 `skill` 则与之前完全一致（通用模板 + 五件套）。技能卡目录与主 agent skills 物理隔离。会话启动时 `spawn_subagent` 工具描述会附加可用卡清单（单行、description 按 60 字符截断、上限 30 张），使主 agent 能发现合法 `skill:` 名；目录缺失/为空则描述保持不变。符号链接的卡在清单与读取间行为一致。

---

## 1. 2026-09-06 — 移除声明式 subagent 定义（agent definitions）

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | 删除 `crates/tact/src/agent_def.rs`；`crates/tact/src/tool/subagent.rs`（`SubagentInput.agent`、`resolve_agent_model`、spawn 的 prompt/权限/model/工具集覆盖）、`crates/tact/src/tool/registry.rs`（`subagent_toolset_for`、`allowed_tool_names`、过滤工具集构造器）、`crates/tact/src/tool/mod.rs`（`ToolContext.agent_registry`）、`crates/tact/src/consts.rs`（`TactPath::agents_dir`）、插件功能记账（`crates/tact/src/plugin/{model,install}.rs` 的 `InstalledPlugin.agent_count`、`PluginFeatures.agent_count`）、`crates/tact-ui/src/plugin_cli.rs`、`crates/tui/src/widgets/state/app/extensions.rs`、`crates/agent_tui_kit/src/i18n.rs`（`plugin_list_header`）；Ch 7、12、21 |

**症状 / 动机:** `spawn_subagent` 带着一条「声明式 agent 定义」路径——`<workdir>/.tact/agents/*.md` 与已安装插件 `agents/*.md`（命名空间 `plugin:<name>`）里的 Markdown+YAML-frontmatter 文件——可以替换子代理 system prompt 并覆盖其工具集、模型与权限模式。实现面与价值不成比例：frontmatter 解析器、共享注册表（`Arc<Mutex>` + 本地名歧义消解）、第二套工具集构造器（`subagent_toolset_for` + Claude 名映射，配 fail-closed 的空 router 护栏）、叠在 `[agent.subagent]` 之上的第三层 model 覆盖、安装期 `agent_count` 记账，以及两处插件列表 UI——而这一切服务的 worker，用一个固定五件套上的纯 prompt 就能同样好地完成。

**决策:** 端到端移除该功能。`spawn_subagent` 恒用默认静态 system prompt（"You are a coding subagent at …"）与 `subagent_toolset()`；删除 `SubagentInput.agent`、`resolve_agent_model`、整个 `agent_def` 模块、`ToolContext.agent_registry` 字段与 `subagent_toolset_for`/`allowed_tool_names`/过滤构造器三件套。插件不再统计或宣传 `agents/*.md`：`agent_count` 从 `InstalledPlugin`/`PluginFeatures`、`tact plugin list` 与 `/plugin` 表格（本地化 `plugin_list_header` 去掉对应列）移除；插件 `SubagentStart` hooks 与 skill/command/hook/MCP 插件功能不受影响。仅含 `agents/` 的插件如今没有任何可安装功能，安装时被拒绝。

**改动后行为:** `spawn_subagent` 只接受 `prompt`/`description`/`run_in_background`/`max_turns`/`resume`/`worktree`；所有子代理都跑在固定五件套与通用 prompt 上。按 worker 的定义级覆盖消失；`[agent.subagent]` 配置块与 `/model-subagent` 仍全局设定子代理的 provider/model。插件功能摘要只显示 `skills/commands/hooks/mcp`。

---

## 1. 2026-09-05 — 移除 OpenCode `x-opencode-session` 头接线

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | 移除 `crates/tact_llm/src/opencode.rs`；`crates/tact_llm/src/openai/responses/mod.rs`（`OpenAiResponsesAdapter`、`ResponsesCompatConfig`）、`crates/tact_llm/src/openai/compatible/mod.rs`（`CompatibleConfig::headers`）、`crates/tact_llm/src/models.rs`（`fetch_model_ids`）、`crates/tact_llm/src/client.rs`（`LlmProvider::set_user_id`）；Ch 21 |

**症状 / 动机:** OpenCode 托管端点不再要求 `x-opencode-session` 头（也不需要自定义 `tact/<version>` User-Agent），因此整套「检测 `opencode.ai` 端点并为每个请求附加该头」的机制成为死代码——而且它迫使把 Tact session id 一路透传（`Agent::with_session` → `LlmProvider::set_user_id` → `OpenAiResponsesAdapter::set_session_id`）仅仅是为了填充这一个头的值。

**决策:** 整体移除 OpenCode 会话逻辑：删除 `opencode` 辅助模块，并停止在 Responses SDK 配置、直接 `POST /responses/compact`、Chat Completions 配置以及 `/v1/models` 选择器拉取上附加 `x-opencode-session` / `tact/<version>`。从 Responses adapter 移除 `session_id`/`opencode_session` 字段与 `set_session_id`，去掉 `LlmProvider::set_user_id` 的 `OpenAiResponses` 分支，并删除 `TACT_OPENCODE_SESSION` 环境变量覆盖。

**改动后行为:** 任何端点的请求都不再携带 `x-opencode-session` 头（也无自定义 OpenCode User-Agent）；`opencode.ai` 端点与其它 OpenAI 兼容 base URL 一视同仁。DeepSeek 经 Chat Completions adapter 的 `user_id` KV 缓存隔离不受影响。

---

## 1. 2026-09-05 — 兼容 `/responses` 端点不再回放历史 reasoning item

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact_llm/src/openai/responses/convert.rs`（`ResponsesRequestPolicy`、`create_response_with_policy`）、`crates/tact_llm/src/openai/responses/mod.rs`（`OpenAiResponsesAdapter::with_replay_prior_reasoning`、`is_official_openai_base_url`、compact POST）；task #58；Ch 22 §6.2/§6.2.3 |

**Symptom / motivation:** 每个 `/responses` 请求都把完整的历史 `reasoning` item（持久化的 opaque encrypted payload signature）重放进 `input`。官方 OpenAI 需要它来延续 turn，但兼容端点（OpenCode Go、自定义 OpenAI-compatible 代理）每轮都会自行重新生成 reasoning，回放每一条历史思维链纯属浪费 input token——实测约占请求字节的 ~17%（DeepSeek 类端点按 token 计高达 ~47%）。

**Decision:** 增加 per-adapter 的 `replay_prior_reasoning` 策略。`OpenAiResponsesAdapter::new` 按 base URL 推导默认值（`is_official_openai_base_url`：`api.openai.com` / Azure OpenAI → 回放；其他一切 → 丢弃）；`with_replay_prior_reasoning` 可覆盖。`create_response` 改为 `create_response_with_policy(request, provider_state, compact_threshold, ResponsesRequestPolicy { native_web_search, replay_prior_reasoning })`。reasoning signature 在每条路径上仍会被解码，使 `fc_*` function-call item id 保持挂在其 `function_call` item 上；只是省略独立的 `reasoning` 载荷。回放被关闭时，构造请求前会先从持久化状态基线中过滤掉过期的 `reasoning` item，返回的 `input_items` 也不含 reasoning，因此旧版本写入的 state 会在升级后的第一次请求自愈。

**Behavior after:** 官方 OpenAI（及 Azure OpenAI）的 Responses 请求与之前完全一致地回放先前 reasoning；其他任何 base URL 都会从普通请求、显式 `/responses/compact` 请求体与持久化状态基线中丢弃历史 reasoning item，在保留 function-call identity 的同时削减约 17% 的 input 字节。对语义与其主机名不同的端点，提供显式覆盖开关。

---

## 1. 2026-09-02 — OpenCode `x-opencode-session` 绑定 Tact session id（缓存隔离）

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/opencode.rs`（`endpoint_headers(base_url, session)`）、`crates/tact_llm/src/openai/responses/mod.rs`（`OpenAiResponsesAdapter::set_session_id`、`ResponsesCompatConfig.opencode_session`、compact POST）、`crates/tact_llm/src/client.rs`（`LlmProvider::set_user_id`）、`crates/tact/src/agent/mod.rs`（`Agent::with_session`）；Ch 21 |

**Symptom / motivation:** 第一版 OpenCode 修复以按进程、按 `base_url` 的 token 作为 `x-opencode-session`。但 OpenCode 用该头作为**区分各会话缓存的键**：两个不同 Tact 会话共享同一 token 会共享/污染彼此的 OpenCode 缓存，而 resume 一个 Tact 会话也不会续上同一 OpenCode 缓存。

**Decision:** 把头绑定到 Tact session id。`Agent::with_session` 本就把 session id 经 `LlmProvider::set_user_id` 转发给 client（DeepSeek KV 缓存隔离所用的同一钩子）；OpenAI Responses adapter 现在保存它（`set_session_id`），SDK 配置与 compact POST 发出 `x-opencode-session = <session id>`。无会话的请求（如 `/v1/models` 选择器拉取）仍回退到按 `base_url` 的 token；`TACT_OPENCODE_SESSION` 只固定该回退值。

**Behavior after:** 一个 Tact 会话（含 resume —— 复用同一 session id）恰好对应一个 OpenCode 会话/缓存；不同 Tact 会话 —— 主 agent、以及各自带子 session id 的每个子代理 —— 得到不同的 `x-opencode-session` 值，因此 OpenCode 缓存按会话隔离。

---

## 1. 2026-09-02 — OpenCode Go 端点发送 `x-opencode-session` 与真实 User-Agent

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/opencode.rs`（新增）、`crates/tact_llm/src/openai/responses/mod.rs`（`ResponsesCompatConfig::headers`、compact POST）、`crates/tact_llm/src/openai/compatible/mod.rs`（`CompatibleConfig::headers`）、`crates/tact_llm/src/models.rs`（`fetch_model_ids`）；Ch 21 |

**Symptom / motivation:** 发往 OpenCode Go（`https://opencode.ai/zen/go/v1`）的请求没有 `x-opencode-session` 头，且只有 reqwest 的通用 `User-Agent`，导致 OpenCode 无法关联/优化会话并把这些请求报为「Unknown client」；09/06 起缺该头的请求可能报错。

**Decision:** 新增 `opencode` 辅助模块，检测 OpenCode 端点（`opencode.ai` 或其子域）并返回按进程与 `base_url` 稳定的 `x-opencode-session` 值，外加 `tact/<version>` 的 `User-Agent`。头附加在访问该端点的所有路径上：Responses SDK 配置（`create_byot` / `create_stream_byot`）、直接 `POST /responses/compact`、Chat Completions 配置（防御性）以及 `/v1/models` 选择器拉取。设置 `TACT_OPENCODE_SESSION` 可固定会话值。

**Behavior after:** 对 OpenCode Go 端点的每个请求都携带稳定的 `x-opencode-session` 头与标识性的 `tact/<version>` `User-Agent`；其他端点不受影响（空头映射）。

---

## 1. 2026-09-02 — 移除嵌套子代理 spawn（恢复 depth-0）

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | `crates/tact/src/tool/registry.rs`（`subagent_toolset` 恢复 5 工具）、`crates/tact/src/tool/mod.rs`（移除 `ToolContext.subagent_depth`）、`crates/tact/src/tool/subagent.rs`（移除 `MAX_SUBAGENT_DEPTH`）；回滚 `6c32665e`；Ch 12 |

**Symptom / motivation:** 嵌套 spawn 功能（提交 `6c32665e`，深度限制 `MAX_SUBAGENT_DEPTH = 3`，9 工具子代理集）当日已交付，但设计对价值而言过于复杂：子代理需在 `ToolContext` 携带 `subagent_depth` 状态、router 需映射 Claude `Task` 名、每个子代理都要传递深度——这一切只是为了 worker 再 spawn worker。

**Decision:** 回滚嵌套 spawn 提交。`subagent_toolset()` 恢复恰好五个工具（`bash`、`read_file`、`write_file`、`edit_file`、`sleep`），`spawn_subagent`/`check_subagent`/`wait_subagent`/`cancel_subagent` 重新仅属主 toolset，删除 `ToolContext.subagent_depth` 与 `MAX_SUBAGENT_DEPTH`，`allowed_tool_names` 移除 Task/Check/Wait/Cancel 映射。resume 24h 过期与 worktree 改进（分支清理、run 校验/审计、索引对账）不受影响。

**Behavior after:** 子代理不能 spawn 嵌套子代理（与 2026-09-02 之前相同的 depth-0 契约）；主 agent 保留完整子代理工具面；声明式 `tools:` 列表只能收窄五件套。

---

## 1. 2026-09-02 — 收尾异步子代理遗留项：resume 过期、worktree 分支清理、索引对账、run 校验/审计

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/tool/subagent.rs`（`RESUME_EXPIRY_HOURS`）、`crates/tact/src/worktree/mod.rs`（`new` orphan 修复、`remove` 分支清理、`run` 校验 + 审计）、`crates/tact/src/tool/worktree.rs`；Ch 12、15 |

**Symptom / motivation:** 异步子代理后续完成后仍有四个设计延期的遗留：(1) `resume` 没有过期策略（2026-08-26 设计标 "TBD"），可能带着过期上下文 resume 数周前的 session；(2) `worktree_remove` 总是保留 backing 分支 `wt/<name>`，需手动 merge 或 `git branch -D`；(3) 手动 `git worktree remove`/`prune` 会永久留下陈旧 DB 记录（索引漂移）；(4) `worktree_run` 绕过 `validate_shell_command` 且不记审计日志。

**Decision:** (1) `resume` 拒绝 `finished_at` 距今超过 `RESUME_EXPIRY_HOURS = 24` 的目标（Claude 的 24h 过期）。(2) `WorktreeManager::remove` 在 `git worktree remove` 后运行 `git branch -d wt/<name>`——仅删除完全合并的分支；未合并分支保留并上报/审计结果。(3) `WorktreeManager::new` 修复 orphan：路径缺失的跟踪泳道从表删除并记为 `worktree.stale-removed`。(4) `WorktreeManager::run` 调用 `validate_shell_command`（与 `bash` 同一门槛），并向审计日志追加 `worktree.run <name> <command>`。

**Behavior after:** resume 24h 过期；`worktree_remove` 只自动删除已合并分支；worktree 索引在启动时与 git 对账；`worktree_run` 拦截高风险命令且每次调用都被审计。

---

## 1. 2026-09-02 — 异步子代理 P1 可靠性修复（唤醒竞态、取消状态、resume 校验、同步生命周期、headless 语义）

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-ui/src/driver.rs`（`run_command_loop_with_account` + `spawn_wakeup_task`）、`crates/tact/src/tool/subagent.rs`（`spawn_subagent`、`terminal_success`）；Ch 12 |

**Symptom / motivation:** 异步子代理路径存在五个 P1 可靠性缺口。(1) **唤醒竞态** —— driver 在任一轮进行中时直接丢弃 `SubagentFinishedNotification`，导致落在「最后一次队列 drain 与轮次退出之间」的结果永远不会被回注（父级要等到下一次手动 turn 才恢复）。(2) **取消状态** —— 异步完成任务用原始 `agent_loop` 结果发 `AgentUpdate::SubagentFinished`，因此一个干净退出的被取消子代理被上报为 `success: true`。(3) **resume 校验** —— `resume` 盲目复用任意 id：复用仍 `Running` 的子代理会与 session 竞争，复用未知 id 会静默新建子代理而非追加。(4) **同步子代理生命周期** —— 同步子代理注册了 cancel handle 但从不写 `subagent_runs` 行，导致 `check_subagent`/`cancel_subagent` 看不到它们，且失败的同步 spawn 会遗留陈旧的 `Running` 行。(5) **headless 语义** —— headless 下的 `run_in_background` 会 spawn 一个脱钩子代理，随后在退出时被取消，静默丢弃其工作。

**Decision:** (1) driver 循环改为对在途 `JoinHandle` 与 `user_cmd_rx` 做 `select!`，保留 `pending_subagent_wakeup` 标志，在活动轮次一完成就提交唤醒轮（新增 `spawn_wakeup_task` 辅助函数；`SubmitTask` 清除标志，因为新一轮会自行 drain 队列）。(2) 用 `terminal_success(success, cancelled) = success && !cancelled` 同时生成入队的 `SubagentResult` 与 `SubagentFinished` 事件。(3) resume 通过 `SubagentManager::get` 校验：目标仍 `Running` 或 id 未知时在任何 spawn 工作前即 bail。(4) 将 `manager.start` 移到同步/异步分支之上，同步路径在退出时记录 `Completed`/`Failed`/`Cancelled`（并在传播错误前注销 handle、释放锁）。(5) `run_async` 要求 `ctx.ui_tx.is_some()`；没有交互通道（headless）时 `run_in_background` 退化为同步并 `warn!`，summary 仍能到达父级。

**Behavior after:** 子代理结果不再因唤醒间隙而丢失；被取消的子代理在队列与卡片中都按不成功呈现；resume 拒绝运行中/未知目标；同步与异步子代理统一对 `check_subagent`/`cancel_subagent`/`wait_subagent` 可见；headless 的 `run_in_background` 同步完成，而非在退出时被取消。

---

## 1. 2026-09-02 — 异步子代理后续：`wait_subagent` + `worktree_remove` + 并发弹窗

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/subagent.rs`（`SharedSubagentManager::wait` / `get`）、`crates/tact/src/tool/subagent.rs`（`WaitSubagentTool`）、`crates/tact/src/tool/worktree.rs`（`WorktreeRemoveTool`）、`crates/tact/src/worktree/mod.rs`（`WorktreeManager::remove`）、`crates/tact/src/store/worktree_store/`（`remove_worktree`）、`crates/tact/src/tool/registry.rs`、`crates/tui/src/widgets/state/{mod.rs,app/popups.rs,app/config.rs,app/construct.rs}`、`crates/tui/src/{handlers,render}`；Ch 12 |

**Symptom / motivation:** 2026-08-26 异步子代理设计已落地 `run_in_background`、`check_subagent`、`resume` 与取消，但遗留三项：其一，父级只能跨多轮调用 `check_subagent` 才能得知运行中子代理的结果——浪费回合；其二，隔离 worktree 泳道（`subagent-<child_id>`）无删除入口，只能手动 `git worktree remove` 清理；其三，TUI 仅持单个 `Option<SubagentPopup>` 槽位，打开第二个并发子代理的 transcript 会丢掉前一个的滚动/选中态。

**Decision:** (1) `wait_subagent { child_id, timeout_ms? }` —— 新增 Read 工具，轮询 `subagent_runs`（250 ms 间隔）直到子代理到达 `Completed`/`Failed`/`Cancelled` 或超时（默认 60 s），返回 summary；即 Codex `wait_agent` 的对应物（新增 `SubagentManager::wait` / `get`）。(2) `worktree_remove { name }` —— 执行 `git worktree remove`（不加 `--force`，脏工作树会失败）、删除跟踪记录、追加审计事件，并保留 backing 分支 `wt/<name>` 以便未合并提交可恢复；拒绝删除仍在 `Running` 的 `subagent-<id>` 泳道（新增 `WorktreeStore::remove_worktree` + `WorktreeManager::remove`）。(3) TUI 多弹窗 —— `App.subagent_popup: Option<_>` 改为 `subagent_popups: HashMap<tool_id, _>` + `active_subagent_popup: Option<tool_id>`；`open_subagent_popup` 对每张卡片 insert-or-reuse，切换并发子代理时保留各自的滚动/选中/布局缓存。

**Behavior after:** 父级可先 spawn N 个后台子代理再逐个 `wait_subagent`（不再跨回合轮询 `check_subagent`）；隔离泳道可通过工具清理；并发子代理的 transcript 不再互相覆盖弹窗状态。

---

## 1. 2026-08-30 — 子代理取消（`cancel_subagent` 工具 + `/subagent_cancel` + tool 卡片 [Cancel] 按钮）

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/subagent.rs`（`register_cancel_handle` / `request_cancel` / `unregister_cancel_handle`）、`crates/tact/src/tool/subagent.rs`（`CancelSubagentTool`、spawn 注册/注销 handle、取消感知结束路径）、`crates/tact/src/tool/registry.rs`、`crates/protocol/src/agent.rs`（`UserCommand::CancelSubagent`）、`crates/tact-ui/src/driver.rs`、`crates/tui/src/handlers/{mod,mouse}.rs`（`/subagent_cancel`、按钮点击）、`crates/agent_tui_kit/src/{components/tool.rs,render/log.rs,state/tool_state.rs,state/mouse_state.rs,i18n.rs}`（`parse_async_launched`、`SubagentCancelButton`、`subagent_child_id`）；Ch 12 |

**Symptom / motivation:** 运行中的后台子代理没有任何取消入口：父级 `/cancel` 只取消主任务（每次 `Agent::new` 为子代理新建独立 cancel_flag，父进程拿不到句柄）；`SubagentManager::cancel` 只能事后改 DB 状态，无法中止运行中的子代理；异步子代理 detach 后只能等 `max_turns` 或自然结束。

**Decision:** 建立协作取消链路：`SubagentManager` 增加内存 cancel-handle 注册表（`child_id → Arc<AtomicBool>`）；`spawn_subagent` 在 `Agent::new` 后注册子代理的 `runtime.cancel_flag`，结束（同步/异步）时注销；新增 `cancel_subagent` 工具（`request_cancel` 翻转标志 + 标记记录），注册进主 toolset；protocol 新增 `UserCommand::CancelSubagent`，driver 直接操作 manager（不依赖父 agent 空闲）；TUI 新增 `/subagent_cancel <child-id>` slash 命令与运行中子代理卡片上的 `[Cancel]` 按钮（`parse_async_launched` 从 `async_launched { id }` 结果提取 child_id，渲染层返回按钮 rect，mouse 点击发送命令）。异步结束路径检测标志：被取消的运行标为 `Cancelled`（而非 Completed），summary 前缀 `(cancelled by user)`。

**Behavior after:** 运行中的后台子代理可通过三种入口取消：`cancel_subagent { child_id }` 工具（模型可调用）、`/subagent_cancel <child-id>`（用户）、live 卡片 `[Cancel]` 按钮（鼠标）。子代理循环在下一个检查点协作退出，运行记录标为 Cancelled，结果回注时 success=false。**父级退出联动**：driver 循环收尾与 headless 运行结束都会调用 `SubagentManager::cancel_all()`，翻转所有存活子代理的取消标志，避免后台子代理成为孤儿。

---

## 1. 2026-08-29 — Claude marketplace 插件全功能兼容（skills / commands / agents / hooks / MCP）

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/plugin/{install,model,store,hooks}.rs`, `crates/tact/src/skill/mod.rs`（`load_plugin_commands`）、`crates/tact/src/mcp/mod.rs`（`installed_plugin_mcp_servers`、`McpProjectConfig`）、`crates/tact/src/agent_def.rs`（声明式 agents）、`crates/tact/src/tool/{subagent,registry}.rs`（`agent` 字段、`subagent_toolset_for`）、`crates/tact/src/hook/mod.rs`（`UserPromptSubmit`）、`crates/tact/src/agent/mod.rs`（`apply_user_prompt_hooks`、`with_post_tool_hook`）、`crates/tact-ui/src/{interactive,headless}.rs`、`crates/tui/src/widgets/state/app/extensions.rs`；设计 `docs/superpowers/specs/2026-08-29-claude-plugin-compat-design.md`、计划 `docs/superpowers/plans/2026-08-29-claude-plugin-compat.md`；Ch 2、8、9、12、21、23 |

**Symptom / motivation:** Tact 的插件只消费 `skills/` 一项，且安装校验硬性要求 `skills/*/SKILL.md`，导致官方 claude-plugins-official 中 15+ 无 skills 的插件（LSP 文档类、`commit-commands`、`code-review` 等）无法安装；`commands/*.md`、`agents/*.md`、plugin.json hooks、`.mcp.json` 全部被忽略（ponytail 的 hooks 完全没跑）。

**Decision:** 以 Claude Code 插件契约为准做五类兼容：
1. **安装/清单** — 校验放宽为"至少一种受支持功能"（skills/commands/agents/hooks/mcp）；完整解析 plugin.json（name/description/version/author/hooks/mcpServers）；`InstalledPlugin` 新增 `command_count`/`agent_count`/`has_hooks`/`has_mcp`（serde default，旧记录兼容）。
2. **Commands** — `commands/*.md` 加载为 `plugin:<name>` 技能（Claude：与 skills 加载方式相同），同一插件内命令覆盖同名技能；frontmatter 解析 `argument-hint`/`allowed-tools`/`model`（v1 不强制）。
3. **MCP** — 扫描已安装插件缓存的 `.claude-plugin/plugin.json` `mcpServers` 与插件根 `.mcp.json`，服务器命名 `plugin__<id>__<server>`；`http`/`url` 类型跳过并告警（客户端仅 stdio）。
4. **Agents** — 新 `agent_def` 注册表加载 `.tact/agents/*.md`（原名）与插件 `agents/*.md`（`plugin:<name>`）；`spawn_subagent` 新增 `agent` 字段：定义正文作 system prompt，`tools` 过滤子代理工具集（Read/Glob/Grep→read_file, Bash→bash, Edit→edit_file, Write→write_file, Sleep→sleep），`model`/`permissionMode` 覆盖（Auto 保持粘性）。
5. **Hooks** — 新 `plugin/hooks.rs`：解析 Claude hooks JSON（matcher/command/commandWindows/timeout/statusMessage/async），`run_command_hook` 以 `sh -c` 执行并注入 `CLAUDE_PLUGIN_ROOT`/`CLAUDE_PROJECT_DIR`，stdin JSON 负载、stdout 双格式解析（`decision` 与 `hookSpecificOutput`）；失败/超时/非法 JSON → warning + Continue（fail-open）。`Hook` 新增 `UserPromptSubmit`（agent_loop 入口对用户消息追加 `additionalContext`）；`SubagentStart` 作为独立 trait 存于 `ToolContext.subagent_start_hooks`（spawn 路径无 LoopState），在 `spawn_subagent` 中注入子代理 system prompt。

**Behavior after:** 官方 marketplace 全部 39 个插件可安装；`/plugin:commit` 等命令即装即用；带 agents 的插件可通过 `spawn_subagent agent=…` 使用；带 hooks 的插件（如 ponytail、官方 hookify/security-guidance 等）在会话启动 / 用户提交 / 工具调用前后 / 子代理启动时执行命令 hook；插件 `.mcp.json` stdio 服务器出现在 `mcp__` 工具集；`tact plugin list` 与 TUI `/plugin list` 显示功能摘要。

**Follow-up fixes (2026-08-30):** hooks 默认发现路径 `hooks/hooks.json`（manifest 缺省时回退，官方 6 个 hooks 插件受益）；声明式 agents 的 `model` 别名处理（`inherit` 不覆盖、`sonnet/opus/haiku` 警告忽略、具体 id 透传）；`tools:` 全部无法映射时报错而非回退默认五件套（防权限扩大）；UserPromptSubmit `Block` 真正阻止提交、SubagentStart `Block` 传播到 `spawn_subagent` 使其失败；SubagentStart 输入补 `agent_type` 字段（ponytail matcher 依赖）；`timeout: 0` 表示不设超时；新格式 `suppressOutput` 解析；SessionStart 纯文本输出显式告警。

**Limitations (v1):** Python SDK `tools/`、http/url MCP、`Notification`/`Stop`/`SubagentStop`/`PreCompact`/`PostCompact`/`SessionEnd` 事件、SessionStart `systemPrompt` 输出、skills `allowed-tools`/`model` 强制执行均不支持（见设计文档 §2）。

---

## 1. 2026-08-29 — slash / select 弹窗长列表滚动（选中项始终可见 + 鼠标滚轮）

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tui/src/render/popups/slash_command.rs`（滚动窗口 / offset）、`crates/tui/src/widgets/state/slash_command.rs`（`step_slash_selection`）、`crates/agent_tui_kit/src/widgets/select_popup_widget.rs`（`select_popup_layout`、footer）、`crates/agent_tui_kit/src/render/popups/select.rs`、`crates/agent_tui_kit/src/render/popups/system_prompt_popup.rs`（session-stats footer）、`crates/agent_tui_kit/src/i18n.rs`（footer 文案）、`crates/tui/src/render/popups/select.rs`（鼠标区域）、`crates/tui/src/handlers/mouse.rs`（滚轮路由）、`crates/tui/src/handlers/insert.rs`（↑/↓ 复用）、`crates/agent_tui_kit/src/state/mouse_state.rs`（`slash_popup_area`、`select_popup_area`）；Ch 23 |

**Symptom / motivation:** 列表过长时弹窗看起来没有滚动效果。slash 弹窗滚动窗口固定按 `max_visible + 2` 计算，没有考虑弹窗真实内容高度（`area.height - 2`）；在矮终端上 `List` 组件会裁掉底部行，而选中项锚定在窗口索引 `max_visible - 1` —— 落在可见内容之外，按 ↑/↓ 时高亮跑到屏幕外、可见行看起来像冻结。select 弹窗（`/model`、permission、ask_user）有同样问题：其 List 区块按 `Constraint::Length(option_count)`（完整选项数）分配，即使弹窗高度已被上限截断，选中项落到弹窗边框之外、列表从不滚动。鼠标滚轮在两类弹窗上同样无效：它们都不属于 overlay 弹窗，滚轮事件落到背后的日志面板。

**Decision:** (1) 两个弹窗的滚动窗口都改为按实际可用高度计算（slash：`min(rows, max_visible + 2, area.height - 2)`；select：共享 `select_popup_layout(state, area, fg)` 助手），窗口与弹窗高度始终一致、不再裁切；offset 保证选中项始终落在窗口内（列表溢出后固定在大约倒数第 2 行，保留正常尺寸下的原有锚点），任何终端高度下高亮都可见。(2) `render_slash_command_popup` 与 select 包装函数改为接收 `&mut App`，每帧渲染并把自身矩形记录到 `MouseState::slash_popup_area` / `select_popup_area`（弹窗关闭时清空）；`handle_mouse_event` 把落在这些矩形内的 `ScrollUp`/`ScrollDown` 路由到选中项。(3) slash 的 ↑/↓ 与滚轮共用同一个 `App::step_slash_selection(delta)` 助手；select 复用 `SelectPopup::move_up`/`move_down`。(4) select 弹窗新增底部边框导航提示（与 code/mermaid 弹窗同款样式：按键用 accent、说明用 muted，居中渲染在 `title_bottom`）——`↑↓/j/k` 选择、`Enter` 确认、`Esc` 取消，多选再加 `Space` 勾选；弹窗宽度自动加宽以容纳提示，替代原来硬编码在内容区的 "Space toggle · Enter confirm" 行（顺带为选项多腾出一行内容高度）。(5) `/stats` 会话统计弹窗（复用 `/view-system-prompt` 的 `SystemPromptPopup`）也加上同样的底部边框提示（`j/k` 滚动 · `Esc` 关闭），与其他所有可滚动弹窗一致。

**Behavior after:** 任意终端高度下选中命令/选项始终可见；slash 或 select 长列表可用 ↑/↓ 或鼠标滚轮平滑滚动（弹窗上的滚轮移动选中项，不再滚动背后日志）；弹窗关闭后清空记录的鼠标区域；select 弹窗底部边框显示导航按键提示；`/stats` 会话统计弹窗同样显示 `j/k 滚动 · Esc 关闭` 底部提示。

---
## 2. 2026-08-27 — 子代理 worktree 隔离（`worktree: true`）+ 隔离子 spawn 的同 wave fan-out

| Field | Value |
|-------|-------|
| **类型** | feature |
| **相关** | `crates/tact/src/tool/subagent.rs`（`SubagentInput.worktree`、`ensure_subagent_worktree`）、`crates/tact/src/agent/tool_dispatch.rs`（`tool_resources_for`）、`crates/tact/src/worktree/mod.rs`（`get`）、`crates/tact/src/tool/mod.rs`（再导出）；计划 `docs/superpowers/plans/2026-08-27-subagent-worktree-isolation.md`；Ch 12 |

**症状 / 动机：** `spawn_subagent` 始终共享父级 `work_dir` 且保持 `ResourcePolicy::Barrier`，多子 agent fan-out 要么串行（同步），要么只能靠 `run_in_background` + `tokio::spawn` 并行。2026-08-26 异步子 agent 设计评审点名了后续项：一旦每个子 agent 拥有作用域化文件系统，阻塞型子 agent 的同一 wave fan-out 就安全了。工具描述（"shares the filesystem"）也没给模型任何请求隔离的方式。

**决策：** (1) `SubagentInput` 新增 `worktree: Option<bool>`；为 `true` 时，handler 同步创建（或 `resume` 时复用）git worktree 泳道 `subagent-<child_id>`（分支 `wt/subagent-<child_id>`）—— 失败立即暴露 —— 并把子级 `ToolContext.work_dir` 指向该泳道。同步与异步返回值都追加 `(worktree: <name> at <path>)` 说明。(2) `execute_tool_call` 按调用解析资源（`tool_resources_for`）：worktree 隔离的 `spawn_subagent` 映射为 `ToolResources::independent()` 而非静态 `Barrier`，隔离子 spawn 可同 wave fan-out；非隔离 spawn 保持 `Barrier`。(3) `WorktreeManager`/`SharedWorktreeManager` 暴露 `get(name)` 供 resume 复用。工具描述重写，明确同步/异步/worktree 策略。

**改后行为：** `spawn_subagent` 带 `worktree: true` 时运行在基于仓库根 `HEAD` 的隔离泳道；子级 `bash`/`read_file`/`write_file`/`edit_file` 都相对于泳道解析；泳道在子 agent 完成后保留（用 `worktree_status`/`worktree_run` 检查，用 `git worktree remove` 删除）。非 git 的 `work_dir` 会让 spawn 明确报错。worktree 是组织边界而非 OS 沙箱 —— `bash` 仍可访问泳道之外。声明式 per-agent 定义（`.tact/agents/*.md`）与 worktree 删除工具仍延后。

---

## 3. 2026-08-27 — 子代理权限继承 + 异步 `run_in_background` + 结果回注

| Field | Value |
|-------|-------|
| **类型** | feature |
| **相关** | `crates/tact/src/permission/mod.rs`（`PermissionSnapshot`）、`crates/tact/src/tool/subagent.rs`、`crates/tact/src/subagent.rs`、`crates/tact/src/store/subagent_store/`、`crates/tact/src/agent/mod.rs` + `tool_dispatch.rs`、`crates/protocol/src/agent.rs`、`crates/tact-ui/src/driver.rs`、`crates/agent_tui_kit/src/components/tool.rs`、`crates/tui/src/…`；设计 `docs/superpowers/specs/2026-08-26-async-subagent-design.md`、评审 `…-design-review.md`、计划 `docs/superpowers/plans/2026-08-26-async-subagent.md`；Ch 12 |

**症状 / 动机：** `spawn_subagent` 是单个同步阻塞工具，子 agent 的 `PermissionManager` 始终以 `PermissionMode::Default` 构建 —— 一个 `Plan`（只读）父级可以 spawn 一个可写文件的 `Default` 子级，逃逸只读意图。也没有后台运行、限制轮数、恢复、或重启后查询生命周期的能力。

**决策：** (1) `PermissionSnapshot { mode, always_allowed_tools, settings }` + `PermissionManager::snapshot()`/`from_snapshot()`；`execute_tool_call` 在 phase-1 pre-flight 后把快照（及结果队列）stamp 到 `ToolContext`，`spawn_subagent` 据此构建子级（Claude 风格继承；`Default`→`Default`、`Plan`→`Plan`、`Auto`→`Auto`，拒绝计数归零；orphan/test context 回退 `Default`）。(2) `SubagentInput` 新增 `run_in_background` / `max_turns` / `resume`；异步 spawn 脱钩任务、返回 `async_launched { id }`，完成后转换 `subagent_runs` 行并入队 `SubagentResult`，在下一轮 LLM 调用前 drain 并注入 `<subagent-finished>` 消息。(3) `AgentUpdate::SubagentFinished` 定稿 keep-live 卡（携带 transcript）；`UserCommand::SubagentFinishedNotification` + driver 唤醒轮让空闲父级恢复；`check_subagent` 暴露持久化生命周期。`spawn_subagent` 保持 `ResourcePolicy::Barrier`（后台并行来自 `tokio::spawn`，而非 wave 调度）。

**改后行为：** 子 agent 继承父级权限上下文（修复只读逃逸）；`run_in_background` 返回异步句柄并把子 summary 回注父 transcript；`max_turns` 限制失控子级；`resume` 复用已完成子 session；`check_subagent` 读取 `subagent_runs`；启动时将 orphan `running` 行修复为 `failed`。per-agent `permissionMode` override 仍延后（尚无声明式 `.tact/agents/*.md`）；worktree 隔离已单独落地（见上文条目）。

---

## 2. 2026-08-24 — `AgentUpdate` 移除内嵌 oneshot；选择请求改用 `request_id` + `UiResponse`

| Field | Value |
|-------|-------|
| **类型** | optimization |
| **相关** | `crates/protocol/src/agent.rs`、`crates/tact/src/ui_responder.rs`（新增）、`crates/tact/src/tool/ask_user.rs`、`crates/tact/src/tool/subagent_ui.rs`、`crates/tact/src/agent/tool_dispatch.rs`、`crates/tact-ui/src/driver.rs`、`crates/agent_tui_kit/src/state/select_popup.rs`、`crates/tui/src/handlers/select.rs`；Ch 25 |

**症状 / 动机：** `AgentUpdate::RequestSelect` / `RequestMultiSelect` 内嵌了 `tokio::sync::oneshot::Sender`，把协议 enum 耦合到了进程内传输句柄。该 enum 无法派生 `Clone`/`Serialize`，每个请求只有一个进程内响应者，且每新增一种「向 UI 提问」都需要一个携带 sender 的新变体。

**决策：** 请求改为携带纯数据 `request_id: u64`；TUI 通过反向通道用 `UserCommand::UiResponse`（`Select` / `MultiSelect`）作答。新增共享的 `UiResponder` 分配全局唯一 id 并把响应路由到精确的等待方；父 agent 与每个 subagent 克隆同一 inner 状态，因此 subagent 经父 tagged 通道转发的请求仍能正确路由。driver 处理 `UiResponse` 时不等待 in-flight 任务，退出时调用 `UiResponder::shutdown`，使 UI 关闭时仍能唤醒等待方。

**改动后行为：** `AgentUpdate` 变为不携带传输句柄的数据（后续可自由派生 `Serialize`/`Clone`）；选择响应按 `request_id` 跨父 agent 与所有 subagent 路由；UI 关闭时 agent 循环被唤醒而非死锁。

---

## 2. 2026-08-24 — 插件更新命令：`tact plugin update` / `/plugin update`

| Field | Value |
|-------|-------|
| **类型** | feature |
| **相关** | `crates/tact/src/plugin/{mod,install}.rs`、`crates/tact/src/config/cli.rs`、`crates/tact-ui/src/plugin_cli.rs`、`crates/tact-ui/tests/plugin_cli_tests.rs`、`crates/tui/src/handlers/plugin.rs`、`crates/tui/src/widgets/state/app/extensions.rs`、`crates/agent_tui_kit/src/i18n.rs`；Ch 2 § 插件 skills、Ch 23 §7 |

**症状 / 动机：** 已安装插件按 revision 锁定，上游 marketplace 发布新版本后只能先卸载再重装才能前移——还会隐式丢失 marketplace 来源关联。

**决策：** 给 plugin worker 增加 `Update` 请求。`PluginInstaller::update` 校验插件 id、刷新所属 marketplace（刷新失败时回退到已拉取的旧 catalog）、解析最新 revision：revision 相同则报告已是最新（缓存不动）；否则安装新 revision 并清理旧 revision 的缓存目录。CLI 增加 `tact plugin update <name>`；TUI slash 命令增加 `/plugin update <name>`。更新成功与 install/uninstall/reload 一样刷新共享 skills。

**改动后行为：** `tact plugin update <name>` 与 `/plugin update <name>` 把已安装插件升级到其 marketplace 的最新 revision 并删除上一 revision 的缓存；已是最新时原样保留并如实报告；插件未安装时报错。

---

## 2. 2026-08-24 — 插件卸载命令：`tact plugin uninstall` / `/plugin uninstall`

| Field | Value |
|-------|-------|
| **类型** | feature |
| **相关** | `crates/tact/src/plugin/{mod,install,store}.rs`、`crates/tact/src/config/cli.rs`、`crates/tact-ui/src/plugin_cli.rs`、`crates/tact-ui/tests/plugin_cli_tests.rs`、`crates/tui/src/handlers/plugin.rs`、`crates/tui/src/widgets/state/app/extensions.rs`、`crates/agent_tui_kit/src/i18n.rs`；Ch 2 § 插件 skills、Ch 23 §7 |

**症状 / 动机：** 插件可以安装、列出、重载，却无法卸载；卸载只能手动编辑 `installed.json` 并手动删除缓存目录。

**决策：** 给 plugin worker 增加 `Uninstall` 请求。`PluginInstaller::uninstall` 校验插件 id、移除已安装注册项，并且仅在缓存内容解析到插件缓存根内部时才删除目录（遗留或越界的路径保持不动），随后把空的父目录向上清理到缓存根为止。新增 `PluginStore::commit_removal` 持久化注册表，无需候选目录（区别于 `commit_install`）。CLI 增加 `tact plugin uninstall <name>`；TUI slash 命令增加 `/plugin uninstall <name>`。卸载成功同样触发与 install/reload 相同的共享 skill 刷新，`plugin:<skill>` 条目立即从注册表消失。

**改动后行为：** `tact plugin uninstall <name>` 与 `/plugin uninstall <name>` 删除插件的缓存目录与注册项；插件未安装时报错；插件缓存之外的任何内容永远不会被删除。

---

## 2. 2026-08-23 — 附件去内联：`@file`/`![alt]` 保留为路径文本

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact-ui/src/user_message.rs` (`build_user_message`), `crates/tact-ui/src/{lib,image_attach}.rs` (移除 `image_attach`), `crates/tact-ui/Cargo.toml` (去除 `image` 依赖); Ch 22 § image attachments |

**Symptom / motivation:** `build_user_message` 在 attach 时把图片文件 base64 内联为 `ContentBlock::Image`、把文本文件整体内联为 `ContentBlock::Text`。这会在模型未必需要时就把图片字节与大文件内容塞进每次请求；且纯文本模型完全无法读图。

**Decision:** 去内联。`@file` 保留为 `@<path>` 文本，`![alt](path)` 保留为 `![alt](path)` 文本，使模型按需用 `read_file`（文本）/ `read_image`（图片）读取。由于 attach 路径不再解码图片，`tact-ui` 的 `image_attach` 模块及其 `image` 依赖被移除（编码逻辑现位于 `tact` 的 `read_image` 工具中）。

**Behavior after:** 图片/文件字节仅在模型显式调用 `read_image`/`read_file` 时进入请求；用户消息携带路径引用而非内联 blob。文本的 `read_file`（offset/limit）不变；图片走 `read_image`。

---

## 2. 2026-08-23 — `read_image` 工具：模型按需读图，工具结果图片折叠进 user 消息

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/tool/read_image.rs`, `crates/tact/src/tool/{mod,registry}.rs`, `crates/tact/src/agent/tool_dispatch.rs` (`ToolCallResult.image` / `ExecResult`), `crates/tact_llm/src/convert.rs` (`messages_to_openai` 工具结果图片折叠); Ch 22 § image attachments |

**Symptom / motivation:** 图片处理只支持用户 attach：`@file.png` 变成内联 `ContentBlock::Image`，纯文本模型无法按需读取本地图片。缺少模型可调用的图片工具，vision 模型无法在不重新 attach 的情况下按路径取图。

**Decision:** 新增 `read_image` 工具（Harness 风格）：先门控当前模型声明 image 输入，读取 PNG/JPEG/WebP/GIF，重编码为有界 JPEG，返回携带文本信封 + 相邻 `ContentBlock::Image` 的 `ToolCallResult`。`ToolCallResult` 新增内存 `image` 字段；`build_tool_results` 发出伴生图片块。由于 Chat Completions `role:tool` content 仅限字符串，`messages_to_openai` 将图片折叠到紧随其后的 `role:user` 消息（`[{text:"Attached image(s) from tool result:"}, {image_url}]`），镜像 DeepSeek Harness `serializeMessagesWithImages`。`ContentBlock::ToolResult` 保持不变，Responses/normalize/compact 消费方不受影响。

**Behavior after:** vision 模型可调用 `read_image("path")`，同时获得稳定文本信封与紧随 user 消息中的图片；纯文本工具结果仍走 `role:tool`。非图片工具不受影响。

---

## 2. 2026-08-23 — 工具卡步骤编号统一按计划位置解析（bugfix，任务 #43 评审）

| 字段 | 值 |
|------|------|
| **类型** | bugfix |
| **相关** | 任务 #43（ToolEvent outbox：`crates/agent_tui_kit/src/components/tool.rs`）；`crates/agent_tui_kit/src/components/plan.rs`；`crates/tui/src/widgets/state/app/agent.rs`；下文条目（ComponentRegistry，任务 #42）；Ch 23 §1 |

**症状 / 动机：** ToolEvent outbox 提取（任务 #43）之后，步骤索引出现三处
漂移，在 tool id 乱序或重启时出错：
1. `PlanComponent` 用 agent 的**原始** `idx` 写 `step.output`，而原始 `idx`
   可能与计划位置不一致（`resolve_step_idx` 正是为此存在）——完成/失败更新
   可能覆盖**错误**步骤的 output，破坏状态栏进度推导与计划面板。
2. `ToolComponent` 内部的 `tool_id → 步骤索引` 映射用**后到覆盖**
   （`HashMap::insert`），而 shell 解析取**首个**计划位置——同一 `tool_id`
   重启后卡片标题（"N. tool"）与 shell 状态行不一致。
3. `StepFailed` 系统消息（"✗ Step N failed: …"）用原始 `idx`，而卡片标题
   用解析后的值。

**决策：**
- `PlanComponent` 增加自己的 `resolve_step_idx`（按 `tool_id` 取首个计划
  位置，回退原始 `idx`——与 shell 完全一致），`StepFinished` / `StepFailed`
  的写入都经它解析。shell tail 保留解析后写入；两者现在写相同值，双重写入
  无害，外部 kit host 独立使用也不受影响。
- `ToolComponent` 的映射改为**首到生效**（`entry().or_insert`，从不覆盖）：
  重启的 `tool_id` 在卡片整个生命周期内保持原始步骤编号，与 shell 一致。
  映射按会话有界（每个不同 `tool_id` 一条，增长与计划本身相同），**不得**
  在 finalize 时清理——清理会让后续重启重新记录最新到达索引，重新引入漂移。
- `StepFailed` 无活动卡的系统消息改用解析后的步骤编号。
- `ToolEvent::Finalized` 携带显式 `had_active: bool`，取代脆弱的
  `phys_idx == 0 && old_rows == 0` 哨兵来区分 allocate/resize；
  `on_step_failed_tail` 移除未使用的 `idx`/`tool_id`/`error` 参数。

**变更后行为：** 步骤 output 与所有展示的步骤编号（卡片标题、系统消息、
计划面板、状态栏）都按该 `tool_id` 的首个计划位置一致，即使 agent 原始
`idx` 不同（tool id 乱序、同 id 重启）。新测试：
`step_finished_resolves_divergent_raw_idx`、`step_failed_resolves_divergent_raw_idx`、
`restart_keeps_first_step_mapping`、`step_failed_missing_uses_resolved_step_number`、
`finalize_without_active_marks_had_active_false`。

**指针：** `crates/agent_tui_kit/src/components/{plan,tool}.rs`、
`crates/tui/src/widgets/state/app/agent.rs`（`apply_tool_events` /
`on_step_failed_tail`）、Ch 23 §1。

---

## 2. 2026-08-23 — TUI 渲染层提取为 `agent_tui_kit`（可复用、与 Tact 解耦）

| 字段 | 值 |
|------|------|
| **类型** | optimization |
| **相关** | 设计：`docs/superpowers/specs/2026-08-18-tui-component-library-design.md`（+ `-ctx-design.md`）；计划：`docs/superpowers/plans/2026-08-18-tui-component-library.md`；Ch 23 |

**症状 / 动机：** 整个渲染栈在 `crates/tui` 单个 crate 内，与 `tact` /
`tact_llm` 强耦合（`App` 持有所有面板状态；render 函数接收 `&App`）。其他
agent 项目无法复用 thinking/tool 卡片、流式 markdown 日志、弹窗族、输入框
或状态栏，除非把 Tact 的 agent 运行时一起拉进来；且 `handle_agent_update`
是一个巨大的 match，没有组件边界可独立测试。

**决策：** 提取可复用的 **agent-TUI kit**（`crates/agent_tui_kit`），只依赖
`tact_protocol` + ratatui：
- 纯渲染函数接收每帧 `RenderCtx`（host 构建的不相交 `&` 借用；唯一变更路径
  是显式 `Vec<RenderCommand>`，帧后 drain）；
- 状态模型（`LogCoordinator`、`ToolState`、`ThinkingState`、`StreamState`、
  `StatusBarState`、`LogScroll`、`PlanPanel`、弹窗状态、widgets）verbatim 移入
  kit——零视觉变化，每个阶段门禁由不变的 scene/render 测试验证；
- 进出契约为 `bridge::Command`（通用 9 变体枚举）+ `AgentBridge` +
  `BridgeExtension`/`ExtensionEvent`；Tact-only 命令（`QueryBalance`）走
  `ExtensionCommand`；
- 需要应用层样式的可变 "prepare" 阶段（log 滚动缓存重建 + skill 高亮、
  diff/subagent 懒缓存、输入光标 clamp）留在 `crates/tui`，为 kit 纯渲染供数。

**变更后行为：** `cargo tree -p agent_tui_kit` 无 `tact` / `tact_llm`；
`crates/tui` 成为 Tact 应用层（shell `App`、handlers、prepare 阶段、应用层
弹窗、编排 `layout.rs`）。headless mock 消费者证明 kit 可独立运行：
`cargo run -p agent_tui_kit --example mock_agent`。测试随代码迁移（tui 413 +
kit 194 = 607 ≥ 604 基线；`tact-ui` 105；clippy 零警告）。

**指针：** `crates/agent_tui_kit/src/render/*`（bar、input、log、popups、
task_panel、render_md、cells）、`state/*`、`bridge.rs`、
`crates/tui/src/widgets/state/app/config.rs`（`App::render_ctx`）、
`examples/mock_agent.rs`；各阶段门禁记录在计划文档 results 段落。后续
（步骤 9）：`Component` 注册表 + handlers 迁移仍未做。

---

## 2. 2026-08-23 — TUI `App` 切换到 kit 的 `ComponentRegistry`（whole-App 重构，任务 #42）

| 字段 | 值 |
|------|------|
| **类型** | optimization |
| **相关** | 计划：`docs/superpowers/plans/2026-08-18-tui-component-library.md`（步骤 9）；设计：`docs/superpowers/specs/2026-08-18-tui-component-library-design.md`；Ch 23 |

**症状 / 动机：** kit 的 `Component` trait 与六个组件
（Thinking/Stream/StatusBar/TaskPanel/Plan/Tool）已存在，但 Tact 应用仍把
它们的状态当作裸 `App` 字段持有，`handle_agent_update` 的巨大 match 仍跑旧
的内联 handlers——组件边界只是编译期草案；任何采用注册表的 host 都得先复刻
整个 shell。

**决策：** whole-App 切换在保持行为不变的前提下让注册表成为唯一状态持有者：
- `App` 移除 `plan` / `thinking` / `stream` / `tools` / `task_panel` /
  `status_bar` 字段，持有 `registry: ComponentRegistry`；共享 `LogCoordinator`
  仍归 shell 所有（决策：coordinator 是 shell 已拥有并测试的 priority-0 面）。
- `app/registry.rs` 类型化访问器（`plan()`/`plan_mut()`，…）访问组件；
  组件 `Deref`/`DerefMut` 到底层状态，`app.<field>.<state>` 调用点改动最小。
- `handle_agent_update` = `coordinator_prepass` → `dispatch_components`
  （注册表分发；`Ctx` 经字段拆分借用 shell 拥有的 log / input mode /
  pending queue）→ `apply_stream_events`（`StreamEvent` outbox；**仅
  StreamChunk**——gap 检查会追加行，旧代码只在流块上执行）→
  `shell_handle`（status/log 效果、tool 卡片生命周期、select 弹窗、
  thinking 卡片）→ `refresh_tail_scroll`。
- `ThinkingChunk` 与 `StepFinished`/`StepFailed` 刻意**不**分发：shell 拥有
  log 锚定的 thinking 卡片与已解析 step 的 tool 生命周期；分发会双重处理。
- `Component` 增加 `Send` 超类约束（持有注册表的 shell 需移入 `tact-ui`
  的 tokio task）与 `ComponentRegistry::get_mut`（供 host 侧 handler 的类型化
  可变 downcast）。

**变更后行为：** 组件状态归注册表所有；shell 在 `shell_handle` 保留丰富行为；
host 以 `app.plan()` / `app.tools_mut()` 替代字段访问。无用户可见变化——
全部 scene/render 测试无需修改期望即通过（tui 413 + kit 221 = 634；
`tact-ui` 105；clippy 零警告）。

**指针：** `crates/agent_tui_kit/src/components/{registry,thinking,stream,
status_bar,task_panel,plan,tool}.rs`、`crates/tui/src/widgets/state/app/
{agent.rs（dispatch_components / shell_handle / apply_stream_events）、
registry.rs、construct.rs、config.rs}`、`crates/tui/src/render/log.rs`
（prepare 的字段拆分借用）、Ch 23 §1。

---

## 2. 2026-08-17 — Responses 模型切换时适配 web-search query 字段

| Field | Value |
|-------|-------|
| **类型** | bugfix |
| **相关** | 第 22 章（LLM / Responses）；`crates/tact_llm/src/openai/responses/wire.rs`、`crates/tact_llm/src/openai/responses/convert.rs` |

**现象 / 动机：** 在同一个 Responses 端点上从 OpenAI 模型切换到 DeepSeek 兼容模型后，旧的 `web_search_call` 使用 `action.query` 被回放，而目标端点要求 `action.queries`，最终返回 HTTP 400：`missing field queries`。

**决策：** 保留现有 Responses 基线，只在发送边界且模型发生变化时适配 hosted web-search action：DeepSeek 目标使用 `queries: [query]`，其他目标取第一条 query 写成单数 `query`。同模型回放保持原样。

**改后行为：** 模型切换不再发送上一个模型不兼容的 `query` / `queries` 字段形状，其余 opaque Responses 基线和逻辑会话保持不变。

**指针：** `crates/tact_llm/src/openai/responses/wire.rs` 的 `normalize_web_search_call_query_shape_in_items`；`crates/tact_llm/src/openai/responses/convert.rs` 的 `create_response` outgoing baseline 处理；双向转换回归测试；第 22 章。

---

## 2. 2026-08-17 — Pending prompt 的 `[Cancel]` 紧跟提示文案

| Field | Value |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23（TUI）；`crates/tui/src/render/input.rs`、`crates/tui/src/handlers/mouse.rs`、`crates/tui/src/widgets/state/app/pending.rs` |

**现象 / 动机：** pending 队列的 `[Cancel]` 控件曾经右对齐在提示行最右侧，虽然它只控制旁边的 pending prompt 区域，但实际操作距离较远、不易点击。

**决策：** 为按钮预留宽度并相应截断提示文案，然后让 `[Cancel]` 在提示文案后紧接显示，中间只保留一个空格；保留现有的渲染时命中矩形和“只取消队列”的语义。

**改后行为：** 宽终端显示 `Message will be submitted after the current task [Cancel]`；按钮紧挨解释性文案，点击只清空排队 prompt，窄终端仍隐藏按钮。

**指针：** `crates/tui/src/render/input.rs` 的 `render_pending_block`；`crates/tui/src/handlers/mouse.rs` 的 `handle_mouse_down` 与 `pending_cancel_click_hits_the_rendered_button`；`widgets/state/app/pending.rs` 的 `App::clear_pending_messages`；Ch 23；`docs/superpowers/specs/2026-08-17-pending-cancel-button-placement-design.md`。

---

## 2. 2026-08-16 — Log 行改用显式来源 metadata，不再从文本推断系统 item

| Field | Value |
|-------|-------|
| **类型** | bugfix / optimization |
| **相关** | Ch 23（TUI）；`crates/tui/src/widgets/state/mod.rs`、`crates/tui/src/widgets/state/app/popups.rs`、`crates/tui/src/render/log.rs` |

**现象 / 动机：** Log 通过原始前缀和缩进推断 user / system 归属。普通 item 只要以两个空格开头就可能进入系统纯文本路径，导致 `  **粗体文本**` 显示字面量 `**`。用户续行识别和行缩进也依赖相邻 raw 字符串，容易被内容格式误导。

**决策：** 为每个 physical log 行增加 `LogItemKind` metadata。插入路径显式标记来源：user、assistant Markdown、system plain / Markdown、system tool 或 Thinking。历史消息使用原始 `Role` / content 路径，实时消息使用对应的 `AgentUpdate` variant。渲染、缩进、类别分隔线和 user gap 都消费该 metadata；显式系统前缀只在来源已经确定为 system 后用于选择颜色。

**改后行为：** 缩进的 assistant / system Markdown 保留 Markdown 样式；user 行和续行使用记录好的来源；tool 与 Thinking placeholder 保留专用缩进；raw 文本不再用于推断行归属或类别。主 Log 路径移除 `RawMessageType`、`is_user_message_line`、`user_line_mask` 和 `classify_system_message`。

**指针：** `crates/tui/src/widgets/state/mod.rs` 的 `LogItemKind` / `SystemMsgStyle`；`app/popups.rs` 的 metadata 同步操作；`app/messages.rs`、`app/agent.rs`、`handlers/mod.rs` 的显式插入路径；`render/log.rs`、`render/log_style.rs`、`app/visibility.rs` 的 metadata 驱动渲染；Ch 23。

---

## 2. 2026-08-16 — 嵌套 Markdown 列表项不再与父项粘在同一行

| Field | Value |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23（TUI）；`crates/tui/src/render/pulldown.rs`（`Writer::start_tag`、`Tag::List`） |

**现象 / 动机：** pulldown-cmark 事件流进入嵌套 `Tag::List` 时，父列表项的行内 span 仍保存在 `pending` 中。因此第一个子项的 marker 会被拼到父项行尾（`• parent    • child one`），后续子项才会单独换行。

**决策：** 进入嵌套列表时，如果 writer 仍有未刷出的父列表行，先刷出该行，再压入嵌套列表上下文。现有列表 marker 与逐级缩进逻辑保持不变。

**改后行为：** 父项与每个嵌套项均渲染为独立行；嵌套 bullet 继续按每级四列缩进。已有有序列表与任务列表行为不变。

**指针：** `crates/tui/src/render/pulldown.rs` 的 `Writer::start_tag`；回归测试 `nested_list_items_render_on_separate_lines`；Ch 23。

---

## 2. 2026-08-16 — 表格数据行之间渲染水平分隔线

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23（TUI）；`crates/tui/src/render/render_md.rs`（`render_table_chunk`、`push_separator`） |

**现象 / 动机：** 管道表格只有表头分隔线；多行数据体渲染成一串连续换行块，长行之间难以区分，表格不像网格。

**决策：** `render_table_chunk` 现在在每个数据行（最后一行除外）之后输出一条分隔线（accent 色短横线，宽度对齐列宽，与表头分隔线同款）。换行成多个视觉子行的行保持在其分隔线上方。数据行后紧跟显式 dash 行（`/skills` 风格分隔线）时不再额外加线。分隔线由同一列宽数据生成，因此与所有行保持 pipe 对齐，包括拆块表格与流式行（`format_table_lines` 共用实现）。

**改后行为：** 多行表格以网格线显示（表头线 + 行间线）；单行表格不变。换行续行保持归组在其行下，分隔线保持显示宽度对齐。

**指针：** `crates/tui/src/render/render_md.rs` 的 `render_table_chunk` / `push_separator`；更新行数断言的测试：`format_table_aligns_cjk_and_ascii`、`format_table_keeps_pipe_inside_cell`、`format_table_splits_chunks_only_when_compact_cannot_fit`、`format_table_renders_row_separators_aligned`。

---

## 2. 2026-08-16 — 超宽表格在紧凑布局能放下时保持完整显示（拆块是最后手段）

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23（TUI）；`crates/tui/src/render/render_md.rs`（`format_table`、`fit_columns_to_width`、`MIN_COL_WIDTH` / `COMPACT_COL_WIDTH`），`crates/tui/src/render/cells/markdown.rs`（渲染宽度回归测试） |

**现象 / 动机：** 表格自然宽度超过主区域时，`format_table` 先把列压缩到可读地板（`MIN_COL_WIDTH = 8`），仍放不下就拆成列块、每块重复表头。贪心拆块产生悬殊的块（如 12 列在 95 宽拆成 8+4，第二块只用一半宽度；4 列长表格在 35 宽拆成 3+1，第二块孤零零一列），即使整张表在更窄列宽下本可以完整显示。

**决策：** 两阶段压缩。先压到可读地板；仍放不下时再压到紧凑地板（`COMPACT_COL_WIDTH = 4`，仍可显示约 2 个 CJK 字符/行），只要紧凑布局能放下就保持整表完整。只有紧凑地板也放不下时才拆块（如 10 列在 40 宽），且拆块前恢复可读地板，保证块内列宽 8。`fit_columns_to_width` 增加 `floor` 参数。

**改后行为：** 超宽表格优先按列压缩（每列 ≥ 4）以一张完整表格 + 单表头显示；只有真正放不下的表格才拆块（每块重复表头），保留既有 `wide_table_chunks_into_fitting_blocks` 保证（行 ≤ 面板宽、块内 pipe 对齐）。

**指针：** `crates/tui/src/render/render_md.rs` 的 `format_table` / `fit_columns_to_width` / `COMPACT_COL_WIDTH`；测试 `format_table_keeps_overwide_table_intact_when_compact_fits`、`format_table_splits_chunks_only_when_compact_cannot_fit`。

---

## 2. 2026-08-16 — 流式表格行不再因回复缩进裁掉最右管道

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23（TUI）；`crates/tui/src/widgets/state/app/visibility.rs`（`App::table_layout_width`），`crates/tui/src/widgets/state/app/agent.rs` + `visibility.rs`（`format_table_lines` 调用点），`crates/tui/src/render/render_md.rs`（`format_table`），`crates/tui/src/render/cells/markdown.rs`（回归测试） |

**现象 / 动机：** 流式与系统管道表格在构建时按 `log_scroll.width`（日志面板的完整内容宽度）布局；渲染时 assistant / 系统行有缩进（`nested_log_indent` 对 LLM 行至少返回 `LOG_THINKING_INDENT + 1 = 3`），实际可用宽度少 3 列。被压缩到满宽的长表格行因此裁掉尾部管道与最右列，竖线看起来对不齐。`MarkdownCell` 路径一直没有此问题（`render_if_needed` 的 `content_width` 已扣缩进），所以只有流式 / 系统表格错位。

**决策：** 新增 `App::table_layout_width()`，返回 `log_scroll.width - (LOG_THINKING_INDENT + 1)`（下限 1）；所有 `format_table_lines` 调用点（`agent.rs` 流式 flush、`/plugin` 与 `/marketplace` 列表、`visibility.rs::flush_stream_pending`）改用该宽度布局。所有表格行渲染缩进 ≥ 3（LLM 行 = 3，SysTool 行 = 4，`/plugin` 表格归类为 LLM），因此按缩减宽度布局的表格必然放进真实渲染宽度。

**改后行为：** 流式与系统表格行永不超出渲染内容宽度；任意面板宽度下尾部管道保持可见、列保持显示宽度对齐。回归测试 `streamed_table_rows_stay_aligned_after_reply_indent` 在 40/60/80 列下断言真实 buffer 的管道坐标（块内对齐、尾部管道存在）。

**指针：** `App::table_layout_width` 在 `crates/tui/src/widgets/state/app/visibility.rs`；调用点在 `widgets/state/app/agent.rs`（约 790/810/872/887/972/1028 行）与 `visibility.rs`（`flush_stream_pending`）；测试在 `crates/tui/src/render/cells/markdown.rs`。

---

## 2. 2026-08-16 — 单元格内包含 `|` 的表格不再裂成幻影列

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23（TUI）; `crates/tui/src/render/render_md.rs`（`format_table`、`format_table_lines`）、`crates/tui/src/render/pulldown.rs`（`flush_table`）、`crates/tui/src/widgets/state/app/agent.rs` + `visibility.rs`（流式刷出点） |

**Symptom / motivation:** `format_table` 接收 `| a | b |` 原始字符串并按 `'|'` 重新切分。`pulldown.rs::flush_table` 把收集到的单元格拼回这种字符串再传进去，所以单元格里含有字面量管道（转义 `\|`、行内代码）时会多出幻影列、整行错位。行式流式渲染器（`agent.rs` / `visibility.rs` 的 table_buffer）也有同样的隐患。

**Decision:** `format_table` 改为接收结构化单元格（`headers: &[String]`、`rows: &[Vec<String>]`）；内部做 trim、合成 GFM 表头分隔线、保留"全破折号"正文行作为 `/skills` 风格的分隔线。`flush_table` 直接把 pulldown 收集的单元格传进去——不再有 `|` 往返。基于原始源码行的调用方（流式缓冲、`/plugins`、`/marketplace` 列表）改走新增的 `format_table_lines`：把缓冲行拼接后交给标准 pulldown 管线渲染，转义规则只应用一次。已知上游限制：pulldown-cmark 在解析表格行时连行内代码里的未转义管道也会切分（`| x | `a|b` |`），所以端到端只支持转义管道形式；`format_table` 本身对收到的任何单元格内容都是正确的。

**Behavior after:** 转义管道和行内代码里的管道是单元格数据，不再是列分隔符；所有行共享统一的显示宽度 padding，结构性管道（列边界、行边缘）保持对齐。测试断言单元格内容完整 + 结构性管道对齐（`format_table_keeps_pipe_inside_cell`、`table_cell_with_pipe_stays_aligned_end_to_end`）。

**Pointers:** `crates/tui/src/render/render_md.rs` 中的 `format_table` / `format_table_lines`；`crates/tui/src/render/pulldown.rs` 的 `flush_table`；`widgets/state/app/agent.rs` 与 `visibility.rs` 的流式刷出点。

---

## 2. 2026-08-16 — 超宽表格拆分为可放下的列块（不再出现被折断的管道行）

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23（TUI）; `crates/tui/src/render/render_md.rs`（`format_table`、`split_into_fitting_chunks`、`render_table_chunk`、`row_width`）、`crates/tui/src/render/pulldown.rs`（`flush_table`）、`crates/tui/src/render/cells/markdown.rs`（`wide_table_chunks_into_fitting_blocks` 测试） |

**Symptom / motivation:** 宽度超过面板的 Markdown 管道表格（例如 40 列面板里的 10 列表格）渲染出的行比可用宽度还宽：`fit_columns_to_width` 收缩到 `MIN_COL_WIDTH = 8` 可读性下限就停（10×8 + 3×10 + 1 = 111 列放不进 40 列）。随后 `MarkdownCell::render_if_needed` 里面板级的 `wrap_line` 把每个表格行拦腰折断，丢失行首管道符、后续所有行全部错位——表格看起来像被撕碎了一样。

**Decision:** `format_table` 现在保证每一行都放得下可用宽度。全局收缩碰到列宽下限后如果整表仍然过宽（`row_width > available_width`），就把列拆成若干连续的、各自放得下的块（`split_into_fitting_chunks`）；每个块渲染成独立的小表格，表头与分隔线重复（`render_table_chunk`）。单独一列也放不下时，该列继续收缩到下限以下（最小 1），保证它的块必然放得下。这样 `wrap_line` 永远见不到超宽的表格行，也就不会折断管道对齐。无限宽路径（`available_width = None`，日志/弹窗正文）保持不变——那些行由 widget 裁剪，而不是被折断。

**Behavior after:** 任何表格都能放进面板；真正很宽的表格显示为纵向堆叠的列块，表头重复，块内各行保持对齐（CJK 感知的显示宽度填充、表格内单元格换行）。宽度感知的消息路径中，行宽永远不会超过面板宽度。

**Pointers:** `crates/tui/src/render/render_md.rs` 中的 `format_table` 分块分发与 `split_into_fitting_chunks`/`render_table_chunk`；回归测试 `wide_table_chunks_into_fitting_blocks`（`crates/tui/src/render/cells/markdown.rs`）。

---

## 2. 2026-08-16 — `/stats` 通过共享 stats 快照立即响应（不再等待运行中的任务）

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 25; `crates/tact/src/stats.rs`、`crates/tact/src/agent/mod.rs`、`crates/tact/src/agent/tool_dispatch.rs`、`crates/tact/src/hook/rtk_filter.rs`、`crates/tact-ui/src/driver.rs` |

**Symptom / motivation:** `UserCommand::QueryStats` 落在命令驱动的 `other =>` 分支，该分支会先 await 在途的 `SubmitTask` handle 才能访问 `Agent`（运行中的任务独占 Agent 所有权）。长任务期间 `/stats` 看起来毫无响应，直到任务结束——有时要等好几分钟。

**Decision:** 让 stats 不再归 Agent 独占：`AgentRuntime.stats` 改为 `Arc<RwLock<SessionStats>>`（std 锁；所有更新点用 `write().unwrap()`，rtk 原子计数器保持不变）。命令循环进入 receive 循环前 clone Arc，`QueryStats` 提升为独立 match 分支：直接 `stats.read().unwrap().summary()` 并通过预 clone 的 `ui_tx` 发出 `SessionStats`——不访问 `Agent`、不 await。`headless.rs` / `interactive.rs` 的结束摘要也通过同一把锁读取。

**Behavior after:** `/stats` 在任务运行中也立即响应，显示截至当前已记录的全部统计快照（进行中的 LLM 调用尚未计入——与之前一致，stats 只在每次调用后写入）。Agent 本身不受影响；其他命令（`/compact`、`/model`、`/background` 等）仍与运行中的任务串行。

**Pointers:** `crates/tact-ui/src/driver.rs`（`run_command_loop_with_account` 的 QueryStats 分支 + `stats` clone）、`crates/tact/src/stats.rs`、`agent/mod.rs` 的 stats 更新点（主循环 + 压缩）、`agent/tool_dispatch.rs`（工具计数）、`hook/rtk_filter.rs`（`record_rtk`）；测试 `query_stats_responds_immediately_while_task_runs`（阻塞 responder，释放前断言收到 SessionStats）。

---

## 2. 2026-08-16 — Codex 风格排队消息：agent 忙时提交"当前任务结束后自动提交"

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | Ch 23; `crates/tui/src/handlers/insert.rs`、`crates/tui/src/handlers/skills.rs`、`crates/tui/src/widgets/state/app/pending.rs`、`crates/tui/src/render/input.rs`、`crates/tui/src/lib.rs` |

**Symptom / motivation:** agent 处于 `Planning`/`Executing` 时按 Enter 只会闪现"⏳ 上一个任务还在处理中"并丢弃输入——工具运行期间打的消息全丢了。Codex CLI 的做法是排队："Messages to be submitted after next tool call (press esc to interrupt and send immediately)"。

**Decision:** 用 Codex 风格队列取代忙时拒绝。`App.pending_messages` 存放 `PendingMessage { display, agent_task }`；`Planning`/`Executing` 时按 Enter 清空输入并入队（字符长度校验在入队时执行）。主循环在排空 `agent_rx` 后调用 `handlers::skills::flush_pending_when_idle`：状态进入 `Idle`/`Done` 后，按序把每条排队消息各自派发为一个 `SubmitTask`——tact-ui 命令驱动本就会串行处理在途 `SubmitTask`，因此每条都成为下一个用户回合。提交**纯自动**——不存在"立即发送"路径（`[Send now]` 按钮与 Normal 模式 `s` 键按用户要求移除："send now 去掉吧，自动处理即可"）。丢弃排队消息的**唯一**途径是 pending 块的 `[Cancel]` 按钮（`pending_cancel_btn_area` 命中测试在 mouse handler）——只清空队列、不影响运行中的任务。`/cancel` 与 Normal 模式 `c` 与队列**无关**：只取消在途任务，与功能引入前一致（用户决定："/cancel 也不用处理 prompt 队列"）。Esc 保持原语义（始终退出插入模式——误触不会中断任务）。提示行与 `↳ 消息` 行渲染在输入框上方（`render/input.rs` 的 `render_pending_block`，窄终端隐藏按钮）；布局把 `pending_display_lines()`（提示 + 每条一行，上限 4 行）计入输入高度。`submit_user_task` 拆成 `task_within_limits` / `dispatch_user_task`，使排队与自动提交共用派发。`/compact` 仍保留旧的忙时闪现（`input_busy_msg`）。

**Behavior after:** 忙时输入的消息被排队，显示在输入框上方并带 Codex 风格提示，当前任务结束后自动提交（包括被 `/cancel` 结束的任务）；`[Cancel]` 按钮只丢弃队列。多条排队消息按顺序各自成为独立回合。超长消息在入队时即被拒绝。忙时提交不再丢失。

**Pointers:** `handlers/insert.rs`（`handle_enter_submit`、Esc 分支）、`handlers/skills.rs`（`submit_user_task`、`flush_pending_when_idle`、`interrupt_and_submit_pending`）、`widgets/state/app/pending.rs`、`render/input.rs`（`render_pending_block`、`truncate_to_width`）；测试 `submit_queued_while_agent_busy`、`esc_with_pending_interrupts_and_submits_immediately`、`flush_pending_when_idle_submits_all_queued_in_order`、`input_box_renders_pending_block_above_input`；Ch 23 §6.6。

---

## 2. 2026-08-16 — 行内代码改用强调色文字，不再绘制背景补丁

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/render/pulldown.rs`、`crates/tui/src/render/log_style.rs` |

**现象 / 动机：** 上一个修复已经阻止含行内代码的正文行被重绘成整行代码块，但行内代码 span 自身仍携带窄的 `code_block_bg` 背景补丁。这个矩形背景在普通正文和列表中仍然显得过重，换行后尤其明显。

**决策：** 行内代码使用 `theme.accent` 作为前景色，不再绘制背景。日志重绘阶段对旧的行内代码背景 span 应用相同规则；真正的围栏代码行继续使用 `code_block_bg` 与 `code_block_fg`。

**变更后行为：** 行内代码通过强调色文字区分，不再出现矩形背景补丁。围栏代码块仍保留主题背景和前景色，因此代码块边界仍然清晰。

**指针：** `crates/tui/src/render/pulldown.rs`（`push_inline_code`）、`crates/tui/src/render/log_style.rs`（`restyle_log_line_with_skills`）；两个模块中的 `inline_code_uses_accent_without_background` 测试；Ch 23。

---

## 2. 2026-08-16 — 含行内代码的正文/列表行不再整行绘制代码背景块

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/render/log_style.rs` |

**现象 / 动机：** `restyle_log_line_with_skills` 只要某行的 span 里出现 `theme.code_block_bg()` 就把整行当作围栏代码行，重绘成整行代码背景。像 `- run `cargo build`` 这种常见列表项因此渲染成一整块高亮背景；当条目换行时，`wrap_line` 会把背景重新切片到每一续行，帧间残留成阴影状色带（反复出现的 "shadow" 类问题）。

**决策：** 只有**每个** span 都带代码背景的行（即 `flush_code_block` 产生的真正围栏代码行）才按代码块重绘。混合正文/列表行保留原样式；行内代码 span 保留其窄背景补丁（文字/前景特殊渲染仍在）。

**变更后行为：** 含行内代码的列表项与段落不再渲染成整行代码块，换行续行不再出现阴影色带。围栏代码块仍带主题代码背景（浅色主题下仍通过 `restyle_code_line` 修正前景色）。

**指针：** `crates/tui/src/render/log_style.rs`（`restyle_log_line_with_skills`、`restyle_code_line`）；测试 `inline_code_line_keeps_narrow_patch_not_full_block_bg`；Ch 23 渲染管线。

---

## 2. 2026-08-16 — 任务统计行支持多语言，并去掉过宽的 📊 图标

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/i18n.rs`、`crates/tui/src/widgets/state/app/messages.rs`、`crates/tui/src/handlers/mouse.rs` |

**现象 / 动机：** 每轮结束的统计行被硬编码为 `📊 任务统计：…`（`messages.rs` 中的 `TASK_STATS_PREFIX`），即使 UI 切到英文模式仍是中文，且过宽的 `📊` emoji 在日志里显得太大。`[copy]` 按钮同样是一段硬编码英文。

**决策：** 前缀与复制按钮移入 i18n `Messages` 表：EN `Task stats:` / `[copy]`，ZH `任务统计：` / `[复制]`，不再带 emoji 图标——过宽的 `📊` 前缀与 model 前的 `🧠` 都被移除。复制按钮渲染在**统计正文之前**，只有点击按钮字形本身才触发复制（其余位置正常做文本选择）。`add_task_stats_block` 通过 `self.msgs()` 读取；`is_task_stats_line` 现在识别所有支持语言的前缀（可带前置按钮），**并兼容旧的 `📊 任务统计：` 行**（保证旧会话里 `[copy]` 仍然可用）；鼠标处理器通过 `find_task_stats_copy_button` 定位本地化按钮的字节区间。

**变更后行为：** 统计行在英文下渲染为 `[copy]  Task stats:⏱ mm:ss · model · N tokens …`，中文下为 `[复制]  任务统计：⏱ mm:ss · model · N tokens …`，前缀与 model 前都不再有宽 emoji。旧会话的统计行仍可复制。

**指针：** `crates/tui/src/i18n.rs`（`task_stats_prefix`、`task_stats_copy_btn`）、`crates/tui/src/widgets/state/app/messages.rs`（`is_task_stats_line`、`find_task_stats_copy_button`、`add_task_stats_block`）、`crates/tui/src/handlers/mouse.rs`；测试 `task_stats_block_localizes_prefix_and_copy_button`、`task_stats_line_detection_covers_all_languages_and_legacy_rows`。

---

## 2. 2026-08-16 — `install.sh` 不再报 `tmp: unbound variable`，也不再泄漏克隆目录

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | `scripts/install.sh` |

**现象 / 动机：** 在 `set -u` 下，成功安装 release 后安装器打印 `bash: line 351: tmp: unbound variable`。`try_install_release` 对 `local tmp` 设置了 `trap 'rm -rf "$tmp"' RETURN`；RETURN trap 会被继承，并在调用者 `main` 返回时再次触发，此时 `tmp` 已越界，未加保护的 `$tmp` 展开因 `nounset` 中断。另外，`main` 把 `work` 声明为 `local` 并用 `EXIT` trap 清理；该 trap 在脚本退出时触发，此时 `main` 的局部变量已销毁，`${work:-}` 恒为空，导致 `git clone` 目录在源码构建 / 非仓库目录路径下泄漏。

**决策：** (1) release 临时目录 trap 改为 `trap '[[ -n "${tmp:-}" ]] && rm -rf "$tmp"' RETURN` —— 加保护后，继承到调用者上下文的那次触发成为 no-op，同时在 `try_install_release` 自身返回时仍能正确清理。(2) `work` 不再是 `local`，改为初始化为 `""` 的全局变量，使已有的 `EXIT` trap 能在脚本退出时真正删除克隆目录（包括 `die` / `exit 1` 路径）。

**变更后行为：** `curl … | bash`（以及 `./scripts/install.sh`）以 `Done. Run: tact-ui --help`、退出码 0 收尾，无 `unbound variable` 报错，release 临时目录与克隆仓库目录均被删除。

**指针：** `scripts/install.sh`（`try_install_release`、`main`）。

---

## 2. 2026-08-16 — `plugin install` 不再 panic，并支持官方 `url` 类型的插件源

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 02；`crates/tact-ui/src/main.rs`、`crates/tact/src/plugin/marketplace.rs` |

**现象 / 动机：** `tact-ui plugin install <plugin>@claude-plugins-official` 以两种方式失败。其一，主入口的自更新提前返回用了 `args.command.take()`，它会把*任何*非 `Upgrade` 的命令消费掉并置 `args.command = None`；于是 `Plugin`（以及 `Headless`）落到 `run_interactive`，而 `plugin` 命令解析配置时不会初始化 LLM provider（`install_without_llm`），交互路径因此在 `get_provider()` panic：`LLM provider not initialized`。其二，官方 `anthropics/claude-plugins-official` 目录使用了 `"source": "url"` 对象形式（286 个插件中的 150 个：`url` + `sha`，部分带 `path`），而 `PluginSource::from_catalog_value` 无法识别，导致整个目录解析失败，报 `invalid marketplace source: url`。

**决策：** (1) 自更新提前返回改为先 `matches!(args.command, Some(CliCommand::Upgrade { .. }))` 判断，仅在命中分支内 `take()`，非 upgrade 命令得以进入后续分发。(2) `from_catalog_value` 将 `git-subdir` 与 `url` 统一视为 Git 仓库源（克隆 `url`、可选 `path`、锁定修订），优先使用 `sha` 锁定，回退到 `ref`。

**变更后行为：** `plugin install`（以及 `headless`）正确分发；`plugin install frontend-design@claude-plugins-official` 会克隆官方目录、解析全部 286 条并安装 `frontend-design`（1 个 skill），锁定到其固定修订。插件命令从不依赖 LLM provider。

**指针：** `crates/tact-ui/src/main.rs`（自更新提前返回）、`crates/tact/src/plugin/marketplace.rs`（`RawPluginSource::Object.sha`、`PluginSource::from_catalog_value`）；测试 `parses_url_plugin_source`、`parses_url_plugin_source_with_subdirectory`、`git_source_falls_back_to_named_ref_without_a_sha`；spec `docs/superpowers/specs/2026-07-20-plugin-install-design.md`。

---

## 2. 2026-08-16 — 覆盖式列表弹窗限定在主区域内

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/lib.rs`、`crates/tui/src/render/test_harness.rs` |

**现象 / 动机：** 主帧循环里渲染的四个覆盖式弹窗（`command_palette`、`select`、`file_picker`、`slash_command`）以**整屏**为基准居中，高度上限是 `frame.height - 4`。终端较矮、命令/文件/选项较多时，弹窗高过日志面板，盖住命令行输入框和底栏；弹窗列表行与输入框边框字形（弹窗 `Clear` 矩形之外的部分）交错，看起来像一团阴影/错乱，用户也看不到正在输入的过滤词。

**决策：** 弹窗调用点传入**主区域**（`chunks[1]`：下方是状态栏、上方是输入框）而不是整帧。弹窗只在主区域内居中并限制高度。

**变更后行为：** palette / select / 文件选择 / 斜杠命令弹窗无论列表多长、终端多矮，都保持在输入框和底栏之上；过滤输入时输入框始终完整可见。

**指针：** `crates/tui/src/lib.rs`（帧循环弹窗调用）、`crates/tui/src/render/test_harness.rs`（`draw_full_ui`，同步保持）、`crates/tui/src/render/popup_scene_tests.rs`（`full_frame_palette_popup_stays_inside_main_area`）；Ch 23。

---

## 2. 2026-08-16 — 主区域标题不再绘制高亮色块

| 字段 | 内容 |
|-------|-------|
| **类型** | bugfix |
| **相关** | Ch 23；`crates/tui/src/render/log_style.rs` |

**现象 / 动机：** restyle 通道（pulldown-cmark 迁移 #69 时加入）会给 H1 标题涂上 `theme.highlight` 背景——这是 tui-markdown 直接给 H1 上背景的遗留行为。日志面板里色块只覆盖标题的字形列；标题换行时每一行都会带色块，于是「长标题 + 列表」在文字后面形成一大片类似阴影的色块。整段 Markdown 路径（`MarkdownCell`，如 `/skills` 分页）从不绘制该色块，两条主区域路径表现不一致。

**决策：** restyle 通道不再给标题 span 赋任何背景（pulldown 渲染器本身就不带背景）。同时删除已失效的 `Color::Rgb(70, 90, 140)` → `theme.highlight` 映射（fork 移除后无来源）。

**变更后行为：** H1 标题在两条主区域路径中都渲染为无背景的加粗+下划线标题色文本；（换行的）列表标题后面不再出现高亮阴影块。

**指针：** `crates/tui/src/render/log_style.rs`（`restyle_log_line_with_skills`、`heading_keeps_no_background`）、`crates/tui/src/render/log_render_tests.rs`（`heading_rows_carry_no_highlight_band`）；Ch 23 渲染管线。

---

## 2. 2026-08-15 — `/stats` 弹窗直接用 ratatui-markdown 渲染

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | system-prompt 弹窗（`/stats` 会话统计与组装后的 system prompt 视图共用）此前走 pulldown-cmark 管线 + Tact 自研 width-aware pipe 表格，并按弹窗内容宽度布局。对一个快速统计弹窗而言，这套额外布局机制不值得。 |
| 决策 | `render_system_prompt_popup` 改为通过 `render_markdown_ratatui`（`crates/tui/src/render/render_md.rs`）渲染：按弹窗内容宽度使用普通 `ratatui_markdown::markdown::MarkdownRenderer`，复用与 Mermaid 渲染器相同的 `TuiRichTextTheme`。width-aware 表格与 Mermaid 路由保留给主区域 Markdown cell。 |
| 改后行为 | `/stats` 与 system-prompt 弹窗由 ratatui-markdown 默认渲染器布局（含表格）；弹窗测试（`session_stats_popup_renders_gfm_table`）原样通过。 |
| 指针 | `crates/tui/src/render/popups/system_prompt_popup.rs`、`crates/tui/src/render/render_md.rs`（`render_markdown_ratatui`）；`docs/token_usage_schema.md` Session Stats Display。 |

---

## 2. 2026-08-15 — 自动压缩的摘要调用不再开启 thinking

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | 本地压缩摘要调用此前会转发 agent 的 Claude 式 `thinking_budget`（`with_thinking`，并限制在线上 `max_tokens` 之下）与显式 `reasoning_effort`，并在文本预算之上按 effort 分档预留 reasoning 份额。对手交摘要而言思考价值不大，且会从同一个 `max_tokens` 信封（effort 模型）中占用输出 token——用户要求自动压缩时不再开启 think。 |
| 决策 | 摘要请求不再携带任何 thinking：不转发 `thinking` 块、不转发 `reasoning_effort`（主循环的 thinking 配置不受影响），输入预留也不再扣除 thinking budget。**服务端默认** reasoning 预留仅保留给 DeepSeek / Kimi K3（固定为文本预算的 75% 追加在文本之上）：即使请求省略 effort，它们服务端默认 thinking 开启 + effort high，没有该预留其强制 reasoning 会挤占摘要文本、触发截断续写。原生 `/responses/compact` 请求本就只带 `{model, input}`，其无效的 `.with_reasoning_effort` 一并移除。 |
| 改后行为 | 摘要调用为普通非流式 `create_message`，`max_tokens` = 经典文本预算（OpenAI / Anthropic）或文本 + 75% 预留（DeepSeek / Kimi K3）；压缩期间发出的 `AgentUpdate::ModelInfo` 不报告 thinking/effort。 |
| 指针 | `crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode`、`compact_summary_reasoning_reserve_percent`、`compact_responses_native`）、book [Ch 5](./05_chapter_compact_zh.md) §摘要调用。 |

---

## 2. 2026-08-15 — 底栏 `out` 更名为 `max_out_token`，显示真实输出额度

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | 底栏输出段此前标记为 `out`/`输出`，直接显示原始 `max_tokens` 信封。对 effort 语义模型（openai / deepseek / kimi k3），reasoning 与输出文本算在同一个信封内，`think high` 旁边的 `out 128K` 高估了真正留给文本的 token——用户要求该段显示 **max output token** 值，并扣除 reasoning 份额。 |
| 决策 | 标签改为 `max_out_token`（两种语言统一用该标识符），数值改为真正的文本输出额度：effort 语义模型按压缩预留的同一分档约定扣除 reasoning 份额（预留为文本预算的百分比并追加在文本之上 → 文本 = 信封 × `100/(100+pct)`；128K 信封 + `high` → 73K）。budget 语义模型（Anthropic 式 `thinking_budget`）的 thinking 走独立信封，仍显示完整 `max_tokens`。在 TUI 内由 `status_bar.model_max_tokens` + `model_thinking_budget` + `model_reasoning_effort` 计算，无协议改动。 |
| 改后行为 | 底栏第 2 行显示 `max_out_token {n}` 取代 `out {n}`；无 think / budget 语义时 `n` = `max_tokens`，显示 effort 时 `n` = `max_tokens × 100/(100+pct)`（`none`/无 effort → 不变，`low` → 80%，`medium` → ~67%，`high` → ~57%，`xhigh`/`max` → 50%）。 |
| 指针 | `crates/tui/src/render/bar.rs`（`format_max_out_tokens`）、`crates/tui/src/i18n.rs`（`bottom_out`）、book [Ch 23](./23_chapter_tui_zh.md) §6.6、`docs/token_usage_schema.md`。 |

---

## 2. 2026-08-15 — 模型→上下文窗口映射覆盖手工 `model_context_window` 配置

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 现象 / 动机 | `agent.model_context_window` 此前完全手工指定（CLI/TOML，默认 `200_000`），没有任何模型推断。使用 `deepseek-v4-pro`（真实窗口 1M）时，底栏 `ctx` 计量因残留的 256k 配置显示 `…/256K`，自动压缩也在 ~80%（约 205k）处触发而非 ~800k，导致过早压缩。`max_tokens` 已有按模型的默认值可参照（`kimi_k2x → 32_000`），而窗口没有等价机制。 |
| 决策 | 在 `resolve.rs` 新增 `model_context_window_for_model(model)`，并按 **模型→窗口映射（最高）→ CLI/TOML → 默认 `200_000`** 解析窗口。数值依据官方模型文档（2026-08）：OpenAI `gpt-5.6` 系列 + `gpt-5.5` → `1_050_000`、`gpt-5.4` → `1_000_000`、`gpt-5`…`gpt-5.3`/`gpt-5.4-mini` → `400_000`、`gpt-4o` 系列 → `128_000`；Anthropic（API 与 Claude Code 同 ID）`claude-sonnet-5`/`claude-fable-5`/`claude-opus-5`/`claude-opus-4-8`/`claude-opus-4-7`/`claude-opus-4-6`/`claude-sonnet-4-6` → `1_000_000`、`claude-sonnet-4-20250514`/`claude-opus-4-20250514`/`claude-haiku-4-5`/`claude-haiku-4-20250514` → `200_000`；DeepSeek V4 → `1_000_000`、`k3-256k` → `256_000`。命中映射时**刻意**覆盖用户文件配置，避免过时的手工窗口低估已知模型。 |
| 改后行为 | `ctx` 底栏计量与派生的自动压缩阈值（窗口的 80%）对已映射模型使用映射后的窗口。GPT-5.6/5.5 系列显示 `…/1.05M`、Claude 1M 模型（含 Claude Code ID）`…/1M`、DeepSeek V4 `…/1M`、GPT-5.x `…/400K`、GPT-4o `…/128K`、`k3-256k` `…/256K`。手工 `model_context_window` 仅对无内置映射的模型生效。非零 `model_context_window > max_tokens` 的校验仍作用于解析后的最终值。 |
| 指针 | `crates/tact/src/config/resolve.rs`（`model_context_window_for_model`，解析位于 ~`:587`）；`config.example.toml` `[agent]`；book [Ch 21](./21_chapter_config_zh.md) §5、[Ch 5](./05_chapter_compact_zh.md) 设置表。 |

*（2026-09-13 被取代——优先级已反转：显式的 CLI 标志或 `[agent]` 配置现在优先，映射降级为未配置模型时的回退。见最新条目；其中还保留"过时手工窗口会低估长上下文模型"这一代价，但改为文档说明而非强制。）*

---

## 2. 2026-08-15 — Markdown 正文迁移到 pulldown-cmark；ratatui-markdown 仅保留 Mermaid

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Plan | `docs/superpowers/plans/2026-08-15-pulldown-cmark-migration.md` |
| Symptom / motivation | 下方条目所述的整合方案最终落在本地 `ratatui-markdown` fork 上，仅为了让 Tact 的正文渲染对齐 `tui-markdown` 的输出，就携带了约 350 行、跨 8 个文件的补丁（H4–H6、有序编号、硬换行、嵌套强调、CJK 左翼、主题代码色槽）。fork 是 rebase 负担，且是只在本机可解析的 path 依赖；`steer` 与 xAI 的 `grok-build` 都用 `pulldown-cmark` 解析 CommonMark、在自己代码里渲染，而非 fork 一个 Markdown 库。 |
| Decision | 用 `pulldown-cmark` 0.13 的事件循环（`crates/tui/src/render/pulldown.rs`）替代 fork 的块渲染器，复用 Tact 的宽度感知管道表 `format_table`、`▎` 引用块 gutter、fenced-code 无框主题样式与 Mermaid 路由。`ratatui-markdown` 改为上游 git 依赖（`celestia-island/ratatui-markdown` @ `3a8bcbe`，仅 `mermaid` feature），只用于非 sequence 的 Mermaid 图；`sequenceDiagram` 仍是 Tact 自研的 `mermaid_sequence.rs`。删除 `feat/tact` fork 与 `TuiRenderHooks`/`RenderHooks` 适配器。 |
| Behavior after | 正文/标题/列表/任务/表格/引用块渲染由 Tact 依据 `pulldown-cmark` 事件自持。刻意关闭 `ENABLE_SMART_PUNCTUATION`，使 `...` 不被转成 `…`（系统消息与用户文本保持字节稳定）。GFM 任务列表现在同样作用于有序列表（`1. [X]` → `1. ☑`）。Mermaid 输出不变。 |
| Pointers | `crates/tui/src/render/pulldown.rs`、`render_md.rs`；`Cargo.toml`（`ratatui-markdown` git 依赖 + `pulldown-cmark`）；book [Ch 23](./23_chapter_tui_zh.md) §6.7；下方条目记录了中间的 fork 方案。 |

---

## 2. 2026-08-15 — 主区域 Markdown 渲染统一到 ratatui-markdown

| 字段 | 内容 |
|-------|-------|
| 类型 | `optimization` |
| 计划 | `docs/superpowers/plans/2026-08-15-ratatui-markdown-migration.md` |
| 症状 / 动机 | TUI 主区域并行维护两套 Markdown 栈：`tui-markdown` 0.3.x（crates.io）渲染日志面板的正文 / 标题 / 列表，`ratatui-markdown`（celestia-island git fork，按分支固定）渲染 Mermaid 与 `/tasks-dag` 弹窗。两套样式适配器、两套调色板，以及各种 fork 规避（任务列表标记转义、`log_style.rs` 中的硬编码色重映射、围栏标记簿记）都必须同步维护。 |
| 决策 | 统一到 `ratatui-markdown`（本地 fork 位于 `../ratatui-markdown`，分支 `feat/tact`，基于 `chore/update-ratatui-0.30` @ `3a8bcbe`）并补齐能力：H4–H6 标题、有序列表保留编号、每级 4 列嵌套缩进、递归嵌套强调（`**bold _x_ italic**`）、链接保留 URL 后缀、行内代码 / 围栏代码的主题色槽位、软换行折叠为空格 + 硬换行保留、连续空格保留、CJK 标点友好的强调判定。Tact 侧：`render_plain_markdown` 用 fork 的 parse+render 替换 `tui_markdown::from_str_with_options`，并通过 `TuiRenderHooks`（隐藏围栏 / 边框装饰、直接上代码背景）；引用 `▎` gutter 与 H1 高亮背景移入后处理 / restyle 阶段；表格通过 `table` RenderHooks 适配器委托给 Tact 的宽度感知 `format_table`（管道风格），因此 fork 自带的 `render_table` 不再被使用、保持上游原样；三击代码块检测改为匹配代码背景而非 ````` ``` ````` 标记；diff 弹窗直接使用 `syntect` 高亮（同一 Base16 Ocean Dark 主题），从而移除 `tui-markdown` 与直接依赖的 `pulldown-cmark`。 |
| 行为后 | 日志面板经单一 crate 渲染 Markdown。无序列表渲染为 `•`，任务项渲染为 `☐` / `☑`（原先为字面 `-` / `[ ]` 文本）；围栏代码保持主题背景且无围栏标记；raw 复制行镜像渲染文本（标记在解析阶段被消费）；`/stats` 的 GFM 表格经 `format_table` 渲染为管道表；diff 弹窗保留语法高亮。 |
| 指针 | `crates/tui/src/render/render_md.rs`（`TuiRenderHooks`、`render_plain_markdown`、`apply_blockquote_indicator`）、`crates/tui/src/render/log_style.rs`（H1 高亮规则）、`crates/tui/src/render/popups/diff_popup.rs`（syntect）、`crates/tui/src/widgets/state/app/popups.rs`（`find_code_block_containing_logical`）、`Cargo.toml`（`ratatui-markdown` path 依赖 + `syntect`）；fork 仓库 `../ratatui-markdown` `feat/tact`；[Ch 23](./23_chapter_tui_zh.md) §6.7。 |

---

## 2. 2026-08-15 — Thinking、命令输出与 Read 卡片顶部移除重复行数，统一由底部栏承载

| 字段 | 内容 |
|------|------|
| 类型 | `removal` |
| 症状 / 动机 | Thinking 卡片把总行数显示了两遍——顶部标题（`🧠 Thinking (N lines)`）与底部栏（`↕ 可见/N 行 …`）各一次；`bash` 命令输出卡片同样重复（顶部 `Live output (N lines)` / `Command output (N lines)`，底部 `preview/total 行` 提示）；`read_file` 卡片也是如此（顶部 `Read <路径> (N lines)`）。两者同时可见时，顶部计数与底部栏数字冗余。 |
| 决策 | 卡片顶部标题不再携带行数：`🧠 Thinking`（active 与 completed 一致）、`Live output`（运行中 bash）、`Command output`（已完成 bash）、`Read <路径>`（read_file）。底部栏成为唯一计数来源（Thinking 的 `↕ visible/total 行`；命令输出溢出预览时的 `preview/total 行`）。删除不再使用的 `thinking_card_title_pl` 字段；`tool_live_output_title_tmpl` 去掉 `{}` 占位符并更名为 `tool_live_output_title`。 |
| 改后行为 | Thinking 卡片显示 `🧠 Thinking` / `🧠 思考中`；运行中的 bash 卡片显示 `Live output` / `实时输出`；完成的命令卡片显示 `Command output`；Read 卡片显示 `Read <路径>`。所有行数都在卡片底部栏。Popup 标题不变（本就用命令文本或裸 `Command output`）。 |
| 指针 | `crates/tui/src/i18n.rs`、`crates/tui/src/render/cells/thinking.rs`、`crates/tui/src/widgets/tool_widget.rs`（`detail_card_title`）、`crates/tui/src/render/cells/tool.rs`（`card_bottom_text`）；测试 `live_output_total_excludes_command_prefix_but_popup_keeps_it`、`log_tool_card_renders_when_scrolled_into_placeholder_rows`；[Ch 23](./23_chapter_tui.md) §render pipeline。 |

## 2. 2026-08-15 — Log 按词边界折行；文字选择交互对称化

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | （1）`wrap_line` 在显示宽度处硬切每行（`split_at_display_width`），长 URL、路径与单词从中间断开且无续行提示；（2）跨折行的部分选择在续行上丢失 REVERSED 高亮——旧折行路径把所有 span 摊平为一个基础样式；（3）双击选词只认 ASCII，双击中文选不中任何内容；（4）选择交互不对称：点击/拖入整段 Markdown 行会产生看不见的选择（MarkdownCell 渲染器不画叠加层），点空白区或面板外则保留过期选择；（5）鼠标 hit-test 按面板宽度模拟硬折行，而渲染按宽度 − 缩进折行，缩进行上最多偏移一个缩进宽。 |
| Decision | 新增共享的 `wrap_break_offsets` 一次性计算视觉行起始偏移，`wrap_line` 与 `visual_pos_to_byte_offset` 共用，渲染与 hit-test 不可能再分歧。折行改为贪心词边界折行：在最后一个放得下的空白处断开；仅当连续词超过行宽才硬切；尾随空白留在上一行（不可见），保证分段字节连续。`wrap_line` 按分段重新切片原始样式 span，使逐段样式（含 REVERSED）延续到续行。`find_word_bounds` 按光标下字符分类为 ASCII 词或 CJK 连续段（汉字/假名/谚文）并在同类内扩展。`handle_log_click`/`handle_mouse_drag`/`handle_log_triple_click` 拒绝在 Markdown 行上开始或扩展选择；点击日志下方空白或面板外的任意位置清除选择。hit-test 的折行宽度减去行缩进。 |
| Behavior after | 单词不再从中间断开（URL/路径/CJK 保持完整直到确实超宽）；选择高亮在每条续行都可见；双击可选中整段中文；Markdown 卡片不再被"静默选中"；误点击清除过期选择而非保留；缩进行上的点击映射到正确字节。 |
| Pointers | `crates/tui/src/render/util.rs`（`wrap_break_offsets`、`wrap_line`、`visual_pos_to_byte_offset`、`col_to_byte_offset`）、`crates/tui/src/widgets/state/app/visibility.rs`（`find_word_bounds`、`is_markdown_row`、`byte_offset_from_log_position`）、`crates/tui/src/handlers/mouse.rs`（点击/拖拽/三击守卫与面板外点击清除）、`crates/tui/src/render/cells/text.rs`；测试 `wrap_break_offsets_prefers_word_boundaries`、`wrap_line_keeps_word_intact_and_preserves_span_styles`、`wrap_break_offsets_agree_with_byte_offset_hit_testing`、`partial_selection_reverses_target_span_across_wrapped_lines`、`double_click_selects_cjk_run`、`click_below_last_message_clears_selection`、`click_on_markdown_row_does_not_create_invisible_selection`、`drag_into_markdown_row_does_not_extend_selection`、`click_outside_log_clears_selection`；[第 23 章](./23_chapter_tui_zh.md) 渲染管线节。 |

## 2. 2026-08-15 — Log 滚动改为视觉行；`/skills` 分页

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Log 面板按逻辑消息行滚动（`j`/`k`/滚轮每格 `log_scroll.offset ± 1`）。高于 viewport 的整段 Markdown 消息——例如 `/skills` 约 60 个 skill、400+ 渲染行的管道表格——只能看到首尾两屏：`resolve_visual_scroll` 在最大逻辑偏移处钉底，位于中间的行（按字母序排列的 `lark-*`）双向都滚不到。 |
| Decision | viewport 的首条可见**视觉**行成为权威状态（`LogScroll.visual_top`，`usize::MAX` = 钉底哨兵）；`offset` 变为派生的逻辑镜像，仅供只读消费方（鼠标 hit-test、code 弹窗）使用。纯函数 `visual_step_up/down` 在高于 viewport 的 cell 内部按 `j`/`k` 半屏、滚轮 3 行步进，其余情况按行边界跳转；从下方进入高 cell 时落在其底部，保证向上遍历连续。删除 `resolve_visual_scroll` / `effective_max_logical_scroll`。`/skills` 输出额外按 15 个 skill 一页分块，每块一条带 `(n/k)` 标题的 Markdown 消息。 |
| Behavior after | 任何高于 viewport 的 cell（长表格、展开的工具卡片）都可用 `j`/`k`/滚轮双向完整遍历；`g`/`G` 仍跳转顶/底，自动跟随流式输出保持可用（`is_log_pinned_to_bottom` 改为比较视觉位置）。`/skills` 每页渲染 15 个 skill 并带页码标题。 |
| Pointers | `crates/tui/src/widgets/state/app/scroll.rs`（步进函数与滚动 API）、`crates/tui/src/widgets/state/log_scroll.rs`（`visual_top`）、`crates/tui/src/render/log.rs`（视觉钳制与镜像派生）、`crates/tui/src/handlers/{normal,mouse,mod}.rs`（按键、滚轮、`/skills` 分页）、`crates/tui/src/widgets/state/app/{agent,messages,visibility}.rs`（钉底辅助）；回归测试 `tall_markdown_cell_is_fully_traversable`、`skills_command_paginates_long_lists`；[第 23 章](./23_chapter_tui_zh.md) 渲染管线节。 |

## 2. 2026-08-15 — 主区域渲染打磨：Markdown 缩进、主题化链接、代码背景、隐藏标记

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | 渲染路径审查发现：(1) 整段 Markdown 消息（`MarkdownCell`，如 `/skills`）贴左边渲染，而流式回复/工具卡有缩进；(2) 链接使用硬编码调色板 `Blue`，永不随主题变化；(3) `is_user_message_line` 每渲染一行就向块头回退扫描，长段粘贴时呈平方复杂度；(4) `TextCell` 把所有 span 背景压平为面板底色，流式回复里的围栏代码丢失背景（与 `MarkdownCell` 不一致）；(5) 原始 Markdown 标记（`# `、`> `、``` 围栏）泄漏进渲染文本。 |
| Decision | (1) `append_markdown` 统一使用 `LOG_THINKING_INDENT + 1` 缩进；(2) 链接改用 `theme.heading`，restyle 里旧 `Blue` 重映射；(3) 每帧预计算单遍 `user_line_mask`，restyle 与缩进共用；(4) `TextCell` 保留 span 自带背景（代码 bg、H1 highlight），restyle 仅把代码背景的 span 当代码处理；(5) styled 行隐藏围栏行（渲染为空白）并剥除 `#{1,6} ` / `> ` 前缀，`raw_messages` 保留原始 Markdown 供复制、代码块检测与 hit-test；流式文本改用与最终行一致的 fg，回复完成时不再变色。 |
| Behavior after | 流式回复中的代码块有背景；H1 保留 highlight 色带；引用渲染为 `▎ text`；标题不带 `## `；链接随主题适配；长粘贴不再触发平方级回退扫描；`/skills` 等 Markdown 通知与回复对齐。 |
| Pointers | `crates/tui/src/render/{log.rs,log_style.rs,render_md.rs}`、`crates/tui/src/render/cells/{text.rs,markdown.rs}`、`crates/tui/src/widgets/state/app/{popups.rs,visibility.rs}`；测试 `span_backgrounds_survive_rendering`、`heading_keeps_no_background`、`user_line_mask_matches_the_per_row_walk`、`hardcoded_blue_links_remap_to_theme_heading`、`render_markdown_fenced_code_block`、`render_markdown_heading_markers_are_stripped`、`indented_cell_shifts_content_right`；[第 23 章](./23_chapter_tui_zh.md) 渲染管线节。 |

## 2. 2026-08-14 — 移除 cron 调度功能

| 字段 | 内容 |
|------|------|
| 类型 | `removal` |
| 现象 / 动机 | cron 功能（`cron_create` / `cron_list` / `cron_delete`）只持久化调度记录；没有任何代码解析表达式或将存储的 prompt 注入 `agent_loop`。用户要求设提醒后得到的是错误的安全感——记录存在但永远不会触发。进程内 tick loop 被判定为方向错误：需要交互式 TUI 进程常驻，且与早已可靠运行的系统 cron 重复造轮子。 |
| 决策 | 整体移除：`crates/tact/src/cron/`（调度器 + 模拟）、`crates/tact/src/store/cron_store/`（trait + SQLite 实现）、`crates/tact/src/tool/cron.rs`（工具）、`ToolContext.cron_scheduler` 字段、registry 路由、`headless.rs` / `interactive.rs` 启动接线、文档中的 `cron_tasks` 表行、TUI 工具名映射与相关测试。删除 book 第 16 章（中英）；清理第 1 / 4 / 7 / 12 / 14 / 15 / 19 / 23 章、`index.md`、`mindmap.md`、`ARCHITECTURE.md` 中的 cron 引用。 |
| 改后行为 | 不再有 `cron_*` 工具；模型无法再创建定时提示。存量 `cron_tasks` 行与遗留 `.tact/cron/` 文件保留在磁盘上不动（死数据，可手动清理）。 |
| 指针 | 已删除：`crates/tact/src/cron/*`、`crates/tact/src/store/cron_store/*`、`crates/tact/src/tool/cron.rs`、`book/16_chapter_cron*.md`；已编辑：`crates/tact/src/lib.rs`、`crates/tact/src/tool/{mod,registry}.rs`、`crates/tact/src/tool/test_support.rs`、`crates/tact/src/store/mod.rs`、`crates/tact-ui/src/{headless,interactive}.rs`、`crates/tact-ui/tests/{subsystem_tools.rs,harness/mod.rs}`、`crates/tui/src/widgets/tool_widget.rs`、`book/01_chapter_store*`；[Ch 1](./01_chapter_store_zh.md)、[Ch 7](./07_chapter_tool_zh.md)。 |

## 2. 2026-08-14 — 后台输出改为混合存储：全量日志文件 + 记录上的 `output_path`

| 字段 | 内容 |
|------|------|
| 类型 | `optimization` |
| 现象 / 动机 | `background_run` 的输出只存在 `background_tasks` DB 记录里，且上限为前 50,000 字符——cap 保留的是长日志的*开头*（通常最没用），丢弃的结尾恰恰是报错所在。模型无法深挖大输出：轮询 `check_background` 会把整个 ≤50k JSON 拉进 context，而 `grep`/`tail` 因为没有文件而无从谈起。 |
| 决策 | 混合存储：DB 记录保留元数据 + 前 50k 字符（不变，轮询便宜），**全量** stdout+stderr 流随到达即追加写入 `<workdir>/.tact/background/<id>.log`。`BackgroundTaskRecord` 新增 `output_path`（SQLite 列 `output_path TEXT NOT NULL DEFAULT ''`，对存量库用 `PRAGMA table_info` + `ALTER TABLE` 迁移）。日志文件创建为 best-effort——失败时退化为仅截断记录。`check_background` 列表每行追加 `(log: <path>)`。 |
| 改后行为 | 轮询 JSON 带 `output_path`；agent 可用 `bash tail <path>` / `grep error <path>` 深挖全量日志，而非吞下 50k blob。日志文件从任务启动即存在（spawn 前记录已写入路径），长任务可实时查看。 |
| 指针 | `crates/tact/src/background.rs`（`BackgroundTaskRecord.output_path`、`open_log_file`、`log_write`、`run_background_process`）、`crates/tact/src/store/background_store/sqlite.rs`（schema + 迁移 + upsert/读取）、`crates/tact/src/tool/background_run.rs`（列表）；测试 `run_writes_full_output_to_log_file_and_truncates_db_record`、`migrates_legacy_table_without_output_path`；[Ch 13](./13_chapter_background_zh.md) §2、§3、§6、§8；[Ch 1](./01_chapter_store_zh.md)。 |

## 2. 2026-08-13 — Plan mode 只读 shell 分类加固：拒绝换行符命令分隔

| 字段 | 内容 |
|------|------|
| 类型 | `bugfix` |
| 现象 / 动机 | `split_plain_command` 把 `\n` / `\r` 当作普通空白跳过，但对 `sh -c` 而言裸换行（以及 CRLF 输入中的 `\r`）是命令分隔符而非词分隔符。于是 `ls\nrm file` 能通过纯命令切分，白名单首词 `ls` 被归为 Read，在 plan mode 下自动放行——第二条变更命令被静默携带执行。 |
| 决策 | 切分器在扫描分隔符时一旦遇到裸 `\n` / `\r` 立即返回 `None`，任何含换行的多命令字符串保持未分类（回退到 `Write` / 提示）。单/双引号内的字面换行仍是词字符，继续放行。同一次改动把 git 全局选项处理统一进单张 `GIT_GLOBAL_OPTIONS` 表（每条记录是否消费下一个 token），同时驱动 `find_git_subcommand` 的跳过逻辑与 `git_has_unsafe_global_option`，避免两处检查再次漂移。 |
| 改后行为 | `ls\nrm file`、`echo hi\nrm -f x`、CRLF 变体及行首/行尾裸换行一律归为 **Write**（plan mode 下提示/拒绝）；`echo "line1\nline2"`、`cat "file\nname"`（引号内字面换行）仍为 Read。 |
| 指针 | `crates/tact/src/tool/readonly_shell.rs`（`split_plain_command`、`GIT_GLOBAL_OPTIONS`、`find_git_global_option`、`git_has_unsafe_global_option`）；同文件回归测试；[Ch 10](./10_chapter_permission_zh.md) §7。 |

## 2. 2026-08-13 — OpenAI 兼容 Chat Completions 将传输失败以 `LlmError::Request` 呈现

| 字段 | 内容 |
|------|------|
| 类型 | `bugfix` |
| 现象 / 动机 | OpenAI 兼容适配器把发送/连接/读响应失败报成 `LlmError::Unsupported("HTTP request failed: …")`，混淆了"端点不支持"与"请求根本没发出去"；token 数用 `u64 as u32` 强转（超限时截断误导）；工具调用 `arguments` 载荷非法 JSON 时被静默替换为 `{}`，无任何痕迹。 |
| 决策 | 新增 `LlmError::Request(String)` 变体承载请求传输/反序列化错误（API HTTP 错误仍走 `HttpError`）。流式与非流式路径都改发已序列化的 JSON 字节（`body` + 显式 `Content-Type`），不再用 `.json()` 二次序列化。token 数 `u64 → u32` 饱和转换（`u32_token_count`）。非法工具参数在 `debug` 级记录（error、工具名、原始 args）后回退为空对象。 |
| 改后行为 | 端点不可达/连接断开时显示 `request error: …` 而非 `unsupported: …`；超限 token 数饱和而非回绕；非法工具参数可在 debug 日志中看到。 |
| 指针 | `crates/tact_llm/src/error.rs`（`LlmError::Request`）、`crates/tact_llm/src/openai/compatible/mod.rs`（`OpenAiAdapter` chat/流式路径、`u32_token_count`、`tool_use_block_from_parts`）；[Ch 22](./22_chapter_llm_zh.md)。 |

## 2. 2026-08-13 — TUI 输入框长行软换行，光标随折行后的显示行定位

| 字段 | 内容 |
|------|------|
| 类型 | `bugfix` |
| 现象 / 动机 | 输入框高度与行数统计只数显式 `\n`，单条超长行会溢出 3 行上限；光标/滚动按逻辑行计算，与实际渲染不一致（长输入时光标画错行列）。 |
| 决策 | `render/input.rs` 新增 `wrap_line`（按字符边界软换行、CJK 双宽感知；`Paragraph` 保持不换行、逐行绘制这些切分）与 `caret_in_wrapped`（逻辑光标列 → 显示行列）。框高、行数统计、滚动钳制与光标定位全部改用显示行。 |
| 改后行为 | 长行在框内折行而非溢出；高度随折行自动扩展（1–3 显示行 + border）；光标与滚动跟随折行行。提交文本不变。 |
| 指针 | `crates/tui/src/render/input.rs`（`wrap_line`、`caret_in_wrapped`）；`crates/tui/src/lib.rs`（输入框高度）；测试见 `input.rs`（`wrap_line_splits_at_column_width`、`caret_in_wrapped_maps_logical_column_to_display_row`、`input_box_soft_wraps_overlong_line`、`input_box_scrolls_to_caret_on_wrapped_line`）；[第 23 章](./23_chapter_tui_zh.md) §6.2、§6.6。 |

## 2. 2026-08-13 — Plan mode 可运行可证明只读的 shell 命令（`ls`、`grep` 等）

| 字段 | 内容 |
|------|------|
| 类型 | `optimization` |
| 现象 / 动机 | Plan mode 拒绝一切归类为 `Write` 的工具，而 shell 命令此前一律归为 `Write`（仅 `sudo ` / `su ` 开头为 `High`），导致 `ls` / `grep` 这类规划 agent 最需要的探查命令在 plan mode 下也被硬拒绝。 |
| 决策 | `PermissionPolicy::ShellCommand::resolve` 现在在命令**可证明只读**时将其归为 `Read`，依托新的保守分类器 `crates/tact/src/tool/readonly_shell.rs`：(1) 纯命令切分，拒绝任何 shell 元字符（管道、重定向、`$`、反引号、glob、转义等），保证分类不会与 `sh -c` 实际执行内容产生分歧；(2) 白名单程序（仅凭选项无法写入），如 `ls`、`grep`、`cat`、`head`、`tail`、`wc`、`git status/log/diff/show/branch`，以及排除危险旗标的 `find`/`rg`/`base64`/`sed` 等，镜像 OpenAI Codex 的 `is_known_safe_command`（`codex-rs/shell-command/src/command_safety/is_safe_command.rs`）。任何含糊输入一律保持 `Write` ——分类器刻意偏向漏判，确保变更类命令不可能在 plan mode 下被静默执行。 |
| 改后行为 | Plan mode 下 `ls -la`、`grep -rn x .`、`git status` 无需提示即可运行；`cargo test`、管道、重定向、未知程序与不安全选项（`find -delete`、`git push` 等）仍被拒绝。`bash` 与 `background_run` 共用同一分类，因此只读命令在 Default 模式下也会自动放行。 |
| 指针 | `crates/tact/src/tool/readonly_shell.rs`；`crates/tact/src/tool/metadata.rs`（`ShellCommand::resolve`）；测试见 `crates/tact/src/tool/readonly_shell.rs` 与 `crates/tact/src/permission/mod.rs`（`plan_mode_allows_readonly_shell_commands_and_denies_others`）；[Ch 10](./10_chapter_permission_zh.md) §2、§4、§7。 |

## 2. 2026-08-12 — async-openai 从 `vendor/async-openai` 切换到本地维护的 fork `../async-openai`

| 字段 | 值 |
|------|-----|
| 类型 | `docs`（依赖管理） |
| 症状 / 动机 | `vendor/async-openai` 下的 vendored 拷贝（2026-08-10 条目）能用，但把整个 crate 复制进了仓库：每次同步上游都要 diff、重新打补丁，还要在 Tact 的最小 feature 集下保持 crate 级 doctest 可编译。 |
| 决策 | fork 改为独立仓库，位于 `../async-openai`（克隆自 `https://github.com/rust-infra/async-openai`，分支 `feat/tact`，commit `ca74607` = 上游 main 0.41.3），直接维护并包含四个本地提交：(1) `CreateResponse` 增加类型化字段 `context_management: Option<Vec<ContextManagementParam>>`；(2) `ReasoningEffort` 增加 `Max` 变体；(3) 两个调用 `client.chat()` 的 doctest 增加 feature gate；(4) package 改名为 `async-openai-local` 并显式 `[lib] name = "async_openai"`（fork workspace 移除 examples，因为它们仍引用上游包名）。Tact workspace 依赖变为 `async-openai-responses = { package = "async-openai-local", path = "../async-openai/async-openai", version = "0.41.3", features = ["responses", "byot"] }`；删除 `vendor/async-openai/`。代码仍 `use async_openai_responses::…`，无需改引用。 |
| 改后行为 | 无用户可见变化：配置阈值时 wire body 仍携带 `context_management`。维护移到仓库外：直接改本地 fork（`/Users/rg/Projects/async-openai`，分支 `feat/tact`），不再 re-vendor。 |
| 指针 | `/Users/rg/Projects/async-openai`（fork，`feat/tact` 上提交 `7de8bb4` / `5e22785` / `12488eb`）；`Cargo.toml` 的 `async-openai-responses` 依赖；`crates/tact_llm/src/openai/responses/convert.rs`（`create_response` builder 注入）；[Ch 22](./22_chapter_llm_zh.md) §6.2。 |

## 2. 2026-08-12 — Worktree 存储从 JSON 文件迁移到 SQLite（`WorktreeStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | worktree 元数据 + 审计日志以单一 JSON 索引持久化（`worktrees/index.json`，`Store<WorktreeIndex>`），读-改-写无事务；重名检查与索引写入存在竞态。 |
| 决策 | worktree 状态移入现有 `<workdir>/.tact/tact.db`，建 `worktrees` + `worktree_events` 表，通过新的异步 `WorktreeStore` trait（`crates/tact/src/store/worktree_store/`，sqlx 实现的 `SqliteWorktreeStore`）访问。`worktrees.name` UNIQUE（并发兜底）；自增 `id` 保持插入顺序；`worktree_events` 按自身 `id` 排序。新增 `session_id` 列与索引，`worktree_create` 时从工具上下文填充。`WorktreeManager` 变为 `Box<dyn WorktreeStore>` 之上的 async 门面；`SharedWorktreeManager` 去掉 mutex（`Arc<WorktreeManager>`，连接池已串行化写入——`worktree_run` 不再阻塞其他 worktree 工具）。遗留 `worktrees/index.json` 不再读取、留在磁盘。至此无领域模块使用 JSON store（`StoreRoot`/`Store`/`CollectionStore` 作为通用原语保留，自带单元测试）。 |
| 改后行为 | 泳道与事件持久化在 `tact.db`（旧 `worktrees/index.json` 条目若不手动导出则丢失）；`worktree_*` 表面不变；`session_id` 出现在 worktree 记录中。 |
| 指针 | `crates/tact/src/store/worktree_store/{mod,sqlite}.rs`、`crates/tact/src/worktree/mod.rs`、`crates/tact/src/tool/worktree.rs`、`crates/tact-ui/src/{headless,interactive}.rs`；[Ch 1](./01_chapter_store_zh.md) §5–6、[Ch 15](./15_chapter_worktree_zh.md) §2–5。 |

## 2. 2026-08-12 — Team 存储从 JSON 文件迁移到 SQLite（`TeamStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | roster 以单一 JSON 索引持久化（`team/config.json`，`TeamConfig` 包装），inbox 为每个 owner 一个 JSONL 文件（`team/inbox/{owner}.json`）。两者都是无事务的读-改-写、无跨进程锁；重名检查与 roster 写入存在竞态。 |
| 决策 | team 状态移入现有 `<workdir>/.tact/tact.db`，建 `teammates` + `inbox_messages` 表，通过新的异步 `TeamStore` trait（`crates/tact/src/store/team_store/`，sqlx 实现的 `SqliteTeamStore`）访问。`teammates.name` 为 PRIMARY KEY；重复 spawn 用 `INSERT OR IGNORE` + `rows_affected == 0` 拒绝（保留 `teammate {name} already exists` 错误且无竞态）。`inbox_messages` 增加自增 `id` 以保持读取的插入顺序（遗留 JSONL 追加语义）+ `owner` 索引。`TeammateManager` 变为 `Box<dyn TeamStore>` 之上的 async 门面；`SharedTeammateManager` 去掉 mutex（`Arc<TeammateManager>`，连接池已串行化写入）。旧 JSON 文件不再读取、留在磁盘。 |
| 改后行为 | roster 与 inbox 持久化在 `tact.db`（旧 `team/` JSON 条目若不手动导出则丢失）；`spawn_teammate` / `broadcast` / `read_inbox` / `plan_approval` / `shutdown_*` 表面不变；跨进程 inbox 写入不再在文件追加上竞态。 |
| 指针 | `crates/tact/src/store/team_store/{mod,sqlite}.rs`、`crates/tact/src/team.rs`、`crates/tact/src/tool/team.rs`、`crates/tact-ui/src/{headless,interactive}.rs`；[Ch 1](./01_chapter_store_zh.md) §5–6、[Ch 14](./14_chapter_team_zh.md) §3–5。 |

## 2. 2026-08-12 — Cron 与后台任务从 JSON 文件迁移到 SQLite（`CronStore` / `BackgroundStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | cron 以单一 JSON 索引持久化（`cron/scheduled_tasks.json` 含 `next_id` 计数器），后台以每条记录一个 JSON 文件持久化（`background/tasks/{id}.json` 加内存 `Mutex<HashMap>` 镜像）。两者都是无事务的读-改-写、无跨进程锁；后台 manager 持有磁盘 + 内存双份状态，可能漂移。 |
| 决策 | cron 与后台移入现有 `<workdir>/.tact/tact.db`，建 `cron_tasks` + `background_tasks` 表，通过新的异步 trait `CronStore`（`crates/tact/src/store/cron_store/`）与 `BackgroundStore`（`crates/tact/src/store/background_store/`）访问，仿照 `TaskStore` 模式。`cron_tasks` 的 id 由 `INTEGER PRIMARY KEY AUTOINCREMENT` 分配、对外以 8 位十六进制字符串暴露（`format!("{rowid:08x}")`）——与遗留索引的线上契约一致；`background_tasks` 保留时间戳毫秒 hex `id`，`status` 带 `CHECK` 约束。两张表都新增 `session_id` 列与索引，`cron_create` / `background_run` 时从工具上下文填充。`CronScheduler` / `BackgroundManager` 变为 async 门面；`SharedCronScheduler` 去掉 mutex（`Arc<CronScheduler>`），`BackgroundManager` 去掉内存镜像（DB 为唯一数据源；spawn 的 tokio 任务通过克隆的 store 句柄写回）。旧 JSON 文件不再读取、留在磁盘；`TactPath::cron_dir()` / `CRON_SUBDIR` 作为死代码删除。 |
| 改后行为 | cron id 从 `00000001` 重新开始（旧条目若不从 `.tact/cron/` 手动导出则丢失）；`cron_*` / `background_*` / `/background` 表面不变；启动孤儿修复（`running` → `error`）改为扫表；`session_id` 出现在 cron JSON 与后台记录中。 |
| 指针 | `crates/tact/src/store/cron_store/{mod,sqlite}.rs`、`crates/tact/src/store/background_store/{mod,sqlite}.rs`、`crates/tact/src/cron/mod.rs`、`crates/tact/src/background.rs`、`crates/tact/src/tool/{cron,background_run}.rs`、`crates/tact-ui/src/{headless,interactive,driver}.rs`；[Ch 1](./01_chapter_store_zh.md) §5–6、[Ch 13](./13_chapter_background_zh.md) §2–5。（Ch 16 已于 2026-08-14 随 cron 功能一并删除。） |

## 2. 2026-08-11 — 任务存储从 JSON 文件迁移到 SQLite（`TaskStore`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | 任务以每条记录一个 JSON 文件（`tasks/task_{id}.json`）加 `tasks/index.json` 的 next-id 计数器持久化。ID 分配与依赖边（`blockedBy` / `blocks` 在两条记录上互相镜像）都是无事务的读-改-写，且没有跨进程锁；完成任务需要 O(n) 全表扫描来清理边。 |
| 决策 | 任务移入现有 `<workdir>/.tact/tact.db`，建 `tasks` + `task_dependencies` 表，通过新的 `TaskStore` trait（`crates/tact/src/store/task_store/`，sqlx 实现的 `SqliteTaskStore`）访问。边为行（复合主键，`INSERT OR IGNORE`），无镜像字段、无外键；每次变更都在 `BEGIN IMMEDIATE` 事务内，完成时用一条 `DELETE` 清边。ID 由 `INTEGER PRIMARY KEY AUTOINCREMENT` 分配（删除 `TaskIndex`）。`TaskManager` 变为 `Box<dyn TaskStore>` 之上的 async 门面；`SharedTaskManager` 去掉 mutex（`Arc<TaskManager>`，连接池已串行化写入）。新增 `session_id` 列与索引，`task_create` 时从工具上下文填充。旧 JSON 文件不再读取、留在磁盘。`crates/tact/Cargo.toml` 的 tokio features 增加 `macros` + `rt-multi-thread`，使 `-p tact` 单独构建时 `#[tokio::test]` 可用。 |
| 改后行为 | 新任务 ID 从 1 开始（旧的 1–233 条记录若不从 `.tact/tasks/` 手动导出则丢失）；依赖更新原子化；`task_*` 工具表面不变（`session_id` 出现在任务 JSON / 快照中）。 |
| 指针 | `crates/tact/src/store/task_store/{mod,sqlite}.rs`、`crates/tact/src/task/mod.rs`、`crates/tact/src/tool/task.rs`；[Ch 1](./01_chapter_store_zh.md) §6、[Ch 19](./19_chapter_persistent_tasks_zh.md) §2–3。 |

## 2. 2026-08-11 — 摘要器 thinking budget 限制在 `max_tokens` 之下；Kimi K3 默认 reasoning 预留

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 2026-08-10 的摘要预算改动与主循环一样用 `with_thinking(self.thinking_config())` 把配置的 Claude 式 thinking budget 转发给压缩摘要请求，但摘要器的 `max_tokens` 独立封顶为 `min(窗口 × 20%, 2,000)`。Anthropic 在 wire 上要求 `budget_tokens < max_tokens`，于是默认 8k/32k 的 thinking budget 会生成非法请求（`thinking.budget_tokens = 8,000` 而 `max_tokens = 2,000`），导致所有开启 thinking 的 Anthropic 用户本地压缩以 400 失败。另外，reasoning 预留只把 DeepSeek 视为默认开启 reasoning，但 Kimi K3 服务端同样默认 thinking 开启 + effort high，未显式配置 effort 时 Kimi 摘要仍可能被 reasoning 挤占。 |
| 决策 | 新增 `compact_summary_thinking(configured_budget, summary_max_tokens)`，把转发的 budget 限制为 `summary_max_tokens - 1`（输出预算退化到 ≤ 1 token 时完全禁用 thinking），并通过一个小的 builder 闭包同时应用于首次与续写的摘要请求。输入侧预留仍按配置的 budget 扣除（偏保守）。`compact_summary_reasoning_reserve_percent` 现在对 `ProviderKind::Kimi` 与 DeepSeek 一样预留默认 high 档（75%）。 |
| 改后行为 | 使用大 thinking budget 的 Anthropic 压缩会发送 `budget_tokens = max_tokens - 1` 而非以 400 失败；本来就放得下的 budget 原样透传。未显式配置 effort 的 Kimi K3 获得与 DeepSeek 相同的 75% reasoning 预留。 |
| 指针 | `crates/tact/src/agent/mod.rs` 中 `compact_summary_thinking` 与 `compact_summary_reasoning_reserve_percent`（`compact_history_local_with_mode`）；测试 `compact_summary_thinking_clamps_below_max_tokens`、`local_compact_clamps_thinking_budget_below_summary_max_tokens`、`compact_summary_reasoning_reserve_percent_tiers`；[Ch 5](./05_chapter_compact_zh.md) §5 步骤 3。 |

## 2. 2026-08-10 — 本地 vendor async-openai 为 `async-openai-local`，获得类型化 `context_management`

| 字段 | 值 |
|------|-----|
| 类型 | `docs`（依赖管理） |
| 症状 / 动机 | OpenAI Responses API 官方支持 `context_management` / `compact_threshold`（服务端压缩），官方 Python/Node SDK 也有类型化支持；但 Rust 的 `async-openai` crate（截至 2026-08-10 最新 0.41.3）只定义了 `ContextManagementParam` 类型，从未把它接进 `CreateResponseArgs` builder——Tact 只能通过 byot JSON 路径注入（`body["context_management"] = serde_json::json!(...)`）。 |
| 决策 | 将 async-openai 0.41.3 源码 vendor 到 `vendor/async-openai`，并把 package 名改为 `async-openai-local`，使 path 依赖只命中 Responses 协议的 0.41.x、不会与 Chat Completions 路径使用的旧版 `async-openai 0.20` 冲突（workspace 清单：`async-openai-responses = { package = "async-openai-local", path = "vendor/async-openai", ... }`；代码仍 `use async_openai_responses::…`，无需改引用）。与上游源码有两处差异：给 `CreateResponse` 加类型化字段 `context_management: Option<Vec<ContextManagementParam>>`；给 `ReasoningEffort` 加 `Max` 变体（上游只到 `Xhigh`；DeepSeek / Kimi K3 接受 `max`）。`convert.rs` 现在全部通过类型化 builder 构造——`context_management(...)` setter，以及 `Reasoning { effort: request.reasoning_effort.map(Into::into), summary }`（经 `crates/tact_llm/src/types.rs` 中的 `impl From<OpenAiReasoningEffort> for ReasoningEffort`）——不再使用 `serde_json::json!` / `Value::String` 注入。`vendor/async-openai/README.fork.md` 记录与上游同步的方法。 |
| 改后行为 | 无用户可见变化：配置阈值时 wire body 仍携带 `context_management`。维护改为本地化：新的 Responses 字段可以直接加到 vendor，不必等待上游 Rust crate。 |
| 指针 | `vendor/async-openai/`（`README.fork.md`、`src/types/responses/response.rs`）；`Cargo.toml` 的 `async-openai-responses` 依赖；`crates/tact_llm/src/openai/responses/convert.rs`（`create_response` builder 注入）；[Ch 22](./22_chapter_llm_zh.md) §6.2。 |

## 2. 2026-08-10 — Responses 端点未实现 `/responses/compact` 时给出明确报错

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 兼容 `/responses` 端点（如 `opencode.ai/zen/go/v1`）往往不实现 `POST /responses/compact`，返回 404 HTML 页面。SDK 的 `compact_byot` 随后会抛出把整个 HTML body 塞进错误的 JSON 反序列化错误；且该消息可能命中瞬时错误重试列表（"unavailable"），导致无意义的退避重试后才失败。 |
| 决策 | Responses 适配器的 `compact()` 改为通过共享原始 HTTP client 发送 compact 请求（与 SDK 同一传输层），从而可以检查状态码。HTTP 404/405 映射为 `LlmError::Unsupported("endpoint does not support POST /responses/compact (HTTP {status}): native Responses compaction is not implemented by base URL {base_url}")` —— 措辞刻意避开瞬时错误关键词，避免进入重试循环。其它非 2xx 状态沿用既有 `LlmError::HttpError { status, body }`。 |
| 改后行为 | 在未实现 `/responses/compact` 的端点上触发压缩时，会立即显示点名缺失端点和 base URL 的明确错误（无 HTML 倾倒、无重试）；会话状态保持不变。 |
| 指针 | `crates/tact_llm/src/openai/responses/mod.rs` 的 `compact()`；测试 `compact_reports_missing_endpoint_clearly`；[Ch 22](./22_chapter_llm_zh.md) §6.2、[Ch 5](./05_chapter_compact_zh.md)。 |

## 2. 2026-08-10 — Responses 适配器在兼容端点无终态事件关闭流时恢复

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 兼容 `/responses` 端点（如 `opencode.ai/zen/go/v1`）偶尔会在没有任何终态事件（`response.completed` / `response.incomplete` / `response.failed`）的情况下关闭 SSE 流。`ResponsesStreamState::finish()` 会以 `unsupported response state: OpenAI Responses stream ended without a terminal event` 硬失败，即使流已经交付了完整的 `output_item.done` 序列或可见文本，也会中止整个 agent 回合。 |
| 决策 | 当流本身完整时，无终态事件的干净 EOF 现在被视为终态：若所有已 announce 的 item 均完成（`output_item.done` 序列连续且无 pending `added`），则从 done 序列重建输出；否则恢复已流式输出的可见文本（与既有兼容端点恢复同一分支）。合成一个最小 completed `Response` 后走既有的 normalize/恢复路径，因此 stop reason 推断（含工具调用时的 `ToolUse`）与 provider-state baseline 构建保持不变。缺失 compaction 边界（`pending_compactions` 非空）与空流仍然是硬协议错误——恢复绝不能静默丢弃已压缩的 baseline。 |
| 改后行为 | 之前因 "stream ended without a terminal event" 直接失败的回合，现在在响应已完整交付时从 done 序列/流式文本正常完成；真正空流或 compaction 不完整的流仍会大声失败。 |
| 指针 | `crates/tact_llm/src/openai/responses/stream.rs` 的 `finish()`；测试 `no_terminal_event_recovers_from_complete_done_sequence`、`no_terminal_event_recovers_visible_text`、`no_terminal_event_empty_stream_is_error`、`no_terminal_event_with_pending_compaction_is_error`；[Ch 22](./22_chapter_llm_zh.md) §6.2。 |

## 2. 2026-08-10 — 本地压缩为 reasoning / thinking token 预留输出预算

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | `compact_history_local_with_mode`（所有非 OpenAI-Responses 压缩的摘要器）把摘要请求的 `max_tokens` 定为 `min(窗口 × 20%, 2,000)` 并当作**文本**预算，但 reasoning-effort 类 provider（OpenAI o 系 / DeepSeek / Kimi K3）把 reasoning token 计入同一个 `max_tokens` 信封。配置 `high`/`max` effort（或 DeepSeek 服务端默认 thinking 开启 + effort high）时，reasoning 会烧掉 2,000 预算的大半，留给摘要文本的额度不足，每次调用都撞 `StopReason::MaxTokens`，续写循环（≤3 次，且每次都共享同一上限）最终只能接受 best-effort 的部分摘要。摘要请求也从未转发配置的 Claude 式 thinking budget（主循环会），输入侧预留同样忽略了它。 |
| 决策 | 拆分摘要输出预算：摘要**文本**沿用经典 `min(窗口 × 20%, 2,000)`；当配置了 reasoning effort（或 provider 为 DeepSeek 且未显式配置 effort）时，在文本预算**之上**追加分档预留（minimal\|low / medium / high / xhigh\|max 分别为文本预算的 25/50/75/100%；DeepSeek 默认 ≈ high = 75%），使 wire 上的 `max_tokens` = 文本 + 预留，reasoning 不再挤占文本额度。摘要请求现在会转发配置的 thinking budget（`with_thinking(self.thinking_config())`，与主循环一致），并从输入侧预留中扣除。不改变任何 effort 语义 —— 只做预算核算。 |
| 改后行为 | 配置了 reasoning effort（或在 DeepSeek 上）的压缩摘要会获得更大的 wire `max_tokens`（例如 128k 窗口 + high effort → 2,000 + 1,500 = 3,500），而文本部分仍拿到完整经典预算；摘要请求携带与主循环相同的 thinking 配置；输入预留同时计入 reasoning 与 thinking 余量，当窗口在扣除这些预留后放不下提示词时，仍以原有的 "too small" 错误提前失败。 |
| 指针 | `compact_summary_reasoning_reserve_percent` 与 `crates/tact/src/agent/mod.rs` 中 `compact_history_local_with_mode` 的预算计算；测试 `compact_summary_reasoning_reserve_percent_tiers`、`local_compact_reserves_reasoning_budget_and_forwards_thinking`、`local_compact_input_reservation_subtracts_thinking_budget`；[Ch 5](./05_chapter_compact_zh.md) §5 步骤 3。 |

## 2. 2026-08-10 — `background_run` 实时输出到工具卡片（类 bash）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | `background_run` 用 `Command::output()` 一次性缓冲全部输出，工具卡片立即终结（"started"），用户不轮询 `check_background` 就看不到任何内容 —— 与 `bash` 卡片实时流出输出完全不同。 |
| 决策 | 新增 keep-live 卡片契约：`ToolPresentationInfo.keep_live`（由新 `LiveOutputPolicy::Background` 映射）让 TUI 在 `StepFinished` 后仍保留卡片活动；manager 改为增量读取 stdout/stderr（`read_pipe` + `Utf8Decoder` + 约 50ms 节流的 `ToolProgress`，实时预览保留最近 ~4 KB），并以新 `AgentUpdate::BackgroundTaskFinished { tool_id, success, message, output }` 关闭卡片，携带 ✓/✗、耗时与有上限的最终输出。`background_run` 将 `BackgroundProgressSink`（tool_id + `ui_tx`）传入 `SharedBackgroundManager::run`；记录持久化与 120s 超时不变。 |
| 改后行为 | `background_run cargo build` 在 TUI 卡片中显示 spinner + 实时构建输出，进程退出时以 ✓/✗ 与耗时收尾 —— 即使 agent 那一轮早已结束。模型仍无完成 push，须轮询 `check_background`。 |
| 指针 | `crates/tact/src/background.rs`（`BackgroundProgressSink`、`run_background_process`）；`crates/tact/src/tool/background_run.rs`；`crates/tact/src/tool/metadata.rs` 的 `LiveOutputPolicy::Background`；`crates/protocol/src/agent.rs` 的 `AgentUpdate::BackgroundTaskFinished`；TUI `on_step_finished` / `on_background_task_finished`（`crates/tui/src/widgets/state/app/agent.rs`）；[Ch 13](./13_chapter_background_zh.md)、[Ch 25](./25_chapter_protocol_zh.md)。 |

## 2. 2026-08-10 — `/background` slash 命令查看后台任务状态

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 症状 / 动机 | `background_run` 启动的后台任务只能通过让模型调用 `check_background` 工具来查看；TUI 没有直接入口，用户要轮询任务必须专门输入一句 prompt。 |
| 决策 | 新增 TUI slash 命令 `/background`（`/background <id>` 查看单个任务），由新协议变体 `UserCommand::QueryBackground(Option<String>)` 承载。命令 driver（`crates/tact-ui/src/driver.rs`）调用共享的 `ToolContext.background_manager.check(id)` —— 与 `check_background` 工具同一代码路径 —— 并发出 `AgentUpdate::MdInfo`，内容为 `## ⚙️ Background Tasks` 围栏代码块（未知 id 则发出 `AgentUpdate::Error`）。命令加入 `PALETTE_COMMANDS`、`i18n.rs`（中/英）本地化，面板图标为 `🖥`。 |
| 改后行为 | `/background` 每行输出一个任务（id、状态、命令）；`/background <id>` 输出该任务 pretty JSON；未知 id 显示错误。不新增状态、不做完成推送 —— 命令只读取持久化/内存中的记录。 |
| 指针 | `crates/protocol/src/agent.rs` 中的 `UserCommand::QueryBackground`；`crates/tact-ui/src/driver.rs` 的 driver 分支；`crates/tui/src/widgets/state/mod.rs` 的 `PALETTE_COMMANDS`；`crates/tui/src/handlers/mod.rs` 的 `execute_palette_command`；[Ch 13](./13_chapter_background_zh.md)、[Ch 23](./23_chapter_tui_zh.md) §3。 |

## 2. 2026-08-09 — OpenAI Responses 托管 web search（`protocol = "responses"`）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| PR | https://github.com/laohanlinux/tact/pull/62（分支 `feat/responses-web-search`） |
| 症状 / 动机 | Responses adapter 只发送 function tools，OpenAI `protocol = "responses"` 会话没有托管（provider 执行）web search；用户只能自接 MCP `web_search` function tool，或退回 Chat Completions。 |
| 决策 | Hosted web search 是 **Responses 协议级能力**，与协议背后的端点/provider 无关：只要选择 `protocol = "responses"`，adapter 就在每次普通 `/responses` 请求中注入 `Tool::WebSearch`（`create_response(..., native_web_search = true)`；只有 `/responses/compact` 传 `false`——压缩端点不接受 tools）——OpenAI、DeepSeek 与 custom OpenAI-compatible 端点一视同仁，没有按 provider 的开关（`OpenAiResponsesAdapter` 不再有 `native_web_search` 标志；`ResponsesCapabilities::hosted_tools` 对每个 Responses 端点都包含 `WebSearch`）。Provider 在服务端执行搜索，Tact 只通过真实 Step 事件渲染工具卡片（`output_item.added` → `StepStarted`，每个 index 首次 `output_item.done` → `StepFinished`/`StepFailed`；`done` 时仍为 `in_progress`/`searching` 一律判失败）。`web_search_call` 永远不会变成 `ContentBlock::ToolUse`，stop reason 保持 `completed`。兼容端点若在 search action 返回 `queries` 数组而非单数 `query`，由 `wire::normalize_web_search_call_query` 处理（仅在 typed 解析时回填 `query`，原始 item 按原样回放）。`AgentUpdate::StepFailed` 新增 `arg_summary`，失败卡片标题能保留 query。DeepSeek 保留代码路径，但配置解析仍按 #57 拒绝，直到其 Responses 支持重新启用。 |
| 改后行为 | 任意 `protocol = "responses"` 会话——OpenAI、DeepSeek 或 custom OpenAI-compatible——都自动获得托管 web search；TUI 显示 `🔍 Web Search` 卡片，标题为 query，sources 为可展开详情；失败携带 status/query/action 诊断。 |
| 指针 | `crates/tact_llm/src/openai/responses/{convert,stream,wire,mod}.rs`、`crates/tact_llm/src/provider.rs`（`build_openai_responses`）、`crates/tui/src/widgets/tool_widget.rs`、AGENTS.md "Hosted tools (Provider-executed) — design invariants"、[Ch 22 §6.2.1.1](./22_chapter_llm_zh.md)。 |

## 2. 2026-08-09 — 任务统计行 `[copy]` 复制最近一轮

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 回合结束后，用户无法从任务统计行一键复制本轮对话。 |
| 决策 | 在每条 `📊 任务统计：` 行追加 `[copy]` 按钮；点击后复制「上一轮统计行之后（或会话开头）到当前统计行之前」的日志文本，跳过空行与任务结束分隔线。 |
| 变更后行为 | 点击统计行的 `[copy]` → 剪贴板为本轮用户/助手内容；不包含更早回合。 |
| 指针 | `messages.rs` 的 `add_task_stats_block` / `copy_turn_ending_at_stats`；`handlers/mouse.rs` 命中；回归测试 `copy_turn_ending_at_stats_copies_last_turn_only`。 |

## 2. 2026-08-09 — Mermaid 图双击弹窗复制源码

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 成功渲染的 Mermaid 在把 ASCII 图拼进日志时丢掉了 fence 正文，用户无法再取回源码以便编辑。 |
| 决策 | 每次成功渲染保留 `MermaidBlock { start_idx, end_idx, source }`；双击打开 Mermaid 弹窗；弹窗 `y` 复制源码。主区选区 yank 仍为 ASCII。 |
| 变更后行为 | 双击任意 diagram 行 → 源码弹窗（`y` / `j/k` / `Esc`）；失败 Mermaid 仍走 code-card 路径。 |
| 指针 | Spec `docs/superpowers/specs/2026-08-09-mermaid-diagram-copy-popup-design.md`；`finish_stream_code_block`；`popups/mermaid_popup.rs`；回归测试 `log_renders_streamed_mermaid_without_code_card`、`mermaid_popup_copy_uses_source_not_ascii`。 |

## 2. 2026-08-09 — Mermaid 时序图自消息改为 U 形回环

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 自消息（`A->>A`）只画成单格 `<│◀`，看起来像断开的尖括号，而不像指向自己的回环箭头。 |
| 决策 | 将自消息画成两行盒线回环（`│──┐` / `│◀─┘`；末列参与者用左侧 `┌──│` / `└─▶│`），标签放在回环旁。 |
| 变更后行为 | 自调用在生命线上呈现清晰的 U 形折返；末列自消息向左回环，避免画出图外。 |
| 指针 | `crates/tui/src/render/mermaid_sequence.rs`（`self_loop_rows`）；回归测试 `self_message_draws_u_shaped_loop`、`self_message_on_last_participant_loops_left`。 |

## 2. 2026-08-09 — Mermaid 时序图标签不再掉字或错位

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 自有 `sequenceDiagram` 渲染器会丢弃落在生命线上的标签字形（如 `submitTask` → `ubmitTask` / `submi│Task`），且每个 2 列宽 CJK 字形后留下幽灵空格，使标签行比生命线/箭头行更宽——多参与者图上生命线看起来断裂，箭头也像缺段。 |
| 决策 | 在 `label_row` 中把宽字形的续格清空为空 span，并将标签字符绕过已占用的生命线单元格重排，而不是直接跳过。 |
| 变更后行为 | 长 ASCII / CJK 箭头标签保留全部字符（必要时在 `│` 两侧拆开），各行显示宽度一致，生命线保持纵向对齐。 |
| 指针 | `crates/tui/src/render/mermaid_sequence.rs`（`label_row`）；回归测试 `cjk_label_keeps_same_display_width_as_lifeline_row`、`long_ascii_label_is_not_eaten_by_lifelines`、`self_message_keeps_lifeline_intact`。 |

## 2. 2026-08-08 — TUI 使用自有渲染器绘制 Mermaid 时序图

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 上游 `ratatui-markdown` 的时序图渲染器对三类常见输入处理有误：`participant A as 用户` 别名被原样展示；`+`/`-` 激活简写（`A->>+B`）会产生幻影参与者列（`+B`、`-B` 等）；2 列宽的 CJK 箭头描述可能盖住生命线（或被丢弃），导致带描述的箭头看起来对不齐。 |
| 决策 | 在 TUI 中把 `sequenceDiagram` 代码块路由到 Tact 自有的渲染器（`crates/tui/src/render/mermaid_sequence.rs`）；其他 Mermaid 图类型继续使用 `ratatui-markdown`。新渲染器解析 `as` 别名，在参与者查找前去除 `+`/`-` 激活前缀，并按显示列放置标签字形，仅当字形宽度内所有单元格都空闲时才绘制。 |
| 变更后行为 | 只有声明的参与者渲染为列；`A->>+B` 指向参与者 `B`；CJK 标签在生命线之间居中，且不会覆盖 `│`。无法解析的源码仍回退到普通代码渲染。 |
| 指针 | `crates/tui/src/render/mermaid_sequence.rs`；路由：`crates/tui/src/render/render_md.rs`（`render_mermaid_block`）；回归测试位于 `mermaid_sequence.rs`。 |

## 2. 2026-08-08 — Subagent 模型选择器使用自身 provider

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | `/model-subagent` 会把 subagent 配置的模型与主 agent 当前 provider 的 API 模型列表合并；当两者使用不同 provider 时，选择器可能显示错误的模型。 |
| 决策 | 使用已解析的 subagent provider 的 `base_url` 和 `api_key` 查询 `/models`；保留 provider 配置中的 `models = [...]` 作为主要候选，并继续按 `(base_url, api_key)` 缓存。 |
| 变更后行为 | Subagent 选择器只显示属于 subagent provider 的配置模型和 API 发现模型；主 agent 的 `/model` 选择器仍使用主 provider。 |
| 指针 | `crates/tact_llm/src/models.rs`、`crates/tui/src/handlers/select.rs`；回归测试 `explicit_provider_model_query_uses_subagent_credentials`；设计：`docs/superpowers/specs/2026-08-08-subagent-model-picker-provider-design.md`；计划：`docs/superpowers/plans/2026-08-08-subagent-model-picker-provider.md`。 |

## 2. 2026-08-08 — DeepSeek 与 Kimi Responses 保持配置门控

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | 通用 Responses adapter 可以为 OpenAI-compatible 端点构造，但 DeepSeek/Kimi 的原生压缩与状态续传尚未验证达到生产契约；若直接允许正常配置，会把未支持的 fallback 行为误认为已支持。 |
| 决策 | 继续在配置解析阶段拒绝 DeepSeek/Kimi 的 `protocol = "responses"`。底层 adapter 构造仍可用于隔离端点测试；生产配置在原生 Responses 能力验证完成前使用 Chat Completions。 |
| 变更后行为 | DeepSeek/Kimi 用户会得到明确的配置错误，不会进入未经验证的 Responses 路径。OpenAI 与明确配置的自定义 OpenAI-compatible provider 保留现有 Responses 路由。 |
| 指针 | `crates/tact/src/config/resolve.rs`；provider 构造：`crates/tact_llm/src/provider.rs`；相关设计：`docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`；压缩行为：第 5 章。 |


## 2. 2026-08-08 — OpenAI Responses 保留未知 wire item

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 症状 / 动机 | typed `async-openai` Responses 枚举会在 Tact 有机会保留新 output item 之前直接拒绝它，使 provider state 无法前向兼容；同时共享的 Chat/Anthropic 请求模型没有明确的 Responses 专用字段扩展边界。 |
| 决策 | 在 typed normalization 之前先解析 raw Responses envelope；已知 item 正常转换，未知 input/output item 作为 raw JSON 保留。增加只由 Responses adapter 消费的 `ResponsesRequestOptions`，并提供保守的 provider capability metadata；只有出现可复现的 SDK 阻塞时才 fork `async-openai`。 |
| 变更后行为 | 无害的未知流事件不再中断响应。未知 output item 可以跨普通/流式 turn、session state 序列化和下一次 Responses 请求保留。Responses 专用请求字段不会出现在 Chat Completions 或 Anthropic payload 中。 |
| 指针 | `crates/tact_llm/src/openai/responses/wire.rs`、`request_options.rs`、`stream.rs`、`provider.rs`；设计：`docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`；计划：`docs/superpowers/plans/2026-08-08-responses-compatibility-foundation.md`；压缩：第 5 章与 `docs/compaction.md`。 |


## 2. 2026-08-08 — 主区域 Markdown 将完整 Mermaid fence 渲染为终端图

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 相关 | `crates/tui/src/render/render_md.rs`、`crates/tui/src/widgets/state/app/agent.rs`、`crates/tui/src/widgets/state/app/visibility.rs`、`crates/tui/src/widgets/state/stream_state.rs`、第 23 章 §6.7 |
| 症状 / 动机 | 所有带显式语言标签的流式 fence——包括 ```mermaid——闭合时都会被提升为 `CodeBlock` card overlay，因此 Mermaid 源码只显示为语法着色的代码，而不是图。 |
| 决策 | 在 `render_md.rs` 中把完整、顶层 `mermaid` fence 路由到共享的 `render_mermaid_block` 辅助函数（`ratatui-markdown::mermaid::render_mermaid` + 应用主题适配器）；在 `stream_state.rs` 中标记当前缓冲的流式 fence 是否为 Mermaid；`agent.rs` / `visibility.rs` 在合法闭合 fence 时直接把 diagram 行拼接进日志，而不是 push `CodeBlock`。无效、不支持或未闭合的 Mermaid 保留 code-card 回退，绝不丢弃源码。 |
| 变更后行为 | 完整的 ```mermaid fence 以日志宽度渲染为带主题的终端图（定宽 Markdown 路径使用名义 80 列）；合法的流式 Mermaid block 闭合后不再创建 code card；无效/不支持/未闭合的 Mermaid 与普通显式语言 fence 仍走原有 code-card 路径；宽度重排与视口滚动沿用现有 log 布局/缓存行为。 |
| 指针 | `render_md.rs`（`render_mermaid_block`、`route_mermaid_fences`）及测试 `render_mermaid_sequence_returns_terminal_lines`、`render_markdown_mermaid_flowchart_uses_box_art`、`render_markdown_invalid_mermaid_falls_back_to_code`、`render_markdown_unclosed_mermaid_fence_keeps_source`；`stream_state.rs`（`code_block_is_mermaid`）；`agent.rs`（`finish_stream_code_block`）、`visibility.rs`（`flush_stream_pending`）；`render_gap_tests.rs` 回归测试（`log_renders_streamed_mermaid_without_code_card`、`log_falls_back_to_code_card_for_invalid_streamed_mermaid`、`flush_renders_streamed_mermaid_without_trailing_newline`、`flush_falls_back_to_code_card_for_unclosed_streamed_mermaid`）、`cells/markdown.rs`（`markdown_cell_renders_mermaid_at_the_requested_width`）；spec `docs/superpowers/specs/2026-08-08-mermaid-main-rendering-design.md`；plan `docs/superpowers/plans/2026-08-08-mermaid-main-rendering.md`；文档 `book/23_chapter_tui*.md` §6.7 |

---


## 2. 2026-08-06 — OpenAI Responses 显示详细 reasoning summary

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

## 2. 2026-08-06 — 恢复重试消息附带底层错误

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact/src/recovery.rs`（`error_summary`）、`crates/tact/src/agent/mod.rs`（backoff / compact retry 消息点）、第 6 章恢复消息 |
| Symptom / motivation | `[Recovery] backoff (1/10): retrying in 1.9s` 只说明了*何时*重试，没有说明*为什么*——底层传输错误（超时、连接重置、限流……）完全不可见，用户看到 8 次以上退避却不知道哪里失败。压缩摘要的重试消息（`[compact retry 1/3] retrying in 1.9s`）也有同样的问题。 |
| Decision | 在 `recovery.rs` 新增 `error_summary`：将空白/换行折叠为单行，超过 200 字符以省略号截断。主循环 backoff 消息追加完整 anyhow 链（外层上下文 → 根因，以 `": "` 连接）；两处压缩重试消息追加客户端错误字符串。原有标签、计数与延时文本保持不变，因此匹配 `contains("Recovery") && contains("backoff")` 的测试仍可通过。 |
| Behavior after | 恢复重试会报告原因，例如 `[Recovery] backoff (2/10): retrying in 4.3s — http request failed: error sending request for url`。 |
| Pointers | `error_summary` 及其单元测试位于 `crates/tact/src/recovery.rs`；消息点在 `crates/tact/src/agent/mod.rs`；文档 `book/06_chapter_recovery*.md` 恢复消息一节 |

---

## 2. 2026-08-06 — 未知 provider 名称放行为自定义 OpenAI 兼容 provider

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 相关 | `crates/tact_llm/src/types.rs`（`ProviderKind::Custom`、`FromStr`）、`crates/tact_llm/src/provider.rs`（`build_client`、`model_uses_effort`）、`crates/tact_llm/src/hook_select.rs`（`body_hook_for`）、`crates/tact_llm/src/models.rs`（`is_models_query_supported`）、`crates/tact/src/config/resolve.rs`（`resolve_provider_kind`、`resolve_llm`、`resolve_subagent`）、Ch 21 §3–§4 |
| 症状 / 动机 | `ProviderKind::from_str` 拒绝 `anthropic | openai | deepseek | kimi` 之外的任何名称，因此 `llm.provider = "moonshot"`（或任何自建 / 网关 provider）即使条目里配好了可用的 OpenAI 兼容 `base_url`，也会报 "unknown provider"。配置层无法表达第三方 OpenAI 兼容端点。 |
| 决策 | 为所有非内建名称新增 `ProviderKind::Custom(String)`。自定义 provider 全链路复用 OpenAI 协议：`build_client` 派发到 OpenAI 兼容适配器（默认 `chat_completions`，可选 `responses`），`body_hook_for` 与 `openai` 使用相同的端点启发式，支持 `/v1/models` 补充，并接受 `reasoning_effort`。它们**没有默认 `base_url`**——条目未设置时 resolve 报 "base_url not configured"。内建门禁不变：`responses` 协议仍限 `openai | deepseek | custom`，`reasoning_effort` 限 OpenAI 兼容 provider（即除 anthropic 外全部）；`resolve_llm` 中的 map key 校验循环已删除（自定义 key 不再报错）。`ProviderKind` 失去 `Copy`（现在持有 `String`），方法接收者改为 `&self`。 |
| 变更后行为 | `llm.provider` / `--provider` 接受任意名称。非内建名称按自定义 OpenAI 兼容 provider 处理，必须在 `[llm.providers.<name>]` 中显式配置 `base_url`。缺失活跃条目仍在 resolve 时报错。 |
| 指向 | `ProviderKind` 位于 `crates/tact_llm/src/types.rs`；测试 `provider_kind_from_str_accepts_unknown_as_custom`（tact_llm）、`custom_provider_resolves_with_openai_protocol` / `custom_provider_without_base_url_errors` / `custom_provider_in_map_resolves`（tact config resolve）；文档 `book/21_chapter_config*.md` §3–§4、`config.example.toml` |

---

## 2. 2026-08-06 — 账户轮询每次故障只提示一次，而非每个退避周期都提示

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 关联 | `crates/tact-ui/src/account.rs`（`poll_loop`、`spawn_poller`）、`crates/tui/src/widgets/state/app/agent.rs`（`handle_account_update` flash）、Ch 22 §9 |
| 症状 / 动机 | `spawn_poller` 把每次失败的余额 / 用量查询都转发为 `AccountUpdate::Error`。在持续故障（如断网）时，TUI 每 10 s → 20 s → … → 5 min 弹一次错误提示，永不停歇——变成通知风暴（「骚扰」），掩盖了应用的真实状态。 |
| 决策 | 把循环抽取为可测试的 `poll_loop(query, tx, next_delay)`，并加入 `error_notified` 标志：连续故障期间只转发**第一条**失败；后续重试保持静默，退避继续。一次成功查询会复位标志，因此恢复后的新一轮故障会再次提示一次。`NotSupported` 仍静默终止循环（不变）；启动查询与 `/balance` 命令保留一次性错误上报（用户主动触发，不算骚扰）。 |
| 行为变化 | 一次故障 = 一条 flash 提示，之后静默退避重试直到恢复；恢复后恢复正常 5–15 s 轮询，下一次故障再提示一次。 |
| 指针 | `crates/tact-ui/src/account.rs` 中的 `poll_loop` / `spawn_poller`；测试 `poller_forwards_error_once_per_outage_then_resumes`、`poller_stops_on_not_supported_without_error_flash`；文档 `book/22_chapter_llm*.md` §9 |

---

## 2. 2026-08-06 — Kimi Code 用量查询仅限官方 `https://api.kimi.com/coding` 端点

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tact_llm/src/account.rs`（`query_kimi_code_usage`、`kimi_usage_url_from_base_url`）、`crates/tact_llm/src/provider.rs`（`is_kimi_usage_supported`、`is_account_query_supported`）、`crates/tact-ui/src/account.rs`（`query_once`）、Ch 22 §3 / §9 |
| 症状 / 动机 | `kimi_usage_url_from_base_url` 从任意配置的 base URL 推导 `{origin}/v1/usages`，导致 `kimi-for-coding` 模型挂在自定义 OpenAI 兼容代理后时，会把代理的 API key 发往猜测出来的用量端点。`is_kimi_usage_supported` 此前等于 `is_kimi_coding(&model)`——任何提供 `kimi-for-coding` 的代理都返回 true，因此 TUI 配额组件也会轮询代理。 |
| 决策 | 与 DeepSeek / Kimi 余额的「凭据边界」对齐：`kimi_usage_url_from_base_url` 仅在 HTTPS 且主机精确为 `api.kimi.com`、路径含 `/coding`（允许 `/v1` 后缀）时返回官方 URL；其余返回 `None`，`query_kimi_code_usage` 报错「Kimi Code usage API is only available for the official endpoint https://api.kimi.com/coding」。`ProviderInfo::is_kimi_usage_supported` 要求同样的官方主机 / 路径，因此代理配置下 `is_account_query_supported` 为 false，TUI 隐藏配额组件。`is_kimi_coding` 本身不变——它仍用于识别 Kimi Code 平台（含代理）以决定 wire shape。 |
| 行为变化 | Kimi Code 用量轮询仅在 `base_url` 指向官方 `https://api.kimi.com/coding` 端点时可用。自定义代理（即使 model 是 `kimi-for-coding`）视为不支持；代理配置的 API key 永远不会发送到 `api.kimi.com`。 |
| 指针 | `crates/tact_llm/src/account.rs` 中的 `kimi_usage_url_from_base_url` + 测试 `kimi_usage_url_derivation`；`crates/tact_llm/src/provider.rs` 中的 `is_kimi_usage_supported` + 测试 `is_kimi_usage_supported_only_for_official_endpoint`；文档 `book/22_chapter_llm*.md` §3 / §9 |

---

## 2. 2026-08-06 — DeepSeek 余额查询仅限官方 `https://api.deepseek.com` 端点

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tact_llm/src/account.rs`（`query_deepseek_balance`、`deepseek_balance_url_from_base_url`）、`crates/tact_llm/src/provider.rs`（`is_deepseek_balance_supported`、`is_account_query_supported`）、`crates/tact-ui/src/account.rs`（`query_once`）、Ch 22 §3 / §9 |
| 症状 / 动机 | `query_deepseek_balance` 从任意配置的 base URL 推导 `{origin}/user/balance`，导致 DeepSeek 模型挂在自定义 OpenAI 兼容代理后时，会把代理的 API key 发往猜测出来的余额端点。DeepSeek 只在官方主机提供 `GET /user/balance`；该回退逻辑错误，可能把凭据泄露到错误主机或产生令人困惑的 404/403。 |
| 决策 | 与 Kimi 的「凭据边界」对齐：`deepseek_balance_url_from_base_url` 仅在 base URL 为空（配置默认）或为 HTTPS 且主机精确为 `api.deepseek.com`（允许 `/v1` 后缀）时返回官方 URL；其余返回 `None`，`query_deepseek_balance` 报错「DeepSeek balance API is only available for the official endpoint https://api.deepseek.com」。`ProviderInfo::is_deepseek_balance_supported` 门控 `is_account_query_supported`，因此代理配置下 TUI 底栏余额组件直接隐藏而非反复报错。 |
| 行为变化 | DeepSeek 余额轮询 / `/balance` 仅在 `base_url` 指向官方端点时可用。自定义代理（即使 model 是 `deepseek-*`）视为不支持；代理配置的 API key 永远不会发送到 `api.deepseek.com`。 |
| 指针 | `crates/tact_llm/src/account.rs` 中的 `deepseek_balance_url_from_base_url` + 测试 `deepseek_balance_url_derivation`；`crates/tact_llm/src/provider.rs` 中的 `is_deepseek_balance_supported` + 测试 `is_deepseek_balance_supported_only_for_official_endpoint`；文档 `book/22_chapter_llm*.md` §3 / §9 |

---

## 2. 2026-08-06 — `/tasks-dag` 弹窗打开期间新建的任务不显示

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tui/src/widgets/state/app/agent.rs`（`on_tasks_changed`）、`crates/tui/src/render/popups/task_dag_popup.rs`、Ch 23（TUI） |
| 症状 / 动机 | `/tasks-dag` 弹窗打开时渲染一次 Mermaid 行。`TasksChanged` 更新只刷新 `task_panel.snapshot`，从不刷新弹窗内容；渲染循环只在弹窗宽度变化时重渲染——因此弹窗打开期间新建的任务（或在打开与渲染之间加入的任务）在关闭重开前永远不会显示。 |
| 决策 | `on_tasks_changed` 现在会刷新已打开的 DAG 弹窗：用最新快照在弹窗当前 `render_width`（首帧宽度感知前回退到 `DEFAULT_DAG_RENDER_WIDTH`）重新执行 `render_task_dag_lines`，原地替换 `lines`/`mermaid_source`，保持滚动偏移。与 `render_task_dag_popup` 中既有的宽度变化重渲染互补，两条路径均幂等。 |
| 行为变化 | 弹窗打开期间新建的任务会在下一渲染帧立即出现，无需关闭弹窗。 |
| 指针 | `on_tasks_changed` 位于 `crates/tui/src/widgets/state/app/agent.rs`；回归测试 `tasks_dag_popup_refreshes_when_new_tasks_arrive` |

---

## 2. 2026-08-06 — `/tasks-dag` 依赖边缺失（任务存储不对称）

| 字段 | 值 |
|------|-----|
| 类型  | `bugfix` |
| 关联 | `crates/tact/src/task/mod.rs`（`update`、`clear_dependency`）、`crates/tui/src/widgets/state/task_dag.rs`、Ch 23（TUI） |
| 症状 / 动机 | `/tasks-dag` 只显示任务节点，**不显示通过 `task_update` 的 `addBlockedBy` 建立的依赖箭头**：`update` 会把 `addBlocks` 镜像到被阻塞任务的 `blocked_by`，但 `addBlockedBy` 从不镜像 blocker 的 `blocks`（DAG 出边）。`tasks_to_mermaid` 只从 `blocks` 画边，因此这类依赖完全不可见。另外，`clear_dependency`（任务完成时）只把已完成 id 从他人 `blocked_by` 移除，却留下已完成任务自己的 `blocks`，成为幽灵边来源。 |
| 决策 | `update` 现在双向镜像：`add_blocked_by` 也会把当前任务 id 写入每个 blocker 的 `blocks`（去重排序），与既有 `add_blocks` 分支对称。`clear_dependency` 额外清空已完成任务的 `blocks`；由于 `update` 持有的是 `clear_dependency` 之前取的副本，本地副本也同步清空，避免最终写回复活幽灵边。 |
| 行为变化 | 无论通过 `addBlocks` 还是 `addBlockedBy` 建立的依赖，都会在 `/tasks-dag` 中渲染为 `T{blocker} --> T{blocked}`。完成任务后其出边被移除。 |
| 指针 | `crates/tact/src/task/mod.rs`（`update` 的 add_blocked_by 分支、`clear_dependency`）；测试 `update_add_blocked_by_creates_reverse_outgoing_edge`、`completing_task_clears_blocked_by` |

---

## 2. 2026-08-06 — 任务完成后显示统计块（耗时 · 模型 · tokens）

| 字段 | 值 |
|------|-----|
| 类型  | `optimization` |
| 关联 | `crates/tui/src/widgets/state/app/messages.rs`（`add_task_stats_block`）、`crates/tui/src/widgets/state/app/agent.rs`（`TaskComplete` 分支）、Ch 23（TUI） |
| 症状 / 动机 | 任务结束后日志区只有任务结束分隔线（耗时标签），token 消耗与模型名仅显示在底部状态栏，且下一个任务开始时会被重置。`TaskComplete` 分支中留有 `// TODO Add task stats block` 标记。 |
| 决策 | `TaskComplete` 分支在 `add_task_end_separator()` 之后调用 `add_task_stats_block()`。统计块直接读取已冻结的状态——`last_prompt_elapsed_secs`（由分隔线设置）、`status_bar.model_name`（来自 `ModelInfo`）与 `status_bar.token_*`（来自 `TokenUsage`）——因此不新增统计结构体或重复收集（YAGNI）。通过现有 `add_system_message` 路径渲染一行 markdown：`📊 任务统计：⏱ mm:ss · 🧠 model · N tokens (prompt X · completion Y · cache Z · reasoning W)`；无模型或零 token 时省略相应片段。 |
| 行为变化 | 每个完成的任务都会在结束分隔线下方留下一行持久统计：耗时、模型名以及可用的 token 明细。取消或失败的任务不显示统计块。 |
| 指针 | `add_task_stats_block` 位于 `crates/tui/src/widgets/state/app/messages.rs`；测试 `task_complete_appends_task_stats_block`、`task_stats_block_skips_empty_parts` 位于 `crates/tui/src/widgets/state/app/agent.rs` |

---

## 2. 2026-08-06 — `/tasks-dag` 改用 ratatui-markdown 渲染 Mermaid（替换 meraid）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 相关 | `crates/tui/src/widgets/state/task_dag.rs`、`crates/tui/src/render/popups/task_dag_popup.rs`、`crates/tui/src/theme.rs`、根 `Cargo.toml`（`ratatui-markdown` git 依赖）、Ch 23（TUI） |
| 现象 / 动机 | DAG 弹窗原先用 `meraid` crate 的 Mono 渲染器（纯文本、无主题、无 markdown 结构）；而 workspace 里已有一个闲置的 `ratatui-markdown` git 依赖，且其分支名 `update-ratatui-0.30` 已失效（真实分支是 `chore/update-ratatui-0.30`），该依赖根本无法解析。 |
| 决策 | `/tasks-dag` 改走 `ratatui-markdown`：`tasks_to_mermaid` 仍生成 `flowchart TD`，`render_task_dag_lines` 将其包装为 `## Tasks DAG` + ` ```mermaid ` 代码块 + `### Legend` 列表（把 `#id` 映射回各任务 subject；节点标签保持窄：状态字形 `○`/`◐`/`✓` + `#id`，因为 fork 的 mermaid 语法遇到第一个 `]` 就结束 `[...]` 文本）。新增 `DagTheme` 适配器把应用 `Theme` 映射为 `RichTextTheme`/`MermaidTheme`（明暗主题由 `MermaidTheme::for_background` 决定），弹窗在首帧按真实宽度重渲染（`render_width` 缓存）。根依赖修正为正确分支并精简为 `default-features = false, features = ["markdown", "mermaid"]`（去掉 `image`/`scroll`/`tree`/`viewer`）。 |
| 改后行为 | 弹窗显示带主题的框线流程图 + 列出任务 subject 的图例；`y` 复制快捷键仍复制原始 Mermaid 源码。`meraid` 不再是 tui crate 的依赖。 |
| 指针 | `crates/tui/src/widgets/state/task_dag.rs`（`tasks_to_mermaid`、`render_task_dag_lines`、`DagTheme`）、`crates/tui/src/render/popups/task_dag_popup.rs`（按宽度重渲染）、测试 `tasks_dag_popup_renders_mermaid_markdown`、`ratatui_markdown_renders_diagram_and_legend` |

---

## 2. 2026-08-06 — 压缩摘要 MaxTokens 截断后自动续写

| 字段 | 值 |
|------|-----|
| 类型 | `bugfix` |
| 相关 | Ch 5 §3（摘要调用）、Ch 5 §4（校验）、`crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode`）、`crates/tact/src/recovery.rs`（`MAX_CONTINUATION_ATTEMPTS`、`continuation_message`） |
| 症状 / 动机 | 压缩摘要的 LLM 调用返回 `MaxTokens`（输出上限）时，摘要循环把它当作非法 stop reason 直接报错 `compaction summary ended with invalid stop reason: MaxTokens`，压缩整体失败——尽管部分摘要完全可用。对推理模型而言，摘要经常撞上输出预算。 |
| 决策 | 摘要调用现在有两条独立的恢复轴：瞬时传输错误仍按有界退避重试；`MaxTokens` 截断则把已产生的部分摘要作为 assistant 消息、追加一条续写提示（与主循环相同的 `continuation_message` 选择器：第 1 次用直接续写提示，之后用收敛提示）再次调用，最多 `MAX_CONTINUATION_ATTEMPTS`（3）次。每次尝试按增长的消息历史重建请求：`[User(摘要提示), Assistant(部分摘要), User(继续), …]`。续写次数耗尽后，部分摘要被接受为 best-effort 而不是报错（Codex-style 重建反正会保留最近的真实用户消息）；`MaxTokens` 因此不再是"非法 stop reason"。 |
| 变更后行为 | 截断的摘要会发出 `[compact continue n/3] summary truncated, continuing` Info，并把所有部分块合并进最终摘要，压缩成功而不是报错。拒绝 / 其它异常终止原因和空文本仍会失败且不替换旧 context。 |
| 指针 | `crates/tact/src/agent/mod.rs`（`compact_history_local_with_mode` 摘要循环、stop reason 校验），测试 `local_compact_continues_truncated_summary`、`local_compact_continues_through_multiple_truncations`、`local_compact_accepts_partial_summary_when_continuations_exhausted`、`crates/tact-ui/tests/recovery_compaction.rs`（`compact_summary_continues_truncated_response`），Ch 5 §3/§4 |

---

## 2. 2026-08-06 — 首次运行自动生成默认配置 ~/.tact/config.toml

| 字段 | 值 |
|------|-----|
| 类型 | `feature` |
| 相关 | Ch 21 §3（配置来源与优先级）、`config.example.toml`、`crates/tact/src/config/load.rs` |
| 症状/动机 | 安装脚本只装二进制，不带配置。首次启动无配置文件时必然在 resolve 阶段报 "LLM provider not configured" 退出，用户只能靠文档才知道要手动复制 `config.example.toml`——运行时没有任何提示。 |
| 决策 | `load_toml_config` 在所有搜索路径都找不到配置时，向 `~/.tact/config.toml` 写入默认模板并解析返回。模板通过 `include_str!("../../../../config.example.toml")` 在编译期嵌入，首启默认值永远与仓库内 example 同步（当前为 `deepseek` + `protocol = "chat_completions"`）。只有用户全局位置会被自动创建；项目级候选（`./.tact/config.toml`、`./config.toml`）从不写入，避免污染仓库。打印提示：`[config] no config found; wrote default template to ... — edit it to add your API key`。 |
| 改后行为 | 首次运行且任何位置都没有配置：tact-ui 创建 `~/.tact/config.toml`（模板，`api_key` 为占位符）、打印编辑提示，随后仍在占位符 key 处按原样 resolve 报错——用户编辑文件后再次启动即可。已有配置永不被触碰或覆盖。显式 `--config /path` 不存在时仍报错（不会自动创建）。写入失败（HOME 未知/不可写）回退到原先的空默认行为。 |
| 指针 | `crates/tact/src/config/load.rs`（`DEFAULT_CONFIG_TEMPLATE`、`write_default_config`、`load_toml_config`）、`config.example.toml`（头部注释）、Ch 21 §3 |

---

## 2. 2026-08-06 — Session 统计新增 RTK 输出过滤器指标

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 相关 | `docs/token_usage_schema.md`（Session Stats Display）、`crates/tact/src/hook/rtk_filter.rs`、`crates/tact/src/stats.rs` |
| 症状/动机 | 开启 `tools.rtk_filter = true` 后，bash 输出会经 `rtk pipe` 压缩，但没有任何指标说明过滤器是否真的生效、删掉了多少输出、花了多久——用户无法判断 RTK 是在省 token 还是悄悄原样透传。 |
| 决策 | `SessionStats` 新增六个 relaxed 原子计数器（`rtk_calls`、`rtk_success_calls`、`rtk_failure_calls`、`rtk_saved_chars`、`rtk_input_chars`、`rtk_elapsed_ms`），由 post-tool 钩子直接累加。必须用原子量（而非普通 `u64`），因为钩子只能拿到 `&Agent`（不可变）。只有 `rtk pipe` 以 0 退出且 stdout 非空才算一次成功；节省字符数仅在成功时按 `raw_len − filtered_len`（饱和计算，按字符而非字节）累加。`rtk_input_chars` 累计每次尝试的原始长度（成功与失败都计），使会话级节省率能把失败尝试按零节省计入。会话结束摘要新增 `RTK tokens saved` 估算（1 token ≈ 4 字符的长度启发式）与 `RTK savings rate` 行（节省字符 / 输入字符）。 |
| 改后行为 | 只要记录到至少一次 RTK 尝试，会话统计弹窗 / 退出摘要即显示 `RTK calls (s/f)`、`RTK chars saved`、`RTK tokens saved`（chars/4）、`RTK savings rate`（saved/input %）与 `RTK time`。未开启 `rtk_filter` 或没有 bash 输出被过滤时，这些行完全不显示。失败的 bash 执行（`StepStatus::Failed`）既不参与过滤也不计入 RTK 统计——其输出完整进入 LLM 上下文。 |
| 指针 | `crates/tact/src/stats.rs`（`SessionStats::record_rtk`、`summary()` 中的 RTK 行）、`crates/tact/src/hook/rtk_filter.rs`（`pipe_through_rtk` → `(output, succeeded, elapsed_ms)`、`saved_chars`、`should_filter`、`create_rtk_post_tool_hook`）、`crates/tact/src/hook/mod.rs`（`PostToolUseFn` 现接收 `StepStatus`）、`docs/token_usage_schema.md` |

---

## 2. 2026-08-05 — 统一工具族卡片文案（background + team）

| 字段 | 值 |
|------|-----|
| 类型 | `optimization` |
| 相关 | Ch 7、Ch 26（2026-07-28 条目：工具卡片标签区分） |
| 症状/动机 | 两个工具族文案仍不统一：`background_run`（`⚙️ Background Run`）vs `check_background`（`🔍 Check`）——孤零零的 `Check` 看不出在检查什么；team 协作工具 `send_message` / `broadcast` / `read_inbox` / `plan_approval`（`✉️ Send` / `📢 Broadcast` / `📬 Inbox` / `✅ Approve`）不带 `Team` 前缀，而 `spawn_teammate` / `list_teammates`（`👥 Team Spawn` / `👥 Team List`）带。 |
| 决策 | Background 族：`check_background` 改为 `⚙️ Background Check`，与 `background_run` 共用 `⚙️ Background` 前缀。Team 族：四个协作工具补上 `Team` 族名，保留各自图标（`✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`）。两族的 TUI `tool_display_name` fallback 同步为与 metadata 一致。Task 保持 `format_task_tool_title` 的 `# Task…` 人类标题。 |
| 改后行为 | 每个工具族呈现为同一族：`⚙️ Background Run` / `⚙️ Background Check`；`👥 Team Spawn` / `👥 Team List` / `✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`；`⏰ Cron …`；`🌿 Worktree …`；`🔌 Shutdown …`。 |
| 指针 | `crates/tact/src/tool/background_run.rs`（`CHECK_BACKGROUND_METADATA`）；`crates/tact/src/tool/team.rs`；`crates/tui/src/widgets/tool_widget.rs`（`tool_display_name`） |

---

## 2. 2026-08-05 — `/model` 按 provider 分流 budget/effort + model→档位映射 + effort/model per-agent

| 字段 | 值 |
|------|-----|
| 类型 | `feature` |
| 相关 | `docs/superpowers/specs/2026-08-05-llm-presets-design.md`、Ch 21（配置：`[llm.model_profiles]`、`reasoning_effort` 校验）、Ch 22（§2 ProviderInfo 静态化、§6.3 wire 表） |
| 症状/动机 | `/model` 对任何 provider 都弹同一 5 档 thinking budget，但 openai/deepseek/kimi-k3 实际发的是 `reasoning_effort`；effort 无选择入口、运行时不可改；effort 是进程全局共享（subagent 会污染主 agent）；OpenAI Responses 的 effort 是 client 构建时 snapshot，运行时修改不生效；"模型↔档位"没有静态配置表达。 |
| 决策 | 1) `/model`（及 `/model-subagent`）第二步按 `model_uses_effort` 分流：openai/deepseek/kimi k3/k3-256k → effort 选择器（deepseek 3 档 low/high/max、kimi k3 3 档、openai 6 档 minimal..max，无 none 档）；anthropic/kimi coding 系 → budget 选择器。2) 新增 `[llm.model_profiles."<model>"]`（`thinking_budgets` / `reasoning_efforts` 数组）限定第二步档位，TOML 逐字段覆盖内置 `builtin_model_profiles()`。3) **effort/model per-agent**：`CreateMessageParams.reasoning_effort` + `AgentSettings.model/reasoning_effort`；删除全局 `set_model`、`ProviderInfo.reasoning_effort`、`current_reasoning_effort_from_budget` 及 budget→effort 波段映射（不做存量兼容）；`/model` 改发 `UserCommand::SetModel` / `SetReasoningEffort`（busy 排队）。4) wire 注入全部从 request 读；DeepSeek 纯 effort 驱动（None=不传，默认 ON+high，按官方文档）；Kimi k3 支持 effort（None=默认 high；不提供关闭 thinking——会路由到 K2.6）；OpenAI Responses `create_response` 不再 snapshot effort。5) 持久化：effort 语义写 provider/subagent 的 `reasoning_effort` 字段（`[llm.model_profiles]` 是静态选项集合，不被持久化触碰）；resolve 校验放宽为 openai/deepseek/kimi。 |
| 行为变化 | `/model` 选 openai/deepseek/kimi-k3 模型 → effort 选择器（映射档位或 provider 默认）；选 anthropic/kimi-coding 模型 → budget 选择器（现状）。运行中改 model/effort 只影响当前 agent（主/subagent 独立），wire 立即跟随；Responses 也跟随。持久化后重启生效。Kimi 关闭 thinking 会被路由到 K2.6，本期不提供该 UI 入口。 |
| 指针 | `crates/tui/src/handlers/select.rs`（分流/选择器）、`crates/tact/src/config/{types,resolve,persist,mod}.rs`（model_profiles/校验/持久化）、`crates/tact_llm/src/{provider,deepseek,kimi,openai/*}.rs`（per-request effort/wire）、`crates/tact/src/agent/mod.rs`（SetModel/SetReasoningEffort）、Ch 21/22。 |

---

## 3. 2026-08-04 — `tact upgrade` 自升级命令

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 相关 | README（CLI / 自升级）、第 21 章（配置：`install_without_llm` 路径） |
| 现象 / 动机 | 用户没有就地升级到新版本的方式；升级只能手动重跑 `scripts/install.sh`（或重新源码构建）。 |
| 决策 | 新增 `tact upgrade` CLI 子命令。它会扫描 GitHub release 列表（`GET /repos/{repo}/releases?per_page=100`），找到最新一个非 draft、非 prerelease、且资产里包含当前平台 `tact-ui-v<ver>-<triple>.tar.gz` 的 release——跳过没有构建资产的 tag（例如 `v1.1.1` 最初发布时资产为 0）——下载归档，对照该 release 发布的 `SHA256SUMS` 校验，然后在 Unix 上原子替换正在运行的二进制。参数：`--check`（只打印）、`--yes`（跳过 y/N 确认）、`--repo owner/name` 或 `TACT_UPGRADE_REPO`（跟踪 fork）。Windows 暂不支持就地升级，命令会引导用户重跑 `scripts/install.ps1`。该命令经 `install_without_llm` 解析配置，无需配置任何 LLM provider。 |
| 改后行为 | `tact-ui upgrade --check` 打印当前版本与最新可安装版本；`tact-ui upgrade` 提示确认后（`y` 或 `--yes`）下载、校验 SHA-256 并替换可执行文件，最后提示重启。校验和不匹配会在替换前中止。 |
| 指针 | `crates/tact/src/upgrade.rs`（`run_upgrade`、`find_latest_release_with_asset`、`replace_current_binary`）、`crates/tact/src/config/cli.rs`（`CliCommand::Upgrade`）、`crates/tact-ui/src/main.rs`（分发）、README §3 Run |

---

## 2. 2026-08-04 — Google 语音转写遵循标准代理环境变量

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 21、23 章 |
| 现象 / 动机 | Google Speech-to-Text 客户端通过 `reqwest::ClientBuilder::no_proxy()` 构建。在只能经 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 访问 `speech.googleapis.com` 的网络中，录音虽能完成，转写请求却会绕过已配置代理，最终超时或连接失败。 |
| 决策 | 移除 Google 客户端强制绕过代理的设置，改为使用 reqwest 的标准代理环境解析。继续保持 API key 安全的错误输出：由于 Google API key 目前位于查询参数中，连接错误不会连同可能含完整请求 URL 的底层错误一起展示。 |
| 改后行为 | Google 语音转写会遵循进程代理环境，包括对应的小写变量和 `NO_PROXY`；未配置代理时仍直接连接。子进程回归测试会验证请求抵达已配置的 HTTP 代理，而不是原始主机。 |
| 指针 | `crates/tact/src/voice/transcriber.rs`（`GoogleTranscriber::new`、`google_transcriber_honors_http_proxy`）；第 21 章（语音配置）、第 23 章（TUI 语音流程） |

---

## 2. 2026-08-02 — pre-push 钩子不再把 `GIT_DIR` / `GIT_WORK_TREE` 泄漏给 `cargo test`

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | `scripts/check-rust.sh`、`.githooks/pre-push`、`crates/tact-ui/tests/subsystem_tools.rs` |
| 现象 / 动机 | Git 会把 `GIT_DIR` / `GIT_WORK_TREE` 导出给钩子进程。pre-push 钩子运行 `cargo test -p tact-ui`，其中 `worktree_create_lists_and_shows_status` 测试派生的 `git` 命令继承了这些变量。结果 git 命令没有操作测试的隔离临时仓库，而是指向了真实仓库：测试的 `git worktree add` 注册了一个多余 worktree，其 setup 的 `git init` / `git add` / `git commit` 把破坏性的 `init` 提交追加到当前分支 HEAD——于是 `git push` 可能推送一个刚被自己 pre-push 测试污染过的仓库。 |
| 决策 | 在两个钩子入口、任何子进程运行之前清除钩子注入的变量：在 `.githooks/pre-push` 与 `scripts/check-rust.sh` 顶部 `unset GIT_DIR GIT_WORK_TREE`。同时把 `core.hooksPath` 重新指向 `.githooks`，确保 git 使用受版本控制的钩子（之前安装的 `.git/hooks/pre-push` 是过期的内联副本）。 |
| 改后行为 | `git push` 在干净的 git 环境中运行 fmt/clippy/build/test；集成测试只在 `tact-tool-test-*` 临时目录内创建 worktree 与提交，绝不动真实仓库。 |
| 指针 | `.githooks/pre-push`；`scripts/check-rust.sh`；`scripts/install-git-hooks.sh`；`crates/tact/src/worktree/mod.rs`（仍基于 `current_dir`，钩子不得泄漏环境变量）；`crates/tact-ui/tests/subsystem_tools.rs` |

---

## 2. 2026-08-02 — Google Cloud API key 语音转文字 provider

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 相关 | 第 21、23 章 |
| 现象 / 动机 | 语音输入已支持 OpenAI 兼容转写和本地 `whisper.cpp`，但持有 Google Cloud Speech-to-Text API key 的用户无法直接选择 Google provider。 |
| 决策 | 新增 `VoiceProvider::Google`，使用同步 `POST {base_url}/speech:recognize?key=...`，发送 base64 编码的 LINEAR16、单声道、16 kHz WAV JSON。复用 `voice.api_key`、`voice.language`、`voice.model`；默认 `https://speech.googleapis.com/v1` 与 `latest_short`。Google 录音限制为 `1..=60` 秒。Service Account、OAuth、长任务识别、流式识别和自动分段仍不在范围内。 |
| 改后行为 | 配置 `provider = "google"` 后，短录音会发送到 Google Cloud，并将返回的 `results[].alternatives[0].transcript` 合并后沿用现有 TUI 输入流程。缺少 key、HTTP 失败、JSON 错误、空结果和取消都会报告，且不暴露凭证。 |
| 指针 | `crates/tact/src/config/{types.rs,resolve.rs}`；`crates/tact/src/voice/transcriber.rs`；`docs/superpowers/specs/2026-08-02-google-voice-transcription-design.md`；第 21、23 章 |

---

## 2. 2026-08-02 — 压缩交接摘要改为类型化消息 cell

| 字段 | 值 |
|------|------|
| 类型 | `optimization` |
| 相关 | 第 5 章 |
| 现象 / 动机 | Codex 风格重建把交接摘要当成普通 `Role::User` 文本消息追加，唯一的"特殊处理"是字符串前缀匹配（`is_summary_message`）。模型无法区分系统生成的 handoff 与真实用户输入；`[User: summary][User: prompt]` 连续 user 消息有被 provider 合并的风险；检测也很脆弱（仅前缀、非 Text cell 失效）。 |
| 决策 | 让 handoff 成为一等消息 cell：在 `tact_llm::Message` 上加 `MessageKind::Summary`（`#[serde(skip)]`，仅内存——Anthropic wire、OpenAI 转换、JSONL transcript 字节级不变），并在 cell 文本里加 `<context-handoff>` … `</context-handoff>` 包裹。检测优先按类型，SQLite store（只持久化 role + content）重载的会话回退到 `SUMMARY_PREFIX` / 标签字符串匹配。 |
| 改后行为 | `build_compacted_history` / `compacted_context` 产出带包裹、带类型标记的 cell：`<context-handoff>\nThis conversation was compacted…\n\n{summary}\n</context-handoff>`。`collect_user_messages` 按类型跳过它；重载会话按内容重新识别。普通消息的 wire 格式不变；Anthropic 永远看不到 `kind`。 |
| 指针 | `crates/tact_llm/src/content.rs`（`MessageKind`、`Message::with_kind/is_summary`）；`crates/tact/src/compact/mod.rs`（`summary_message`、`is_summary_message`、`build_compacted_history`、`compacted_context`）；`crates/tact/src/store/session_store/sqlite.rs`（`load_session`）；`book/05_chapter_compact_zh.md` |

## 2. 2026-08-02 — DeepSeek 现在可以使用 OpenAI Responses 协议

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 相关 | 第 21、5 章 |
| 现象 / 动机 | `protocol = "responses"` 对除 OpenAI 外的所有 provider 一律拒绝，DeepSeek 因此被钉死在 Chat Completions，尽管 Responses 适配器本身与端点无关，DeepSeek 端点可以服务 `/responses`。 |
| 决策 | 在 `resolve_llm` 中接受 DeepSeek 使用 `responses`，并让 `ProviderInfo::build_client()` 按 protocol 路由：DeepSeek + `chat_completions` 继续使用专用 `DeepSeekAdapter`；DeepSeek + `responses` 构建与 OpenAI 相同的通用 `OpenAiResponsesAdapter`，指向 DeepSeek `base_url`。自动 `context_management` 压缩、由 `thinking_budget` 派生的 `reasoning.effort`、Responses 会话状态续传原样生效。Kimi 与 Anthropic 仍拒绝 `responses`。 |
| 改后行为 | DeepSeek 条目可设置 `protocol = "responses"`；请求发往 `{base_url}/responses`，具备自动压缩与 reasoning 语义。显式 `POST /responses/compact` 在 DeepSeek 端点未实现（2026-08-02 实测），因此 DeepSeek + Responses 走本地摘要压缩并清掉失效基线；OpenAI Responses 仍保持严格"不回退"契约。默认仍为 `chat_completions`。 |
| 指针 | `crates/tact/src/config/resolve.rs`（`resolve_llm` 校验）；`crates/tact_llm/src/provider.rs`（`build_client`）；`docs/superpowers/specs/2026-08-02-deepseek-responses-design.md`；`docs/superpowers/plans/2026-08-02-deepseek-responses.md`；第 21 章（配置）、第 5 章（压缩） |

## 2. 2026-08-01 — Responses 压缩阈值现在会进入普通 `/responses` 请求（原生 `context_management`）

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 5、22、23 章 |
| 现象 / 动机 | `responses_compact_threshold`（以及推导值）虽然被解析并校验，却从未传给 Responses adapter：普通 `stream_message` / `create_message` 构建的 `/responses` body 中 `context_management` 被硬编码禁用（`None`）。因此生产环境下自动 provider 侧压缩被静默关闭，只有显式 `/responses/compact` 路径生效。 |
| 决策 | 把解析后的阈值贯穿整条 配置 → adapter 链路，并让它进入**每一个普通** `/responses` 请求：`LlmSettings.provider_info()` → `ProviderInfo.responses_compact_threshold` → `OpenAiResponsesAdapter` → `create_response`（`context_management: [{ "type": "compaction", "compact_threshold": N }]`）。原生状态会被持久化并回放：不透明基线（`input_items`、`compaction_id`、`logical_context_hash`）与消息在同一事务中提交，后续请求原样回放。缺少原生 Responses 压缩的端点不受支持——**绝不**回落本地摘要。 |
| 改后行为 | 配置或推导出阈值后，每个普通 `/responses` 请求（流式与非流式）都会携带 `context_management`。端点可在对话中途自动压缩基线；返回的 `compaction` item 以不透明状态往返，绝不渲染。显式压缩（`/compact`、自动触发、恢复）发送 `POST /responses/compact` 并原子替换基线；诊断只显示 item 数与 compaction id，绝不显示 `encrypted_content`。回归测试断言：配置阈值时 wire body 包含 `context_management`，未配置时省略。 |
| 指针 | `crates/tact_llm/src/openai/responses/convert.rs`（`create_response` → `context_management`）；`crates/tact_llm/src/openai/responses/mod.rs`（`OpenAiResponsesAdapter::build_wire_request`、wiremock 回归测试）；`crates/tact_llm/src/provider.rs`（`ProviderInfo.responses_compact_threshold`）；`crates/tact/src/config/types.rs`（`LlmSettings::provider_info`）；`crates/tact/src/config/resolve.rs`（阈值推导）；`crates/tact/src/agent/mod.rs`（`compact_responses_native`、原子 `replace_persisted_context_and_state`）；`docs/token_usage_schema.md`（自动 vs 显式压缩记账）；第 5、22、23 章 |

---

## 2. 2026-08-01 — Markdown 列表后的空 fenced block 不再把尾行劫持进代码卡片

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23、24 章 |
| 现象 / 动机 | 在 TUI 日志流式渲染中，如果一个**空语言** fenced block（普通 ```）紧跟在进行中的 markdown 列表/段落之后，系统会过早把它提升成独立 code card。这样围栏后的尾行会被渲染进代码卡片，而不是继续留在普通 markdown 流程里，看起来像尾行被“吞掉”或错位。这是 Tact 自身的渲染 bug，不是 Responses 协议问题。 |
| 决策 | 保留真实流式代码块（例如 ```rust）的 code-card 路径，但当 **空语言** fence 直接出现在进行中的 markdown 段落/列表后时，不再将其提升成 code card，而是继续保留在 markdown paragraph buffer 中，交给普通 markdown renderer 处理。补充一条高层日志回归测试，覆盖 list → empty fence → tail line 场景；并补一条低层 markdown 测试，证明解析层本身并未丢失尾行。 |
| 改后行为 | markdown 列表后出现的空 fence 片段，不会再把后续尾行渲染成 `Click for full code` 卡片内容。真正带语言标签的流式代码块仍保持 code card 渲染。 |
| 指针 | `crates/tui/src/widgets/state/app/agent.rs`（stream fence promotion guard）；`crates/tui/src/render/render_gap_tests.rs`（`log_markdown_list_then_empty_fence_stays_in_markdown_flow`）；`crates/tui/src/render/render_md.rs`（`render_markdown_list_then_fenced_code_then_list_tail`）；第 23、24 章 |

## 2. 2026-07-28 — 主题检测回退使用了错误主题（Ink 而非 Retro）

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23 章 |
| 现象 / 动机 | `detect_terminal_theme()` 文档注释写「Fallback: Retro」，但代码返回的是 `ThemeName::Ink`。对应的单测接受 `Dark`、`Light`、`Retro` 三者之一，`Ink` 不在其中，导致任何未设置 `COLORFGBG` / `COLORTERM` 环境变量且无 macOS 深色模式覆盖的 CI runner 都会稳定失败。 |
| 决策 | 将回退值从 `ThemeName::Ink` 改为 `ThemeName::Retro`，与文档注释及测试预期保持一致。 |
| 改后行为 | 当无终端主题环境变量设置时，`detect_terminal_theme()` 返回 `Retro`（中性暗色），而非 `Ink`。 |
| 指针 | `crates/tui/src/theme_detection.rs` |

---

## 2. 2026-07-28 — Log 左边框滚动条残影

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23 章 |
| 现象 / 动机 | Ink 等主题下，Thinking 卡片标题含宽字符（如 🧠）时，部分终端光标会短暂错位；右侧 accent 色滚动条滑块（原 `█`）的残影会留在 Log 左边框上，看起来像间断的浅蓝「阴影」。因为左边框单元格在后续帧未变，`Buffer::diff` 不再重发，残影会一直挂着。 |
| 决策 | 每帧在内容与滚动条绘制之后强制重印左边框竖线，并标记 `CellDiffOption::AlwaysUpdate`。滑块改为半块 `▐`，降低瞬时错位时的视觉冲击。 |
| 改后行为 | 左边框每帧都会以主题 `border` 色重绘到终端；宽字符标题导致的左侧 accent 残影无法持久残留。 |
| 指针 | `crates/tui/src/render/log.rs`（`restamp_log_left_border`）；`crates/tui/src/render/log_render_tests.rs` |

---

## 2. 2026-07-28 — CRUD 类工具族卡片标签按动作区分

| 字段 | 值 |
|------|------|
| 类型 | `optimization` |
| 相关 | 第 7、13–16、23 章 |
| 现象 / 动机 | Cron / worktree / team 等同族工具共用一个 display 标签（例如所有 cron 操作都显示 `⏰ Cron`）。标题几乎一样，只能靠解析 `arg_summary` JSON 区分。`visual_kind = Generic` 时还会忽略 metadata 的 `display_name`，一律走 TUI fallback 表。 |
| 决策 | 同族标签补上动词（`⏰ Cron Create` / `Delete` / `List`，Worktree / Team / Shutdown 同理）。同步 `tool_display_name` fallback。当 presentation `display_name` 非空且不等于原始 tool id 时优先使用它，让 Generic 工具以 metadata 为准。Task 不改——已有 `format_task_tool_title` 的 `# Task…` 人类标题。 |
| 改后行为 | 工具卡片标题一眼可区分动作。`background_run` / `check_background` 的 fallback 与 metadata 对齐（`⚙️ Background Run` / `⚙️ Background Check`）。 |
| 指针 | `crates/tact/src/tool/{cron,worktree,team}.rs`；`crates/tui/src/widgets/tool_widget.rs`（`display_name_from_presentation`、`tool_display_name`） |

---

## 2. 2026-07-28 — Bash 工具卡片标签恢复为 `$ Bash`

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 7、23 章 |
| 现象 / 动机 | 内置工具 `ToolPresentation` 绑到 handler 旁路后，`bash` 的 `display_name` 写成了 `"$ Shell"`。TUI 卡片显示 **Shell**，尽管工具 id 与旧回退仍是 `bash` / `$ Bash`。 |
| 决策 | 将 `BASH_METADATA.presentation.display_name` 改回 `"$ Bash"`。运行时仍用 `sh -c` 启动（不变）。 |
| 改后行为 | `bash` 工具的卡片与标题再次显示 `$ Bash`。 |
| 指针 | `crates/tact/src/tool/bash.rs`；`crates/tui/src/widgets/tool_widget.rs` 回退仍为 `$ Bash` |

---

## 2. 2026-07-28 — 语音快捷键吞掉全部键盘输入

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 21、23 章 |
| 现象 / 动机 | 配置了 `voice.voice_keybind` 后，TUI 用 `if let Some(keybind) = … else if …`：只要 option 存在就把整条分发链占住；未命中快捷键的按键到不了 `handle_insert_mode` / Normal，输入框表现为无法打字。 |
| 决策 | 先精确匹配快捷键；仅命中时跳过常规分发。未命中则照常走 slash / overlay / 模式处理。 |
| 改后行为 | `voice_keybind = "ctrl+g"` 仅对该组合键切换录制。其它键输入与导航与从前一致。未配置快捷键时仍为仅鼠标。 |
| 指针 | `crates/tui/src/lib.rs`（按键分发）；`crates/tui/src/widgets/state/app/voice.rs`（`toggle_voice_recording`）；第 21 章 `[voice]`、第 23 章 §6.6 |

---

## 2. 2026-07-28 — 输入框顶边恢复；语音按钮居中

| 字段 | 值 |
|------|------|
| 类型 | `bugfix` |
| 相关 | 第 23 章 |
| 现象 / 动机 | 用空格填充把语音标签“居中”，并带背景色时，会盖住 Input 标题与 `🎙 Voice` 之间的 Block 顶边单元格，横线看起来被“吃掉”。 |
| 决策 | 左侧 Input 标题与语音标签拆成两个 `Block` title（左对齐 + `Alignment::Center`），不再用带背景的填充空格。点击热区使用同一居中几何。 |
| 改后行为 | 启用语音时，足够宽的终端上 Input 标签与居中语音控件之间顶边可见。过窄时仍可能与左侧标题碰撞（ratatui 左侧 title 后画）。 |
| 指针 | `crates/tui/src/render/input.rs`（`voice_title`、`update_voice_button_area`）；第 23 章 §6.6 |

---

## 2. 2026-07-28 — 可配置语音录制快捷键

| 字段 | 值 |
|------|------|
| 类型 | `feature` |
| 现象 / 动机 | 语音录制只能通过鼠标点击标题栏按钮触发，键盘用户无法在不使用鼠标的情况下启动。 |
| 决策 | 新增 `voice.voice_keybind` 配置项，支持 `ctrl+<char>` 格式（如 `"ctrl+g"`、`"ctrl+r"`）。配置后，在任意输入模式下按下该快捷键即可切换语音录制（空闲→录制，录制中→停止）。未配置时（默认），语音仍仅支持鼠标操作。在帮助面板（`Ctrl+?`）全局快捷键区动态显示当前配置。仅精确匹配时消费按键事件。 |
| 改后行为 | `[voice] voice_keybind = "ctrl+g"` 启用键盘触发语音。快捷键全局生效（任意输入模式）。未命中的按键仍进入 Insert/Normal。帮助面板动态显示。空字符串、多字符键、非 ctrl 修饰符会在配置解析阶段被拒绝。 |
| 指针 | 配置：`crates/tact/src/config/types.rs`、`config/resolve.rs`、`config.example.toml`；TUI 分发：`crates/tui/src/lib.rs`（全局快捷键部分）、`crates/tui/src/widgets/state/app/voice.rs`；帮助：`crates/tui/src/widgets/help_widget.rs`、`render/popups/help.rs`；国际化：`crates/tui/src/i18n.rs`（`help_voice_record_tmpl`）；第 21、23 章 |

---

## 2. 2026-07-28 — 权限：shell 标为 Write、High 尊重 settings allow、headless ask 默认

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 10 章 |

**症状 / 动机：** 三处逻辑错误：(1) `PermissionPolicy::ShellCommand` 把非提权命令标为 Read，导致 `bash` / `background_run` / `worktree_run` 绕过 Default 提示；(2) headless `ask_user` 一律 deny，无 TUI 时 Default 几乎不可用；(3) High 风险工具忽略 settings **allow**，始终询问。

**决策：** 非提权 shell → Write；`sudo`/`su` → High。非交互 `ask_user(tool, risk)` 对 Write/Read 允许一次、对 High 拒绝。Settings 的 Deny/Allow 适用于所有风险；无 Deny/Allow 的 High 仍 ask，且跳过会话内裸名 allowlist。

**改后行为：** 普通 shell 与其它 write 一样会提示（或 headless 放行）。项目 allow 规则可按输入模式批准 High。无人值守的 High 仍需 Auto 模式或显式 allow 规则。

**指针：** `crates/tact/src/permission/mod.rs`、`crates/tact/src/tool/metadata.rs`、`crates/tact/src/agent/tool_dispatch.rs`；第 10 章。

---

## 2. 2026-07-28 — `/model` 思考预算未同步到状态栏

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 21、23 章 |

**症状 / 动机：** `/model` 已保存新的思考预算（如 32K），底栏仍可能显示旧值（如 `think high(64K)`）。落盘成功，但运行中的 agent 与状态栏未跟上。

**决策：** `UserCommand::SetThinkingBudget` 要等当前任务结束后才处理；进行中任务的旧 `ModelInfo` 会覆盖 TUI 的乐观更新，而 `set_thinking_budget` 此前不会再发 `ModelInfo`。改为在 `set_thinking_budget` 中发出 `ModelInfo`，并在 TUI 应用路径同步/扩展会话 `max_tokens`，使 `out` / `think` 一致。

**改后行为：** 确认预算后状态栏立即更新；排队的 agent 命令执行时再发一次 `ModelInfo`，重新同步 `thinking_budget` 与可能自动扩展的 `max_tokens`。

**指针：** `crates/tact/src/agent/mod.rs`（`set_thinking_budget` / `emit_model_status`）、`crates/tact/src/config/mod.rs`（`update_llm_model_and_thinking_budget`）、`crates/tui/src/handlers/select.rs`（`apply_model_and_budget_pick`）、`crates/tact-ui/src/driver.rs`。

---

## 2. 2026-07-28 — 可点击的语音转文字输入（标题栏）

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 21、23 章；`docs/superpowers/specs/2026-07-28-voice-to-text-design.md`；`docs/superpowers/plans/2026-07-28-voice-to-text-input.md` |

**症状 / 动机：** macOS 上纯键盘输入长提示不便；需要免提录音并在提交前审阅。

**决策：** 增加 `[voice]` 配置（独立 API 密钥）、`tact::voice` 工作线程（cpal 采集 → WAV → OpenAI 兼容转写），以及 TUI 标题栏右侧按钮。成功转写按 UTF-8 光标插入；转写中的 `/help` 在按 Enter 前仅为普通文本。录音/转写在事件循环外执行；`Esc` 或停止可取消。

**改后行为：** 默认 `enabled = false` 隐藏控件。`enabled = true` 显示按钮；缺少 `[voice].api_key` 时点击会提示配置。本版无实时转写、自动提交或本地 Whisper。

**指针：** `crates/tact/src/voice/`、`crates/tui/src/widgets/state/voice.rs`、`crates/tui/src/render/input.rs`、`crates/tui/src/handlers/mouse.rs`、`crates/tui/src/handlers/insert.rs`、`crates/tui/src/lib.rs`、`crates/tact-ui/src/interactive.rs`。

---

## 2. 2026-07-28 — 子 agent 元数据显示在工具卡片头部

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 12、23 章；`docs/token_usage_schema.md` |

**现象 / 动机：** 子 agent 的 `TokenUsage` 和 `ModelInfo` 作为 `ToolProgress` 内联块转发到共享父 UI 通道，在输出流中产生重复的 `⚡ N tokens` 和 `🤖 Model: …` 行。同时 TokenUsage 还会覆写主 agent 的底栏数据。

**决策：** 引入 `AgentUpdate::ToolMeta` — 专用更新路径，将模型名和 token 数量直接写入父级工具卡片的头部行，与现有的阶段/耗时信息并列显示。转发器不再为这些事件生成 `ToolProgress` 块，也不再转发到共享通道。子 agent 调用的工具卡片元数据行现在显示 `🤖 {model} · ⚡ {total}`。

**改后行为：** 底栏始终显示主 agent 的 token 统计。子 agent 的模型和 token 总数出现在工具卡片的元数据行中（如 `⠋ 运行中 · 🤖 deepseek-v3 · ⚡ 4.2K · 3.2s`），通过 `ToolMeta` 实时更新并在完成后保留。输出流中不再出现内联的元数据行。

**指针：** `crates/tact/src/tool/subagent_ui.rs`、`crates/tui/src/widgets/tool_widget.rs`、`crates/tui/src/render/cells/tool.rs`、`crates/tui/src/widgets/state/app/agent.rs`、`crates/protocol/src/agent.rs`；`docs/token_usage_schema.md`；第 12、23 章。

---

## 2. 2026-07-27 — 权限设置持久化（基于 JSON 的动态规则）

| 字段 | 值 |
|------|-----|
| **类型** | docs |
| **相关** | 第 7、21 章；`docs/superpowers/specs/2026-07-27-permission-settings-design.md`；`docs/superpowers/plans/2026-07-27-permission-settings.md` |

**现象 / 动机：** 权限决策仅存储在会话级内存（`always_allowed_tools`）中。「总是允许此工具」每次授予的是裸工具名、无参数感知的全局放行，会话之间不持久化，且无法在不修改 `config.toml` 的前提下预配置 deny 或 ask 规则（TOML 文件不适用于动态规则写入）。

**决策：** 引入基于 JSON 的权限设置，分为全局范围（`$HOME/.tact/settings.json`）和项目范围（`<workdir>/.tact/settings.json`）两层。规则采用 Claude 风格的工具+参数语法（`tool(field:pattern)`）并支持 glob 匹配。优先级为 `deny > ask > allow`，与数组顺序无关。项目写入采用原子操作（临时文件 + rename），保留未知 JSON 字段，去重。格式错误的文件或非法规则视为软失败（告警 + 跳过）。高风险确认始终强制，不受 allow 规则影响。

**改后行为：** 动态 allow/ask/deny 规则存储在 JSON 设置文件中，而非 `config.toml`。「总是允许此工具」会写入一条参数感知的规则（例如 `bash(command:cargo test *)`）到项目文件。缺少文件等同于空策略。TOML `[permission].mode` 继续仅控制模式（`default` | `plan` | `auto`）。Plan 和 Auto 模式的语义保持不变。

**指针：** `crates/tact/src/permission/settings.rs`、`crates/tact/src/permission/mod.rs`、`crates/tact/src/consts.rs`、`crates/tact/src/agent/tool_dispatch.rs`、`crates/tact/src/tool/subagent.rs`、`crates/tact-ui/src/interactive.rs`、`crates/tact-ui/src/headless.rs`；`docs/superpowers/specs/2026-07-27-permission-settings-design.md`；`docs/superpowers/plans/2026-07-27-permission-settings.md`；`docs/state_machines.md §5`；`config.example.toml`；第 7、21 章。

## 2. 2026-07-27 — 日志滚动恢复主题背景

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章；`docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`；`docs/superpowers/plans/2026-07-27-log-scroll-artifact-fix.md` |

**现象 / 动机：** 从 code-card 或其他带样式的 Log 内容滚动离开后，普通文本行可能保留前一帧的背景样式。深色 Ink 主题下该问题尤其明显，文字后方会出现阴影。

**决策：** 保留 Log viewport 的重置，并让 `TextCell` 写入每个普通字形时显式应用当前 `theme.bg`。该规则与主题无关；卡片与 overlay 层保留既有背景和绘制顺序。

**改后行为：** 滚动新露出的任意普通 Log 行都使用当前主题背景，同时保留前景样式和选区反色 modifier。不使用 Ink 专用分支或全局终端清屏策略。

**指针：** `crates/tui/src/render/log.rs`；`crates/tui/src/render/cells/text.rs`；`crates/tui/src/render/log_render_tests.rs`；`docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`；第 23 章。

---

## 2. 2026-07-27 — 子 agent 弹窗显示所用模型

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 12、23 章；`docs/token_usage_schema.md` |

**现象 / 动机：** 实时/完成后的 `spawn_subagent` 弹窗会显示子调用的 token 总数、缓存命中率和 prompt 上下文，却不显示生成这些数据的模型。agent 会发出 `ModelInfo`，但子 agent UI 转发器此前直接丢弃了该事件。

**决策：** 将子级 `ModelInfo` 格式化为弹窗转录中的结构化行：`🤖 Model: {model}`。它只走 `ToolProgress` 路径，不转发到共享的父级 UI 通道。

**改后行为：** 每次子级模型调用都会在该子 agent 弹窗中、既有 token 行旁显示模型名。父级底栏继续保留父 agent 的模型名（配套的 TokenUsage 修复见 2026-07-28）。

**指针：** `crates/tact/src/tool/subagent_ui.rs`；`docs/token_usage_schema.md`；第 12、23 章。

---

## 2. 2026-07-27 — Ink 主题 + 统一弹出层 Chrome

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 21、23 章；`docs/tui_rendering.md` |

**现象 / 动机：** 默认主题为 `retro`；弹出层覆窗口边框类型不一致、颜色硬编码、缺乏共享 chrome。

**决策：** 添加 `ink`/`ink-light` 主题，颜色精确匹配像素；新增 `heading`/`version`/`muted` Theme 字段；所有 overlay 统一使用 `render_popup_chrome`。默认主题改为 `ink`。

**改后行为：** 默认主题为 `ink`；所有 overlay 弹窗共享一致的边框、标题栏（粗体标题、`[x]` 提示）与底栏布局；弹窗代码 DRY。

**指针：** `crates/tui/src/theme.rs`、`crates/tui/src/render/popups/mod.rs`、`crates/tui/src/render/render_md.rs`、`crates/tact/src/config/resolve.rs`

---

## 2. 2026-07-26 — 子 agent 工具改名 `task` → `spawn_subagent`

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 7、10、11、12、19 章 |

**现象 / 动机：** spawn 子 agent 的工具原名 `task`，与四个持久化任务工具（`task_create` / `task_get` / `task_list` / `task_update`）共享前缀，语义却完全不同。模型与读者会把「`task` 工具跑完」当成「任务记录已完成」——实际观测到一次：子 agent 已返回，清单项仍停在 Pending。第 1、11、12、19 章各挂一句免责说明作为绕过。

**决策：** 工具改名 `spawn_subagent`（动词 + 对象，与 description 一致）；包装类型 `TaskTool` → `SpawnSubagentTool`，handler `task()` → `spawn_subagent()`。持久化任务工具保留 `task_*` 前缀。`spawn_subagent` 仍为 `CapabilityRisk::High`，仍是调度 barrier。

**改后行为：** 面向模型的工具名为 `spawn_subagent`，不再存在名为 `task` 的工具。含历史 `task` tool_use 块的旧 session 仍可恢复 —— `load_history` 只渲染 `Text` 块，router 仅在实时 dispatch 时按名解析，缺名不会报错。内存态 `always_allowed_tools` 按会话重建，无需迁移。

**指针：** `crates/tact/src/tool/subagent.rs`、`crates/tact/src/tool/registry.rs`、`crates/tact/src/permission/mod.rs`

---

## 2. 2026-07-26 — `TasksChanged` 不再追加 Log 卡片

| 字段 | 值 |
|------|-----|
| **类型** | removal |
| **相关** | 第 19、23 章 |

**现象 / 动机：** `on_tasks_changed` 原会追加一条 `📋 # Task.N · …` 系统消息，与已渲染同样标题的 `task_*` 工具行重复。commit `4116c23` 把这段发送逻辑注释掉（属于该 commit 的误伤）而非删除，于是 `format_tasks_log_card` 挂着 `#[allow(dead_code)]` 空转，`tasks_changed_shows_panel_and_appends_log` 长期变红。

**决策：** Log 中只保留工具行这一种表示。删除 `format_tasks_log_card`、`focus_changed_task`、`primary_action_for_change`；测试改为断言 sticky 已更新且 Log 长度不变。`AgentUpdate::TasksChanged` 保留 `reason` 字段 —— 生产端与协议不变。

**改后行为：** 一次 `task_create` / `task_update` 只产生一条 Log 行（工具卡）加一次 sticky 刷新，不会出现两条。

**指针：** `crates/tui/src/widgets/state/app/agent.rs`、`crates/tui/src/widgets/state/task_panel.rs`

---

## 2. 2026-07-26 — sticky 主机分隔 tab 与正文

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章 |

**现象 / 动机：** `sticky_host_content_height` 只预留 `1 + body` 行，渲染器把正文画在 `inner.y + 1`，于是 tab 行（`[Tasks] [Subagent] …`）紧贴 `── Pending ──` / 子 agent 日志，上方又紧邻 Log 框边框，整体挤成一块。

**决策：** 多预留一行（Tasks 为 `2 + body`，Subagent 为 `3 + header + lines`），并在 tab 行与正文之间画一条全宽淡色 `─` 分隔线。

**改后行为：** 展开的 sticky 依次为 tab、分隔线、内容。折叠高度仍为 1 行。

**指针：** `crates/tui/src/render/task_panel.rs`

---

## 2. 2026-07-26 — Bash 非 0 退出记为 Failed

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 7 章 |

**现象 / 动机：** `bash` 收集了 `ExitStatus` 却未使用，`cargo test` 失败等非 0 退出仍显示 `Success · …`，而输出里已是错误信息。

**决策：** 进程正常结束后若 `!status.success()`，经 `error_with_partial` 返回 `Err`（`exit code N` 或 `terminated by signal`），映射为 `StepStatus::Failed`，并保留已捕获输出给模型。

**改后行为：** shell 非 0 退出在 TUI 显示 Failed；0 退出不变。

**指针：** `crates/tact/src/tool/bash.rs`

---

## 2. 2026-07-25 — Subagent sticky tab（主 Log 保持干净）

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 12 / 23 章；`docs/superpowers/specs/2026-07-25-subagent-sticky-pane-design.md` |

**现象 / 动机：** 子 agent 共用父级 `ui_tx`，Stream/Step/Thinking 混进主 Log，子级 `TokenUsage` 覆盖底栏。

**决策：** 子更新打成 `AgentUpdate::Subagent`；sticky 主机 tab：Tasks | Subagent；主 Log 只留父 `task` 工具行；`RequestSelect*` 透传；首次自动切 tab，之后仅角标。

**改后行为：** 嵌套工作在 Subagent 可见；`task` 期间主 Log 与 ctx 仪表保持父级语义。

**指针：** `crates/tact/src/tool/subagent_ui.rs`、`crates/tui/src/widgets/state/subagent_pane.rs`、`crates/tui/src/render/task_panel.rs`

---

## 2. 2026-07-25 — 子 agent session 经 `ref_id` 关联

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 1 / 12 章；`docs/superpowers/specs/2026-07-25-subagent-session-ref-design.md` |

**现象 / 动机：** `task` 子 agent 无 `session_id` / store — 轮次、token 用量与 DeepSeek `user_id` 隔离都缺失；`task` 中途崩溃则子历史全丢。

**决策：** 每个子 agent 新建 session 行，`sessions.ref_id` = 父 id（父无 session 则为 `''`）。`list_sessions` 只返回顶层（`ref_id = ''`）。`delete_session` 级联删子。子会话不抢 `SessionLock`。

**改后行为：** 子 agent 消息 / `token_usages` 落在子 id 下；`--list-sessions` 仍只见父；删父带走其子。

**指针：** `crates/tact/src/tool/subagent.rs`、`crates/tact/src/store/session_store/sqlite.rs`、`ToolContext.session_id` / `session_store`

---

## 2. 2026-07-25 — 低占用时 ctx 进度条可见

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 23 章；`docs/token_usage_schema.md` |

**现象 / 动机：** 上下文窗口为 1M 时，约 1%（`13.7K/1M`）会画 `▏`（1/8 格）。紧挨 `·` 时这条发丝几乎看不见，数字已是 `1%` 但条看起来仍是空的。

**决策：** 任意正小数格至少钳到 `▍`（3/8）；`frac > 0` 时不再回退成 `·`。

**改后行为：** 非零 ctx 占用在 `[…]` 内必有清晰半格（例如 1% → `[▍·······]`）。

**指针：** `crates/tui/src/render/bar.rs`（`partial_block_char` / `render_usage_bar`）

---

## 2. 2026-07-25 — Task 工具标题、Log 短卡、sticky 树、`/tasks-dag`

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **相关** | 第 11 / 19 / 23 / 25 章；`docs/superpowers/specs/2026-07-25-task-tool-ui-redesign.md` |

**现象 / 动机：** `task_*` 工具行是 raw JSON；Log 卡重复整板 checklist；终端里难看依赖关系。

**决策：** 可读 tool 标题（`# Task.N · …`）；sticky 默认展开为 `blocks` 树并带 `#id`；`/tasks-dag` 弹窗渲染 Mermaid DAG（节点仅状态+id；2026-08-06 起改用 ratatui-markdown 渲染）。`TaskSnapshot` 携带 `blocks`/`blocked_by`。Log **不再**追加任务系统卡（进度看 sticky + tool 行）。

**改后行为：** tool 行可读；sticky 树形；slash 可看 DAG；Log 不再刷任务系统消息。

**指针：** `crates/tact/src/task/display.rs`、`crates/tui/src/widgets/state/task_panel.rs`、`crates/tui/src/widgets/state/task_dag.rs`

---

## 2. 2026-07-25 — 任务清单完整渲染（去掉 `… +N`）

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 19 / 23 章 |

**现象 / 动机：** Log 详情卡与 sticky 展开最多只显示 6 行（`… +N`），8 条任务时即使已全部更新也像未完成。

**决策：** 去掉 `STICKY_BODY_CAP`；sticky 高度与 Log 卡列出全部任务。

**改后行为：** sticky 展开与每次 `TasksChanged` Log 卡均显示完整清单。

**指针：** `crates/tui/src/widgets/state/task_panel.rs`

---

## 2. 2026-07-25 — 同一 turn 内串行持久化 `task_*` 工具

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **相关** | 第 11 / 19 章 |

**现象 / 动机：** 模型常在一轮里发出大量 `task_update` / `task_create`。若落在同一 wave 并行执行，TaskManager 更新与 `TasksChanged` UI 事件会交错，Log 挤成一团，进度卡也不完整。

**决策：** 将 `task_create` / `task_update` / `task_get` / `task_list` 标为合成资源 `__tact_tasks__` 的写者，保证分属不同 wave（保序），但仍可与无关的 `read_file` 重叠。

**改后行为：** 同一 assistant 工具批次内，task 工具逐个执行；每次 mutating 调用可按序各自发出 `TasksChanged`。

**指针：** `crates/tact/src/agent/tool_schedule.rs`

---

## 2. 2026-07-24 — 持久任务 sticky 进度 + Log 详情卡

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

## 2. 2026-07-24 — 底栏去掉冗余 `[Log]`

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
## 3. 2026-07-24 — Slash 弹窗 Esc 提示 + 优先于 overlay

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
## 4. 2026-07-24 — 空闲底栏 `Up` 低开销走秒

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
## 5. 2026-07-24 — 任务耗时挪到 task-end 分隔线

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

## 6. 2026-07-24 — 底栏可读性回补

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

## 7. 2026-07-24 — Slash 弹出：Tab 补全，Enter 运行 skill

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

## 8. 2026-07-24 — 移除 TUI 左侧 Execution Plan 面板

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

## 9. 2026-07-24 — 项目配置文件 `tact.toml` → `config.toml`

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

## 10. 2026-07-24 — Session Stats GFM 单元格填充以对齐纯文本

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

## 11. 2026-07-24 — 额外 `skill_dirs` + 项目本地 `.tact/skills`

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

## 12. 2026-07-24 — `/skills` 列表改用 tui-markdown（不用 pipe 表）

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

## 13. 2026-07-24 — Session Stats 用 GFM 表格 + tui-markdown 渲染

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

## 14. 2026-07-24 — Session Stats 用 comfy-table 排版

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

## 15. 2026-07-24 — `/model` 从 `/v1/models` 补充配置

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

## 16. 2026-07-24 — `read_file` 分页与删除 `batch_read`

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

## 17. 2026-07-24 — 底部栏视觉优化

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
