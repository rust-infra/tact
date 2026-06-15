# TUI 渲染文档

本文档描述 `crates/tui/src/render` 模块的渲染架构、模块划分、渲染流程及性能优化策略。

---

## 1. 架构总览

TUI 基于 [ratatui](https://docs.rs/ratatui) 绘制，采用**分层渲染**设计：

- `lib.rs` 中的主循环负责初始化终端、处理事件、调度渲染。
- `render/` 目录包含所有绘制逻辑，按功能拆分为多个子模块。
- 渲染以 `Frame` 为单位，每一帧将 `App` 状态转换为终端画面。

```
crates/tui/src/render/
├── mod.rs              # 模块导出
├── layout.rs           # 主区域布局
├── bar.rs              # 顶部/底部状态栏
├── input.rs            # 输入框与命令行
├── log.rs              # 日志面板
├── log_column.rs       # 日志列渲染器
├── plan.rs             # 执行计划面板
├── render_md.rs        # Markdown 渲染
├── renderable.rs       # Renderable trait
├── util.rs             # 文本换行工具
├── welcome.rs          # 启动 Logo 组件
├── cells/              # 卡片渲染单元
│   ├── text.rs
│   ├── thinking.rs
│   ├── diff.rs
│   └── code.rs
└── popups/             # 弹窗
    ├── command_palette.rs
    ├── select.rs
    ├── help.rs
    ├── history.rs
    ├── thinking_popup.rs
    ├── diff_popup.rs
    └── code_popup.rs
```

---

## 2. 主循环与渲染入口

主循环位于 `crates/tui/src/lib.rs` 的 `run_tui()` 中：

1. **处理 Agent 更新**：在渲染前先消费 `agent_rx` 中的消息，保证状态一致性。
2. **脏检查**：仅当 `app.dirty` 为 `true` 或状态为 `Status::Done` 时才重绘。
3. **计算布局**：根据终端大小、输入框行数、余额信息行数切分区域。
4. **按层次渲染**：状态栏 → 主区域 → 输入框 → 底部栏 → 弹窗。
5. **清理状态**：如 `Done` 高亮 2 秒后恢复 `Idle`，`flash_msg` 3 秒后清除。

```rust
terminal.draw(|f| {
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),          // 顶部状态栏
            Constraint::Min(3),              // 主区域
            Constraint::Length(input_height),// 输入框
            Constraint::Length(bottom_height),// 底部栏
        ])
        .split(size);

    render_status_bar(f, chunks[0], &app);
    render_main_area(f, chunks[1], &mut app);
    render_input_box(f, chunks[2], &mut app);
    render_bottom_bar(f, chunks[3], &app);

    if app.input_mode == InputMode::Palette { render_command_palette(f, size, &app); }
    if app.input_mode == InputMode::Select  { render_select_popup(f, size, &app); }
})?;
```

---

## 3. 布局模块 (`layout.rs`)

`render_main_area()` 负责主内容区：

| 显示状态 | 布局行为 |
|---|---|
| `show_history == true` | 全屏显示历史任务面板 |
| `show_help == true` | 全屏显示帮助面板 |
| `plan.visible == true` | 左侧 20% 计划面板，右侧 80% 日志面板 |
| 默认 | 100% 日志面板 |

同时根据布局结果更新 `app.mouse.plan_area` 和 `app.mouse.log_area`，用于后续鼠标命中测试。

---

## 4. 状态栏 (`bar.rs`)

### 顶部状态栏 (`render_status_bar`)

- 显示当前输入模式（`Normal` / `Insert` / `Search` / `Palette` / `Select`）
- 显示当前焦点面板（`[Log]` / `[Plan]`）
- 根据 `Status` 显示任务状态：
  - `Idle`：主题、语言、快捷键提示
  - `Planning`：正在规划中
  - `Executing`：执行到第 N / 总 M 步
  - `WaitingForUser`：等待用户审批
  - `Done`：任务完成（绿色高亮 2 秒）
- 特殊状态覆盖：
  - `party_mode`：派对模式全彩横幅
  - `flash_msg`：临时通知（3 秒）

### 底部栏 (`render_bottom_bar`)

- 焦点面板提示
- 工作目录、Git 分支
- 当前模型、最大 token、thinking budget
- Token 统计（prompt / completion / cache hit）
- 任务耗时与 TUI 运行时间
- DeepSeek 账户余额（可选第三行）

---

## 5. 输入区域 (`input.rs`)

### 命令行 (`render_command_line`)

用于 `Search`（前缀 `/`）和 `Palette`（无前缀）模式：

- 显示 `cmd_line` 内容
- 光标定位在文本末尾

### 主输入框 (`render_input_box`)

- 支持最多 3 行的多行输入
- 在 `Insert` 模式下渲染带圆角边框的输入框
- 在 `WaitingForUser` 状态下渲染审批横幅
- 光标按字符宽度计算（支持中文等宽字符）

---

## 6. 日志面板 (`log.rs`)

日志面板是最复杂的渲染组件，核心流程如下：

### 6.1 可见性索引 (`visible_indices`)

- 某些物理消息行可能被隐藏（如思考块的详细内容、代码块占位符）
- 维护 `visible_indices`：逻辑行 → 物理行
- 维护 `phys_to_logical_cache`：物理行 → 逻辑行

### 6.2 视觉缓存 (`visual_cache`)

- 每行按面板宽度自动换行
- 缓存 `visual_cache` 和 `visual_start_cache`
- 当 `messages.len()` 或宽度变化时重建缓存

### 6.3 视口裁剪

- 根据 `log_scroll.offset` 计算可见的逻辑行范围
- 只渲染落在当前视口内的 `TextCell`

### 6.4 卡片覆盖层

在日志面板之上叠加三种卡片：

| 卡片类型 | 文件 | 说明 |
|---|---|---|
| Thinking 卡片 | `cells/thinking.rs` | 折叠显示思考块，最多 3 行预览 |
| Diff 卡片 | `cells/diff.rs` | 文件写入预览，显示行号与 `+` 前缀 |
| Code 卡片 | `cells/code.rs` | 已完成代码块卡片，支持语法高亮 |

### 6.5 滚动条

- 基于**视觉行总数**计算滚动条位置
- 使用自定义符号：`▲` / `▼` / `│` / `█`

---

## 7. 渲染单元 (`cells/`)

### `Renderable` trait (`renderable.rs`)

所有可渲染单元实现该 trait：

```rust
pub(crate) trait Renderable {
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize);
    fn height(&self, width: u16) -> u16;
}
```

### `TextCell` (`cells/text.rs`)

日志基本渲染单元，支持：

- 预换行缓存
- 搜索高亮（黄色背景）
- 鼠标选中（反色）
- 词级双击选择
- 思考块折叠指示前缀

### 卡片单元

- `thinking.rs`：紫色调边框，显示最近最多 3 行思考内容
- `diff.rs`：绿色 `+` 前缀，显示文件路径与行号
- `code.rs`：深蓝灰背景，显示语言标签与代码预览

---

## 8. Markdown 渲染 (`render_md.rs`)

使用 `tui-markdown` 将 Markdown 转换为 `Line` 列表：

- 自定义 `TuiStyleSheet`：标题、代码、链接、引用样式
- 代码块后处理：统一深蓝灰背景
- 表格格式化：对齐列、加粗表头
- 水平线检测

> 注意：不处理超链接 OSC 8 序列，因为 ratatui 会剥离转义序列。

---

## 9. 弹窗 (`popups/`)

| 弹窗 | 文件 | 说明 |
|---|---|---|
| 命令面板 | `command_palette.rs` | `:` 触发，模糊过滤命令 |
| 选择弹窗 | `select.rs` | 代理请求用户选择 |
| 帮助面板 | `help.rs` | `Ctrl+?` 触发，快捷键说明 |
| 历史面板 | `history.rs` | `Ctrl+H` 触发，可重试历史任务 |
| 思考详情 | `thinking_popup.rs` | 查看完整思考内容 |
| 文件详情 | `diff_popup.rs` | 查看写入文件的完整内容 |
| 代码详情 | `code_popup.rs` | 查看完整代码块 |

弹窗通常：

- 占据屏幕 80% × 80%
- 先渲染 `Clear` 清除背景
- 显示 `[y] Copy`、`[Esc] Close`、`[j/k] Scroll` 提示
- 记录区域到 `app.mouse.*_popup_area` 用于点击外部关闭

---

## 10. 性能优化

### 10.1 脏渲染 (Dirty Rendering)

- 仅在 `app.dirty == true` 或 `Status::Done` 时调用 `terminal.draw()`
- 空闲时以 1 秒间隔 poll，降低 CPU 占用

### 10.2 缓存策略

| 缓存 | 位置 | 失效条件 |
|---|---|---|
| `visible_indices` | `log_scroll` | `messages.len()` 变化 |
| `visual_cache` | `log_scroll` | `messages.len()` 或宽度变化 |
| `phys_to_logical_cache` | `log_scroll` | `messages.len()` 变化 |
| 代码块 `styled` | `CodeBlock` | 块创建时 |
| Diff 预览行 | `DiffBlock` | 块创建时 |

### 10.3 视口裁剪

- `LogColumnRenderer` 只渲染落在当前视口内的单元
- 每个 `TextCell` 支持 `render_partial` 跳过不可见行

### 10.4 自适应事件轮询

| 状态 | 轮询间隔 | 原因 |
|---|---|---|
| `Done` 或 `flash_msg` | 200ms | 及时清理超时状态 |
| `dirty == true` | 10ms | 快速触发重绘 |
| 空闲 | 1000ms | 降低 CPU 占用 |

---

## 11. 主题与国际化

### 主题 (`theme.rs`)

- 内置 9 套主题：`Dark`、`Light`、`SolarizedDark`、`SolarizedLight`、`GruvboxDark`、`Nord`、`Retro`、`Kawaii`、`Japanese`
- 每个主题定义背景、前景、强调色、警告色、边框等
- 通过 `Ctrl+T` 循环切换

### 国际化 (`i18n.rs`)

- 支持 `English` 和 `Chinese`
- 所有 UI 字符串集中定义在 `Messages` 结构体
- 命名约定：
  - `_tmpl`：含 `{}` 占位符的模板
  - `_pl`：复数形式
- 通过 `Ctrl+L` 循环切换

---

## 12. 相关状态机

渲染状态受以下状态机驱动，详见 `docs/state_machines.md`：

- `Status`：Idle / Planning / Executing / WaitingForUser / Done
- `InputMode`：Normal / Insert / Search / Palette / Select
- `SelectPopup`：Inactive / Active / Confirmed / Cancelled
- `StreamState` / `ThinkingState`：流式输出解析

---

## 13. 调试与扩展

### 添加新面板

1. 在 `render/` 下新建渲染模块
2. 在 `layout.rs` 中根据状态分配区域
3. 在 `state/mod.rs` 的 `App` 中添加所需状态
4. 如有需要，在 `mouse_state.rs` 中记录区域用于鼠标命中

### 添加新弹窗

1. 在 `render/popups/` 下新建模块
2. 在 `popups/mod.rs` 中导出
3. 在 `layout.rs` 或主循环中调用
4. 在 `App` 中添加弹窗状态与打开/关闭/滚动方法

### 性能分析

- 关注 `log.rs` 中缓存命中率
- 使用 `app.dirty` 控制重绘频率
- 避免在 `render()` 中执行文件 I/O（如 `diff_popup.rs` 已使用 `cached_content` 懒加载）
