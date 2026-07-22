# Session Restore — TUI Log 区域渲染设计

## 问题

`tact sessions restore <id>` / `tact --resume-last` 恢复指定会话后，Agent 内部上下文（`self.runtime.context`）正确加载了历史消息，可以继续与 LLM 对话。但 TUI 的 Log 区域为空，用户看不到历史对话记录。

## 根因

`Agent::ensure_session()`（`crates/tact/src/agent/mod.rs`）只将 `store.load_session()` 返回的消息赋值给 `self.runtime.context`，没有向 `ui_tx` 通道发射任何 `AgentUpdate` 事件。TUI 的 `app.messages` / `app.raw_messages` / `app.raw_message_types` 完全依赖 `agent_rx` 通道上的 `AgentUpdate` 来填充，因此始终为空。

## 方案

### 架构选择：TUI 直接在初始化时从 session_store 加载历史

**不走 Agent 的事件通道。** Agent 的 `ui_tx` 通道设计用于实时流式事件（`StreamChunk`、`Thinking`、`StepAdded`、`ToolProgress` 等），不适合批量历史回放。回放历史还可能误触实时事件处理臂中的副作用。

TUI 已有 `session_store` 和 `session_id` 字段：

```
crates/tui/src/widgets/state/mod.rs:
  session_store: Option<tact::store::DynSessionStore>
  session_id: String
```

在 `run_tui()` 中，`App::new()` 之后、进入渲染循环之前，直接从 store 读取历史并填充 `app.messages` 等展示结构。后续 `agent_rx` 收到的实时事件追加在末尾，不会与历史重叠。

### 新增方法：`App::load_history(messages: Vec<Message>)`

定义在 `crates/tui/src/widgets/state/app/messages.rs`（或附近的 `construct.rs`），功能是将 `Vec<Message>` 转换为 Log 显示行，同步填充 `messages` / `raw_messages` / `raw_message_types` 三个向量。

### 新增调用点：`run_tui()` 中 `App::new()` 之后

```rust
// crates/tui/src/lib.rs, run_tui()
let app = App::new(...);

// 从 session_store 加载历史并渲染到 Log 区域
let history = app.session_store
    .as_ref()
    .map(|store| store.load_session(&app.session_id))
    .transpose()
    .await
    .unwrap_or_default();
if let Some(messages) = history {
    if !messages.is_empty() {
        app.add_new_line();  // 与实时会话的开头空行对齐
        app.load_history(messages);
    }
}

// 然后进入渲染循环
render_loop(&mut app, ...).await?;
```

### 历史消息 → Log 显示行的转换规则

输入：`Vec<Message>`，每条消息包含 `role` + `MessageContent::Blocks { content: Vec<ContentBlock> }`。

| Role | ContentBlock | 恢复动作 | 说明 |
|---|---|---|---|
| `User` | `Text { text }` | 拼合所有 Text 块文本 → `app.add_user_message(text)` | 复用已有样式（slash 高亮、前缀 `>>>` / 续行 `..`） |
| `User` | `Image { source }` | 占位行 `[图片: {media_type}]` → `append_msg()` | 非 Text 块按序插入占位 |
| `User` | 非 Image/Text 块 | 跳过 | User 侧不会出现 |
| `Assistant` | `Text { text }` | `render_markdown_tui(text)` → `app.extend_msgs(...)` | 与流式最终产物一致的 markdown 渲染 |
| `Assistant` | `ToolUse { name }` | 折叠为一行 `🔧 使用工具: {name}` → `append_msg()` | 摘要行，`RawMessageType::LLM` |
| `Assistant` | `ToolResult { ... }` | 折叠为一行 `📎 工具返回` → `append_msg()` | 摘要行，用户不需要看完整返回 |
| `Assistant` | `Thinking { ... }` | 跳过 | 推理过程不保留在历史视图中 |
| `Assistant` | `RedactedThinking { ... }` | 跳过 | 同上 |
| `Assistant` | `Image { source }` | 占位行 `[图片: {media_type}]` → `append_msg()` | 只标记存在，不显示内容 |

#### 同一条 Assistant 消息包含多个块的渲染顺序

按 `ContentBlock[]` 的顺序逐个处理：
1. `Thinking` / `RedactedThinking` → 跳过
2. `ToolUse` / `ToolResult` → 按序插入折叠摘要行
3. `Text` → 渲染为 markdown

最终在 Log 中表现为：

```
>>> 用户的提问
... 用户的续行
Assistant 的 markdown 回答（文字部分）   ← render_markdown_tui
🔧 使用工具: bash                        ← ToolUse 折叠
📎 工具返回                              ← ToolResult 折叠
Assistant 继续回答（工具结果后的文字）     ← 另一个 Text block
```

### 边界情况

- **空历史：** `load_history(Vec::new())` 是空操作，不修改 `app.messages`。
- **只包含 Thinking 的消息：** 跳过所有块，不会增加 Log 行。
- **纯 ToolUse/ToolResult 的消息：** 只显示折叠摘要行，不显示完整 JSON 输入。
- **多 User 消息连续（中间无 Assistant）：** 每个 `add_user_message()` 自动插入空行作为分隔。
- **历史为空时不影响启动：** `if !messages.is_empty()` 守卫确保空历史不走渲染路径。

### 不修改的路径（零改动）

| 模块 | 原因 |
|---|---|
| `Agent::ensure_session()`（`agent/mod.rs`） | 继续填充 `self.runtime.context`，供 LLM 使用，无需变更 |
| stream chunk 渲染（`agent.rs` 的 `apply_stream_chunk`） | 实时事件处理逻辑不变，追加在新消息末尾 |
| Session store 写入路径（`sqlite.rs:append_message`） | 写入逻辑不变 |
| `AgentUpdate` 枚举 | 不新增变体，实时事件通道不变 |

### 副作用排查

| 潜在问题 | 结论 | 依据 |
|---|---|---|
| TUI 读 store 后写回 store？ | 不会 | `load_history()` 只操作内存向量，没有 store 句柄 |
| Agent 后续 `push_message()` 重复历史？ | 不会 | `push_message()` 通过 `ordinal = context.len()` 写新消息 |
| agent_rx 与 restore 历史重叠？ | 不会 | 时序上 `load_history()` 在渲染循环之前完成，后续 agent_rx 纯新消息 |
| `raw_messages`/`raw_message_types` 与 `messages` 对齐？ | 对齐 | `load_history()` 同步填充三个向量，使用现有的 `append_msg`/`extend_msgs` 方法 |

## 实现计划概要

1. 在 `crates/tui/src/widgets/state/app/messages.rs`（或 `construct.rs`）实现 `App::load_history()`
2. 在 `crates/tui/src/lib.rs` 的 `run_tui()` 中 `App::new()` 之后添加加载逻辑
3. 测试：单元测试验证每种 ContentBlock 的渲染结果
4. 集成测试：启动 TUI 并 restore 会话，确认 Log 区域显示历史
