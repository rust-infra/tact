# xAI (Grok) Provider Integration — Design

Date: 2026-07-12
Status: Approved (方案 1, clean-room; Steer 仅作为 API 形状参考，不复制其 AGPL 代码)

## Goals

- `provider = "xai"` 成为一等公民：默认 `base_url = https://api.x.ai/v1`，走 OpenAI 兼容的
  `/chat/completions`（SSE 流式）。
- 正确的 thinking 控制映射：请求中**绝不**发送 Anthropic 形状的
  `thinking: { type, budget_tokens }`；对支持推理档位的模型改发 `reasoning_effort`。
- 流式 `reasoning_content` 与 tool_calls 正常进入现有 agent 循环与 TUI。

## Non-goals

- 不接 xAI Responses API（stateful）与 gRPC / 非官方 crate。
- 不做 xAI 余额/配额查询（`is_account_query_supported` 不变，poller 收到
  `NotSupported` 后自动停止）。

## Engineering decision: 语义隔离、传输复用

xAI 的 HTTP/SSE 传输与现有 `OpenAiAdapter` 完全一致（Chat Completions + SSE +
`reasoning_content` delta，适配器已解析）。为避免复制 ~600 行 SSE 传输代码，
实现为：

- 新模块 `crates/tact_llm/src/xai.rs` 集中**全部 xAI 语义**：
  - `DEFAULT_BASE_URL = "https://api.x.ai/v1"`
  - `ProviderInfo::is_xai()`（provider 名 / base_url 含 `api.x.ai` / model 以 `grok-` 开头）
  - `reasoning_effort(model, thinking_requested) -> Option<&'static str>` 映射
- 传输复用 `OpenAiAdapter`；`build_client()` 的 `"xai"` 分支走
  `build_openai_compatible()` 并取 xai 默认 URL。
- `openai.rs::inject_thinking_param` 增加 xai 分支：调用
  `xai::reasoning_effort`，命中则注入 `body["reasoning_effort"]`，随后直接
  return（不落入 Anthropic 默认分支）。

## reasoning_effort 映射规则

依据 xAI REST 文档：`reasoning_effort` 目前仅 grok-4.3 系列接受
（`none|low|medium|high`）；always-on 推理模型（grok-4、grok-4.5）不接受该参数。

| 条件 | 注入 |
|------|------|
| 未配置 thinking | 无 |
| thinking 已配置且 model 含 `grok-4.3` / `grok-4-3` | `reasoning_effort: "high"` |
| thinking 已配置但模型不支持档位（如 grok-4.5） | 无（模型自身默认推理） |

后续若 xAI 扩展支持面，只需改 `xai::reasoning_effort` 一处。

## 配置

```toml
[llm]
provider = "xai"
model = "grok-4.5"
api_key = "xai-..."
# base_url 默认 https://api.x.ai/v1
```

同步更新：`config/resolve.rs::default_base_url`、`config/cli.rs` 帮助文本、
`build_client()` 未知 provider 错误信息、`tact.example.toml`、
`book/21_chapter_config.md`、`book/22_chapter_llm.md`。

## 测试

- `xai::reasoning_effort` 单元测试（4.3 命中 / 4.5 不命中 / 未开 thinking 不命中）。
- `inject_thinking_param`：xai + grok-4.5 → 无 `thinking` 无 `reasoning_effort`；
  xai + grok-4.3 → 有 `reasoning_effort`、无 `thinking`。
- `build_client("xai")` 默认 URL；`resolve` 层 xai 默认 base_url。
- `is_xai` 检测（provider 名 / URL / 模型名三路）。

## 风险

- Chat Completions 在 xAI 标为 legacy：首版可接受，Responses API 留作后续。
- 模型对 `reasoning_effort` 的支持面可能变化：规则集中在单函数，易调整。
