# `batch_read` Tool Execution & TUI Interaction Flowcharts

This Mermaid diagram document describes the complete data flow from the Agent main loop, through LLM streaming responses, to the actual execution of `batch_read`, and finally to the `AgentUpdate` messages sent to the TUI where they are consumed and rendered.

> Files involved: `crates/tact/src/tool/batch_read.rs`, `crates/tact/src/agent/mod.rs`, `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact_llm/src/`, `crates/tui/src/lib.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/tool_widget.rs`, `crates/tui/src/render/cells/tool.rs`  
> Tool UI design: [`tool_rendering.md`](./tool_rendering.md)

---

## 1. Overall Interaction Sequence

```mermaid
sequenceDiagram
    participant TUI as TUI Main Thread
    participant Agent as Agent Runtime<br/><Tact>
    participant LLM as LLM Provider<br/>(Anthropic/OpenAI/DeepSeek/Kimi)
    participant Toolset as ToolSet
    participant BatchRead as batch_read impl
    participant FS as File System

    TUI->>Agent: Create and start Agent (with_ui_channel)
    TUI->>TUI: Hold agent_rx to receive AgentUpdate

    Agent->>LLM: stream_message(request, ui_tx)

    loop LLM streaming response
        LLM->>Agent: StreamChunk / ThinkingChunk
        Agent->>TUI: AgentUpdate::StreamChunk(text)
        Agent->>TUI: AgentUpdate::ThinkingChunk(Started|Delta|Finished)
    end

    LLM->>Agent: ToolUse {name: "batch_read", input}
    Agent->>TUI: AgentUpdate::StepAdded(...)
    Agent->>TUI: AgentUpdate::StepStarted(step_idx, tool_id, tool_name, arg_summary)

    Agent->>Toolset: tools.call(ctx, "batch_read", input)
    Toolset->>BatchRead: Match and execute batch_read

    loop For each file entry
        BatchRead->>FS: Read file content
        alt success
            BatchRead->>BatchRead: Record result
        else failure
            BatchRead->>BatchRead: Record error
        end
    end

    BatchRead->>Toolset: Return JSON result
    Toolset->>Agent: ToolResult {content}

    Agent->>TUI: AgentUpdate::Info("Executing batch_read(...)")
    Agent->>TUI: AgentUpdate::StepFinished(step_idx, StepResult)

    Agent->>LLM: Append result to context, continue request

    Note over TUI: TUI asynchronously receives all AgentUpdate
    TUI->>TUI: handle_agent_update(msg) updates state
    TUI->>TUI: terminal.draw(...) renders log/cards/popups
```

---

## 2. Agent Main Loop Internal Flow

```mermaid
flowchart TD
    A[agent_loop start] --> B[Build system prompt]
    B --> C[Assemble CreateMessageParams]
    C --> D[emit_update AgentUpdate::ModelInfo]
    D --> E[stream_message request]

    E --> F{stop_reason?}
    F -->|stop / max_tokens| G[Exit loop]
    F -->|tool_use| H[execute_tool_call]

    H --> I[Iterate ContentBlock]
    I --> J{block type}
    J -->|ToolUse| K[StepAdded + StepStarted]
    J -->|Text/Thinking| L[Skip]

    K --> M[Permission check]
    M -->|Allow| N[execute tool]
    M -->|Deny| O[StepFailed]
    M -->|Ask| P[RequestSelect popup]
    P -->|User approves| N
    P -->|User denies| O

    N --> S[tools.call]
    S --> T[Return ToolResult]

    T --> U[emit_update StepFinished]
    U --> V[Generate ToolResult content block]
    V --> W[Append to context]
    W --> E
```

---

## 3. `batch_read` Batch Read Flow

```mermaid
flowchart TD
    A[Input file list with path/offset/limit] --> B[Iterate each file entry]
    B --> C[Read file content]
    C --> D{Success?}
    D -->|Yes| E[Slice by offset/limit]
    D -->|No| F[Record error]
    E --> G[Aggregate result array]
    F --> G
    G --> H[Return JSON result]
```

### Key Design

- Supports `offset`/`limit` parameters to avoid reading huge files at once.
- Each file returns its own `content` or `error` independently.
- A failed file does not affect the reading of other files.

---

## 4. TUI Consumption of AgentUpdate

```mermaid
flowchart TD
    A[TUI main loop] --> B{agent_rx.try_recv}
    B -->|No message| C[Continue waiting]
    B -->|Has message| D[handle_agent_update]

    D --> E{AgentUpdate type}

    E -->|ModelInfo| F[Update app.model_call_params]
    E -->|StreamChunk| G[Append text message or append to current reasoning/output]
    E -->|ThinkingChunk lifecycle| H[Append / close thinking block]
    E -->|StepAdded| I[Add to plan.steps]
    E -->|StepStarted| J[Mark current executing step]
    E -->|StepFinished| K[Update step result status]
    E -->|StepFailed| L[Mark step failed]
    E -->|RequestSelect| O[Open select popup wait for user]
    E -->|TokenUsage| P[Update token statistics]

    F --> Q[Set app.dirty = true]
    G --> Q
    H --> Q
    I --> Q
    J --> Q
    K --> Q
    L --> Q
    O --> Q
    P --> Q

    Q --> R[terminal.draw render]
```

---

## 5. Key Code Mapping

| Flow Node | Code Location |
|---|---|
| Agent main loop | `crates/tact/src/agent/mod.rs` `Agent::agent_loop()` |
| Tool call dispatch | `crates/tact/src/agent/tool_dispatch.rs` `Agent::execute_tool_call()` |
| Tool registration & routing | `crates/tact/src/tool/registry.rs` |
| `batch_read` implementation | `crates/tact/src/tool/batch_read.rs` |
| Streaming response Anthropic | `crates/tact_llm/src/anthropic.rs` `stream_message()` |
| Streaming response OpenAI | `crates/tact_llm/src/openai.rs` `stream_message()` |
| AgentUpdate definition | `crates/protocol/src/agent.rs` |
| TUI handle AgentUpdate | `crates/tui/src/widgets/state/app/agent.rs` `handle_agent_update()` |

---

## 6. Extension Guide

### Add a new batch tool

1. Create a new module under `crates/tact/src/tool/` (refer to `batch_read.rs`).
2. Register it in `toolset()` / `ToolRouter::route`.
3. If you need a special card, define a new `AgentUpdate` variant and handle it in `handle_agent_update`.
4. Add the corresponding renderer in `crates/tui/src/render/cells/`.
