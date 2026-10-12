# Subagent 弹窗重绘 — 设计

日期：2026-10-10
可视稿：`docs/design/subagent-popup.html`

移植说明：Ported onto the post-migration layout (2026-10-12).

## 1. 背景

子 agent 的整条会话在转发层（`crates/tact_extensions/src/tool/subagent_ui.rs`）被**压成一坨纯文本**：

| 事件 | 现在落成的文本 |
|---|---|
| `StreamChunk` | 原样追加 |
| `ThinkingChunk::Finished` | `\n\n🧠 Thinking\n\n<全文>\n\n` |
| `StepStarted` | `\n\n→ {tool_name} {arg_summary}\n\n` |
| `StepFinished` | `\n\n✓ {message}\n\n` |
| `StepFailed` | `\n\n✗ {error}\n\n`（stderr） |
| `Info` | 原样 |
| `Error` | `error: {err:?}` |

这些 chunk 进父级工具卡的 `live_output: ToolOutputBuffer`；弹窗
（`agent_tui_kit/src/render/popups/subagent_popup.rs`）再从它取文本：

- **运行中**：`live_output.full_detail_text()` → `source.lines()` 每行 `Line::from(...)`，全部
  `theme.fg` on `code_block_bg()`。
- **完成后**：`output.detail_full` → 整段过 `render_markdown_tui`。

由此产生的问题（可视稿 A 节有对照图）：

1. **结构在进 UI 之前就丢了。** 工具名/参数/结果/耗时没有分层，`→` `✓` `🧠` 只是三个 emoji 前缀，
   全篇一个颜色。
2. **live 与完成是两套排版。** live 显示 Markdown 源码（`**$78,629**`），结束瞬间整段重排成渲染后的
   Markdown —— 滚动位置跟着跳。
3. **`Prompt:` 与提示词粘成一行**（模板是 `"Prompt:\n{p}\n\n"`，单个 `\n` 在 Markdown 里是软换行 = 空格）。
4. 没有运行身份：模型、token、耗时、child id、终态在弹窗里都看不到（它们只到了卡片头的 `ToolMeta`）。

## 2. 目标 / 非目标

**目标**

- 弹窗用**主 agent 同一套渲染单元**渲染子 agent 的会话：`MarkdownCell` / `ThinkingCell` / `ToolCell`。
- 数据来自子 agent 自己发出的**结构化更新**，不从扁平文本反推。
- **live 与完成后同一套渲染**，结束时只有状态词与行数定格，不重排。
- 弹窗头部补上运行身份（child id / model / tokens / elapsed / status）。
- 键位、鼠标选择、`y` 复制行为不变。

**非目标（本次不做）**

- 不动父级 `spawn_subagent` 卡片本身（2 行头 + 8 行 live 预览 + Summary 卡）。
- 不做回合分隔线（主日志也没有）。
- 弹窗内工具卡的 `[ Open ]`（嵌套详情弹窗）与子 agent 权限请求的就地渲染 —— 见 §9 阶段 2。

## 3. 数据模型（`tact_protocol`）

新增 `crates/tact_protocol/src/subagent_transcript.rs`：

```rust
/// 子 agent 会话里的一条结构化记录。
#[derive(Debug, Clone)]
pub enum SubagentTranscriptEntry {
    /// 一段助手文本（连续 StreamChunk 合并成一段）。
    Assistant(String),
    /// 一个已结束的思考块。
    Thinking(String),
    /// 一次工具调用；`StepStarted` 与 `StepFinished`/`StepFailed` 各发一次，
    /// 靠 `tool_id` 在 UI 侧更新同一张卡。
    Step(SubagentStep),
    /// 子 agent 发出的信息行（`Info` / `MdInfo`）。
    Info(String),
    /// 错误行（`Error`）。
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SubagentStep {
    /// 子 agent 本地的 tool id —— 同一张卡的两次更新的键。
    pub tool_id: String,
    pub tool_name: String,
    pub arg_summary: String,
    pub arg_full: String,
    pub status: StepStatus,          // Running | Success | Failed
    pub message: String,             // 结果摘要 / 错误文本
    pub detail: Option<String>,      // 完整结果（预览卡与将来的 Open 用）
    pub duration_us: Option<u64>,
    pub presentation: ToolPresentationInfo,
}
```

`AgentUpdate` 新增一个变体：

```rust
/// 子 agent 会话的一条结构化记录。与喂卡片预览的扁平 `ToolProgress`
/// 并行发出 —— 前者给弹窗，后者给卡片的 8 行 live 预览与 `y` 复制的兜底。
ToolTranscript { tool_id: String, entry: SubagentTranscriptEntry },
```

**为什么保留扁平文本**：`ToolWidget::with_live_output` 的卡片预览（`preview_lines`）和
`copy_subagent_popup` 的兜底都读它，改动这两条路径的收益为零、风险不为零。扁平文本从「弹窗的数据源」
降级为「卡片预览 + 复制兜底」。

## 4. 转发层（`crates/tact_extensions/src/tool/subagent_ui.rs`）

`tagged_ui_channel_with_progress` 已经在 match 每一个事件，改动只是**在既有分支里多推一条
`ToolTranscript`**：

- `StreamChunk` → 累积进一个 `assistant_buf`；在**下一条非 StreamChunk 事件**或
  `TaskComplete`/`TaskCancelled` 到来时，先 flush 成一条 `Assistant`。这样连续 chunk 合并成一段
  Markdown（而不是每 chunk 一段）。
- `ThinkingChunk::Finished` → 现有的 `format_thinking_block` 文本 chunk 照发（卡片预览用），
  同时发 `Entry::Thinking(全文)`。
- `StepStarted` → 现有 `→ …` chunk + `Entry::Step { status: Running, … }`。
- `StepFinished` → 现有 `✓ …` chunk + `Entry::Step { status: Success, message, detail, duration_us,
  presentation, … }`（同一个 `tool_id`）。
- `StepFailed` → 现有 `✗ …` chunk（stderr）+ `Entry::Step { status: Failed, message: error, … }`。
- `Info` / `MdInfo` → `Entry::Info`。
- `Error` → `Entry::Error`。
- `TokenUsage` / `ModelInfo` → 不变（`ToolMeta`），弹窗额外从 `ToolMeta` 读（见 §6）。
- `RequestSelect*` / `TaskComplete` / `TaskCancelled` → 不变。

`progress.report(...)` 与 `inner.send(ToolTranscript …)` 是两个通道，同一个分支里发出，顺序固定为
先 transcript 后扁平文本（保证弹窗不会先看到文本、后看到结构）。

## 5. 状态与生命周期（`agent_tui_kit`）

**存在哪里。** `ToolState` 新增旁路 map：

```rust
/// tool_id → 子 agent 的结构化会话。与 `active`/`blocks` 平行，
/// 卡片终态后仍然保留（弹窗要能重开）。
pub transcripts: HashMap<String, SubagentTranscript>,
```

**不放进 `ToolRenderOutput`**：日志渲染器每帧 `output.clone()`，把整条会话塞进去就是每帧复制一次。

`SubagentTranscript`：

```rust
pub struct SubagentTranscript {
    pub entries: Vec<SubagentTranscriptEntry>,
    /// 追加计数 —— 布局缓存的失效键，帧间不变就不重排。
    pub revision: u64,
}
```

**生命周期**

- `ToolComponent::on_update(ToolTranscript)` → 追加进 `transcripts[tool_id]`，`revision += 1`。
  同一 `tool_id` 的 `Step` 就地替换最后一条匹配项（`StepStarted` → `StepFinished`）。
- 同一 `tool_id` 重启（`StepStarted` 且已有 active）→ 清空该 id 的 transcript（与卡片 `Cancelled` 同步）。
- `StepFinished` / `SubagentFinished` → **不动 transcript**（不再需要像今天那样
  `take_full_detail()` 把文本搬进 `detail_full` 才能给弹窗用；`detail_full` 继续服务卡片 Summary）。

## 6. 渲染

### 6.1 布局缓存

`SubagentPopup` 改为：

```rust
pub struct SubagentPopup {
    pub title: String,
    pub scroll: u16,
    pub tool_id: String,
    pub selection: Option<PopupTextSelection>,
    /// 已构建的段 + 选择文本；键 = (revision, 宽度, 主题, is_live)。
    pub layout: Option<SubagentPopupLayout>,
}
```

`SubagentPopupLayout`：

```rust
pub struct SubagentPopupLayout {
    pub revision: u64,
    pub width: u16,
    pub theme: ThemeName,
    pub is_live: bool,
    /// 视觉行总数（滚动钳制用）。
    pub total_rows: usize,
    /// 可选中文本 = 各文本段按顺序拼接；每段记自己的字节区间。
    pub selection_text: String,
    pub segments: Vec<PopupSegment>,
}

pub enum PopupSegment {
    Text { vis_start: usize, cell: TextCell },
    Markdown { vis_start: usize, cell: MarkdownCell },
    Thinking { vis_start: usize, cell: ThinkingCell },
    Tool { vis_start: usize, cell: ToolCell },
    Meta { vis_start: usize, lines: Vec<Line<'static>> },
}
```

`prepare_subagent_popup` 只在键变化时重建（沿用现有 `is_valid` 指纹思路，键换成
`revision` 而不是文本长度）。

### 6.2 段序

```
[Meta]       🤖 model · ⚡ tokens · ⏱ elapsed · ● status                  （不可选，1 行）
[Text]       💬 <prompt>                                                  （可选）
[Text/Markdown/Thinking/Tool]  ← transcript 逐条
```

- **提示词按 Markdown 渲染**（`💬 ` 标记与渲染后的首行同一段）。主日志把**人打的** user 行画成纯文本，
  但子 agent 的提示词是**父级生成的简报**（标题、编号清单、粗体引导语），而弹窗就是读它的地方——
  照原样显示 `##` / `**` 会让弹窗里读者一定会看到的那一段看起来是坏的。标记同时进选中文本与首行
  （字节数相同），所以选区偏移仍然对得上。
- `Assistant(text)` → Markdown 渲染成行，作为**可选中的文本段**。
- `Thinking(text)` → **自包含块**：`▎ 🧠 Thought · ⏱ 1.2s` 一行 + 正文（rail `│`，全文不封顶）。
  推理只在这里可见（日志里的思考卡属于父级），封顶等于藏起来。
- `Step(step)` → **自包含块**：标题行（`▎ 1. ⚡ Bash  args`，来自 `ToolWidget` 的 `title_raw`）、
  阶段行（`✓ Success · 133ms · 42 lines`）、输出预览（rail `│` + code band）、以及
  `⋯ N more lines`。**没有边框、没有 `[ Open ]`** —— 块是只读的。
  运行中时阶段行的 `●` 在**绘制时**换成 `TOOL_RUNNING_SPINNER` 的当前帧（`BlockRow::spinner` 记列号）：
  布局里烤进去的仍是 `●`，所以 copy 缓冲、字节范围、缓存键都不受影响（见 §7）。
- `Info` / `Error` → 可选中的文本段（`Info` 用 `theme.muted`，`Error` 用 `theme.error`）。
- **块不做交互**：弹窗自己就是一层 overlay，块再承诺「点开看全文」就是弹窗套弹窗，Esc 该关谁没有答案。
  剩余输出**计数**而不是给按钮（2026-10-11 起那行计数在**视图**里就是展开键，见 §8）；
  `y` 仍复制块的文本，块行本身也可拖选（见 §7）。
  调研与下一步见 `docs/design/subagent-presentation.html`。
- **步骤号**用独立的 step 计数器（0 基），不是 transcript 的 entry index —— 思考/助手也是 entry，
  按 entry 编号会让子 agent 的第一次工具调用显示成 `4.`。
- **输出预览**直接读 `detail_full`（不用卡片的 `detail_preview` —— 那个切片是按「流里两行」调的），
  上限 `POPUP_TOOL_PREVIEW_LINES = 8`；Command 取**尾部**（与 `ToolWidget::build` 一致）并丢掉
  详情开头那行 `$ <command>`（标题里已经有命令）。

缩进沿用主日志常量：思考卡 `LOG_THINKING_INDENT`(2)，工具卡 `LOG_TOOL_BLOCK_INDENT`(8)，
正文与提示词 0。

### 6.3 绘制

```rust
let mut renderer = LogColumnRenderer::new().with_viewport(scroll, body_area.height as usize);
for seg in &layout.segments { renderer.push(seg.vis_start(), seg.cell()); }
frame.render_widget(renderer, body_area);
```

复用 `LogColumnRenderer` 的视口裁剪（`cell_slice` + `render_partial`），不新写行循环。

**不变量（沿用 `AGENTS.md` 的渲染规则）**：每个单元必须自己把 `theme.bg` 刷满自己的 `area`；
`TextCell`/`MarkdownCell` 已满足，`Meta` 段用 `Paragraph::new(...).style(bg)` 显式刷底；
新单元带一条缓冲区测试断言空白格携带 `theme.bg`。

### 6.4 live == 完成后

删掉 `prepare_subagent_popup` 里的 `is_live` 排版分叉：两个状态都走同一套段。
`is_live` 只影响：

- meta 行的状态词（`● running` / `✓ Success` / `✗ Failed` / `⏹ Cancelled`）；
- 最后一条 `Assistant` 段（流式增长中）**只在 `revision` 变化时**重建 —— 而 `revision` 本来就只在
  追加时 +1，所以帧间不重排。

## 7. 选择与复制

**已落地（2026-10-11）：视图里凡是能读的文字都能选，块行也在内。**

- 可选中文本 = 各段按顺序拼接（提示词 + Assistant + Thinking 正文 + Step 的
  `arg_full` 与 `message`/`detail` + Info/Error），段间以空行分隔；每个段记
  `bytes: Range<usize>`（它在拼接串里的位置）。
- 段内的 `DisplayRow` 偏移是**全局**的（`base + line.start`），所以弹窗那一个选区能直接与任意段的
  `bytes` 求交（`segment_selection`），**结果留在全局空间**——行命中的字节也是全局的，减掉段的起点
  会让除第一段以外的高亮全部消失（第一段的 `bytes.start` 恰好是 0，所以看着一直是对的）。
- **块行（Thinking / Tool）同样可选**：行由 `selectable_text::display_row_from_line` 构建，rail 是
  *有格无字节*（拖动经过它选不到东西，高亮从正文开始）；copy 缓冲由每个 copy 行**自己按顺序追加**
  （行间 `\n`、块间空行），所以每行的字节范围是构造出来的、绝对精确，不从画出来的字形反推
  （`wrap_line` 会把一行切成多个视觉行，事后累加迟早错位）。`BlockCell` 走
  `DisplayRow::spans(selection)` 上 `REVERSED`。
- 只有 **Meta 行**（模型/token/耗时/状态）与面包屑、溢出 toggle 行不可选：它们是 chrome，且不在 copy 缓冲里。
- 命中行仍走 `PopupMouseSurface::hit_rows` + `PopupHitRow`，文本段与块段都产出 hit row（按屏幕序）。
- `y`：有选区复制选区，否则复制 `layout.selection_text`（未建布局时退回 `SubagentTranscript::selection_text`）。
- **缓冲不变量**：`TextRowsCell` / `BlockCell` 每行先 `buf.set_style(rect, bg)` 再画文本，所以行尾空白携带
  `theme.bg`。唯一例外是**紧跟宽字形（emoji / CJK）的那一格**：ratatui 在 diff 里故意跳过它
  （终端用字形自身画满两格），缓冲区测试必须跳过它。

## 8. 交互

**已落地（2026-10-11，视图化之后）：** 路由在 `handlers/normal.rs` 的 `show_subagent_view` 分支与 `handlers/mouse.rs` 的文本弹窗分支（**不是** `handlers/overlay.rs`——子代理已不在 overlay 体系里）。

| 键 / 操作 | 行为 |
|---|---|
| `j`/`k`/`↑`/`↓`、滚轮 | 滚动（`SubagentPopup::scroll`，渲染时钳制到 `total_rows - height`） |
| `g`/`G` | 顶 / 底（`G` 用 `u16::MAX` 哨兵） |
| `y` | 复制（有选区复制选区，无选区复制整段） |
| **点某个块底部的 `⋯ N more lines · ▾ expand`** | **只展开这一块**（8 行 ↔ 50 行）；展开后同一行是 `▴ collapse`（若 50 行上限仍截掉了内容，计数附在其后），再点收起。状态 = `SubagentPopup::expanded_steps`（按子 agent 本地 step id），命中矩形随 `PopupMouseSurface::block_toggles` 交回 host（`MouseState::subagent_block_toggles`） |
| `Enter` | 批量：全部展开 / 全部收起（键位版的粗粒度手势） |
| `Esc` / 点击面包屑行 | 退回主日志（`subagent_back_area`） |
| 鼠标拖选 | 选中文本（body 矩形；toggle 行在命中阶梯里**先于**选择，否则点击会变成开始拖选） |

粒度是**每块**而不是每视图：子 agent 实测 34 步，一次展开全部会把读者要找的那段埋掉。计数行从「只说明事实」变成「既是说明也是按钮」——视图不是弹窗，原地变长不是套娃，所以这条允许画可点字形（`▾`/`▴`）。

面包屑行右侧画 `Enter expand all` / `Enter collapse all`：视图没有 footer，这是唯一能广告真键位的地方；宽度不够时先丢提示。

## 9. 下一步（见 `docs/design/subagent-presentation.html`）

弹窗内的 `[ Open ]` 死按钮**已消除**。调研三个开源项目后的结论——正确的下一步不是把交互接回弹窗，而是**换容器**：

1. **日志内嵌套**（Crush 做法）：子 agent 的块作为 `spawn_subagent` 卡片的子项，同一个滚动、缩进一级。**未采用**：Tact 子 agent 跑完整 agent loop（实测 34 步），内联进父日志＝把父对话埋掉；且 `subagent_toolset()` 里没有 `spawn_subagent`，深度恒为 1，树退化成一层。
2. **进入视图**（OpenCode / Roo Code 做法）：面包屑 + `Esc` 退一层。**已落地 2026-10-11** —— `render_subagent_view` 占主区域，面包屑 `‹ N. Subagent · 标题`，块自己的溢出行可点展开（`Enter` 是批量键）。
3. **弹窗退役**，键盘 / 鼠标路由平移。**已落地**：`render_subagent_popup` 与 `title_close_suffix_width` 已删，子代理从 overlay 列表 / `has_overlay_popup` / `close_overlay_popup` 中摘除。

视图里**没有**恢复主日志那种可交互卡片（`[ Open ]` → diff/code 弹窗）：Rg 的判断是「不用和主 agent 一致」——块是紧凑形态，输出封顶 8 行、点自己的溢出行提到 50，信息全在这一列里，靠滚动读完。

## 10. 文件清单

| 文件 | 改动 |
|---|---|
| `crates/tact_protocol/src/subagent_transcript.rs` | 新增：`SubagentTranscriptEntry` / `SubagentStep` |
| `crates/tact_protocol/src/agent.rs` | `AgentUpdate::ToolTranscript` |
| `crates/tact_protocol/src/lib.rs` | 导出 |
| `crates/tact_extensions/src/tool/subagent_ui.rs` | 各分支多发一条 `ToolTranscript`；`assistant_buf` 合并 |
| `crates/agent_tui_kit/src/components/tool.rs` | 处理 `ToolTranscript`；重启时清空 |
| `crates/agent_tui_kit/src/state/tool_state.rs` | `ToolState::transcripts`；`SubagentPopup` 重塑 |
| `crates/agent_tui_kit/src/render/popups/subagent_popup.rs` | 重写：建段 + `LogColumnRenderer` |
| `crates/agent_tui_kit/src/render/cells/markdown.rs` | `MarkdownCell` 选区支持（如需要） |
| `crates/tui/src/render/popups/subagent_popup.rs` | prepare 阶段重建键换成 `revision` |
| `crates/tui/src/widgets/state/app/popups.rs` | 弹窗构造（去 `cached_markdown`/`layout_cache`）、复制 |
| `crates/tui/src/handlers/{overlay,mouse}.rs` | 滚动语义、hit row |

## 11. 测试计划

- `subagent_ui.rs`：每个事件产出对应的 `Entry`；连续 `StreamChunk` 合并成一条 `Assistant`；
  `StepStarted`+`StepFinished` 是同一 `tool_id` 的两条 `Step`。
- `components/tool.rs`：`ToolTranscript` 进 map；同 id 重启清空；`revision` 递增。
- `render/popups/subagent_popup.rs`（kit）：段序（meta → prompt → 各条）、总行数、live/completed
  两态**段数相同**、空白格携带 `theme.bg`。
- `tui`：`prepare` 只在 `revision`/宽度/主题变化时重建；滚动钳制；选中区间映射。
- 缓冲区级测试用 `render_app_text` 全 UI 顺序画一帧（照药丸那条测试的做法）。

## 12. 风险与开放问题

- **`MarkdownCell` 的选区支持** —— 已解决，且不需要动 `MarkdownCell`：助手文本按 Markdown 渲染成行后
  作为**普通文本段**（`DisplayRow` + 全局字节偏移）绘制，所以 Markdown 段可选中，且不必在渲染器里
  维护「渲染后文本 ↔ 源码字节」的映射（选区空间就是渲染后的文本，与改动前的口径一致）。
- **`ThinkingCell::completed` 需要 `ThinkingBlock`** —— 已解决：转发层在 `ThinkingChunk::Started` /
  `Finished` 之间用 `Instant` 实测耗时，随 `Thinking { text, duration_us }` 带过来。
- **性能**：一条长会话可能有上千条 entry。`revision` 键保证帧间不重建，但**每次追加都会重建整段布局**
  （O(n) 重排）。若实测卡顿，退化为「按 entry 增量建段 + 只重建最后一段」。
- **`AgentUpdate` 新变体的匹配点** —— 已确认 `crates/tui/src/widgets/state/app/agent.rs` 的
  `shell_handle` 是穷尽 match（无通配），已补 `ToolTranscript` 一支。
- **脱敏**：结构化通道走的是与扁平流不同的发送口，必须显式过
  `ToolProgressReporter::redact_text`，否则弹窗会显示扁平流已经抹掉的内容。有测试钉住。
- **容器本身是待定项**：块解决了「弹窗里不该有交互」，但弹窗是否该存在，见 §9 与
  `docs/design/subagent-presentation.html`。
