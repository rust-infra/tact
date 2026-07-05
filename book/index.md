# Agent Development Tutorials

This directory collects design notes and hands-on tutorials for Tact and related agent runtimes. It is aimed at developers who want to understand or extend agent capabilities.

---

## Overall Architecture

```mermaid
graph TB
    subgraph UI
        TUI[tact-ui TUI]
    end

    subgraph Runtime["tact runtime"]
        Agent[Agent]
        Prompt[System Prompt]
        Scheduler[Tool Scheduler]
        Permissions[Permission Manager]
        Memory[Memory Manager]
        Hooks[Pre/Post Tool Hooks]
    end

    subgraph Tools
        Native[Native Tools]
        MCP[MCP Servers]
    end

    subgraph Providers
        Anthropic[Anthropic]
        OpenAI[OpenAI / Kimi / DeepSeek]
    end

    subgraph Store
        SQLite[(SQLite Session Store)]
    end

    TUI -->|user input / updates| Agent
    Agent -->|render| Prompt
    Agent -->|stream| Anthropic
    Agent -->|stream| OpenAI
    Agent -->|schedule| Scheduler
    Scheduler -->|call| Native
    Scheduler -->|call| MCP
    Agent -->|check| Permissions
    Agent -->|load / save| Memory
    Agent -->|persist messages & token usage| SQLite
    Agent -->|run| Hooks
```

---

## Prompt Flow: User Input to LLM Request

Each turn of `Agent::agent_loop` turns user input into a fully assembled prompt, streams it to the provider, and either finishes or loops through tool results. The sequence below focuses on how the **system prompt** is built and attached before the model runs.

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant TUI as tact-ui
    participant Agent as Agent
    participant Store as Session Store
    participant Prompt as SystemPrompt (Tera)
    participant Memory as Memory / Skills
    participant LLM as LLM Provider

    User->>TUI: task / follow-up message
    TUI->>Agent: agent_loop(initial_user_message)

    Agent->>Store: ensure_session() / restore history
    Agent->>Agent: push_message → runtime.context
    Agent->>Store: persist user message

    loop each LLM turn
        Agent->>Agent: micro_compact(context)
        opt context over limit
            Agent->>Agent: compact_history()
        end

        Note over Agent,Prompt: build_system_prompt()
        Agent->>Memory: load memory prompt, skill list
        Agent->>Agent: load CLAUDE.md, directory snapshot
        Agent->>Prompt: builder → render template
        Prompt-->>Agent: system string (static prefix + dynamic suffix)

        Agent->>Agent: CreateMessageParams<br/>system + context + tools + thinking
        Agent->>TUI: ModelInfo (model, max_tokens, …)

        Agent->>LLM: stream_message(request)
        LLM-->>TUI: stream tokens / thinking blocks
        LLM-->>Agent: assistant blocks + stop_reason

        Agent->>Store: persist assistant message + token usage

        alt stop_reason = ToolUse
            Agent->>Agent: execute_tool_call()<br/>(hooks → schedule → tools)
            Agent->>Agent: append ToolResult messages
            Agent->>Store: persist tool results
            Note over Agent: loop — rebuild system prompt next turn
        else stop_reason = end_turn
            Agent->>TUI: TaskComplete / final text
            Note over User,LLM: prompt cycle complete
        end
    end
```

**Stable vs. dynamic sections:** everything above `=== DYNAMIC_BOUNDARY ===` (role, guidelines, CLAUDE.md) is rebuilt but intended to stay byte-identical for prefix caching. Memory and dynamic context below the boundary refresh every turn. See [System Prompt](./02_chapter_prompt.md).

**Tool turns:** when the model returns `ToolUse`, the loop does not exit — tool results are appended to `runtime.context` and the next iteration runs steps 5–12 again with an updated message list and a freshly rendered system prompt.

---

## Table of Contents

| Chapter | Description |
|---------|-------------|
| [MCP Protocol and Agent Integration](./01_chapter_mcp.md) | Model Context Protocol fundamentals, step-by-step protocol flow, and MCP integration in Tact (configuration, handshake, tool calls, dynamic updates, graceful shutdown) |
| [System Prompt](./02_chapter_prompt.md) | How Tact assembles the system prompt from role, skills, guidelines, memory, and dynamic context, and how it stays cache-friendly across turns |
| [Tasks and Tool Scheduling](./03_chapter_task.md) | How a single agent turn runs tools through pre-flight, parallel wave execution, and post-processing while keeping conflicting operations ordered |
| [Agent Lifecycle Hooks](./04_chapter_hook.md) | PreToolUse / PostToolUse extension points, `HookControl`, registration API, and where hooks sit in the tool pipeline |
| [Cron Scheduling](./05_chapter_cron.md) | Scheduled prompt registry: data model, `.claude/cron/` persistence, `cron_create` / `cron_list` / `cron_delete`, and current runtime gaps |

---

## How to Read

- **Protocol first, code second**: Each “Step N” in the tutorials maps cleanly to `crates/tact/src/mcp/mod.rs`.
- **Tact as the reference implementation**: Examples and code maps reflect this repository. Other agent frameworks follow similar ideas with different details.

---

## Planned Chapters

These topics are not written yet; they will be added over time:

- Agent main loop (`agent_loop`) and tool scheduling
- Permission model and `PermissionManager`
- Context compaction and session persistence

---

## Related Resources

- Project architecture: [ARCHITECTURE.md](../ARCHITECTURE.md)
- MCP official docs: <https://modelcontextprotocol.io/docs/learn/architecture>
- Tact MCP source: [crates/tact/src/mcp/mod.rs](../crates/tact/src/mcp/mod.rs)
- Tact hook source: [crates/tact/src/hook/mod.rs](../crates/tact/src/hook/mod.rs)
- Tact cron source: [crates/tact/src/cron/mod.rs](../crates/tact/src/cron/mod.rs)

---

## Video Generation (AI Workflow)

Turn a chapter into slide + narration video with minimal manual work:

1. Generate `scenes.json` using the LLM prompt in [prompts/scene-generator.md](./prompts/scene-generator.md)
2. Run the pipeline: `./book/scripts/generate.sh <chapter> --all`

Full docs: [scripts/README.md](./scripts/README.md)
