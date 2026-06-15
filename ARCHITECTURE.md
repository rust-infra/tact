# Architecture & Flow

This document describes the overall architecture, core data flow, and terminal UI layout of `tact` using Mermaid diagrams.

---

## 0. Workspace Structure

This project is a Cargo Workspace containing the following crates:

| Directory | Package | Responsibility |
|---|---|---|
| `crates/core` | `tact_core` | Shared types: `AgentUpdate`, `UserCommand`, `PlanStep`, `StepResult`, `StepStatus` |
| `crates/tools` | `tools` | `Sandbox`: secure wrappers for file I/O and command execution |
| `crates/tui` | `tui` | Terminal UI built with `ratatui` |
| `crates/tact` | `tact` | Agent runtime, main loop, tool router, CLI entry point |
| `crates/tool_refactor_macros` | `tool_refactor_macros` | Proc macros for tool refactoring |

Dependency graph:

```mermaid
flowchart TB
    tact --> tact_core
    tact --> tui
    tact --> tool_refactor_macros
    tui --> tact_core
    tact_core --> tools
```

---

## 1. Module Architecture

```mermaid
flowchart TB
    subgraph main["main.rs"]
        M["main()<br/>Initialize runtime, channels, spawn tasks"]
    end

    subgraph agent_mod["tact/src/lib.rs — Agent Core"]
        A["Agent struct"]
        AG["generate_plan()<br/>Call LLM API"]
        AE["execute_step()<br/>Call sandbox tools"]
        A --> AG
        A --> AE
    end

    subgraph tools_mod["tools crate — Sandbox Tools"]
        S["Sandbox"]
        SR["read_file()"]
        SW["write_file()"]
        SC["run_command()"]
        S --> SR
        S --> SW
        S --> SC
    end

    subgraph tui_mod["tui/ — Terminal UI"]
        T["mod.rs<br/>Event loop"]
        TH["handlers.rs<br/>Key handling"]
        TR["render.rs<br/>Panel rendering"]
        TS["state.rs<br/>App state"]
        TT["theme.rs<br/>Theme colors"]
        T --> TH
        T --> TR
        T --> TS
        TR --> TS
        TH --> TS
        TS --> TT
    end

    M -- "mpsc channel" --> A
    M -- "mpsc channel" --> T
    A -- "Arc<Sandbox>" --> S
    AE -- "tool calls" --> S
    T -- "UnboundedSender" --> A
    A -- "AgentUpdate" --> T
```

---

## 2. Agent Task Execution Flow

```mermaid
sequenceDiagram
    actor U as User
    participant TUI as TUI Module
    participant Agent as Agent Module
    participant LLM as LLM API
    participant SB as Sandbox

    U ->> TUI: Enter task and press Enter
    TUI ->> Agent: UserCommand::SubmitTask
    Agent ->> LLM: generate_plan(task)
    LLM -->> Agent: JSON plan array
    Agent ->> TUI: AgentUpdate::PlanGenerated

    loop Execute step by step
        Agent ->> TUI: AgentUpdate::StepStarted(idx)
        alt need_approval = true
            Agent ->> TUI: AgentUpdate::NeedApproval
            TUI ->> U: Show approval prompt (y/n)
            U -->> TUI: y / n
            TUI -->> Agent: oneshot::Sender<bool>
            alt User rejects
                Agent ->> TUI: AgentUpdate::StepFailed
                Note over Agent,TUI: Terminate task
            end
        end
        Agent ->> SB: execute_step(step)
        SB -->> Agent: Result / Error
        alt Execution succeeded
            Agent ->> TUI: AgentUpdate::StepFinished
        else Execution failed
            Agent ->> TUI: AgentUpdate::StepFailed
            Note over Agent,TUI: Terminate task
        end
    end

    Agent ->> TUI: AgentUpdate::TaskComplete
    TUI ->> U: Show completion message
```

---

## 3. TUI Render Layout

```mermaid
block-beta
    columns 1
    space
    block:status
        columns 1
        status_bar["Status Bar (height 1)"]
    end
    block:main
        columns 2
        plan["Plan Panel<br/>(40% width)<br/>Execution plan list<br/>▼ expanded / ▶ collapsed"]
        log["Log Panel<br/>(60% width)<br/>Message scroll area<br/>Supports search highlight"]
    end
    block:input
        columns 1
        input_box["Input Box (height 3)<br/>Insert mode: task input<br/>Command mode: :cmd<br/>Search mode: /term"]
    end
    space

    style status_bar fill:#2e3440,color:#eceff4
    style plan fill:#2e3440,color:#eceff4
    style log fill:#2e3440,color:#eceff4
    style input_box fill:#2e3440,color:#eceff4
```

### Overlays (popup panels)

```mermaid
block-beta
    columns 1
    space
    block:overlay
        columns 1
        help["Help Panel<br/>Keyboard shortcuts reference"]
        history["History Panel<br/>Task history"]
        palette["Command Palette<br/>Filterable command list"]
    end
    space

    style help fill:#1e1e28,color:#eceff4
    style history fill:#1e1e28,color:#eceff4
    style palette fill:#1e1e28,color:#eceff4
```

---

## 4. Event Loop Flow

```mermaid
flowchart TD
    Start([Start TUI]) --> Init["enable_raw_mode<br/>EnterAlternateScreen"]
    Init --> InitApp["Initialize App state"]
    InitApp --> LoopStart{Main loop}

    LoopStart --> Draw["terminal.draw()<br/>Render all panels"]
    Draw --> PollAgent["try_recv()<br/>Consume Agent updates"]
    PollAgent --> PollEvent["event::poll(50ms)<br/>Detect terminal events"]

    PollEvent -- "No event" --> CheckQuit{should_quit?}
    PollEvent -- "Event received" --> HandleEvent["Handle Key / Mouse / Resize"]

    HandleEvent --> KeyCheck{Key type?}
    KeyCheck -- "Ctrl+C" --> SetQuit["should_quit = true"]
    KeyCheck -- "Ctrl+H" --> ToggleHist["toggle show_history"]
    KeyCheck -- "Ctrl+T" --> ToggleTheme["toggle_theme()"]
    KeyCheck -- "Ctrl+?" --> ToggleHelp["toggle show_help"]
    KeyCheck -- "Regular key" --> ModeDispatch["Dispatch by input_mode"]

    ModeDispatch --> Normal["handle_normal_mode()"]
    ModeDispatch --> Insert["handle_insert_mode()"]
    ModeDispatch --> Command["handle_command_mode()"]
    ModeDispatch --> Search["handle_search_mode()"]
    ModeDispatch --> Palette["handle_palette_mode()"]

    HandleEvent --> Mouse["Mouse event:<br/>scroll wheel / drag select"]
    HandleEvent --> Resize["Resize event:<br/>recalculate layout"]

    SetQuit --> CheckQuit
    ToggleHist --> CheckQuit
    ToggleTheme --> CheckQuit
    ToggleHelp --> CheckQuit
    Normal --> CheckQuit
    Insert --> CheckQuit
    Command --> CheckQuit
    Search --> CheckQuit
    Palette --> CheckQuit
    Mouse --> CheckQuit
    Resize --> CheckQuit

    CheckQuit -- "No" --> LoopStart
    CheckQuit -- "Yes" --> Cleanup["disable_raw_mode<br/>LeaveAlternateScreen"]
    Cleanup --> End([Exit])
```

---

## 5. Channel Communication Architecture

```mermaid
flowchart LR
    subgraph Channels["Tokio MPSC Channels"]
        direction LR
        TX1["ui_tx<br/>(UnboundedSender&lt;AgentUpdate&gt;)"]
        RX1["agent_rx<br/>(UnboundedReceiver&lt;AgentUpdate&gt;)"]
        TX2["user_cmd_tx<br/>(UnboundedSender&lt;UserCommand&gt;)"]
        RX2["cmd_rx<br/>(UnboundedReceiver&lt;UserCommand&gt;)"]
    end

    subgraph AgentTask["Agent async task"]
        A["Agent"]
    end

    subgraph MainThread["Main thread"]
        TUI["TUI event loop"]
    end

    A -- "Send status updates" --> TX1
    TX1 -- "AgentUpdate" --> RX1
    RX1 --> TUI

    TUI -- "Send user commands" --> TX2
    TX2 -- "UserCommand" --> RX2
    RX2 --> A

    style TX1 fill:#bf616a,color:#eceff4
    style RX1 fill:#bf616a,color:#eceff4
    style TX2 fill:#a3be8c,color:#2e3440
    style RX2 fill:#a3be8c,color:#2e3440
```

---

## 6. Sandbox Safe Path Resolution

```mermaid
flowchart TD
    Input["safe_path(relative_path)"] --> Filter["Filter path components:<br/>- Keep Normal<br/>- Pop ParentDir(..)<br/>- Ignore RootDir / Prefix"]
    Filter --> Join["Join with workspace_root"]
    Join --> Exist{"File/directory<br/>exists?"}

    Exist -- "Yes" --> Canonical["canonicalize()<br/>Resolve symlinks"]
    Exist -- "No" --> ParentExist{"Parent directory<br/>exists?"}

    ParentExist -- "Yes" --> ParentCano["parent.canonicalize()<br/>+ file_name"]
    ParentExist -- "No" --> PrefixCheck{"starts_with<br/>workspace_root?"}

    PrefixCheck -- "No" --> Err1["Return error:<br/>Path escapes workspace"]
    PrefixCheck -- "Yes" --> Return1["Return full path"]

    Canonical --> Check{"starts_with<br/>canonical_root?"}
    ParentCano --> Check

    Check -- "No" --> Err2["Return error:<br/>Path escapes workspace"]
    Check -- "Yes" --> Return2["Return safe PathBuf"]

    Err1 --> End([End])
    Err2 --> End
    Return1 --> End
    Return2 --> End
```
