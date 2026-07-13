# `batch_read` / `batch_edit` Tool Execution & TUI Interaction Flowcharts

This Mermaid diagram document describes the complete data flow from the Agent main loop, through LLM streaming responses, to the actual execution of `batch_read` / `batch_edit`, and finally to the `AgentUpdate` messages sent to the TUI where they are consumed and rendered.

> Files involved: `crates/tact/src/tool/batch_read.rs`, `crates/tact/src/tool/batch_edit.rs`, `crates/tact/src/agent/mod.rs`, `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact_llm/src/`, `crates/tui/src/lib.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/tool_widget.rs`, `crates/tui/src/render/cells/tool.rs`  
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
    participant BatchEdit as batch_edit impl
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

    LLM->>Agent: ToolUse {name: "batch_edit", input}
    Agent->>TUI: AgentUpdate::StepAdded / StepStarted
    Agent->>Toolset: tools.call(ctx, "batch_edit", input)
    Toolset->>BatchEdit: Match and execute batch_edit

    BatchEdit->>BatchEdit: Atomically validate all edits

    alt Any edit validation fails
        BatchEdit->>Toolset: Return error, no files modified
    else All pass
        loop For each edit
            BatchEdit->>FS: Read original file
            BatchEdit->>BatchEdit: Replace old_string with new_string
            BatchEdit->>FS: Write new content
        end
        BatchEdit->>Toolset: Return summary of all changes
    end

    Toolset->>Agent: ToolResult
    Agent->>TUI: AgentUpdate::Info("Executing batch_edit(...)")
    Agent->>TUI: AgentUpdate::StepFinished(step_idx, StepResult)

    Agent->>LLM: Append edit result to context
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

    N --> Q{batch_edit?}
    Q -->|Yes| R[tools.call batch_edit]
    Q -->|No| S[tools.call other tool]

    R --> T[Return ToolResult]
    S --> T

    T --> U[emit_update StepFinished]
    U --> V[Generate ToolResult content block]
    V --> W[Append to context]
    W --> E
```

---

## 3. `batch_edit` Atomic Execution Flow

```mermaid
flowchart TD
    A[Input file list with old_string/new_string] --> B[Iterate validate each edit]
    B --> C{Is old_string unique and present?}
    C -->|Any failure| D[Return error: no files modified]
    C -->|All pass| E[Execute all edits]

    E --> F[Read original file]
    F --> G[Exactly replace old_string with new_string]
    G --> H[Write back to file system]
    H --> I[Generate change summary]
    I --> J[Return JSON result]
```

### Key Design

- **Atomicity**: All `old_string` values are validated first; if any fails, no file is modified.
- **Exact match**: Uses `str::replace_once` semantics, requiring `old_string` to appear exactly once in the file.
- **TUI cards**: If the tool context contains `ui_tx`, `batch_edit` calls `emit_file_write_cards` to push `AgentUpdate::FileWrite` and `AgentUpdate::WriteFinalized` to the TUI, which renders the right-side diff cards.

---

## 4. `batch_read` Batch Read Flow

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

## 5. TUI Consumption of AgentUpdate

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
    E -->|FileWrite| M[Register diff preview card]
    E -->|WriteFinalized| N[Convert diff card to code card]
    E -->|RequestSelect| O[Open select popup wait for user]
    E -->|TokenUsage| P[Update token statistics]

    F --> Q[Set app.dirty = true]
    G --> Q
    H --> Q
    I --> Q
    J --> Q
    K --> Q
    L --> Q
    M --> Q
    N --> Q
    O --> Q
    P --> Q

    Q --> R[terminal.draw render]
```

---

## 6. TUI Rendering Layers

```mermaid
flowchart LR
    A[terminal.draw] --> B[render_status_bar]
    A --> C[render_main_area]
    A --> D[render_input_box]
    A --> E[render_bottom_bar]
    A --> F[Popup layer]

    C --> G{Plan visible?}
    G -->|Yes| H[Left plan.rs<br/>Right log.rs]
    G -->|No| I[Only log.rs]

    I --> J[cells/text.rs text lines]
    I --> K[cells/thinking.rs thinking cards]
    I --> L[cells/diff.rs diff cards]
    I --> M[cells/code.rs code cards]

    F --> N[command_palette]
    F --> O[select_popup]
    F --> P[help / history]
    F --> Q[thinking_popup]
    F --> R[diff_popup]
    F --> S[code_popup]
```

---

## 7. Key Code Mapping

| Flow Node | Code Location |
|---|---|
| Agent main loop | `crates/tact/src/agent/mod.rs` `Agent::agent_loop()` |
| Tool call dispatch | `crates/tact/src/agent/tool_dispatch.rs` `Agent::execute_tool_call()` |
| Concrete tool execution | `crates/tact/src/agent/tool_dispatch.rs` native/MCP dispatch helpers |
| Tool registration & routing | `crates/tact/src/tool/mod.rs` `ToolSet::call()` |
| `batch_read` implementation | `crates/tact/src/tool/batch_read.rs` `BatchRead::run()` |
| `batch_edit` implementation | `crates/tact/src/tool/batch_edit.rs` `BatchEdit::run()` |
| Streaming response Anthropic | `crates/tact_llm/src/anthropic.rs` `stream_message()` |
| Streaming response OpenAI | `crates/tact_llm/src/openai.rs` `stream_message()` |
| AgentUpdate definition | `crates/protocol/src/lib.rs` |
| TUI main loop consumption | `crates/tui/src/lib.rs` `run_tui()` |
| TUI handle AgentUpdate | `crates/tui/src/state/app/agent.rs` `handle_agent_update()` |
| Log / card rendering | `crates/tui/src/render/log.rs`, `crates/tui/src/render/cells/` |

---

## 8. Performance & Concurrency Notes

- `AgentUpdate` is sent via a **tokio unbounded channel**; `emit_update` uses `let _ = tx.send(...)`, so it never blocks even if the TUI is not connected.
- `batch_edit` performs **atomic validation and writes** inside the tool; it does not concurrently modify the same file; all file writes happen sequentially.
- `batch_read` reads each file independently in sequential order; it can be changed to concurrent `tokio::fs::read_to_string` if needed.
- The TUI uses `app.dirty` for **dirty rendering**, so it only redraws when new messages or state changes arrive, avoiding idle spinning.

---

## 9. Extension Guide

### Add a new batch tool

1. Create a new module under `crates/tact/src/tool/` (refer to `batch_read.rs` / `batch_edit.rs`).
2. Register it in `ToolSet::tool_specs()` and `ToolSet::call()`.
3. If you need a special card, define a new `AgentUpdate` variant and handle it in `handle_agent_update`.
4. Add the corresponding renderer in `crates/tui/src/render/cells/`.

### Adjust TUI rendering

- Modify `render/log.rs` `LogColumnRenderer` to change log layout.
- Modify `cells/*.rs` to change thinking/diff/code card styles.
- Modify `popups/*.rs` to change popup layout and interaction hints.
