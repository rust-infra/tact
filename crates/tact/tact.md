# sfull: Complete Agent Harness

`sfull` is the integrated version of all previous chapters. It converges the minimal agent loop, tool system, skills, context compaction, permissions, hooks, memory, tasks, background processes, cron, team collaboration, worktree, MCP, and tool routing into a single Rust agent runtime.

This section does not introduce a new feature — it answers an engineering question:

```text
When an agent harness accumulates more and more capabilities, how do you organize the loop, tools, state, permissions, and external plugins into a clear structure?
```

## Running

Configure `.env` at the repository root:

```bash
  # Configure the provider
  # Options: "anthropic" | "openai" | "deepseek" | "kimi"
  export TACT_LLM_PROVIDER=anthropic

  # API keys (set at least the one for the active provider)
  export ANTHROPIC_API_KEY=your_anthropic_api_key
  export ANTHROPIC_BASE_URL=your_anthropic_compatible_base_url
  export DEEPSEEK_API_KEY=your_deepseek_api_key
  export DEEPSEEK_BASE_URL=your_deepseek_base_url
  export KIMI_API_KEY=your_kimi_api_key
  export KIMI_BASE_URL=your_kimi_base_url
  export OPENAI_API_KEY=your_openai_api_key
  export OPENAI_BASE_URL=your_openai_base_url
```

Run:

```bash
cargo run -p tact-ui          # launches tact-ui (default TUI)
# or
cargo run -p tact-ui -- headless "your prompt"
```

At startup, choose a permission mode:

```text
Default
Plan
Auto
```

Exit:

```text
exit()
```

## Goals

- Integrate capabilities from previous chapters into a runnable agent.
- Express the main runtime boundary with `Agent`.
- Manage local tools with `ToolRouter`.
- Inject shared domain managers into tools via `ToolContext`.
- Converge domain state persistence with `Store<T>` / `CollectionStore<T>`.
- Route both local tools and MCP tools through the same tool use loop.
- Enforce unified permission and hook checks before tool execution.
- Handle recovery for oversized context, output truncation, and transient errors.

## Code Structure

```text
crates/
├── tact-ui/                      # Binary (TUI + headless)
│   └── src/
│       ├── main.rs               # CLI dispatch, config::init(), SQLite session store
│       ├── interactive.rs        # TUI session: Agent, managers, MCP, run_tui
│       ├── headless.rs           # Headless session + completion notify
│       ├── permission.rs         # permission_mode_from_config()
│       ├── user_message.rs       # build_user_message()
│       └── sessions.rs           # --list-sessions
└── tact/                         # Agent runtime library
    └── src/
        ├── lib.rs                # Module re-exports
        ├── agent/
        │   ├── mod.rs            # Agent, agent_loop
        │   ├── tool_dispatch.rs  # execute_tool_call, MCP/native dispatch
        │   └── tool_schedule.rs  # Parallel wave scheduler
        ├── lsp/                  # LSP client (config, protocol, diagnostics, …)
        ├── tool/
        │   ├── mod.rs            # Tool trait, ToolRouter, ToolContext
        │   ├── registry.rs       # toolset(), subagent_toolset()
        │   └── …                 # bash, read_file, write_file, …
        ├── store.rs              # StoreRoot / Store / CollectionStore
        ├── prompt/               # System prompt builder
        ├── permission/           # PermissionManager
        ├── hook/                 # Pre/PostToolUse hooks
        ├── compact.rs            # Context compaction
        ├── recovery.rs           # Error recovery
        ├── memory/               # MemoryManager
        ├── skill/                # SkillRegistry
        ├── task/                 # TaskManager
        ├── background.rs
        ├── cron/
        ├── team.rs
        ├── worktree/
        ├── mcp/                  # MCP client + MCPToolRouter
        └── stats.rs              # SessionStats
```

Suggested reading order: [`../tact-ui/src/main.rs`](./../tact-ui/src/main.rs) → [`../tact-ui/src/interactive.rs`](./../tact-ui/src/interactive.rs) or [`headless.rs`](./../tact-ui/src/headless.rs) → [`src/agent/mod.rs`](./src/agent/mod.rs) → domain managers and tools.

## Startup Flow

CLI entry is [`../tact-ui/src/main.rs`](./../tact-ui/src/main.rs); session setup lives in [`interactive.rs`](./../tact-ui/src/interactive.rs) / [`headless.rs`](./../tact-ui/src/headless.rs). Startup sequence:

```text
config::init()
  → open SQLite session store (main.rs)
  → dispatch headless or interactive
Create LLM client
  → Resolve PermissionMode (permission.rs / TUI prompt)
  → Scan skill roots (legacy skills/ → ~/.tact/skills → .claude/skills)
  → Create .claude StoreRoot
  → Initialize task/background/cron/team/worktree managers
  → Initialize memory manager
  → Scan .claude-plugin/plugin.json and connect MCP servers
  → Construct ToolContext + toolset() ToolRouter
  → Create Agent
  → Enter agent loop (TUI or stdout)
```

The main agent uses a dynamic system prompt. Subagents use a static prompt, allowing them to act as fresh-context coding subagents, complete their assigned task, and return a summary.

## Agent

Core structure in [`src/agent/mod.rs`](./src/agent/mod.rs):

```rust
pub struct Agent {
    pub runtime: AgentRuntime,
    pub tool_context: ToolContext,
    pub tools: ToolRouter,
    pub mcp_router: MCPToolRouter,
    pub hooks: Vec<Hook>,
    pub system_prompt: AgentSystemPrompt,
}
```

It decomposes the agent into:

- `AgentRuntime` — model client, context, compaction state, recovery state, permission manager.
- `ToolContext` — business dependencies accessible to tools.
- `ToolRouter` — local tool registration and invocation.
- `MCPToolRouter` — external MCP tool routing.
- `hooks` — extension points before and after tool calls.
- `system_prompt` — dynamic or static prompt.

The key design decision: tools receive only `ToolContext`, never the full `Agent`.

## Agent Loop

`Agent::agent_loop()` is the complete main loop:

```text
micro compact
  → If context exceeds limit, auto compact
  → Build model request
  → Merge local tool schemas and MCP tool schemas
  → Call model
  → Handle prompt too long / transient error / max tokens
  → If no tool_use, end this round
  → Execute tool_use
  → Push tool_result back
  → If compact tool was invoked, manually compact
  → Continue loop
```

This is still the s01 closed loop, but each stage now has the engineering boundaries a real agent needs.

## ToolRouter

The tool system continues the s20 structure, centered in [`src/tool/mod.rs`](./src/tool/mod.rs):

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;

    async fn call(&self, context: ToolContext, input: Value) -> Result<String>;
}
```

`ToolRouter` maintains a mapping from tool names to tool implementations:

```rust
pub struct ToolRouter {
    tools: HashMap<String, Box<dyn Tool>>,
}
```

Local tools register via chaining:

```rust
ToolRouter::new()
    .route(BashTool)
    .route(ReadFileTool)
    .route(TaskCreateTool)
    .route(WorktreeRunTool)
```

Each tool uses strongly-typed input and generates model-visible `input_schema` via `schemars`. This keeps schemas and Rust input types in sync.

## ToolContext

`ToolContext` is the dependency injection object for the tool layer:

```rust
pub struct ToolContext {
    pub skill_registry: Arc<Mutex<SkillRegistry>>,
    pub memory_manager: Arc<Mutex<MemoryManager>>,
    pub work_dir: PathBuf,
    pub task_manager: SharedTaskManager,
    pub background_manager: SharedBackgroundManager,
    pub cron_scheduler: SharedCronScheduler,
    pub teammate_manager: SharedTeammateManager,
    pub worktree_manager: SharedWorktreeManager,
    pub ui_tx: Option<UnboundedSender<AgentUpdate>>,
    pub progress_reporter: ToolProgressReporter,
    pub cancel_flag: Arc<AtomicBool>,
    pub bash_timeout_secs: u64,
}
```

It contains the business dependencies and per-invocation execution controls
tools need. `for_invocation(tool_id)` binds progress to one call; the shared
cancellation flag and resolved bash timeout allow an in-flight command to stop
promptly. It still does not include:

- LLM client
- Conversation context
- Permission policy
- Recovery state
- Hooks

These belong to the agent runtime. This boundary prevents the tool layer from inversely controlling the entire agent.

## Local Tools

`toolset()` registers the complete tool set:

- Basic: `bash`, `read_file`, `write_file`, `edit_file`
- Skill: `load_skill`
- Memory: `save_memory`
- Compact: `compact`
- Subagent: `task`
- Task: `task_create`, `task_get`, `task_list`, `task_update`
- Background: `background_run`, `background_check`
- Cron: `cron_create`, `cron_delete`, `cron_list`
- Team: `spawn_teammate`, `list_teammates`, `send_message`, `broadcast`, `read_inbox`, `plan_approval`, `shutdown_request`, `shutdown_response`
- Worktree: `worktree_create`, `worktree_list`, `worktree_status`, `worktree_run`, `worktree_events`

Subagents use a separate `subagent_toolset()`, which only exposes:

- `bash`
- `read_file`
- `write_file`
- `edit_file`

This allows subagents to independently explore and modify files without recursively creating new teams, cron jobs, background tasks, or worktree control planes.

## Store

The complete version introduces an important abstraction: [`src/store.rs`](./src/store.rs).

The Store layer handles only persistent file I/O and does not express business rules:

- `StoreRoot` — represents the `.claude` state root directory and enforces paths stay within root.
- `Store<T>` — represents a typed JSON file, also supports JSONL append/read_all.
- `CollectionStore<T>` — represents a set of typed JSON files.

Domain managers hold stores and expose business methods.

Example — tasks:

```rust
pub struct TaskManager {
    tasks: CollectionStore<TaskRecord>,
    index: Store<TaskIndex>,
}
```

External code only calls:

```rust
task_manager.create(...)
task_manager.update(...)
task_manager.list(...)
```

Callers don't need to know how task files are named, nor should they directly manipulate `CollectionStore<TaskRecord>`.

## State Directory

The default state root is `.claude` in the current workspace:

```text
.claude/
  background/
    tasks/
      <id>.json
  cron/
    scheduled_tasks.json
  memory/
    MEMORY.md
    *.md
  tasks/
    index.json
    <task>.json
  team/
    config.json
    inbox/
      <owner>.json
  worktrees/
    index.json
```

These files form the agent harness's durable state. They don't depend on the current model context, enabling cross-session recovery and queries.

## Domain Managers

The complete version consolidates all state with clear business semantics into managers.

`TaskManager`:

- Create tasks.
- Query tasks.
- Update status, owner, and dependencies.
- Clean up dependencies on task completion.

`BackgroundManager`:

- Launch background commands.
- Persist background task state.
- Query task output and exit status.

`CronScheduler`:

- Create scheduled tasks.
- Delete scheduled tasks.
- List schedule.

`TeammateManager`:

- Save teammate configs.
- Send messages and broadcasts.
- Read inboxes.
- Send protocol requests like plan approval and shutdown.

`WorktreeManager`:

- Create git worktrees.
- List worktrees.
- Check status.
- Execute commands inside worktrees.
- Record worktree events.

Store is responsible for "how to persist." Manager is responsible for "how this business domain should change."

## Permission

The permission system is in [`src/permission.rs`](./src/permission.rs). Every tool execution is checked before dispatch.

Modes:

- `Default` — allow reads, ask for writes and high-risk operations.
- `Plan` — allow reads, deny writes.
- `Auto` — allow reads and non-high-risk writes, ask for high-risk operations.

Permission checks happen before tool dispatch:

```text
tool_use
  → PermissionManager::check
  → allow / ask / deny
  → ToolRouter or MCPToolRouter
```

Therefore `PermissionManager` belongs to `AgentRuntime`, not `ToolContext`.

## Hooks

Hooks are defined in [`src/hook.rs`](./src/hook.rs), currently with three types:

- `SessionStart`
- `PreToolUse`
- `PostToolUse`

The main loop has wired up `PreToolUse` and `PostToolUse`:

```text
PreToolUse
  → permission check
  → execute tool
  → PostToolUse
  → tool_result
```

`PreToolUse` can modify tool input or block the call. `PostToolUse` can modify tool output or block the result.

## Compaction & Recovery

The complete version handles both context compaction and error recovery.

Compaction mechanisms:

- Run `micro_compact` before each round.
- Auto-compact when estimated context exceeds `CONTEXT_LIMIT`.
- `compact` tool triggers manual compaction.
- Write transcript before compaction.
- Large `bash` output is persisted to disk with preview.
- Recent files read are recorded in the compaction prompt.

Recovery mechanisms:

- Prompt too long → compact then retry.
- Transient transport error → exponential backoff then retry.
- Max tokens → inject continuation message to resume generation.

These mechanisms prevent long tasks from being terminated by oversized context or temporary network issues.

## System Prompt

The dynamic system prompt is generated by [`src/prompt.rs`](./src/prompt.rs) and [`src/system_prompt_template.md`](./src/system_prompt_template.md).

It contains:

- Agent role and working directory.
- Behavioral constraints.
- Summary of available skills.
- Memory content.
- `CLAUDE.md` instructions.
- Dynamic context: current date, working directory, model, platform.
- Memory usage guidance.

The main agent builds a dynamic prompt each loop. Subagents use a static prompt to avoid inheriting the main agent's full context.

## Skills & Memory

The skill system scans (later root wins on name clash):

```text
<workdir>/skills/*/SKILL.md          # legacy
~/.tact/skills/*/SKILL.md            # user
<workdir>/.claude/skills/*/SKILL.md  # project (canonical)
```

At startup, only skill summaries go into the system prompt (`describe_available`). Full content is loaded on demand via `load_skill`, or injected as a user task from the TUI when the user runs `/skill-name` (see book Ch 2 / Ch 23).

The memory system uses `.claude/memory`, writing preferences, facts, feedback, and references via `save_memory`. The system prompt loads a memory summary, allowing the agent to retain important information across sessions.

## MCP

MCP integration is in [`src/mcp.rs`](./src/mcp.rs).

On startup, it scans:

```text
.claude-plugin/plugin.json
```

MCP servers declared in the manifest are launched and connected. Each external tool is converted to a model-visible tool spec with the naming format:

```text
mcp__<plugin>__<server>__<tool>
```

At execution time:

- Regular tools are routed through `ToolRouter`.
- `mcp__`-prefixed tools are routed through `MCPToolRouter`.

Both tool categories pass through permission checks before execution, and results are pushed back into context as `tool_result`.

## Worktree & Subagents

The `task` tool can launch fresh-context subagents. A subagent has its own `Agent` instance and independent context but shares the base dependencies in `ToolContext`.

Worktree tools let the agent place tasks into isolated git worktrees:

```text
worktree_create
  → task tool launches subagent
  → worktree_run / worktree_status
  → worktree_events
```

The current `sfull` worktree implementation is a minimal integration, focused on providing the isolated execution entry point in the complete runtime — not a full branch merge flow.

## Relationship to Previous Chapters

`sfull` represents the culmination of a complete Rust agent harness roadmap:

- s01–s04: agent loop, tools, plans, subagents.
- s05–s08: skills, compaction, permissions, hooks.
- s09–s14: memory, prompts, recovery, tasks, background, cron.
- s15–s18: team, protocols, autonomous workers, worktree isolation.
- s19–s20: MCP plugins and tool router refactoring.

In `sfull`, these capabilities are no longer scattered across independent crates — they're organized through a unified runtime, router, context, and store.

## Limitations

- `ToolContext` is still a single type; root agent, subagent, and teammate don't have independent contexts.
- Team protocol is a minimal message protocol; a full autonomous teammate runtime is not yet implemented.
- Worktree covers creation, status, execution, and events only — no merge/rebase/conflict handling.
- Store has no cross-process file locking.
- MCP covers stdio servers and tool calls only — no resources, prompts, OAuth, or auto-reconnect.
- Hooks have types and registration methods but no full config-file-driven system.
- Image attachments (`@file.png`, `![alt](path)` in `tact-ui`) require a vision-capable model; OpenAI-compatible adapters send `image_url` with no capability gate, so text-only endpoints may return HTTP 400.

## Recommended Reading Order

1. [`../tact-ui/src/main.rs`](./../tact-ui/src/main.rs) — CLI dispatch and session store.
2. [`../tact-ui/src/interactive.rs`](./../tact-ui/src/interactive.rs) or [`headless.rs`](./../tact-ui/src/headless.rs) — session wiring.
3. [`src/agent/mod.rs`](./src/agent/mod.rs) — agent loop, tool dispatch, and scheduling.
3. [`src/tool/mod.rs`](./src/tool/mod.rs) — understand ToolRouter and ToolContext.
4. [`src/store.rs`](./src/store.rs) — understand StoreRoot / Store / CollectionStore.
5. [`src/permission.rs`](./src/permission.rs), [`src/compact.rs`](./src/compact.rs), [`src/recovery.rs`](./src/recovery.rs) — understand runtime controls.
6. [`src/task.rs`](./src/task.rs), [`src/team.rs`](./src/team.rs), [`src/worktree.rs`](./src/worktree.rs) — understand domain managers.
7. [`src/mcp.rs`](./src/mcp.rs) — understand how external MCP tools enter the same tool pipeline.

## Verification

Check the complete version:

```bash
cargo check -p tact
```

Run tests:

```bash
cargo test -p tact
```

Check the entire workspace:

```bash
cargo check --workspace
```
