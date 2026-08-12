# Responses 流事件接入设计

- 日期：2026-08-09
- 类型：小范围兼容性增强
- 范围：事件接入与状态维护，不改变 Agent 工具执行语义

## 目标

让 Responses API 中已被 `async-openai` 识别、但 Tact 当前未消费的常规流事件能够安全通过解析层和状态机，不因事件白名单过滤而中断或静默丢弃整个流。

## 非目标

- 不接通 function call、MCP call、file search、code interpreter 或 image generation 的 Agent 执行。
- 不为 hosted web search 增加专用 SSE 生命周期事件；继续使用 `response.output_item.added/done`。
- 不用增量事件重建最终响应；终态 `response.output` 仍是权威数据源。
- 不新增 TUI 事件类型或渲染抽象。

## 设计

### 解析白名单

在 `crates/tact_llm/src/openai/responses/mod.rs` 中允许以下合法事件进入 SDK 反序列化：

- 响应生命周期：`response.created`、`response.queued`、`response.in_progress`
- 输出生命周期：`response.content_part.added/done`、`response.output_text.done`、`response.refusal.done`
- reasoning 完成事件：`response.reasoning_summary_part.added/done`、`response.reasoning_summary_text.done`、`response.reasoning_text.done`
- function-call 参数事件：`response.function_call_arguments.delta/done`

现有文本 delta、输出 item、终态和错误事件保持不变。

Hosted tool 的专用事件继续不加入白名单，因为当前协议设计要求由 `output_item.added/done` 覆盖其完整生命周期。

### 状态机行为

在 `crates/tact_llm/src/openai/responses/stream.rs` 中：

- 响应/内容/reasoning 的生命周期和 `*.done` 事件只返回空 `AgentUpdate`，避免重复渲染；文本与拒答仍只由 delta 驱动可见输出。
- function-call arguments 的 delta/done 事件只安全忽略，不执行工具、不创建新的 AgentUpdate，也不覆盖终态响应。
- 终态 `completed/incomplete/failed` 仍调用现有 `set_terminal`，并以完整响应 output 归一化。
- 使用显式 match 分支记录“已知但当前无行为”的事件，避免 `_` 掩盖未来 SDK enum 变化。

## 测试

新增单元测试覆盖：

1. 每个新增白名单事件均可解析。
2. 生命周期、done 和 function-call 参数事件通过 `ResponsesStreamState::apply` 后不产生更新。
3. output text delta 后接 output text done 不重复产生文本。
4. 现有终态响应、web search 和 compaction 行为不变。

## 验证

运行：

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses
```

再运行格式检查：

```bash
cargo fmt --all -- --check
```
