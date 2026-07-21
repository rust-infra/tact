# Agent–TUI Protocol
> Language: [English](./25_chapter_protocol.md) · [中文](./25_chapter_protocol_zh.md)

This chapter documents the `tact_protocol` crate: message types exchanged between the agent runtime and the terminal UI, and how each `AgentUpdate` variant drives state transitions on both sides.

Implementation: `crates/protocol/src/agent.rs`, `crates/protocol/src/biz.rs`. TUI consumer: `crates/tui/src/widgets/state/app/agent.rs`. Agent emitter: `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact_llm` (streaming).

Related chapters: [Ch 18 Agent Loop](./18_chapter_agent_loop.md), [Ch 23 TUI](./23_chapter_tui.md). Other state machines (input mode, permissions, tasks) live in [docs/state_machines.md](../docs/state_machines.md).

---

## 1. Channels

```mermaid
graph LR
    Agent[Agent runtime] -->|AgentUpdate| TUI[TUI App]
    TUI -->|UserCommand| Driver[tact-ui driver]
    Account[account service] -->|AccountUpdate| TUI
```

| Channel | Type | Direction | Purpose |
|---------|------|-----------|---------|
| `agent_tx` / `agent_rx` | `AgentUpdate` | Agent → TUI | Progress, streaming, metadata |
| `user_cmd_tx` / `user_cmd_rx` | `UserCommand` | TUI → driver | Submit task, cancel, balance query |
| `account_tx` / `account_rx` | `AccountUpdate` | Account → TUI | Balance / quota (separate from agent protocol) |

All three use `tokio::sync::mpsc::unbounded_channel`. `RequestSelect` embeds a `oneshot::Sender` for in-process request–response; it is not serializable and cannot be replayed from session storage.

---

## 2. Core Types

### `AgentUpdate`

```rust
pub enum AgentUpdate {
    StepAdded(PlanStep),
    StepStarted { idx, tool_id, tool_name, arg_summary, arg_full },
    ToolProgress { tool_id, chunks: Vec<ToolOutputChunk> },
    StepFinished { idx, tool_id, result: StepResult },
    StepFailed { idx, tool_id, error },
    TaskComplete(String),
    /// Cancelled mid-task — TUI must leave Planning/Executing
    TaskCancelled,
    Error(AgentErrorKind),
    TokenUsage(TokenUsageInfo),
    ModelInfo(ModelCallParams),
    Info(String),
    RequestSelect { prompt, options, respond, log_confirm }, // single; permission=`false`, ask_user=`true`
    RequestMultiSelect { prompt, options, respond },         // multi; ask_user only (`multi_select`)
    StreamChunk(String),
    ThinkingChunk(ThinkingChunk),
}

pub enum ThinkingChunk {
    Started,
    Delta(String),
    Finished,
}
```

`ThinkingChunk` is a lifecycle enum: producers emit `Started`, zero or more `Delta` fragments, then `Finished`. OpenAI-compatible adapters that only expose `reasoning_content` deltas synthesize `Started` / `Finished` around the stream.

`ToolOutputChunk` carries incrementally decoded text plus a
`ToolOutputStream` (`Stdout`, `Stderr`, or `Other`). A chunk batch preserves the
aggregator-observed order across streams. `ToolProgress` is informational: it
does not indicate success or failure, and unknown or late `tool_id` values are
ignored by the TUI.

### `UserCommand`

```rust
pub enum UserCommand {
    SubmitTask(String),
    Cancel,
    QueryBalance,
}
```

### `PlanStep`

```rust
pub struct PlanStep {
    pub description: String,
    pub tool: String,
    pub tool_id: String,
    pub args: serde_json::Map<String, serde_json::Value>,
    pub output: Option<String>,
}
```

`args` preserves the model's JSON object order and nested values. The TUI does not re-parse tool-specific fields from `args` at runtime — `StepStarted.arg_full` carries the display string computed by the agent.

### `AccountUpdate` (biz module)

Balance and quota updates use `AccountUpdate` on a dedicated channel so provider-specific account state does not leak into `AgentUpdate`. See `crates/protocol/src/biz.rs` and `crates/tact-ui/src/account.rs`.

---

## 3. Plan Step Lifecycle

Each tool call in an assistant turn follows a fixed three-phase emission sequence from `tool_dispatch.rs`:

```mermaid
stateDiagram-v2
    direction LR
    [*] --> Planned: StepAdded
    Planned --> Running: StepStarted
    Running --> Running: ToolProgress *
    Running --> Succeeded: StepFinished
    Running --> Failed: StepFailed
    Succeeded --> [*]
    Failed --> [*]
```

When permission mode is `Ask`, a `RequestSelect` popup may appear **after** `StepStarted` and **before** the tool runs. `Status` stays `Executing`; only `InputMode` switches to `Select` ([§4.3](#43-inputmode-overlay-requestselect)).

```mermaid
stateDiagram-v2
    direction TB
    [*] --> Planned: StepAdded
    Planned --> Running: StepStarted
    Running --> AwaitingChoice: RequestSelect
    AwaitingChoice --> Running: user picks option
    AwaitingChoice --> Failed: user Esc / deny
    Running --> Succeeded: StepFinished
    Running --> Failed: StepFailed
    Succeeded --> [*]
    Failed --> [*]

    note right of AwaitingChoice
        Logical phase only.
        TUI Status remains Executing.
        InputMode = Select.
    end note
```

| Phase | `AgentUpdate` | Agent emitter | TUI effect |
|-------|---------------|---------------|------------|
| Planned | `StepAdded(PlanStep)` | pre-flight | Append to `plan.steps`; `ensure_executing_status` |
| Running | `StepStarted { … }` | pre-flight | Push `ActiveToolBlock`; update `current_step` |
| Progress | `ToolProgress { tool_id, chunks }` | in-flight tool | Update only the matching active block; preserve thinking/loading gates and scroll intent |
| Succeeded | `StepFinished { result }` | post-flight | `finalize_tool_block`; set `plan.steps[idx].output` |
| Failed | `StepFailed { error }` | permission / hooks / execution | Failed tool card or system message; `Status → Idle` |

**`arg_summary` vs `arg_full`:** `arg_summary` is truncated (≤120 chars) for the log title row. `arg_full` is the complete argument string (path, command, or raw JSON) so popups and diff views do not depend on tool-name heuristics in the TUI.

Parallel tools in one turn each run the sequence above. `StepFinished` is emitted as each tool completes — not after the whole scheduling wave joins — so the UI shows concurrent progress ([Ch 11](./11_chapter_task.md)).

### Per-tool emission order

```text
StepAdded
  → StepStarted { arg_summary, arg_full }
  → RequestSelect?          (permission Ask only)
  → ToolProgress*           (after execution starts; informational)
  → StepFinished | StepFailed
```

Independent tools may interleave at the wave level, but each `tool_id` keeps this sequence.
For `bash`, the first progress batch may be immediate; regular batches are at
least 50 ms apart and at most 4 KiB, with a final flush before the terminal
event. The shared output buffer strips ANSI CSI/OSC, applies carriage-return
replacement, retains stream identity for styling, and caps detail at 50,000
characters.

---

## 4. Task-Level Flow

### 4.1 TUI `Status` state machine

The top-level execution state lives in `crates/tui/src/widgets/state/mod.rs`. It drives the status bar and whether a new prompt can be submitted.

```rust
pub(crate) enum Status {
    Idle,
    Planning,
    Executing { current_step: usize, total: usize },
    Done,
}
```

```mermaid
stateDiagram-v2
    [*] --> Idle: startup

    Idle --> Planning: Enter submits task
    Planning --> Executing: StepAdded (ensure_executing_status)
    Executing --> Executing: StepStarted (update current_step)

    Executing --> Done: TaskComplete
    Executing --> Idle: StepFailed
    Executing --> Idle: Error(Other)

    Done --> Idle: 2s timeout (maybe_expire_done_status)

    note right of Planning
        UserCommand::SubmitTask sent.
        StreamChunk / ThinkingChunk may
        arrive before first StepAdded.
    end note

    note right of Executing
        UserCommand::Cancel sets cancel_flag
        and emits Info — Status stays busy until
        TaskCancelled returns to Idle.
    end note
```

| From | To | Trigger | Notes |
|------|-----|---------|-------|
| `Idle` | `Planning` | User presses `Enter` in Insert mode | Clears plan panel; sends `UserCommand::SubmitTask` |
| `Planning` | `Executing` | First `AgentUpdate::StepAdded` | `ensure_executing_status`; `total` from plan length |
| `Executing` | `Executing` | `StepStarted { idx, … }` | Updates `current_step`; may have concurrent `ActiveToolBlock`s |
| `Executing` | `Done` | `TaskComplete` | Sets `task_done_time`; freezes cost timer |
| `Planning` / `Executing` | `Idle` | `TaskCancelled` | Driver after cancelled `agent_loop`; frees input |
| `Executing` | `Idle` | `StepFailed` or `Error(Other)` | Freezes cost timer |
| `Done` | `Idle` | 2 s after `task_done_time` | Main loop calls `maybe_expire_done_status` |
| *(unchanged)* | *(unchanged)* | `UserCommand::Cancel` | `Info("Cancelling…")` + set `cancel_flag`; later `TaskCancelled` |

`TaskComplete` is sent by `crates/tact-ui/src/driver.rs` only when `agent_loop` returns `Ok(())` and `cancel_flag` is false ([Ch 18 §7](./18_chapter_agent_loop.md#7-tui-integration)). Cancelled runs emit `TaskCancelled` instead.

### 4.2 `AgentUpdate` → `Status` mapping

Orthogonal to step lifecycle: which protocol messages actually flip `Status`.

```mermaid
flowchart LR
    subgraph no_change["Status unchanged"]
        SC[StreamChunk]
        TC[ThinkingChunk]
        TU[TokenUsage]
        MI[ModelInfo]
        IN[Info]
        RS[RequestSelect]
        SA2[StepAdded after Executing]
        SS[StepStarted]
        TP[ToolProgress]
        SF[StepFinished]
    end

    subgraph transitions["Status transitions"]
        SA1[StepAdded first] -->|Planning → Executing| EX[Executing]
        TKC[TaskComplete] -->|→ Done| DN[Done]
        TKX[TaskCancelled] -->|→ Idle| ID0[Idle]
        SFL[StepFailed] -->|→ Idle| ID1[Idle]
        ER[Error Other] -->|→ Idle| ID2[Idle]
        DN -->|2s| ID3[Idle]
    end

    P[Planning] --> SA1
    EX --> TKC
    EX --> TKX
    P --> TKX
    EX --> SFL
    EX --> ER
```

| `AgentUpdate` | TUI `Status` / mode | Notes |
|---------------|---------------------|-------|
| `StepAdded` (first) | `Planning → Executing` | `ensure_executing_status` |
| `StepStarted` | `Executing` (update `current_step`) | May have multiple concurrent `ActiveToolBlock`s |
| `ToolProgress` | No status change | Informational live output for one active `tool_id` |
| `StepFailed` / `Error(Other)` | `→ Idle` | Cost timer frozen |
| `RequestSelect` | `InputMode::Select` (Status stays `Executing`) | See [Ch 10](./10_chapter_permission.md) |
| `TaskComplete` | `→ Done` (2s → `Idle`) | Emitted by driver, not `agent_loop` |
| `TaskCancelled` | `→ Idle` | Emitted by driver after cancelled loop; unblocks new prompts |
| `TokenUsage` / `ModelInfo` | No status change | Metadata-only; status bar update |
| `StreamChunk` / `ThinkingChunk` / `Info` | No status change | Log / stream only |

### 4.3 InputMode overlay (`RequestSelect`)

Permission prompts use a separate input-mode state machine. `RequestSelect` does **not** add a `Status` variant — the status bar can still read `Executing` while the select popup is open.

```mermaid
stateDiagram-v2
    [*] --> Normal: startup

    Normal --> Insert: i / Enter
    Insert --> Normal: Esc

    Normal --> Select: RequestSelect
    Insert --> Select: RequestSelect
    Select --> Normal: Enter confirms / Esc cancels

    note right of Select
        Arrives while Status = Executing.
        respond: oneshot::Sender → tool_dispatch
    end note
```

### 4.4 Logical phases within `Executing`

While `Status` is `Executing`, the log panel alternates between streaming and tool phases. This is a **view** state, not a separate `Status` enum value:

```mermaid
stateDiagram-v2
    direction LR
    [*] --> Streaming: StreamChunk / ThinkingChunk
    Streaming --> Streaming: more chunks
    Streaming --> ToolPhase: StepAdded
    ToolPhase --> ToolPhase: StepStarted / StepFinished / StepFailed
    ToolPhase --> Streaming: StreamChunk after tools
    ToolPhase --> Done: TaskComplete
    Streaming --> Done: TaskComplete
    ToolPhase --> Idle: StepFailed / Error
    Streaming --> Idle: Error
```

---

## 5. Message Categories

| Category | Variants | TUI side effects |
|----------|----------|------------------|
| **Content-producing** | `StepAdded`, `StepStarted`, `StepFinished`, `StepFailed`, `StreamChunk`, `ThinkingChunk`, `Info`, `TaskComplete`, `TaskCancelled`, `Error`, `RequestSelect` | Prefer `ThinkingChunk::Finished` to close thinking; safety-flush on other content updates; remove loading placeholder; mutate log / plan |
| **Metadata-only** | `TokenUsage(TokenUsageInfo)`, `ModelInfo(ModelCallParams)` | Update status bar only; keep loading placeholder; **do not** close an open thinking region |
| **Request–response** | `RequestSelect { respond }` | Blocks on user choice via oneshot channel |

Thinking lifecycle is explicit: `ThinkingChunk::Started` opens the region, `Delta` appends text, `Finished` flushes and collapses. As a safety net, content-producing non-thinking updates still call `flush_and_close_thinking()` if a region is still open. `TokenUsage` / `ModelInfo` never close thinking (they may arrive mid-stream).

---

## 6. `UserCommand` Transitions

```mermaid
flowchart TB
    subgraph tui["TUI Status"]
        I[Idle] -->|Enter| P[Planning]
        P -->|StepAdded| E[Executing]
        E -->|TaskComplete| D[Done]
        D -->|2s| I
        E -->|StepFailed / Error| I
    end

    subgraph driver["tact-ui driver"]
        ST[SubmitTask] --> AL[agent_loop]
        AL -->|Ok + !cancel| TC[emit TaskComplete]
        AL -->|cancel_flag| XC[emit TaskCancelled]
        CN[Cancel] --> CF[set cancel_flag + Info]
    end

    subgraph account["Account channel"]
        QB[QueryBalance] --> QO[query_once]
        QO --> AU[AccountUpdate → TUI]
    end

    P -.-> ST
    CF -.-> E
```

| Command | TUI precondition | Handler effect |
|---------|------------------|----------------|
| `SubmitTask(text)` | Enter in Insert mode → `Status::Planning` | `build_user_message` → `agent_loop` |
| `Cancel` | `/cancel` or Normal-mode `c` while `Planning` / `Executing` | Set `cancel_flag`; loop exits; driver emits `TaskCancelled` → `Idle` |
| `QueryBalance` | `/balance` or palette | `account::query_once()` → `AccountUpdate` channel |

---

## 7. Typical Message Ordering

Single assistant turn with one tool call:

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant TUI
    participant Driver as tact-ui driver
    participant Agent as agent_loop
    participant LLM

    User->>TUI: Enter (task text)
    TUI->>TUI: Status → Planning
    TUI->>Driver: UserCommand::SubmitTask

    Driver->>Agent: agent_loop(user_message)
    Agent->>LLM: stream_message
    LLM-->>TUI: ThinkingChunk* (optional)
    LLM-->>TUI: StreamChunk*
    Agent->>TUI: ModelInfo

    Agent->>TUI: StepAdded
    TUI->>TUI: Planning → Executing
    Agent->>TUI: StepStarted

    opt permission Ask
        Agent->>TUI: RequestSelect
        TUI->>TUI: InputMode → Select
        User->>TUI: pick option
        TUI->>Agent: oneshot response
    end

    Agent->>TUI: StepFinished | StepFailed
    LLM-->>TUI: StreamChunk* (continuation)
    Agent->>TUI: TokenUsage

    Agent-->>Driver: Ok(())
    Driver->>TUI: TaskComplete
    TUI->>TUI: Status → Done
    Note over TUI: 2s later → Idle
```

Text timeline (same turn):

```text
ThinkingChunk*          ← LLM reasoning stream (optional)
StreamChunk*            ← assistant text before / between tools
ModelInfo               ← model name / max_tokens (metadata)
StepAdded               ← plan panel entry
StepStarted             ← running tool card (arg_summary + arg_full)
RequestSelect?          ← permission Ask (optional)
StepFinished | StepFailed
StreamChunk*            ← assistant continuation text
TokenUsage              ← final usage chunk (metadata)
TaskComplete            ← driver after agent_loop Ok
```

Streaming chunks may arrive between step events. `TokenUsage` is usually emitted from the final LLM stream chunk when `stream_options.include_usage` is set.

---

## 8. Type Reference

| Type | File | Role |
|------|------|------|
| `AgentUpdate` | `agent.rs` | Agent → TUI event enum |
| `ThinkingChunk` | `agent.rs` | Thinking stream lifecycle (`Started` / `Delta` / `Finished`) |
| `UserCommand` | `agent.rs` | TUI → agent command enum |
| `PlanStep` | `agent.rs` | Plan panel row; serde for session persistence |
| `StepResult` / `StepStatus` | `agent.rs` | Structured tool outcome |
| `TokenUsageInfo` | `agent.rs` | LLM token counters (incl. cache / reasoning) |
| `ModelCallParams` | `agent.rs` | Active model configuration snapshot |
| `AgentErrorKind` | `agent.rs` | Fatal error classification (`Display` + `Error`) |
| `BalanceInfo` / `UsageQuotaInfo` | `biz.rs` | Account query results (`f64` amounts, `Option<f64>` quotas) |
| `AccountUpdate` / `AccountError` | `biz.rs` | Account channel messages |

---

## 9. Related Resources

- Protocol source: [crates/protocol/src/agent.rs](../crates/protocol/src/agent.rs)
- Biz types: [crates/protocol/src/biz.rs](../crates/protocol/src/biz.rs)
- TUI handler: [crates/tui/src/widgets/state/app/agent.rs](../crates/tui/src/widgets/state/app/agent.rs)
- Tool dispatch emitter: [crates/tact/src/agent/tool_dispatch.rs](../crates/tact/src/agent/tool_dispatch.rs)
- Other state machines: [docs/state_machines.md](../docs/state_machines.md)
