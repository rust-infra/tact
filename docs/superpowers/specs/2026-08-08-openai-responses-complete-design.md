# OpenAI Responses 完整适配设计

## 目标

在 Tact 现有 Responses adapter 基础上，尽可能完整覆盖当前 OpenAI Responses API，同时为未来新增的 SSE 事件、output item、tool 字段和响应元数据提供前向兼容能力。

目标不是把任意 OpenAI-compatible 服务都宣称为完整兼容，而是让 OpenAI 官方 Responses endpoint 的核心能力、Hosted Tools、状态持久化和压缩链路具备清晰的能力边界与可验证行为。

## 设计选择

采用“强类型核心 + 原始 JSON 扩展层”：

- 已知且参与 Tact 控制流的 Responses 对象使用 `async-openai-responses` 类型和 Rust 枚举。
- 未知但属于协议状态的 item 使用原始 JSON 保留，并在下一次请求中原样透传。
- 未知且不影响状态的 SSE 事件记录诊断信息后忽略，不阻断普通响应。
- Hosted Tools 转换为 Tact 内部的 hosted-tool 生命周期表示，不伪装成本地 function tool。
- 依赖 crate 尚未覆盖的已知字段通过 JSON request patch 补齐，而不为每个字段创建重复的本地类型。

该方案避免完全绑定第三方 crate 的版本更新，也避免由 Tact 自己重写整个 Responses 类型系统。

## 当前代码边界

主要实现位于：

- `crates/tact_llm/src/openai/responses/mod.rs`：Responses client、普通请求、流式请求和 native compact。
- `crates/tact_llm/src/openai/responses/convert.rs`：Tact 消息到 Responses input 的转换。
- `crates/tact_llm/src/openai/responses/normalize.rs`：terminal response 到 Tact 内容和 provider state 的转换。
- `crates/tact_llm/src/openai/responses/stream.rs`：SSE 事件状态机。
- `crates/tact_llm/src/openai/responses/history.rs`：reasoning 历史和 function-call item id 状态。
- `crates/tact_llm/src/provider_state.rs`：Responses provider state 的版本化持久化模型。
- `crates/tact/src/agent/mod.rs`：Responses state 恢复、自动压缩和 native compact 路由。
- `crates/tact/src/store/session_store/sqlite.rs`：Responses state 和调用记录持久化。
- `crates/tact/src/config/resolve.rs`、`crates/tact_llm/src/provider.rs`：provider/protocol 能力入口。

## 功能范围

### 1. 请求能力

Responses request builder 需要覆盖 Tact 运行时拥有或明确暴露的 Responses-specific 请求字段：

- `model`；
- `input`，包括 message、image、reasoning、function call 和 function call output；
- `instructions`；
- `tools`、`tool_choice`、`parallel_tool_calls`；
- `reasoning`、`reasoning.effort`、`reasoning.summary` 和 encrypted reasoning include；
- `text` response format 相关字段；
- `temperature`、`top_p`、`max_output_tokens`；
- `truncation`；
- `metadata`、`user`、`prompt_cache_key`；
- `store` 与状态复用策略；
- `context_management` 和 native compaction threshold。

当前 `CreateMessageParams` 主要是 Anthropic/Chat Completions 形状，因此不能假设它已经能表达这些字段。实现需要增加一个明确的 `ResponsesRequestOptions` 请求扩展（或等价的 typed/raw boundary），由 Responses adapter 消费；非 Responses adapter 必须忽略或拒绝该扩展，不能把 Responses 字段误映射到 Chat Completions。无法表达的字段必须在配置或请求边界给出明确错误。

### 2. 输入和输出转换

已知核心对象使用强类型转换：

- 文本 message 和多段 message content；
- 图片输入；
- reasoning summary、encrypted reasoning content 和历史关联信息；
- refusal；
- function call 与 function call output；
- output message、annotations 和可见文本；
- response status、incomplete details、usage 和 stop reason。

协议状态与用户可见内容必须分离。由于 typed `Response` 在遇到未知 output item 时可能在反序列化阶段直接失败，raw envelope 解析必须先于 typed normalization：先按 `type` 解析事件/响应外壳，再将已知 item 交给强类型转换，未知 item 保留为原始 JSON。terminal output 的顺序和 state update 必须基于这组 raw item 保持。

- compaction item 不得变成普通 `ContentBlock`；
- hosted tool call 不得变成本地 `ToolUse`；
- 未知 item 必须保留 raw JSON；
- 无法安全转换的已知 item 不得静默丢弃。

### 3. 流式事件

流式状态机覆盖核心生命周期：

- response created / in-progress / completed / incomplete / failed；
- output item added / done；
- output text delta / done；
- reasoning summary delta / done；
- reasoning text delta / done；
- refusal delta；
- function call arguments delta / done；
- hosted tool added / done；
- error。

处理规则：

- 已知且影响输出或状态的事件必须严格校验顺序和完整性。
- 未知且不影响状态的事件记录诊断信息后继续。
- 未知 output item 必须进入 terminal output/state 的 raw item 序列。
- 已声明但未完成的 compaction item 必须失败，不能使用可见文本恢复绕过它。
- 重复的 added/done 事件应幂等处理，不能重复追加 assistant tool item。
- 流结束时没有 terminal event 必须返回协议错误。

### 4. Hosted Tools

Hosted Tools 与本地 function tools 使用不同的数据流：

- 本地 function tool：Tact 调度并执行，使用现有 `ToolUse` / `ToolResult` 流程。
- Hosted Tool：OpenAI 执行，Tact 只记录、展示和保存其生命周期，不进入本地 tool dispatcher。

分阶段支持，并为每个官方 tool family 建立 capability matrix：

1. `web_search`：请求声明、output item 生命周期、查询、来源和失败信息。
2. `file_search`：请求声明、搜索结果、引用和详情展示。
3. `code_interpreter`、`image_generation`、`local_shell`、shell、`custom`、namespace、apply_patch、tool search 和 remote MCP 相关 item：先完成 raw/typed 协议解析和 capability gate，再分别决定是否能映射到 Tact 的执行与展示模型；web search 的 preview/versioned tool names 也必须纳入同一矩阵。
4. `computer` / `computer_use_preview`：先完成协议解析和明确的 capability gate；只有建立权限确认、截图/坐标输入和动作安全边界后，才允许执行。

每个 tool family 必须明确标注为“已支持执行”“仅 provider 执行并展示”“仅解析并拒绝执行”或“未实现且显式报错”，不能以一个笼统的 `hosted_tools` 布尔值代替。

Hosted Tool 的未知字段和原始 action/result 必须保存在 raw JSON 中。TUI 应复用统一 ToolWidget/Step 生命周期，而不是为每种 hosted tool 建立独立渲染分支。

### 5. Provider state 和 session 恢复

Responses state 继续使用版本化模型：

- 保存 provider、base URL、model、input baseline、compaction id；
- 保存 logical message count 和 context hash；
- 保存未知 input/output item 的 raw JSON；
- state schema 变更必须增加版本和迁移路径；
- provider/base URL 不匹配时拒绝复用；
- model 变化允许继续尝试，但 provider 可以在请求时拒绝不兼容的 opaque state；
- state 和逻辑消息必须原子持久化，不能只更新其中一侧。

未知 item 必须满足 state round-trip：读取旧 session 后再次生成请求时，未知 item 的协议语义不能丢失或被转换成文本。

### 6. Compaction 和恢复

- OpenAI Responses 使用 `/responses/compact`，不退回本地 summary。
- 普通 Responses 请求可发送 `context_management` compact threshold。
- native compact 的输入必须是当前 protocol baseline 加上尚未覆盖的 logical message suffix。
- compact 返回的 resource 必须包含且只包含一个有效 compaction item。
- compact 成功后先持久化 usage、state 和逻辑上下文，再提交 runtime counters。
- 传输错误有限重试；协议错误不重试。
- provider 不支持 compact 时必须通过 capability/configuration 明确失败，而不是在运行中表现为普通 summary compact。

### 7. Provider capability

Responses 能力不再只由 `protocol = "responses"` 一个布尔值隐含表达。需要逐步显式表示至少这些 capability：

- `responses`；
- `responses_streaming`；
- `responses_compact`；
- `responses_hosted_web_search`；
- `responses_hosted_file_search`；
- `responses_computer`。

OpenAI 官方 provider 默认开启其已实现能力。自定义 OpenAI-compatible provider 必须通过配置或能力探测声明支持范围。DeepSeek/Kimi 只有在 endpoint 真实通过对应 wire/fixture/live 验证后才能解除当前配置阻断。

## 错误处理和安全边界

- 解析错误必须包含 event/item 类型和必要上下文，但不能打印 API key、encrypted content 或完整敏感输入。
- 未知事件不能被当作成功状态。
- 未知 item 可以保留，但如果无法判断它是否影响 state baseline 完整性，必须停止提交新 state。
- Hosted Tool 的 provider 执行结果不能触发本地命令执行。
- `computer` 相关动作在没有明确权限和确认协议前只能报告“不支持执行”，不能猜测执行。
- 所有 async channel 等待必须有超时；测试不得使用无界 `recv().await`。

## 测试策略

### 单元测试

覆盖：

- request builder 的每个关键字段和 JSON patch；
- message/image/reasoning/tool 转换；
- stop reason 和 incomplete reason；
- 未知 item raw JSON 保留；
- 未知 event 忽略或拒绝规则；
- 重复/缺失 output item 事件；
- compaction item 校验；
- provider state hash、版本和 round-trip。

### WireMock 集成测试

覆盖：

- 普通 `/responses` streaming；
- function call 多轮往返；
- hosted tool lifecycle；
- `/responses/compact` 成功、malformed resource、传输重试和持久化失败；
- session restore 后的 baseline 复用；
- model change 后 state 绑定行为。

测试执行时必须清除代理环境，避免本地 WireMock 请求被代理：

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm responses
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact agent::tests::responses
```

### Fixture/live 验证

- 使用官方 Responses SSE/JSON fixture 作为稳定回归集。
- 对 OpenAI 官方 endpoint 增加显式 `#[ignore]` live tests，覆盖 text、reasoning、function tool、web search、compact。
- live tests 不作为普通 CI 的唯一正确性依据。

## 文档同步

行为变化必须同步：

- `config.example.toml`；
- `docs/compaction.md`；
- `book/05_chapter_compact.md`；
- `book/05_chapter_compact_zh.md`；
- `book/26_chapter_issue.md`；
- `book/26_chapter_issue_zh.md`；
- Responses/API 相关章节和配置说明。

双语章节需保持相同章节结构、表格和流程图语义。

## 分阶段交付

### 阶段 1：协议基础和 raw 扩展层

建立 typed core/raw extension 边界，补齐 request builder、未知事件/item 保存和 state round-trip。完成后不新增 hosted tool，但现有 text/reasoning/function/compact 行为不能回归。

### 阶段 2：核心 Responses 完整化

补齐输入/输出 content 类型、reasoning、refusal、annotations、usage、incomplete/failed/cancelled、function call streaming 和错误恢复。

### 阶段 3：Hosted Tools

先实现 web search，再实现 file search；统一 Step/ToolWidget 生命周期。computer tool 只在权限和执行模型设计完成后开放。

### 阶段 4：能力声明和 provider 解锁

增加 capability 表达，修正 OpenAI/custom provider 路由，并针对 DeepSeek/Kimi 分别进行 endpoint 验证；未验证通过的 provider 保持配置阻断。

### 阶段 5：文档、live 验证和发布验收

完成 fixture/live matrix、双语文档、issue log、migration 说明和完整串行验证。

## 非目标

- 不把所有 provider 的专有协议统一成一个无损的万能抽象。
- 不在没有权限模型的情况下执行 computer action。
- 不为每个未知 OpenAI 字段创建本地强类型。
- 不用本地 summary 静默替代官方 Responses native compact。
- 不承诺所有第三方 OpenAI-compatible endpoint 支持官方全部 Hosted Tools。

## 完成标准

只有同时满足以下条件，才称为“核心 Responses 完整”：

1. OpenAI 官方核心 request/response/stream fixture 全部通过。
2. 未知事件不破坏普通响应，未知 item 可 state round-trip。
3. function tool 多轮、reasoning、图片、refusal、usage 和失败恢复有测试。
4. native compact、session restore 和 SQLite 原子持久化有测试。
5. 每个官方 Responses tool family 都有 capability matrix；已支持的 family 有请求、流式、失败和 TUI 测试，未支持的 family 有明确 gate/错误测试。
6. provider capability 与配置错误信息准确，不支持能力不会静默降级。
7. 双语文档和配置示例与实际行为一致。
8. `cargo fmt --check`、目标 crate 测试和 workspace clippy 串行通过。
