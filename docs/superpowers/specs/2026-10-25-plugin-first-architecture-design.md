# Plugin-first architecture — minimal kernel, capability contributions, explicit composition

Status: **approved** (2026-10-25). The design sections and the revised spec were
approved in the design conversation. It has not been implemented.

Related architecture: [`ARCHITECTURE.md`](../../../ARCHITECTURE.md). Current
plugin marketplace behavior: [`book/25_chapter_protocol_zh.html`](../../../book/25_chapter_protocol_zh.html)
and [`crates/tact/src/plugin/`](../../../crates/tact/src/plugin/).

## Problem

`crates/tact` is the de facto runtime, not a kernel. Agent loop, tools,
permission, hooks, MCP, skills, memory, goals, tasks, teams, worktrees,
background work, voice, storage and configuration all live in one crate and
call each other directly. `crates/tui` imports `tact` for configuration, skills,
voice, session storage, the UI responder and marketplace requests, and its test
fixtures carry the same dependency. `crates/tact-ui` contains the session
bootstrap, the command driver and the goal runner.

That shape has three costs:

- **Every capability changes the core.** Adding a tool, a command, or a prompt
  section means editing the router, the agent prompt builder, or the TUI's
  `SlashCommand` enum. Replacing a capability means editing the agent loop.
- **There is no code-level extension boundary.** The existing "plugin" feature
  is a marketplace package format (Claude/Codex compatibility: skills, commands,
  hooks, MCP). It is valuable, but it is not an extension point for Tact's own
  capabilities, and it is implemented by hardwired modules.
- **Tests and ownership are entangled.** `ToolContext` carries the task, goal,
  team, worktree, background, memory, skill, session, permission and UI
  services. A tool test needs most of the runtime, and a domain cannot be
  understood without the core.

The goal is to invert those dependencies without introducing IPC, dynamic
loading, or a second language boundary. "Everything is a plugin" here means
**everything is outside the kernel in the dependency graph**, not "everything is
an external process".

## Decision summary

| Decision | Choice |
|---|---|
| Plugin meaning | Code-level, in-process Rust extension unit; statically linked into the same binary |
| Kernel scope | Plugin mechanism, capability catalogs, tool execution pipeline, cross-cutting ports, host-facing Agent/Chat/Session contracts |
| Plugin granularity | Capability-domain plugins; tools and commands are contributions; hook definitions are owned by `HooksPlugin` behind the kernel `HookService` port |
| Assembly | Explicit composition root; constructor injection for typed extension services; macro only removes registration boilerplate |
| Crate boundary | New `tact_kernel` crate + existing `tact` crate as the official domain-extensions bundle; views stay with their view crates |
| Model and storage | Extension-level contracts and implementations, wired through typed constructor arguments rather than kernel catalogs |
| Hooks | Kernel defines `HookService`; `HooksPlugin` owns hook definitions, discovery, trust and hook-specific policy |
| Commands | Distributed by owning domain; TUI-local commands live in `TuiPlugin`; no central `CommandsPlugin` |
| Views | TUI and Headless are `View` plugins; TUI/Headless adapters remain outside `tact_kernel` |
| Capability enable/disable | `CapabilityFilter` applies to static contributions and dynamic source snapshots; plugin selection remains the composition root's responsibility |
| Marketplace packages | Package format and `/plugin` UX unchanged; code-level plugins and marketplace packages get distinct type/module names |
| Dynamic loading / IPC / WASM / Node | Out of scope |

## Goals

- A kernel that builds and tests without `tact`, `tui`, `tact-ui`, `tact_llm`,
  `sqlx`, `rmcp`, or `ratatui`.
- Every user-visible capability (agent, chat, session, compaction, providers,
  tools, commands, hooks, MCP, skills, memory, goals, tasks, teams, worktrees,
  background work, subagents, voice, notifications, TUI, headless) implemented
  outside the kernel. Kernel-routed contributions use `Registrar`; domain
  services use explicit typed constructor injection at the composition root.
- Every model-originated tool call made through `AgentService` passes through the
  kernel executor's security, hook and permission pipeline. Statically linked
  plugins are trusted in-process code, not a sandbox or security boundary.
- A TUI that does not depend on the extensions crate.
- An explicit composition root that makes every runtime dependency visible.
- Existing behavior, storage, configuration keys, marketplace packages, TUI
  rendering and headless protocol remain intact, except for the explicitly
  test-backed tool-pipeline hardening described below: revalidate PreToolUse
  rewrites and redact output after post-hook rewrites.

## Non-goals

- No dynamic library loading, subprocess plugin host, WASM host, or JSON-RPC
  protocol. A third-party code plugin must be compiled into the binary; the
  composition root is the extension seam.
- No change to the Claude/Codex marketplace package format, install layout, or
  user-facing `/plugin` commands.
- No provider behavior or prompt-semantics redesign beyond moving ownership.
- No per-domain crate split. Official domain plugins live in `tact` modules;
  `TuiPlugin` lives in `tui`, and `HeadlessPlugin` lives with the host-side view
  adapter in `tact-ui`. The kernel and extensions bundle remain the only new
  runtime crates.
- No Web or Desktop view adapter in this work.
- No TUI visual redesign.

## Architecture

### Crate layout and dependency direction

```text
crates/tact_kernel   new: plugin mechanism, catalogs, executor, ports, contracts
crates/tact          existing: official domain extensions (Agent, Tools, services, etc.)
crates/tui           View library + TuiPlugin + TUI-side ports; no dependency on tact
crates/tact-ui       composition root, CLI, admin subcommands, headless view, TUI backend adapter
crates/tact_llm      model library used by ProvidersPlugin / AgentPlugin
crates/protocol      in-process event/command DTOs (tact_protocol)
```

Dependency direction:

```text
tact_kernel  -> tact_protocol
tact         -> tact_kernel, tact_protocol, tact_llm
tui          -> tact_kernel, tact_protocol, agent_tui_kit
tact-ui      -> tact_kernel, tact, tact_llm, tui, tact_protocol
```

Hard rules enforced by `scripts/check-architecture.sh`:

- `tact_kernel` must not depend on `tact`, `tui`, `tact-ui`, `tact_llm`,
  `sqlx`, `rmcp`, or `ratatui`.
- `tui` must not depend on `tact`, `tact-ui`, or `tact_llm`.
- `tact` must not depend on `tui` or `tact-ui`.
- `tact-ui` is the only crate that depends on both `tact` and `tui`.

The existing public API of `tact` is preserved with re-exports during migration.
At the end of the migration `tact` is the official extensions bundle, not the
runtime kernel.

**Naming:** `Plugin` means an in-process code extension. An installed Claude/Codex
package is a `MarketplacePackage` in code and prose. The current `tact::plugin`
module migrates to `tact::marketplace` (with a temporary re-export if needed);
the user-facing `/plugin` commands and package format do not change.

### Kernel responsibilities

`tact_kernel` contains mechanisms that every plugin and client must share:

1. **Plugin mechanism** — `Plugin`, `PluginManifest`, `PluginHost`,
   `Registrar`, `PluginId`, install/seal lifecycle, duplicate detection,
   capability filter, diagnostics.
2. **Capability catalogs** — `ToolCatalog`, `CommandCatalog`, `PromptCatalog`,
   `ViewCatalog`, and single-slot services (`AgentService`, `ChatService`,
   `SessionService`, permission/security/hook services).
3. **Tool execution pipeline** — `ToolExecutor` with preflight and execute
   phases, one-use permission tickets, security guard, hooks, events and output
   redaction.
4. **Cross-cutting ports and mechanisms** — `EventBus`, `InteractionBroker`,
   `CancellationToken`, `Workspace`/safe-path policy, `PermissionService`,
   `SecurityService`, `HookService`. `HookService` takes neutral hook contexts
   (event, tool use/result, prompt, session/run IDs), never `Agent`/`LoopState`;
   `HooksPlugin` adapts the existing hook engine behind that port.
5. **Shared contracts** — `ToolDescriptor`/`ToolDescriptorSnapshot`, policy-only
   `ToolMetadata`, `ToolCallResult`, `ToolImage`, capability IDs, error
   categories, view/command/prompt traits.

The kernel does not contain a concrete tool, command handler, hook definition,
prompt section, agent loop, chat turn, session store, goal/task/team/worktree
store, MCP client, provider client or view.

### Plugin contract

```rust
#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn register(&self, registrar: &mut Registrar<'_>) -> anyhow::Result<()>;
    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub struct PluginManifest {
    pub id: PluginId,
    pub name: &'static str,
    pub version: &'static str,
    pub kind: PluginKind,
    pub description: &'static str,
}

pub enum PluginKind {
    Policy,
    Infrastructure,
    Runtime,
    Capability,
    View,
}
```

- Registration is synchronous and one-shot. Plugins that need async setup
  (MCP connections, database migrations, session opening) complete it in their
  async constructor before `register` is called.
- Dependencies are constructor arguments, not a runtime service locator and not
  a topological dependency graph. The compiler checks types; the composition
  root supplies dependencies in an explicit order.
- `PluginHost` retains each plugin object through the runtime lifetime. After the
  selected view returns, idempotent `Runtime::shutdown()` signals cancellation,
  withdraws/closes pending interactions, calls plugin shutdown hooks in reverse
  install order, continues after individual shutdown errors, then drops the
  runtime. The host must call it on normal exit; `Drop` only signals cancellation
  as a non-async fallback.
- Registration is transactional: contributions are staged for one plugin and
  committed only if `register` succeeds. Async `PluginHost::install` and `seal`
  shut down a failed plugin and previously installed plugins when registration or
  final validation fails. A failed install leaves no partial catalog entries and
  marks the host unusable.
- `PluginHost` records manifests and validates the final set at `seal()`; install
  errors are fatal and carry the plugin ID. Runtime tool/command errors are
  returned to the caller and do not tear down the host.
- Panic isolation is not attempted. A panic in a statically linked plugin is a
  bug and terminates the process; the runtime's `Drop` path is only a cancellation
  fallback, not a substitute for async shutdown.

`Registrar` contributions:

```rust
impl Registrar<'_> {
    pub fn tool<T: Tool + 'static>(&mut self, tool: T) -> Result<()>;
    pub fn tool_source(&mut self, source: Arc<dyn ToolSource>) -> Result<()>;
    pub fn command<T: Command + 'static>(&mut self, command: T) -> Result<()>;
    pub fn command_source(&mut self, source: Arc<dyn CommandSource>) -> Result<()>;
    pub fn prompt_section<T: PromptSection + 'static>(&mut self, section: T) -> Result<()>;
    pub fn view<T: View + 'static>(&mut self, view: T) -> Result<()>;
    pub fn permission(&mut self, service: Arc<dyn PermissionService>) -> Result<()>;
    pub fn security(&mut self, service: Arc<dyn SecurityService>) -> Result<()>;
    pub fn hooks(&mut self, service: Arc<dyn HookService>) -> Result<()>;
    pub fn agent(&mut self, service: Arc<dyn AgentService>) -> Result<()>;
    pub fn chat(&mut self, service: Arc<dyn ChatService>) -> Result<()>;
    pub fn session(&mut self, service: Arc<dyn SessionService>) -> Result<()>;
}
```

Single-slot contributions reject duplicates. The registrar consults the
`CapabilityFilter` before inserting a contribution; disabled capabilities are
skipped and recorded as diagnostics. `PluginHost::seal` fails if the required
Agent, Chat, Session or at least one View service is missing, or if a required
policy service was never registered.

### Capability catalogs

- `ToolCatalog` holds named tools and `ToolSource`s. Native tools are resolved
  by name; dynamic sources (MCP) are snapshotted per turn. `CapabilityFilter`
  filters each discovered descriptor and every later resolve, not just the
  source registration. Duplicate names in the native map are an install error;
  name conflicts between sources are reported as diagnostics and the later
  source is excluded from that snapshot.
- `CommandCatalog` holds command metadata, aliases, completion and handlers.
  Dynamic `CommandSource`s serve MCP prompts and apply the same per-command
  filter on every snapshot/resolve.
- `PromptCatalog` holds prompt sections ordered by `(priority, id)`. Duplicate
  section IDs are an install error.
- `ViewCatalog` holds view implementations by ID. The host selects one.
- Single slots (`AgentService`, `ChatService`, `SessionService`,
  `PermissionService`, `SecurityService`, `HookService`) are registered by
  plugins and read by the host/executor.

Dynamic sources use one contract; filtering happens on every snapshot:

```rust
#[async_trait]
pub trait ToolSource: Send + Sync {
    fn id(&self) -> &str;
    async fn refresh(&self) -> Result<()>;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}

#[async_trait]
pub trait CommandSource: Send + Sync {
    fn id(&self) -> &str;
    async fn refresh(&self) -> Result<()>;
    fn commands(&self) -> Vec<Arc<dyn Command>>;
}
```

`CapabilityId` is `{ kind, name }` with a stable display form
(`tool:bash`, `command:theme`, `prompt:skills`, `view:tui`). Capability-level
disablement uses the same IDs.

### Tool execution pipeline

The kernel owns the invocation path for **model-issued tool calls**. The
`AgentPlugin` receives a read-only descriptor snapshot and `ToolExecutor`, not
handler-bearing catalog entries; it cannot accidentally call a registered
handler outside the executor. This is an architectural guarantee for the Agent
path, not a sandbox: statically linked plugins are trusted in-process code and
can call operating-system APIs directly.

```rust
impl ToolExecutor {
    /// Refresh dynamic sources, then return model-facing descriptions only.
    /// No handler references are exposed.
    pub async fn descriptor_snapshot(&self) -> Result<ToolDescriptorSnapshot>;

    /// Resolve the tool, run security/hooks/policy and obtain a one-use ticket.
    pub async fn preflight(&self, request: ToolRequest) -> Result<ToolPreflight>;

    /// Consume the ticket, invoke the tool, redact outputs, and run result hooks.
    pub async fn execute(&self, prepared: ToolPreflight) -> Result<ToolCallResult>;
}

pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// Owned policy fields (permission/resources/presentation/output), with
    /// owned display strings. A static `ToolMetadata` cannot be carried here:
    /// a runtime-discovered MCP tool has no `&'static str` name or display.
    pub policy: ToolPolicy,
}

pub struct ToolDescriptorSnapshot {
    pub generation: u64,
    pub tools: Vec<ToolDescriptor>,
}
```

The Agent converts descriptors to provider-specific tool specs. The descriptor
is fully owned: model-facing name, description, schema and display label cannot
require `&'static str`, because MCP metadata is discovered at runtime. Split the
current `ToolMetadata` name/description into this owned descriptor; the owned
`ToolPolicy` carries the permission/resource/presentation/output fields, with an
owned display label. Native helper constructors may use static literals
internally, but source-facing APIs cannot require static strings. The snapshot
also carries the resource claims needed by the Agent's wave scheduler; it never
contains executable handlers. Each snapshot has a generation
ID, which the Agent includes in `ToolRequest`; preflight resolves against that
same generation or returns a stale-snapshot error and makes Agent rebuild the
model request. Dynamic MCP sources are included in a stable per-turn snapshot.

```text
resolve -> initial sensitive-target guard -> PreToolUse hook
        -> validate immutable tool identity and re-check security/risk on final input
        -> permission decision
        -> if Ask: PermissionRequest hook
             Allow -> authorize; Block -> deny; Continue -> Notification -> InteractionBroker
        -> one-use ticket bound to the final tool identity and input
        -> progress events -> tool call -> redact live output
        -> PostToolUse / PostToolUseFailure -> redact final post-hook output
        -> result events
```

The initial credential guard runs before hooks; PreToolUse may veto or rewrite
arguments but may not change tool identity or call ID. Because hooks receive
mutable input, the final input is reclassified and rechecked before permission
is granted. `PermissionRequest` is a decision-only hook (Allow/Block/Continue)
and `Notification` is observational; their adapters reject request mutation.
If any adapter attempts to mutate a request outside the PreToolUse rewrite
stage, the call is refused rather than reusing a decision for changed input. The
one-use ticket binds the resolved handler, final input and call ID, and `execute`
rejects missing, reused or mismatched tickets. A PermissionRequest hook error
remains fail-open as today; Notification errors remain observational. PostToolUse
may rewrite output, so final redaction runs after the hook. Existing
phase-specific hook failure behavior is preserved and pinned by tests.

Wave scheduling and concurrency remain in the Agent plugin; preflight remains
sequential before each wave, preserving the current rule that an earlier
"always allow" choice can govern later calls in the same turn. Tool execution
and post-hook output both pass through redaction before reaching the model or
persistent transcript.

`ToolCallContext` contains only run-scoped mechanisms:

```rust
pub enum ToolOrigin {
    Main,
    Subagent,
}

pub struct ToolCallContext {
    pub run_id: RunId,
    pub tool_use_id: String,
    pub session_id: Option<String>,
    /// Opaque binding for a bounded run; only Goal/Task extensions interpret it.
    pub active_goal_id: Option<String>,
    pub origin: ToolOrigin,
    pub permission_snapshot: Option<PermissionSnapshot>,
    pub workspace: Workspace,
    pub events: EventSink,
    pub interaction: Arc<InteractionBroker>,
    pub cancel: CancellationToken,
    pub redaction: RedactionPolicy,
}
```

No task manager, goal manager, memory manager, skill registry, teammate manager,
worktree manager, background manager, session store or UI responder is in this
context. A tool that needs a domain service holds an `Arc` to it, injected by its
plugin at construction. `SubagentPlugin` injects the parent `SessionStore`,
permission snapshot source and pending-result queue into its tools; only the
run-scoped origin and permission snapshot travel in `ToolCallContext`. This is
the replacement for `ToolContext`.

`ToolCallResult` uses a neutral image type:

```rust
pub struct ToolImage {
    pub mime_type: String,
    pub data: String,
}
```

The Agent converts `ToolImage` to the provider-specific `tact_llm::ImageSource`
at the message boundary. The kernel does not depend on `tact_llm`.

`ToolEffect` becomes a small control-plane enum:

```rust
pub enum ToolEffect {
    CompactHistory { focus: Option<String> },
    YieldTurn { reason: YieldReason },
}
```

`YieldReason` is generic (`NeedsUser`); goal-specific behavior stays in the Goal
plugin and Agent plugin, not in the kernel's type.

### Commands

```rust
#[async_trait]
pub trait Command: Send + Sync {
    fn meta(&self) -> &CommandMeta;
    async fn run(&self, ctx: &CommandContext<'_>) -> Result<CommandOutcome>;
    fn complete(&self, _ctx: &CompletionContext<'_>, _input: &str) -> Vec<Completion> {
        Vec::new()
    }
}

pub struct CommandMeta {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub usage: String,
    pub scope: CommandScope,
    pub needs_args: bool,
}

pub enum CommandScope {
    Runtime,
    View(String),
}

pub enum CommandOutcome {
    Handled,
    Text(String),
    Submit(String),
    Interaction(InteractionRequest),
    View(ViewAction),
}
```

- Commands are owned by the domain that implements them. `/goal` belongs to
  `GoalPlugin`, `/mcp` to `McpPlugin`, `/theme` to `TuiPlugin`.
- `ViewAction` is a closed enum for view-local effects (open a picker, clear the
  log, quit, copy, scroll). A command that needs arbitrary view state must be a
  view-scoped command implemented by `TuiPlugin`.
- `CommandCatalog` drives help, completion and alias resolution. TUI no longer
  has a hardcoded `SlashCommand` enum.
- Runtime commands that currently arrive as `UserCommand` variants are deleted
  as they move; the remaining `UserCommand` transport is reduced to user text,
  cancel and interaction responses.

### Prompt sections

```rust
pub trait PromptSection: Send + Sync {
    fn id(&self) -> &'static str;
    fn priority(&self) -> i32;
    fn render(&self, ctx: &PromptContext) -> Option<String>;
}
```

`PromptCatalog` orders sections by priority and ID and renders them into the
system prompt. The Agent plugin owns only the base template and the dynamic
runtime context. Skills, memory, MCP instructions, goal guidance and subagent
guidance are independent sections owned by their plugins. Section content is
computed per turn, so skill reloads, memory changes and MCP reconnects are
visible without restarting the Agent.

### Host-facing services and extension-level stores

`AgentService`, `ChatService` and `SessionService` are **kernel-owned service
contracts** because the kernel registers their slots and the View/host calls
them. Their implementations live in `tact`; their DTOs are neutral runtime
values and must not expose `tact_llm`, SQLite, or extension-private types.

```rust
pub struct TurnOptions {
    /// Opaque run binding forwarded to tools; only Goal/Task extensions interpret it.
    pub active_goal_id: Option<String>,
    pub max_turns: Option<u32>,
}

pub enum TurnStop {
    Completed,
    Yielded(YieldReason),
    MaxTurns,
    MaxTokens,
    Cancelled,
    BlockedByHook,
}

#[async_trait]
pub trait AgentService: Send + Sync {
    async fn run(
        &self,
        request: TurnRequest,
        options: TurnOptions,
        ctx: TurnContext,
    ) -> Result<TurnOutcome>;
    fn cancel(&self, run_id: &RunId);
    async fn compact(&self, request: CompactRequest) -> Result<()>;
}

#[async_trait]
pub trait ChatService: Send + Sync {
    async fn submit(&self, message: UserMessage, ctx: ChatContext) -> Result<()>;
    fn cancel(&self);
    async fn compact(&self) -> Result<()>;
}

#[async_trait]
pub trait SessionService: Send + Sync {
    async fn open(&self, request: OpenSession) -> Result<SessionHandle>;
    async fn list(&self, workspace: &Path) -> Result<Vec<SessionSummary>>;
    async fn close(&self, session: SessionHandle) -> Result<()>;
}
```

The signatures are representative; the implementation contract must preserve
all existing session operations. `SessionHandle` carries an opaque session ID
and lock epoch; `close` releases only that matching lock. `SessionPlugin` also
exposes an extension-level `SessionStore` to Agent/Compaction through typed
constructor injection. That store retains structured `Message` persistence, provider
conversation state, atomic message/provider-state replacement, token usage,
request bodies, tool-schedule records, input history and lock operations. Those
model-specific persistence types stay outside the kernel. The existing
`SessionStore` method set is the preservation baseline: the implementation plan
must map every current operation (create/ensure/touch, append/replace/load
messages, provider state, list/delete/counts, token usage, request body, tool
schedule, input history and lock acquire/release) to either `SessionService` or
the extension-level store.

- `ChatService` owns one user turn: message assembly, reset, Agent invocation,
  Stop-hook continuation and TaskComplete. It owns the Goal activation loop
  (`GoalRunner`) because that loop invokes `AgentService` with a bounded
  `TurnOptions` and consumes the detailed `TurnStop` result.
- `AgentService` owns the model loop: streaming, tool scheduling, prompt
  assembly, recovery, cancellation and compaction calls. A goal activation
  passes its `active_goal_id` and turn limit through `TurnOptions`, which are
  copied into `ToolCallContext` for goal/task tools.
- `SessionService` owns identity, open/list/close and lock release through
  opaque neutral handles; `SessionStore` owns durable transcript details.
- `CompactionService` owns micro-compaction, summary calls and transcript
  persistence. It is an extension-level trait injected into `AgentPlugin`.
- `ModelService` is an extension-level trait implemented by `ProvidersPlugin`
  and consumed by `AgentPlugin`. The kernel neither defines nor routes it.

### Views and interaction

```rust
#[async_trait]
pub trait View: Send + Sync {
    fn id(&self) -> &'static str;
    async fn run(&self, ctx: ViewContext) -> Result<()>;
}
```

`ViewContext` provides:

- an event subscription;
- `Arc<dyn ChatService>`, `Arc<dyn SessionService>`;
- `Arc<CommandCatalog>`;
- `Arc<InteractionBroker>`;
- workspace and cancellation.

`TuiPlugin` lives in `tui` and implements `View`. `HeadlessPlugin` and the
`TuiBackend` implementation live in `tact-ui` as client adapters. The host owns
client wiring, not runtime business logic.

TUI-side ports are narrow traits declared by `tui`, not `tact` types:

- skill listing/reload/enable/disable;
- model profiles, theme/language/permission/thinking/effort persistence;
- voice recording/transcription;
- marketplace requests/events;
- session history load/append;
- account updates.

`tact-ui` implements these ports over extension services. This is what removes
`tui -> tact`.

The kernel provides `InteractionBroker`, a request/response mechanism for
permission, select, multi-select, confirm and input. A view attaches as a
responder and answers by request ID. Headless attaches no responder, so
requests fail closed as unavailable. Today `RequestSelect` and
`RequestMultiSelect` carry request IDs/options only; the TUI sends
`UserCommand::UiResponse` back through the separate `UiResponder` broker. Phase 4
replaces that broker and reverse-command wiring with `InteractionBroker`; it does
not remove a response handle embedded in `AgentUpdate` because no such field
exists.

### Events and interaction transport

`tact_protocol::AgentUpdate` remains the runtime event DTO during and after the
migration. The kernel's `EventBus` fans it out to subscribers; tools and the
Agent publish, views consume. The protocol stays a contract crate and does not
depend on `tact_llm`.

The request-ID event payloads remain data-only. Phase 4 changes how pending
requests are registered and answered, while preserving request IDs, response
options and TUI behavior. Event fan-out uses per-subscriber unbounded channels,
matching current behavior; no event persistence is added. Transcript and session
persistence remain in the Compaction/Session plugins.

## Plugin inventory

### Policy and infrastructure

| Plugin | Owns | Contributes / exposes |
|---|---|---|
| `PermissionPlugin` | Permission modes, rules, allow-list, approval flow | `Arc<dyn PermissionService>` |
| `SecurityPlugin` | Sensitive-target scanner, secret redaction | `Arc<dyn SecurityService>` |
| `StoragePlugin` | SQLite pool, database lifecycle | Extension-level `DatabaseHandle` |
| `ProvidersPlugin` | `[llm]`, provider selection, account queries | Extension-level `ModelService` |
| `MarketplacePlugin` | Installed marketplace packages, install/update/list | `/plugin` commands, refresh requests to Skills/Hooks/MCP |

### Runtime

| Plugin | Owns | Contributes / exposes |
|---|---|---|
| `AgentPlugin` | Model loop, streaming, recovery, tool scheduling, prompt assembly, per-session stats | `Arc<dyn AgentService>`; `AgentFactory` for subagents |
| `CompactionPlugin` | Micro-compaction, summary compaction, transcript | `Arc<dyn CompactionService>`, compact tool and `/compact` command |
| `SessionPlugin` | Session identity, resume/list/history, lock and release | `Arc<dyn SessionService>` plus extension-level `SessionStore` |
| `ChatPlugin` | User turn lifecycle, Stop continuation, bounded Goal activation loop/`GoalRunner` | `Arc<dyn ChatService>` |

### Capability domains

| Plugin | Owns | Contributes / exposes |
|---|---|---|
| `ToolsPlugin` | `bash`, file read/write/edit/patch, `read_image`, `sleep`, `ask_user`, shell sandbox setup | Tools |
| `HooksPlugin` | Hook discovery, config, trust review, command/MCP hooks, RTK filter | `Arc<dyn HookService>` |
| `McpPlugin` | MCP servers, tools, prompts, resources, auth | `ToolSource`, `CommandSource`, MCP instructions prompt section, `/mcp` commands |
| `SkillsPlugin` | Skill discovery, enable/disable, catalog | `load_skill` tool, skills prompt section, `/skill` commands |
| `MemoryPlugin` | Repository-scoped memory | `save_memory`/`load_memory` tools, memory prompt section |
| `GoalPlugin` | Goal state machine, verification, leases and run persistence | Goal tools, `GoalService`, goal prompt section, `/goal` commands |
| `TaskPlugin` | Persistent task manager | Task tools |
| `TeamPlugin` | Teammate roster/inbox | Team tools |
| `WorktreePlugin` | Worktree lanes | Worktree tools |
| `BackgroundPlugin` | Background shell tasks | Background tools |
| `SubagentPlugin` | Subagent manager/transcripts | Subagent tools using `AgentFactory`, subagent prompt section; installed after AgentPlugin |
| `VoicePlugin` | Recording/transcription | Extension-level `VoiceService`; TUI voice commands via adapter |
| `NotificationsPlugin` | Desktop task-lifecycle notifications | EventBus subscriber injected by the host; platform-specific no-op where unsupported |

### Views

| Plugin | Owns | Contributes |
|---|---|---|
| `TuiPlugin` | Ratatui event loop, view commands, TUI-side ports | `View` (`tui`) |
| `HeadlessPlugin` | Headless event output and exit behavior | `View` (`headless`) |

Command ownership follows the same domains: `/plugin` is
`MarketplacePlugin`, `/mcp` is `McpPlugin`, `/skill` is `SkillsPlugin`, `/goal`
is `GoalPlugin`, `/hooks` is `HooksPlugin`, `/compact` is `CompactionPlugin`,
and session lifecycle commands (`/resume`, `/sessions`) are `SessionPlugin`.
View-only commands stay in `TuiPlugin`; they are registered in the catalog but
never call runtime internals directly.

The existing `stats` and transport-recovery modules are owned by
`AgentPlugin`/`SessionPlugin`; shell and sandbox setup are owned by
`ToolsPlugin`; `upgrade` remains a host admin subcommand. This accounts for the
current user-facing runtime modules without creating one-plugin-per-file.

## Composition root

`tact-ui` performs the following explicit sequence. The order below is a
dependency order, not a plugin-declared graph. Async constructors and host
install/cleanup calls are awaited; each plugin's `register` method remains
synchronous. Kernel-routed contributions are installed through `PluginHost`;
extension-private services (model client, `DatabaseHandle`, `SessionStore`,
`GoalService`) are passed through typed constructor arguments and are never
stored in a generic service locator.

1. Parse CLI, load the raw TOML once, initialize logging, resolve workspace and
   `CapabilityFilter`.
2. Create the kernel ports/catalogs (`EventBus`, `InteractionBroker`,
   `Workspace`, cancellation root) and `PluginHost`.
3. Construct/install `PermissionPlugin` and `SecurityPlugin`.
4. Construct the extension infrastructure: `StoragePlugin`, `ProvidersPlugin`
   and `MarketplacePlugin`; expose their typed service handles to dependants.
5. Install `HooksPlugin` and register its `HookService`.
6. Install `SessionPlugin` and register `SessionService`; pass its full
   extension-level `SessionStore` to Agent/Compaction. Session is available
   before Agent construction.
7. Install capability plugins that do not require an Agent: `ToolsPlugin`,
   `SkillsPlugin`, `MemoryPlugin`, `McpPlugin`, `GoalPlugin`, `TaskPlugin`,
   `TeamPlugin`, `WorktreePlugin`, `BackgroundPlugin`, `VoicePlugin` and
   `NotificationsPlugin`. Their tools and prompt sections enter dynamic
   catalogs; GoalPlugin exposes `GoalService` but does not own GoalRunner.
8. Build `ToolExecutor` from the shared catalog handles and registered
   Permission/Security/Hook services. It exposes descriptors to Agent but keeps
   handler lookup private.
9. Install `CompactionPlugin` and `AgentPlugin` with the executor, prompt
   catalog, ModelService, SessionStore, CompactionService and EventBus. The
   Agent's `TurnOptions` carry goal ID and budget; the Agent exposes detailed
   stop outcomes and `AgentFactory`.
10. Install `SubagentPlugin` after `AgentFactory`, then `ChatPlugin` with
    `AgentService` and `GoalService`. ChatPlugin owns GoalRunner and invokes
    bounded Agent turns; the catalog remains open to late tool sources.
11. Install the selected view (`TuiPlugin` or `HeadlessPlugin`), then await
    `seal()` and run `runtime.view(id).run(ViewContext)`.
12. When the view exits, cancel in-flight work and call
    `runtime.shutdown().await`; plugins stop in reverse installation order and
    session locks are released.

`MarketplacePlugin` gets a refresh coordinator whose Skills/Hooks/MCP handles
are filled after those plugins install; its `/plugin` operations remain
unavailable until the coordinator is complete.

The host also retains admin subcommands (`plugin`, `mcp`, `hooks`, `upgrade`)
and implements the TUI backend adapter. It contains no agent loop, tool
implementation, domain command handler or storage schema.

## Configuration and enablement

The host loads the TOML once into a read-only `ConfigSource`. Each plugin gets
its own typed settings value from the composition root; it does not call global
`settings()` or read another plugin's raw section.

| Section / legacy keys | Owner |
|---|---|
| Paths, logging, workspace | Host |
| `[permission]` | `PermissionPlugin` |
| `[llm]` | `ProvidersPlugin` |
| `[agent]` core loop settings | `AgentPlugin` |
| `[agent].memory_enabled`, `auto_memory_directory` | `MemoryPlugin` via a host-built compatibility projection |
| `[agent].skill_*` fields | `SkillsPlugin` via a host-built compatibility projection |
| `[agent].max_token_usage_bodies` | `SessionPlugin` via a host-built compatibility projection |
| `[agent].notifications_enabled` | `NotificationsPlugin` via a host-built compatibility projection |
| `[agent.subagent]` | `SubagentPlugin` |
| `[goal]` | `GoalPlugin` |
| `[ui]` | `TuiPlugin` |
| `[voice]` | `VoicePlugin` / TUI backend |
| `[tools]` | `ToolsPlugin` |
| `[mcp]` | `McpPlugin` |
| `[hooks]` | `HooksPlugin` |
| `[runtime]` | Kernel/host (new) |

Existing TOML paths and keys remain accepted. The host's compatibility parser
projects the current `[agent]` memory, skill, token-usage-retention and
notification fields into typed constructor values for `MemoryPlugin`,
`SkillsPlugin`, `SessionPlugin` and `NotificationsPlugin`; plugins receive those
values directly and do not read each other's settings. This handoff preserves
keys while ownership moves. `instruction_sources` remains Agent prompt
configuration.

The new `[runtime]` section carries capability disablement:

```toml
[runtime]
disabled_capabilities = ["tool:bash", "tool:mcp__docs__search", "command:theme"]
```

`CapabilityFilter` applies both at static registration and to each dynamic
`ToolSource`/`CommandSource` snapshot and resolution, so MCP refresh cannot
re-enable a disabled item. Skipped capabilities are reported in startup notices.
Unknown configured IDs are reported as unmatched diagnostics after initial
source discovery; a transient MCP failure remains non-fatal. Disabling a
single-slot capability makes `seal()` fail with a clear message rather than
silently starting without an Agent or View. Plugin-level enablement is a
composition-root decision; there is no runtime plugin toggle in this design.

During migration the existing typed `Settings` remains in `tact`; the host
builds typed per-plugin settings from it and the raw TOML. The final state
removes global `settings()` access: each plugin receives its typed settings at
construction and never reads another plugin's configuration.

## Migration

Every phase must build and keep the behavior baseline green. A phase may add a
temporary adapter, but the adapter is owned by the extension crate, has a named
deletion task, and never bypasses the kernel pipeline. Production has one path
per capability at all times.

### Phase 0 — Contracts and boundary

- Add `crates/tact_kernel` with `Plugin`, `PluginManifest`, `PluginHost`,
  `Registrar`, `CapabilityId`, `CapabilityFilter`, capability traits, catalogs,
  event/interaction contracts and architecture check.
- Move declarative contract data from `tact`: policy-only `ToolMetadata`, the
  policy enums, `ToolDomain`, presentation/output/argument-summary policies,
  `CapabilityRisk`, `ToolResources`, `ToolEffect`, owned `ToolDescriptor` and
  neutral `ToolImage`. Tool identity/description/schema move out of the current
  static metadata shape so dynamic MCP sources can use owned strings.
  `PermissionPolicy` remains data-only in the kernel; methods that inspect
  scanners, sensitive paths, shell commands or workspace paths move behind the
  extension-provided Permission/Security services because they depend on
  implementations forbidden from `tact_kernel`.
- `tact` re-exports the moved types so existing imports keep compiling.
- Add a conformance plugin in `tact_kernel/tests` that registers a fake tool,
  command, prompt section and view using only kernel/protocol APIs.
- Add `scripts/check-architecture.sh` for crate edges and
  `scripts/check-module-boundaries.sh` for the critical intra-`tact` import
  rules (Agent must not import Skills/Memory/MCP implementations directly).

Acceptance: workspace builds; all existing tests pass; the conformance plugin
registers fake required Agent/Chat/Session and policy services before `seal()`;
the dependency whitelist and module-boundary checks pass.

### Phase 1 — Tool execution pipeline

- Implement `ToolCatalog`, read-only `ToolDescriptorSnapshot`,
  `ToolExecutor::preflight/execute`, one-use tickets, `PermissionService`,
  `SecurityService`, `HookService` and `InteractionBroker`.
- Adapt the existing `PermissionManager`, security scanner/redaction and hook
  engine to the new ports. Preserve PreToolUse, PermissionRequest, Notification,
  PostToolUse and PostToolUseFailure timing and phase-specific failure behavior.
- Add `LegacyToolAdapter`, which wraps the current `tact::tool::Tool` and builds
  the old `ToolContext` from extension services. All not-yet-migrated tools
  register through it; newly migrated tools implement the kernel `Tool` trait
  directly.
- Add `McpToolSourceAdapter` in this phase. It wraps the current MCP router,
  remote tools, resource tools and prompt tools as kernel sources/tools so every
  Agent-issued native or MCP invocation goes through `ToolExecutor` from the
  first production cutover.
- Change Agent dispatch to use descriptor snapshots for model requests and
  `ToolExecutor` for preflight/execute; remove its duplicate permission/hook
  pipeline. Wave scheduling stays in Agent and uses preflight tickets.
- Preserve sensitive-credential refusal before hooks; after any permitted
  PreToolUse argument rewrite, re-resolve/reclassify and re-check the final input
  before permission. Redact both live output and final output after result hooks.

Acceptance: existing tool, permission, hook, subagent and MCP integration suites
pass; native, MCP, resource and prompt calls all use the executor; tests pin
PermissionRequest/Notification behavior, input revalidation and hook failures;
a reused/mismatched ticket is rejected; Agent receives descriptors but no
handler references; headless remains fail-closed.

### Phase 2 — Domain plugins and prompt sections

- Introduce `PromptCatalog`; move skills, memory, MCP instructions, goal and
  subagent guidance into sections. The Agent's prompt builder consumes only the
  catalog.
- Add domain plugins and move their tools/stores:
  `ToolsPlugin`, `HooksPlugin`, `SkillsPlugin`, `MemoryPlugin`, `McpPlugin`,
  `GoalPlugin`, `TaskPlugin`, `TeamPlugin`, `WorktreePlugin`,
  `BackgroundPlugin`, `VoicePlugin`, `NotificationsPlugin`.
- Replace `LegacyToolAdapter` domain by domain; delete each migrated
  `ToolContext` field when its last tool moves. Keep the temporary MCP adapter
  until `McpPlugin` registers through the native dynamic-source API, then delete
  it.
- Add native dynamic `ToolSource`/`CommandSource` registration for MCP refresh;
  replace hook contributions by source when trust changes, never append duplicate
  hook sets.
- Apply `CapabilityFilter` to each static registration and each refreshed
  `ToolSource`/`CommandSource` item; disabled items are absent from both model
  descriptors and executor resolution.

Acceptance: tool ownership matches the inventory; prompt output is equivalent;
skills/memory/MCP tests pass; MCP refresh, per-item filtering and hook trust
reload work through sources; no Agent code directly calls Skills/Memory/MCP
implementations.

### Phase 3 — Runtime plugins and host

- Promote the policy/infrastructure adapters into plugins:
  `PermissionPlugin`, `SecurityPlugin`, `StoragePlugin`, `ProvidersPlugin` and
  `MarketplacePlugin`.
- Wrap the existing runtime in `SessionPlugin`, `CompactionPlugin`,
  `AgentPlugin` and `ChatPlugin`; add the extension-level `SessionStore` contract
  and preserve all durable session operations.
- Move `bootstrap_session` and command-driver business logic out of `tact-ui`.
  `GoalPlugin` owns goal state, verification and lease/run persistence;
  `ChatPlugin` owns `GoalRunner`, invokes it with GoalService + AgentService, and
  passes goal ID/budget through `TurnOptions`.
- Install `SubagentPlugin` only after `AgentFactory` exists. Both interactive and
  headless frontends drive `ChatService` → `AgentService`.
- Introduce `PluginHost` in production; install the explicit list, seal, run the
  selected view, then call `runtime.shutdown()`.

Acceptance: interactive TUI and headless integration suites pass through the new
services; the host has no business loop; GoalRunner preserves the detailed stop,
budget, active-goal tool binding and recovery behavior; session locks and MCP
connections shut down cleanly.

### Phase 4 — Views, commands and interaction

- Decouple `tui` from `tact`: declare the TUI-side ports, implement them in
  `tact-ui`, and have `TuiPlugin` implement `View`.
- Add `HeadlessPlugin` as a `View`.
- Introduce `CommandCatalog`/`CommandRouter`; move commands to their owning
  plugins and TUI-local commands to `TuiPlugin`; delete the hardcoded
  `SlashCommand` execution enum.
- Replace `UiResponder`/`UiWiring` and the reverse `UserCommand::UiResponse`
  path with `InteractionBroker`. Keep `AgentUpdate` request events data-only
  (request ID, options and prompt); no embedded response channel is removed.
- Reduce `UserCommand` to the target transport and delete command-specific
  variants.

Acceptance: `tui` production and test targets have no `tact`, `tact-ui` or
`tact_llm` dependency; `cargo test -p tui` passes with fake TUI-side ports and
fixtures; pure Goal/Hook formatting helpers and view DTOs have moved to `tui` or
`tact_protocol`;
command help/completion comes from the catalog; permission/select/input
interactions work through the broker in TUI and fail closed in headless.

### Phase 5 — Cleanup and enforcement

- Delete `LegacyToolAdapter`, the remaining `ToolContext`, the old command enum,
  global `settings()` access, and any adapter or dual path.
- Make `tact` the official extensions bundle only; update `ARCHITECTURE.md` and
  add `docs/plugin-architecture.md`.
- Annotate `config.example.toml` sections with their owning plugin and document
  `[runtime] disabled_capabilities`.
- Update book chapters for tool, hook, config, agent loop, TUI and protocol
  behavior that changed; add a `book/26_chapter_issue_zh.html` entry for the
  user-visible capability toggle and module naming. Document the preserved
  `[agent]` compatibility keys and their plugin-owned typed projections.
- Make both crate- and module-level architecture checks part of
  `scripts/check-rust.sh` / CI; run `cargo test -p tui` with no `tact` dependency.

Acceptance: no adapter remains; production and dev/test dependency boundaries
are enforced; configuration keys remain compatible; docs and book checks are
clean.

## Testing and acceptance

- **Baseline**: `scripts/check-rust.sh` (fmt, clippy with `-D warnings`, tests)
  passes. Existing integration suites keep their behavior assertions:
  `driver_integration`, `tool_integration`, `permission_integration`,
  `recovery_compaction`, `subsystem_tools`, MCP suites, headless suites.
- **Configuration compatibility**: fixtures using current `[agent]` paths verify
  that memory, skills, token-usage retention and notification values reach their
  new owners without changing user TOML keys or defaults.
- **Kernel unit tests**: plugin install/rollback, duplicate plugin/capability,
  missing required service, static and dynamic capability filtering, seal
  failure, executor order, descriptor snapshot generation/stale rejection,
  ticket reuse/mismatch, cancellation, interaction fail-closed, reverse-order
  shutdown and shutdown-error aggregation.
- **Conformance test**: an external-style test plugin registers a tool, command,
  prompt section and view using only `tact_kernel` + `tact_protocol`; fake
  required Agent/Chat/Session/policy services let it seal, and the tool runs
  through the permission/hook/event pipeline. Agent receives descriptors but no
  handler references.
- **Architecture tests**: `scripts/check-architecture.sh` parses `cargo metadata`
  for crate edges and includes dev/test dependencies; it rejects `tui` edges to
  `tact`, `tact-ui` or `tact_llm`. A separate
  `scripts/check-module-boundaries.sh` checks the key intra-`tact` import rules;
  neither script claims that Cargo metadata alone can prove module ownership.
- **Behavior matrix**: file/Shell tools, MCP tools/prompts/resources, permission
  modes and sensitive paths, PermissionRequest/Notification hooks, PreToolUse
  argument revalidation, post-hook redaction, hook timing/veto/trust, compaction/
  recovery, goal budgets/stops/active-goal binding, session messages/provider
  state/token usage/input history/locks, tasks, teams, worktrees, background
  tasks, subagents (origin labeling, permission inheritance and result
  re-injection), notifications, voice unit-level behavior, TUI render and
  interaction tests, headless events and exit codes. `cargo test -p tui` must
  pass without `tact`, `tact-ui` or `tact_llm` normal/dev dependencies.
- **Performance**: record startup time, one-turn latency and event throughput
  before/after each phase; no material regression target. Dynamic dispatch
  already exists for tools and MCP, so the expected cost is event fan-out and
  catalog lookups.
- **Docs**: spec/plan, `ARCHITECTURE.md`, `docs/plugin-architecture.md`,
  `config.example.toml`, relevant book chapters, and `python3
  scripts/book-check.py book` clean before pushing.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| `ToolContext` is a god object with 40+ tools depending on it | `LegacyToolAdapter` first; migrate one domain at a time; delete fields with their last user |
| In-process plugins can perform OS side effects outside the kernel | Treat compiled plugins as trusted code; state clearly that the permission pipeline governs Agent-issued calls only and is not a sandbox |
| Plugins own resources that need async cleanup | Runtime retains plugin instances and awaits reverse-order `shutdown`; cancel work and release session locks before dropping services |
| Goal activation needs mutable Agent state and detailed stop outcomes | ChatPlugin owns GoalRunner; `TurnOptions` carries goal/budget, `TurnOutcome` preserves stop reasons, and ToolCallContext carries active goal |
| TUI imports `tact` for config, skills, voice, sessions, marketplace and responder | Narrow TUI-side ports; implement in host; fake ports in tests; remove `tact` from normal and dev dependencies |
| Commands are coupled to TUI state | `ViewAction` closed enum; view-scoped commands stay in `TuiPlugin`; migrate by category |
| Global `settings()` is read everywhere | Keep a compatibility parser for current key paths; pass typed projections; final split per plugin section |
| Temporary dual paths become permanent | Adapters are named in the plan with deletion tasks; production path is unique per capability |
| Hook changes or output rewrites invalidate prior checks | Bind tickets to final input, revalidate after mutating hooks, and redact after PostToolUse rewrites |
| Dynamic sources bypass static capability filtering | Apply the same filter to each source snapshot and resolve path; test MCP refresh after disabling an item |
| Cargo graph checks miss module-level coupling | Use a separate import/visibility checker for critical intra-`tact` boundaries |
| Event request callbacks leak UI into the kernel | Replace `UiResponder`/reverse response wiring with `InteractionBroker`; request events remain data-only |
| Large diff / long-lived branch | Phases are independently buildable; each phase has a behavior baseline; no big-bang merge |
| Naming collision with marketplace plugins | Runtime plugin vs marketplace package in docs and module names; `/plugin` UX unchanged |
| Performance regression from catalogs/events | Trait objects already in use; measure startup/turn/event throughput each phase |

## Deferred

- Dynamic loading, subprocess/IPC plugin hosts, WASM, Node, and language-neutral
  protocol envelopes.
- Third-party code plugins that can be installed without recompiling the binary.
- Per-domain crates beyond `tact_kernel` and `tact`.
- Web and Desktop views; a second Chat implementation.
- Provider behavior changes, prompt semantics redesign, or new marketplace
  package capabilities.
- Kernel `ModelPort`/`StoragePort`. They remain extension-level contracts until
  a kernel mechanism actually needs to route or adjudicate them.
- Capability hot reload, plugin signing, sandboxing of statically linked code.
- A general trajectory/audit store; existing transcript/session persistence is
  preserved, not re-architected.

## Review checklist

- [ ] Kernel boundary is small enough to build without extensions.
- [ ] Every capability in the inventory has an owning plugin and either a
      kernel contribution type or an explicit typed extension-service contract.
- [ ] Every Agent-issued tool call is routed through the kernel executor; the spec
      makes clear this is not a sandbox for trusted in-process plugins.
- [ ] Agent/Chat/Session port definitions live in the kernel, while their
      implementations and model-specific storage stay in extensions.
- [ ] Plugins shut down in reverse install order and release session/MCP resources.
- [ ] TUI/headless are views; the host contains only composition and adapters.
- [ ] Commands are distributed; no central command enum survives.
- [ ] Migration phases are independently shippable and testable.
- [ ] Configuration ownership is unambiguous and existing TOML key paths are
      preserved through compatibility projections.
- [ ] Dynamic MCP/ToolSource items obey the same capability filter as static
      contributions.
- [ ] Marketplace package behavior and user-facing `/plugin` UX are unchanged.
