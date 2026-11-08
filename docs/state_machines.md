# State machines

Tact keeps state transitions in typed Rust enums. The runtime owns execution
state, each frontend projects that state into its own view model, and durable
stores own lifecycle records. This page lists the current enums and the code
that advances them.

## Runtime events

`tact_agent_api::AgentEvent` is the ordered kernel event stream. It is defined
in `crates/agent_api/src/event.rs`.

| Event group | Events |
|---|---|
| Run and turn | `RunStarted`, `TurnStarted`, `TurnFinished`, `RunFinished`, `RunFailed` |
| Model and message | `ModelCallStarted`, `MessageStarted`, `MessageDelta`, `MessageFinished` |
| Tool lifecycle | `ToolQueued`, `ToolStarted`, `ToolProgress`, `ToolFinished` |
| Accounting | `Usage`, `ContextCompacted`, `Notice` |

`AgentEvent::delivery()` separates advisory progress from reliable terminal
events. `MessageDelta` and `ToolProgress` are progress events. Every lifecycle,
result, usage, and failure event is terminal and must remain ordered.

`crates/tact/src/runtime/event_adapter.rs` converts `AgentEvent` into the
host-facing `tact_protocol::AgentUpdate`. The desktop controller consumes the
same product events and updates `DesktopSnapshot`.

## Tool dispatch

The kernel owns the transition from queued to started:

```text
model emits ToolUse
  -> ToolQueued
  -> permission gate
  -> ToolStarted when the tool's wave begins
  -> ToolProgress while output arrives
  -> ToolFinished
```

A denied call still emits `ToolStarted` followed by `ToolFinished`, so every
queued card reaches a terminal state. `ToolQueued` means accepted for this
turn. It does not mean the call is running.

The scheduler and resource conflict rules are in
[`parallel_tool_execution.md`](parallel_tool_execution.md).

## TUI status and input

`crates/agent_tui_kit/src/state/ui_types.rs` defines the reusable TUI states.

```rust
pub enum Status {
    Idle,
    Planning,
    Executing { current_step: usize, total: usize },
    Done,
}

pub enum InputMode {
    Normal,
    Insert,
    Palette,
    Select,
    FilePicker,
}
```

The TUI app in `crates/tui/src/widgets/state/app/` owns the transitions.
`InputMode::Select` is derived from the shared `UiResponder` snapshot in
interactive sessions. Headless sessions do not enter the popup state.

## Desktop session and run

The desktop uses two separate state axes:

| Enum | File | Purpose |
|---|---|---|
| `SessionPhase` | `crates/desktop_core/src/snapshot.rs` | Session ownership and host lifecycle |
| `RunPhase` | `crates/desktop_core/src/snapshot.rs` | Current activation phase |

`SessionPhase` values are `Empty`, `Opening`, `Starting`, `Ready`, `Running`,
`Switching`, `Closing`, `Disconnected`, and `Failed`.

`RunPhase` values are `Idle`, `Waiting`, `Thinking`, `Streaming`,
`RunningTools`, `WaitingForGate`, `Cancelling`, `Complete`, and `Failed`.

`crates/desktop_core/src/controller.rs` owns the transitions. The UI reads the
phase from `DesktopSnapshot`; it does not infer host state from individual
events.

## Desktop transcript rows

`crates/desktop_core/src/transcript/row.rs` defines row identity and status.

| Enum | Meaning |
|---|---|
| `RowBody` | The row kind: user, assistant, thinking, plan, tool group, diff, gate, notice, hook context, hook status, or error |
| `RowStatus` | `Pending`, `Running`, `Success`, `Failed`, `Cancelled`, `Unknown`, or `Resolved` |
| `GateState` | `Pending`, `Submitting`, `Resolved`, `Cancelled`, `Withdrawn`, or `Stale` |
| `GateKind` | `Select` or `MultiSelect` |

`crates/desktop_core/src/transcript/reducer.rs` validates stale sessions and
gate responses before it changes a row. A late event cannot move a terminal row
back to running.

## Tasks and background jobs

Persistent tasks live in `crates/tact/src/task/mod.rs`:

```text
Pending -> InProgress -> Completed
Pending -> Deleted
InProgress -> Deleted
```

Completing a task removes its id from other tasks' `blocked_by` lists.

Background commands live in `crates/tact/src/background.rs`:

```text
Running -> Completed
Running -> Error
```

`WaitOutcome` adds the caller-facing result `Finished`, `TimedOut`, or
`Cancelled`. It does not change the stored task status.

## Goals

`crates/tact/src/goal/mod.rs` defines the durable Goal lifecycle.

| State | Meaning |
|---|---|
| `Draft` | Created locally, not queued |
| `Queued` | Waiting for a runner |
| `Running` | An activation is active or can resume |
| `WaitingUser` | A Goal tool requested user input |
| `WaitingPermission` | Reserved state; no producer writes it today |
| `Paused` | Explicitly paused and resumable |
| `Verifying` | Completion was proposed and verification is running |
| `Succeeded`, `Failed`, `Cancelled` | Terminal states |

`GoalPhase` is orthogonal: `Planning`, `Execution`, or `Verification`.
`GoalRunOutcome` records why one bounded activation ended. The Goal runner and
store enforce lease ownership and write transitions through the SQLite Goal
store.

## Permissions and hooks

`crates/tact/src/permission/mod.rs` defines:

```rust
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}
```

The decision order is `deny`, then `ask`, then `allow`, followed by the active
permission mode. Rules live in the global and project `settings.json` files,
not in `config.toml`.

`crates/tact/src/hook/mod.rs` defines `HookControl::Continue` and
`HookControl::Block(reason)`. A block ends the hook chain and fails the guarded
operation with the supplied reason.

## Where to read next

| Topic | Document |
|---|---|
| Protocol and gate sequencing | [`book/25_chapter_protocol_zh.html`](../book/25_chapter_protocol_zh.html) |
| TUI rendering | [`tui_rendering.md`](tui_rendering.md) |
| Tool cards | [`tool_rendering.md`](tool_rendering.md) |
| Parallel tool scheduling | [`parallel_tool_execution.md`](parallel_tool_execution.md) |
| Goal lifecycle | [`book/28_chapter_goal_zh.html`](../book/28_chapter_goal_zh.html) |
