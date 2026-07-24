# Agent Development Tutorials

> Language: [English](./index.md) · [中文](./index_zh.md)

This directory collects design notes and hands-on tutorials for Tact and related agent runtimes. It is aimed at developers who want to understand or extend agent capabilities.

**Chinese translations** use the `*_zh.md` suffix (e.g. `05_chapter_compact_zh.md`). Full Chinese TOC: [中文首页](./index_zh.md). English remains the canonical source; Chinese chapters mirror the same section structure and diagrams.

---

## Overall Architecture

High-level component map. For module-level detail see [ARCHITECTURE.md](../ARCHITECTURE.md).

```mermaid
graph TB
    subgraph UI
        TUI[tact-ui TUI]
    end

    subgraph Runtime["tact runtime"]
        Agent[Agent / agent_loop]
        Prompt[System Prompt]
        Dispatch[Tool Dispatch]
        Permissions[Permission Manager]
        Hooks[Pre/Post Tool Hooks]
    end

    subgraph Tools
        Native[Native Tools]
        MCP[MCP ToolRouter]
    end

    subgraph Providers
        LLM[tact_llm → LLM APIs]
    end

    subgraph Store
        SQLite[(SQLite Session Store)]
        Files[(.tact/ Store)]
    end

    MCPSrv[MCP Servers]

    TUI -->|user input| Agent
    Agent -->|updates| TUI
    Agent --> Prompt
    Agent -->|stream| LLM
    Agent --> Dispatch
    Dispatch --> Hooks
    Dispatch --> Permissions
    Dispatch --> Native
    Dispatch --> MCP
    MCP --> MCPSrv
    Agent -->|messages & tokens| SQLite
    Agent -->|skills, memory, tasks| Files
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
            Note over Agent: return Ok — loop finished
            Note over TUI: interactive.rs emits TaskComplete after agent_loop Ok (not cancelled)
            TUI->>User: show completion / enable follow-up input
            Note over User,LLM: prompt cycle complete
        end
    end
```

**Stable vs. dynamic sections:** everything above `=== DYNAMIC_BOUNDARY ===` (role, guidelines, CLAUDE.md) is rebuilt but intended to stay byte-identical for prefix caching. Memory and dynamic context below the boundary refresh every turn. See [System Prompt](./04_chapter_prompt.md).

**Tool turns:** when the model returns `ToolUse`, the loop does not exit — tool results are appended to `runtime.context` and the next iteration runs steps 5–12 again with an updated message list and a freshly rendered system prompt.

**Compaction and recovery:** `micro_compact` / `compact_history` in the diagram are covered in [Context Compaction](./05_chapter_compact.md); retries and continuations around the LLM call are covered in [Error Recovery](./06_chapter_recovery.md).

---

## Mind Map

Right-hand tree of all 26 chapters. [Open full page](./mindmap.html) · [Mermaid source](./mindmap.md) · [PNG](./mindmap.png)

<!-- Embedded interactive mind map (renders in HTML/CHM export and VS Code preview; GitHub strips iframes) -->
<iframe
  src="./mindmap.html?embed"
  title="Tact Book Mind Map"
  width="100%"
  height="880"
  style="border:1px solid #2a2a3e;border-radius:10px;background:#1a1a2e;display:block;max-width:100%;"
  loading="lazy"
></iframe>

---

## Table of Contents

**Windows CHM:** run `./book/scripts/build-chm.sh` then `powershell -File book/scripts/build-chm.ps1` on Windows — see [scripts/README.md](./scripts/README.md#chm-windows-compiled-html-help).

Chapters follow **`Agent::agent_loop` execution order**: session → prompt inputs → compaction → LLM recovery → tool pipeline → domain tools → side systems.

| # | Chapter | Description |
|---|---------|-------------|
| 1 | [Store and Persistence](./01_chapter_store.md) ([中文](./01_chapter_store_zh.md)) | `StoreRoot` / JSON file store, SQLite session database, domain consumers, and agent persistence hooks |
| 2 | [Skill Registry](./02_chapter_skill.md) ([中文](./02_chapter_skill_zh.md)) | `SKILL.md` multi-root discovery, prompt summaries, `load_skill`, TUI slash invoke (`$ARGUMENTS`), and `<skill>` tag format |
| 3 | [Persistent Memory](./03_chapter_memory.md) ([中文](./03_chapter_memory_zh.md)) | Markdown memories under `.tact/memory/`, types, system prompt injection, `save_memory`, and `MEMORY.md` index |
| 4 | [System Prompt](./04_chapter_prompt.md) ([中文](./04_chapter_prompt_zh.md)) | How Tact assembles the system prompt from role, skills, guidelines, memory, and dynamic context, and how it stays cache-friendly across turns |
| 5 | [Context Compaction](./05_chapter_compact.md) ([中文](./05_chapter_compact_zh.md)) | `micro_compact` tool-result stubbing, `compact_history` LLM summarization, transcript spill, and large-output persistence |
| 6 | [Error Recovery](./06_chapter_recovery.md) ([中文](./06_chapter_recovery_zh.md)) | `RecoveryState`, transport back-off retries, prompt-too-long compaction, and output-limit continuation in `agent_loop` |
| 7 | [Tool System](./07_chapter_tool.md) ([中文](./07_chapter_tool_zh.md)) | `Tool` trait, `ToolRouter`, `tool/registry.rs`, `ToolContext`, path safety, and `#[tool]` macro |
| 8 | [MCP Protocol and Agent Integration](./08_chapter_mcp.md) ([中文](./08_chapter_mcp_zh.md)) | Model Context Protocol fundamentals, step-by-step protocol flow, and MCP integration in Tact (configuration, handshake, tool calls, dynamic updates, graceful shutdown) |
| 9 | [Agent Lifecycle Hooks](./09_chapter_hook.md) ([中文](./09_chapter_hook_zh.md)) | PreToolUse / PostToolUse extension points, `HookControl`, registration API, and where hooks sit in the tool pipeline |
| 10 | [Permission Model](./10_chapter_permission.md) ([中文](./10_chapter_permission_zh.md)) | Capability risk classification, permission modes, allowlist, TUI approval flow, and shell high-risk detection |
| 11 | [Tasks and Tool Scheduling](./11_chapter_task.md) ([中文](./11_chapter_task_zh.md)) | **Tool** parallel scheduling (waves/barriers) — not [Ch 19 Persistent Tasks](./19_chapter_persistent_tasks.md) |
| 12 | [Subagents](./12_chapter_subagent.md) ([中文](./12_chapter_subagent_zh.md)) | The `task` tool: nested `agent_loop`, restricted toolset, static prompt, permission inheritance, and summary return |
| 13 | [Background Tasks](./13_chapter_background.md) ([中文](./13_chapter_background_zh.md)) | Async shell commands via `background_run` / `check_background`, tokio spawn lifecycle, timeouts, and startup repair |
| 14 | [Team Coordination](./14_chapter_team.md) ([中文](./14_chapter_team_zh.md)) | Teammate roster under `.tact/team/`, JSONL inboxes, broadcasts, and plan-approval / shutdown protocol messages |
| 15 | [Worktree Lanes](./15_chapter_worktree.md) ([中文](./15_chapter_worktree_zh.md)) | Isolated `git worktree` lanes: `worktree_create` / `list` / `status` / `run` / `events`, index file, and audit log |
| 16 | [Cron Scheduling](./16_chapter_cron.md) ([中文](./16_chapter_cron_zh.md)) | Scheduled prompt registry: data model, `.tact/cron/` persistence, `cron_create` / `cron_list` / `cron_delete`, and current runtime gaps |
| 17 | [Desktop Notifications](./17_chapter_notify.md) ([中文](./17_chapter_notify_zh.md)) | macOS native notifications for task completion and step failures, config flags, and platform gaps |
| 18 | [Agent Main Loop](./18_chapter_agent_loop.md) ([中文](./18_chapter_agent_loop_zh.md)) | Capstone: `agent_loop` turn cycle, streaming, `cancel_flag`, `AgentUpdate`, TUI `TaskComplete` wiring |
| 19 | [Persistent Task Manager](./19_chapter_persistent_tasks.md) ([中文](./19_chapter_persistent_tasks_zh.md)) | `TaskManager`, `task_create` / `get` / `list` / `update`, dependencies under `.tact/tasks/` |
| 20 | [Hallucination in Agent Loops](./20_chapter_hallucination.md) ([中文](./20_chapter_hallucination_zh.md)) | LLM fabrication of files, summaries, and tool results; compaction summary hallucination case study; stub-induced content fabrication; mitigation strategies |
| 21 | [Configuration](./21_chapter_config.md) ([中文](./21_chapter_config_zh.md)) | TOML/CLI merge, `ResolvedConfig`, `init()` → `tact_llm::init_provider` |
| 22 | [LLM Providers](./22_chapter_llm.md) ([中文](./22_chapter_llm_zh.md)) | `tact_llm` adapters, streaming, thinking, `user_id`, balance queries |
| 23 | [Terminal UI](./23_chapter_tui.md) ([中文](./23_chapter_tui_zh.md)) | `tui` crate, `AgentUpdate` / `UserCommand` channels, `tact-ui` wiring |
| 24 | [Testing Strategy](./24_chapter_testing.md) ([中文](./24_chapter_testing_zh.md)) | Mock LLM harness, tact-ui driver tests, TUI TestBackend render tests, CI |
| 25 | [Agent–TUI Protocol](./25_chapter_protocol.md) ([中文](./25_chapter_protocol_zh.md)) | `tact_protocol` message types, plan step lifecycle, task-level state transitions |
| 26 | [Engineering Issue Log](./26_chapter_issue.md) ([中文](./26_chapter_issue_zh.md)) | Chronological log of shipped optimizations and bug fixes (problem → decision → pointers) |

---

## How to Read

- **Runtime order**: Chapters 1–11 follow one turn of `agent_loop` (store → prompt → compact → LLM → hooks → permissions → tool dispatch). Chapters 12–15 cover specific tool families; 16–17 are off-path systems. **Ch 18** ties the loop together; **19** covers TaskManager in depth; **20** documents LLM hallucination patterns. **Ch 21–22** cover bootstrap (config, LLM, TUI) — read them first if you are wiring a new binary or provider. **Ch 24** documents the integration test harness. **Ch 25** documents the `tact_protocol` message types and state transitions. **Ch 26** is the engineering issue / optimization log — append when shipping behavior changes (see `AGENTS.md`).
- **Tact as the reference implementation**: Examples and code maps reflect this repository. Other agent frameworks follow similar ideas with different details.

---

## Planned Chapters

Future additions may cover deployment or plugin APIs. Behavioral optimizations and bug fixes go into **Ch 26** as they ship rather than as separate planned chapters.

---

## Related Resources

- Project architecture: [ARCHITECTURE.md](../ARCHITECTURE.md)
- MCP official docs: <https://modelcontextprotocol.io/docs/learn/architecture>
- Tact MCP source: [crates/tact/src/mcp/mod.rs](../crates/tact/src/mcp/mod.rs)
- Tact hook source: [crates/tact/src/hook/mod.rs](../crates/tact/src/hook/mod.rs)
- Tact cron source: [crates/tact/src/cron/mod.rs](../crates/tact/src/cron/mod.rs)
- Tact permission source: [crates/tact/src/permission/mod.rs](../crates/tact/src/permission/mod.rs)
- Tact memory source: [crates/tact/src/memory/mod.rs](../crates/tact/src/memory/mod.rs)
- Tact notifications source: [crates/tact/src/notifications/mod.rs](../crates/tact/src/notifications/mod.rs)
- Tact store source: [crates/tact/src/store/mod.rs](../crates/tact/src/store/mod.rs)
- Tact session store source: [crates/tact/src/store/session_store/](../crates/tact/src/store/session_store/)
- Tact tool source: [crates/tact/src/tool/](../crates/tact/src/tool/) (`mod.rs`, `registry.rs`, individual tools)
- Tact skill source: [crates/tact/src/skill/mod.rs](../crates/tact/src/skill/mod.rs)
- Tact recovery source: [crates/tact/src/recovery.rs](../crates/tact/src/recovery.rs)
- Tact team source: [crates/tact/src/team.rs](../crates/tact/src/team.rs)
- Tact worktree source: [crates/tact/src/worktree/mod.rs](../crates/tact/src/worktree/mod.rs)
- Tact compaction source: [crates/tact/src/compact/mod.rs](../crates/tact/src/compact/mod.rs)
- Tact background source: [crates/tact/src/background.rs](../crates/tact/src/background.rs)
- Tact subagent source: [crates/tact/src/tool/subagent.rs](../crates/tact/src/tool/subagent.rs)
- Tact agent loop source: [crates/tact/src/agent/mod.rs](../crates/tact/src/agent/mod.rs)
- Tact task manager source: [crates/tact/src/task/mod.rs](../crates/tact/src/task/mod.rs)
- Tact config source: [crates/tact/src/config/](../crates/tact/src/config/)
- Tact LLM source: [crates/tact_llm/src/lib.rs](../crates/tact_llm/src/lib.rs)
- Tact protocol source: [crates/protocol/src/agent.rs](../crates/protocol/src/agent.rs)
- Protocol state machines: [book/25_chapter_protocol.md](./25_chapter_protocol.md)
- TUI rendering deep dive: [docs/tui_rendering.md](../docs/tui_rendering.md)

---

## Video Generation (AI Workflow)

Turn a chapter into slide + narration video with minimal manual work:

1. Generate `scenes.json` using the LLM prompt in [prompts/scene-generator.md](./prompts/scene-generator.md)
2. Run the pipeline: `./book/scripts/generate.sh <chapter> --all`

`<chapter>` is the **slug** in the filename (e.g. `mcp` → `08_chapter_mcp.md`, `store` → `01_chapter_store.md`, `compact_zh` → `05_chapter_compact_zh.md`), not the numeric prefix.

Full docs: [scripts/README.md](./scripts/README.md)
