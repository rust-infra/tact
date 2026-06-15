# sfull: Complete Agent Harness

`sfull` is the integrated version of all previous chapters. It converges the minimal agent loop, tool system, skills, context compaction, permissions, hooks, memory, tasks, background processes, cron, team collaboration, worktree, MCP, and tool routing into a single Rust agent runtime.

This section does not introduce a new feature — it answers an engineering question:

```text
When an agent harness accumulates more and more capabilities, how do you organize the loop, tools, state, permissions, and external plugins into a clear structure?
```

## Running

Configure `.env` at the repository root:

```bash
ANTHROPIC_API_KEY=your_api_key
ANTHROPIC_BASE_URL=your_anthropic_compatible_base_url
```

Run:

```bash
cargo run -p tact
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
tact/
├── src/
│   ├── main.rs                   # Initialization and interactive CLI
│   ├── lib.rs                    # Agent runtime and main loop
│   ├── store.rs                  # StoreRoot / Store / CollectionStore
│   ├── prompt.rs                 # System prompt builder
│   ├── system_prompt_template.md
│   ├── permission.rs
│   ├── hook.rs
│   ├── compact.rs
│   ├── recovery.rs
│   ├── memory.rs
│   ├── skill.rs
│   ├── task.rs
│   ├── background.rs
│   ├── cron.rs
│   ├── team.rs
│   ├── worktree.rs
│   ├── mcp.rs
│   └── tool/
│       ├── mod.rs                # Tool trait / ToolRouter / ToolContext
│       ├── bash.rs
│       ├── read_file.rs
│       ├── write_file.rs
│       ├── edit_file.rs
│       ├── load_skill.rs
│       ├── compact.rs
│       ├── memory.rs
│       ├── subagent.rs
│       ├── task.rs
│       ├── background.rs
│       ├── cron.rs
│       ├── team.rs
│       └── worktree.rs
└── tact.md
```

Suggested reading order: start with [`src/main.rs`](./src/main.rs), then [`src/lib.rs`](./src/lib.rs), then read each domain manager and tool.

## Startup Flow

Entry point is [`src/main.rs`](./src/main.rs). Startup sequence:

```text
Create LLM client
  → Select PermissionMode
  → Scan skills/
  → Create .claude StoreRoot
  → Initialize task/background/cron/team/worktree managers
  → Initialize memory manager
  → Scan .claude-plugin/plugin.json and connect MCP servers
  → Construct ToolContext
  → Construct ToolRouter
  → Create Agent
  → Enter interactive loop
```

The main agent uses a dynamic system prompt. Subagents use a static prompt, allowing them to act as fresh-context coding subagents, complete their assigned task, and return a summary.

## Agent

Core structure in [`src/lib.rs`](./src/lib.rs):

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
    pub skill_registry: Arc<SkillRegistry>,
    pub memory_manager: Arc<Mutex<MemoryManager>>,
    pub work_dir: PathBuf,
    pub task_manager: SharedTaskManager,
    pub background_manager: SharedBackgroundManager,
    pub cron_scheduler: SharedCronScheduler,
    pub teammate_manager: SharedTeammateManager,
    pub worktree_manager: SharedWorktreeManager,
}
```

It contains only the business dependencies tools need to execute. It does not include:

- LLM client
- Conversation context
- Permission policy
- Recovery state
- Hooks

These belong to the agent runtime. This boundary prevents the tool layer from inversely controlling the entire agent.

## Local Tools

`toolset()` registers the complete tool set:

- Basic: `add`, `bash`, `read_file`, `write_file`, `edit_file`
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

The skill system scans:

```text
skills/*/SKILL.md
```

At startup, only skill summaries are placed in the system prompt. Full content is loaded on demand via `load_skill`.

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

## Recommended Reading Order

1. [`src/main.rs`](./src/main.rs) — understand the initialization sequence.
2. [`src/lib.rs`](./src/lib.rs) — understand the complete agent loop.
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
