flowchart TD
    A["收到用户消息<br/>(user_turn_message)"] --> B{"auto_compact_due<br/>(按 incoming tokens)?"}
    B -->|"是"| C["compact_history<br/>压缩旧历史"]
    C --> D["push_message 追加用户消息"]
    B -->|"否"| D
    D --> E["build_system_prompt<br/>每个 task 构建一次"]
    E --> F{"进入主循环 loop"}

    F --> G{"cancel_flag 取消?"}
    G -->|"是"| Z1["返回 Ok (Cancelled by user)"]
    G -->|"否"| H["micro_compact 微压缩<br/>+ auto_compact_due(0) 检查<br/>(Responses 提供者跳过)"]

    H --> I["快照上下文<br/>构造 CreateMessageParams<br/>system / tools / streaming / thinking"]
    I --> J["stream_message 调用 LLM<br/>emit ModelInfo, 记录 prompt 统计"]

    J --> K{"LLM 调用成功?"}
    K -->|"否"| L{"错误分类"}
    L -->|"prompt too long<br/>且 compact_attempts 未超限"| M["compact_history 压缩<br/>并重试"]
    L -->|"瞬时网络错误<br/>且 transport_attempts 未超限"| N["指数退避 backoff 后重试"]
    L -->|"其他"| Z2["返回错误"]
    M --> F
    N --> F

    K -->|"是"| O["记录 token / 耗时统计<br/>持久化 assistant 消息<br/>(Responses: 事务更新 provider_state)"]
    O --> P{"stop_reason?"}

    P -->|"MaxTokens<br/>且可续写"| Q["执行截断前未完成的工具<br/>+ 追加 continuation 消息"]
    Q --> F

    P -->|"Refusal"| Z3["返回错误<br/>(模型拒绝请求)"]
    P -->|"Unknown"| Z4["当作结束返回 Ok"]
    P -->|"EndTurn / StopSequence<br/>PauseTurn / None"| Z5["正常结束返回 Ok"]

    P -->|"ToolUse"| R["execute_tool_call<br/>PreToolUse hook<br/>→ 权限审批 (permission)<br/>→ 并行执行工具<br/>→ PostToolUse hook<br/>→ 组装结果"]

    R --> S["把工具结果作为<br/>User 消息追加回上下文"]
    S --> T{"工具请求手动 compact?"}
    T -->|"是"| U["compact_history<br/>(带 focus)"]
    U --> F
    T -->|"否"| F
