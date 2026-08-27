# Async / Multi-Subagent Design

- **Date:** 2026-08-26
- **Status:** Design complete — permission model decided (Claude-style
  inheritance, no sandbox); async/multi-subagent mechanics specified with
  interface drafts; pending implementation plan
- **Related:** `crates/tact/src/tool/subagent.rs`; `crates/tact/src/tool/subagent_ui.rs`; `crates/tact/src/agent/tool_schedule.rs`; `crates/tact/src/agent/tool_dispatch.rs`; `crates/tact/src/permission/mod.rs`; `crates/tui/src/widgets/state/mod.rs` (single `SubagentPopup`); `crates/tui/src/render/layout.rs`; `book/12_chapter_subagent.md` + `_zh.md`; `book/14_chapter_team.md`
- **References:** Claude Code sub-agents docs (`code.claude.com/docs/en/sub-agents`); Codex subagents docs (`developers.openai.com/codex/subagents`) and multi-agent guide

## Goal

Extend Tact's subagent from a single **synchronous, blocking** subagent into a
**multi-subagent** model that supports:

1. **Async / background subagents** — `spawn_subagent` can return immediately
   with a handle; the parent continues and the result is re-injected later.
2. **Concurrent subagents** — `run_in_background` detaches the work so the
   parent is not blocked; same-wave fan-out of blocking subagents comes later
   with worktree isolation (see
   [Concurrency](#concurrency-same-turn-fan-out)).
3. **Claude-style permission inheritance** — the subagent inherits the
   parent's single `PermissionMode` + allow-list + settings instead of always
   starting in `PermissionMode::Default` (see
   [Permission model (Claude-style inheritance)](#permission-model-claude-style-inheritance)).

Both the permission model and the async/multi-subagent mechanics are specified
below with interface drafts; the async parts reuse Tact's existing
`background_run` tool pattern where possible.

## Scope

### In scope

- Claude-style permission inheritance: parent `PermissionMode` + in-session
  allow-list + loaded `PermissionSettings` flow into the subagent.
- `run_in_background` input flag with `async_launched { id }` return semantics.
- Result re-injection into the parent loop (pending queue + `sessions`
  transcript).
- `max_turns` cap on the nested loop.
- `resume` / follow-up messaging against a finished subagent session via the
  existing `sessions.ref_id` linkage.
- UI: multi-card + multi-popup for concurrent subagents, and per-subagent
  permission-prompt queuing.

### Out of scope

- Declarative agent definition files (`.tact/agents/*.md`) — deferred; see
  [Adapt](#adapt-later).
- `bypassPermissions`-style semantics — no analog in Tact's three-mode
  `PermissionMode`; out of scope.
- Per-agent **broadening** override (a subagent may restrict, never exceed, the
  parent's grant).
- `isolation: "worktree"` parallel-edit isolation — noted as a reuse candidate
  of `worktree_manager`, but not required for the first cut.
- Full "agent teams" multi-session coordination — `team` stays a data layer
  for now.

## Current State (verified against code)

- `spawn_subagent` is an `async fn` but **blocks the parent** until the nested
  `Agent::agent_loop` returns; the last assistant text block becomes the tool
  result string (`crates/tact/src/tool/subagent.rs`).
- The subagent toolset is fixed at **5 tools** (`bash`, `read_file`,
  `write_file`, `edit_file`, `sleep`) via `subagent_toolset()`; no MCP, no
  nested `spawn_subagent`, fresh static system prompt.
- Scheduling: `spawn_subagent` metadata sets `ResourcePolicy::Barrier`, so it
  is **never parallel** with other tools in the same wave
  (`crates/tact/src/agent/tool_schedule.rs`, default barrier branch).
- Parent-side risk: `PermissionPolicy::High` → always `Ask` in Default mode.
- **Subagent-side permission is hardcoded**: `spawn_subagent` builds
  `PermissionManager::try_new_with_settings(PermissionMode::Default, settings)`
  — always `Default`, a fresh allow-list (only `read_file`), and re-loads
  `PermissionSettings` from disk. It does **not** read the parent's current
  mode or in-memory allow-list.
- UI: subagent renders as one tool card (`ToolVisualKind::Subagent`); steps /
  thinking / tokens are forwarded as `AgentUpdate::ToolProgress` via
  `tagged_ui_channel_with_progress`; there is a **single**
  `app.subagent_popup: Option<SubagentPopup>`. No sticky region (the "sticky"
  concept belongs to `task_panel` only).

## Claude Code reference (mapping)

Confirmed from Claude Code docs and issue history:

- **Permission context inheritance is the default.** "Subagents inherit the
  permission context from the main conversation and can override the mode,
  except in the cases described below." Parent `auto` mode and
  `bypassPermissions` are **sticky** (frontmatter override is ignored).
- **Async dispatch:** `run_in_background: true` returns `async_launched{id}`
  immediately; final output arrives via `<task-notification>` (not streamed).
  Same-turn multiple `Task` calls run concurrently; the parent collects all
  results at once.
- **Resume / follow-up:** `resume` / `SendMessage(agent_id, msg)` reuses a
  finished subagent session (24h expiry).
- **Isolation:** per-subagent context isolation; `isolation: "worktree"` via
  `EnterWorktree`/`ExitWorktree` for parallel file edits.
- **Declarative definitions:** `.claude/agents/<name>.md` with `description`
  as the selector and a `permissionMode` field for override.

### Mapping tiers

| Tier | Claude Code feature | Tact action |
|------|---------------------|-------------|
| **Copy** | `run_in_background` async semantics; `async_launched{id}` + result re-injection | Add flag + pending-notification queue drained into the parent context (Claude's `<task-notification>`) |
| **Copy** | Permission-context inheritance (parent mode + approved tools) | See [Permission model (Claude-style inheritance)](#permission-model-claude-style-inheritance) |
| **Copy** | `maxTurns` cap | Bound the nested loop |
| **Copy** | `resume` / `SendMessage` reuse a finished session | Reuse `sessions.ref_id` (already stored) |
| **Adapt** | Concurrent same-turn subagents | Change `ResourcePolicy::Barrier` → parallelizable (or worktree-isolated) |
| **Adapt** | Declarative `.claude/agents/*.md` | Later: `.tact/agents/*.md` replacing the hardcoded static prompt + fixed toolset |
| **Adapt** | `isolation: "worktree"` | Later: reuse `worktree_manager` for parallel edits |
| **Skip** | Per-agent `permissionMode` / `bypassPermissions` / `acceptEdits` | Tact has only `Default`/`Plan`/`Auto`; out of scope |
| **Skip** | Full `background agents` / `agent teams` multi-session coordination | `team` stays a data layer |

## Codex reference

Confirmed from Codex CLI docs (`developers.openai.com/codex/subagents`), the config
reference, and the multi-agent guide (Morph, 2026-03):

- **Definition:** TOML files — `~/.codex/agents/*.toml` (personal) and
  `.codex/agents/*.toml` (project). One file = one agent. A `[agents]` block in
  `config.toml` maps a role name to `description` (selector) + `config_file`
  (path to a TOML config layer). Role keys: `model`, `model_reasoning_effort`,
  `sandbox_mode`, `approval_policy`, `developer_instructions`, `mcp_servers`,
  `skills.config`, `tools`, `personality`. **"Any setting not set in the role
  config inherits from the parent session."**
- **Dispatch:** collaboration tools `spawn_agent`, `send_input`,
  `resume_agent`, `wait_agent`, `close_agent`, plus `spawn_agents_on_csv`
  (batch fan-out). `spawn_agent` defaults to `SpawnAgentForkMode::FullHistory`.
- **Permission / sandbox — two orthogonal axes:**
  - `approval_policy`: `untrusted` / `on-request` / `never` / `on-failure`.
  - `sandbox_mode`: `read-only` / `workspace-write` / `danger-full-access`.
  - Subagents inherit the parent session's sandbox policy **and** approval
    policy; live runtime overrides (`--yolo`, `/approvals`) propagate at spawn;
    a per-role `sandbox_mode` can further restrict (e.g. `explorer` is
    read-only) regardless of what the parent allows.
  - Interactive CLI: approval requests can surface from inactive threads (with
    a source-thread label). Non-interactive (`codex exec`, batch): actions
    needing fresh approval fail and surface errors to the parent.
- **Concurrency / orchestration:** parent-orchestrated. `[agents]`
  `max_threads` (concurrent open threads), `max_depth` (nesting, root=0,
  default 1), `max_spawn_depth` (how deep `spawn_agent` stays available),
  `job_max_runtime_seconds` (default 1800). The pattern is **"spawn N, wait for
  all, consolidate"** — no inter-agent messaging.
- **Thread model (distinct):** `thread` (long task line) / `worktree` (parallel
  code workspace) / `handoff` (move the task) / `subagent` (single-task
  temporary collaborator); `resume` / `fork` are thread-level operations.
- **Built-ins:** `default` (RW fallback), `worker` (impl/fix), `explorer`
  (read-only), `monitor` (long-running / polling).
- **Status:** experimental, opt-in `multi_agent = true`, CLI-only visibility.

## Claude Code vs Codex comparison

| Dimension | Claude Code | Codex |
|-----------|-------------|-------|
| Definition format | Markdown + YAML frontmatter (`.claude/agents/*.md`) | TOML config (`~/.codex/agents/*.toml` / `.codex/agents/*.toml`) |
| Selector | `description` field | `description` key in `[agents.<name>]` |
| Dispatch surface | One `Task`/`Agent` tool, many params (`subagent_type`, `prompt`, `run_in_background`, `resume`, `isolation`, `max_turns`, …) | Primitive split: `spawn_agent` / `send_input` / `resume_agent` / `wait_agent` / `close_agent` + `spawn_agents_on_csv` |
| Permission model | Single `permissionMode` enum (7 modes); inherit + frontmatter override; parent `auto`/`bypass` sticky | **Two axes**: `sandbox_mode` (filesystem scope) × `approval_policy` (prompting); inherit + per-role override + live runtime override propagation |
| Async / background | `run_in_background: true` → `async_launched{id}`, result via `<task-notification>` (fire-and-forget) | Parent-orchestrated "spawn N, wait all, consolidate"; `wait_agent`/`monitor` polling; CSV batch |
| Concurrency | Same-turn concurrent `Task` calls; plus `agent teams` (self-coordinating, shared task list, inter-agent messaging) | Parent orchestrates fan-out; **no inter-agent messaging** (results to parent only); `max_threads`/`max_depth` caps |
| Resume / reuse | `resume` + `SendMessage(agent_id, …)`, 24h expiry | `resume_agent` + `thread`/`fork`/`handoff` model |
| Isolation | Per-subagent context; `isolation: "worktree"` (`EnterWorktree`/`ExitWorktree`) | Per-agent thread; worktree as a thread-level concept |
| Nesting | `max_depth` default 1 | `max_depth` default 1 |
| Built-ins | `general-purpose`, `Explore` (read-only), `Plan` (read-only) | `default`, `worker`, `explorer` (read-only), `monitor` |

## What the two designs mean for Tact

- **Permission model (decided 2026-08-26):** reference **Claude Code** — a
  single `PermissionMode` that the subagent inherits from the parent, with the
  parent's `Auto` mode sticky. The Codex two-axis model (`sandbox_mode` ×
  `approval_policy`) was considered and **abandoned** because it requires an OS
  sandbox. See
  [Permission model (Claude-style inheritance)](#permission-model-claude-style-inheritance).
- **Async semantics:** support **both** Claude-style true background
  (`run_in_background` → notification) and Codex-style fan-out (spawn N, wait,
  collect). The "concurrent same-turn" tier already covers Codex-style fan-out.
- **Dispatch surface:** Codex's primitive decomposition (`spawn` / `send_input`
  / `resume` / `wait` / `close`) is cleaner than Claude's single `Task` tool
  with many params. Tact may later split `spawn_subagent` into `spawn` +
  `resume` + `wait` rather than overloading one input struct.
- **Resume / reuse:** Codex's `resume_agent` + `thread`/`fork`/`handoff` is
  richer than Claude's 24h `resume`; Tact's `sessions.ref_id` supports both,
  and worktree isolation maps to Tact's existing `worktree_manager`.

## Permission model (Claude-style inheritance)

Decision (2026-08-26): **reference Claude Code.** Subagents inherit the
parent's permission context (single `PermissionMode`); no two-axis split, no
OS sandbox.

### Semantics

Claude Code's documented rule: subagents inherit the permission context from
the main conversation, and may override via a `permissionMode` field — except
that a parent's `auto` (and `bypassPermissions`) mode is **sticky** (the
override is ignored). Tact maps this to its single `PermissionMode`:

- Parent `Default` → child `Default` (ask for writes).
- Parent `Plan` → child `Plan` (read-only).
- Parent `Auto` → child `Auto` (allow everything) — sticky, no downgrade.

A subagent inherits the parent's **live permission state at spawn time**:

1. `PermissionMode` — the parent's current `Default` / `Plan` / `Auto`.
2. `always_allowed_tools` — the in-session allow-list already approved in the
   parent (e.g. an earlier "Always allow this tool").
3. `PermissionSettings` — the parent's loaded global+project rules (cloned).

The subagent's `consecutive_denials` counter resets to 0. Parent-side
`spawn_subagent` remains `PermissionPolicy::High` → `Ask` (the parent still
asks before spawning at all); inheritance governs the subagent's **internal**
mode, not the spawn prompt.

This is also a correctness/security fix: today a `Plan` (read-only) parent can
spawn a subagent in `Default` that writes files, escaping the read-only intent.

### Implementation sketch

`PermissionManager` is not `Clone` (holds mode + allow-list + denial counters +
`Option<PermissionSettings>`); `PermissionSettings` is `Clone`. Add a `Clone`
snapshot and plumb it through `ToolContext`:

```rust
// crates/tact/src/permission/mod.rs
#[derive(Clone)]
pub struct PermissionSnapshot {
    pub mode: PermissionMode,
    pub always_allowed_tools: Vec<String>,
    pub settings: Option<settings::PermissionSettings>,
}

impl PermissionManager {
    pub fn snapshot(&self) -> PermissionSnapshot { /* clone mode + list + settings */ }
    pub fn from_snapshot(s: PermissionSnapshot) -> Self {
        // consecutive_denials reset to 0; max stays 3
    }
}
```

```rust
// crates/tact/src/tool/mod.rs — ToolContext gains:
pub permission_snapshot: Option<PermissionSnapshot>,
```

Stamp per-invocation in `execute_tool_call`
(`crates/tact/src/agent/tool_dispatch.rs`) before running native tools, so the
subagent always sees the parent's **current** mode (not a launch-time stale
value):

```rust
self.tool_context.permission_snapshot =
    Some(self.runtime.permission_manager.snapshot());
```

Then `spawn_subagent` builds the child manager from the snapshot instead of
hardcoding `Default`:

```rust
// crates/tact/src/tool/subagent.rs
let pm = match ctx.permission_snapshot {
    Some(s) => PermissionManager::from_snapshot(s),
    None => PermissionManager::try_new_with_settings(
        PermissionMode::Default,
        PermissionSettings::load(&TactPath::new(&ctx.work_dir)),
    )?, // fallback for orphan/test contexts
};
```

The `None` fallback preserves today's behavior for callers that construct a
`ToolContext` without a parent agent (e.g. orphan/test harnesses).

### Out of scope

- Codex's two-axis model (`sandbox_mode` × `approval_policy`) and OS sandbox
  (Seatbelt / Landlock+seccomp) — explored and abandoned (2026-08-26).
- Per-agent `permissionMode` override / `bypassPermissions` — no declarative
  agent definitions yet; a later `.tact/agents/*.md` could add `permissionMode`
  with Claude's sticky-precedence rule.

## Async / multi-subagent mechanics

### Reuse: Tact already has the async-tool pattern

Tact's `background_run` tool (`crates/tact/src/tool/background_run.rs`) already
implements "invoke returns immediately, card stays live, completion finalizes
it" via three existing pieces:

- `LiveOutputPolicy::Background` → `ToolPresentationInfo.keep_live = true`
- `BackgroundProgressSink` streaming `AgentUpdate::ToolProgress`
- `AgentUpdate::BackgroundTaskFinished { tool_id, success, message, output }`
  to finalize the card

Async subagent reuses all three. The only genuinely new pieces are (a) the
`run_in_background` / `max_turns` / `resume` input fields, and (b) **result
re-injection into the parent's conversation context** — a background *command*
is never re-injected (the parent polls it via `check_background`), but a
subagent's summary must flow back into the parent LLM's context.

### Input (interface draft)

```rust
// crates/tact/src/tool/subagent.rs
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubagentInput {
    pub prompt: String,
    pub description: Option<String>,
    /// When true, return `async_launched { id }` immediately; the subagent
    /// keeps running and its result is re-injected into the parent context
    /// on completion.
    #[serde(default)]
    pub run_in_background: Option<bool>,
    /// Cap on nested agent-loop turns (prevents runaway subagents).
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Resume an existing subagent session id (from a prior `async_launched`).
    #[serde(default)]
    pub resume: Option<String>,
}
```

### Return (sync vs async)

- **Sync (`run_in_background` false/absent):** unchanged — block on
  `agent_loop`; tool result = last assistant text.
- **Async:** spawn a detached task, return the string
  `async_launched { id }` where `id` is the child session id; the card stays
  live (`keep_live`). Note: `ToolPresentation` is static per-tool metadata
  today — `keep_live` is fixed at `StepStarted` from `LiveOutputPolicy`.
  Keeping the card live only for `run_in_background` requires making the
  presentation input-dependent (a dynamic hook in `tool_dispatch.rs`, or
  accepting an always-live `spawn_subagent` card). Recommend resolving the
  presentation from the input's `run_in_background` at `StepStarted`.

### The subagent task

Refactor the body of `spawn_subagent` into a helper that (1) builds the child
`Agent` from the inherited permission snapshot + `subagent_toolset()`, (2) runs
`agent_loop`, (3) extracts the summary. The sync path awaits it inline; the
async path wraps it in `tokio::spawn` with a completion callback that:

- emits `AgentUpdate::SubagentFinished` (see below) to finalize the card;
- pushes the summary into the parent's pending-results queue for re-injection.

The detached task has no access to the parent's `AgentRuntime`, so the queue
handle is stamped on `ToolContext` (see next subsection). The in-flight
registry also keeps the child's `cancel_flag: Arc<AtomicBool>` so the parent
can stop it (see [Cancellation](#cancellation)).

### Result re-injection (new)

Add a pending queue to the agent runtime and drain it before each LLM call:

```rust
// crates/tact/src/agent/mod.rs — AgentRuntime gains:
pub pending_subagent_results: Arc<Mutex<VecDeque<SubagentResult>>>,

// crates/tact/src/tool/mod.rs — ToolContext gains (stamped per-invocation
// together with the permission snapshot, so the detached child task can push):
pub subagent_results: Option<Arc<Mutex<VecDeque<SubagentResult>>>>,

pub struct SubagentResult {
    pub child_id: String,
    pub summary: String,
    pub success: bool,
}
```

In `agent_loop`, before building the next request, drain the queue and append
a synthetic user message:

```text
<subagent-finished id="<child_id>" success="true|false">
<summary>
</subagent-finished>
```

so the parent LLM sees the result in its next turn. The synthetic message must
go through the normal persistence path (`append_message` on the parent
session), not just `runtime.context.push()`, so the on-disk transcript stays
consistent. Compaction must not drop pending results — drain/re-inject the
queue before a compact or re-queue after the rebuild. If the parent is idle
(no in-flight turn), the driver submits a lightweight wake-up turn that only
carries the notification (mirrors Claude's `<task-notification>`); the
completion task signals the driver through the existing `ui_tx` channel.

Re-injection is **live-only** (same-process). After a restart, finished
results are **not** re-delivered automatically — the parent transcript already
contains the `async_launched { id }` tool result, and the model decides
whether to `resume` the finished child or spawn a new subagent. This keeps the
re-run decision with the model instead of building re-delivery machinery.

Durability follows Claude Code's delivery model (`<task-notification>` into the
transcript), **plus** an explicit persisted lifecycle state (see next
subsection — the `sessions` table has no status column today, so "completed"
cannot be recovered from the transcript alone):

- The child's full transcript is persisted in the `sessions` table
  (`ensure_session_row` + `load_session(child_id)` + `ref_id`).
- The pending queue (`pending_subagent_results`) is in-memory and is drained
  before the next turn, mirroring Claude's pending `<task-notification>`
  queue (which flushes on the next interaction — see Claude Code issue #39335).
- The `team` inbox (`book/14_chapter_team.md`) is **not** the result path; it
  is for cross-agent coordination (Claude's "agent teams" direct messaging +
  shared task list).

### Subagent state & lifecycle (persisted)

The `sessions` table has **no status column** (only `id`, timestamps,
`root_dir`, `locked_by`, `lock_epoch`, `ref_id`), so a restart cannot tell
"still running" from "finished" — the gap the in-memory queue alone leaves
open.

Why stateful (not Claude's transcript-only model): Claude's subagents are
**detached first-class sessions**, so "transcript is the state" works there and
recovery is resume/replay. Tact's `spawn_subagent` is a **tool**, and Tact's
tool layer is already stateful — `background_run` persists
`BackgroundTaskStatus { Running, Completed, Error }` +
`BackgroundTaskRecord { finished_at, output }` and, on startup, rewrites any
`Running` record to `Error` ("Process interrupted (agent restarted)"). A tool
has a lifecycle, so it needs persisted state; mirror `background_run`.

Add a dedicated `subagent_runs` table (cleaner than overloading the generic
`sessions` table), keyed by child session id:

```rust
pub enum SubagentStatus { Running, Completed, Failed, Cancelled }

pub struct SubagentRun {
    pub child_id: String,         // == session id; `ref_id` links to parent
    pub status: SubagentStatus,
    pub summary: Option<String>,  // final assistant summary
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}
```

Lifecycle:

1. Async spawn → insert `Running`.
2. Completion → set `Completed` / `Failed` (+ `summary`, `finished_at`).
3. Parent restart → **orphan repair**: any `Running` row becomes `Failed`
   ("Process interrupted (agent restarted)"). Finished results are **not**
   automatically re-injected — the model decides what to do (spawn a new
   subagent, or `resume` the finished child), keeping the re-run logic in the
   model's control.

Scope note: the async child is a `tokio::spawn` task **inside the parent
process** — it does not outlive the process (unlike Claude's detached
background agents). "Async" decouples the parent's *turn*, not the process
lifetime; orphan repair is the recovery path for a process that died mid-run.

The in-memory queue is the **live fast path** (immediate same-process
re-injection); the persisted `subagent_runs` state is the **source of truth**
for crash recovery (orphan repair) and a record the model can consult via
`resume` — there is no automatic cross-restart re-delivery.

### New protocol event (interface draft)

```rust
// crates/protocol/src/agent.rs — AgentUpdate gains:
SubagentFinished {
    tool_id: String,   // parent tool-card to finalize
    child_id: String,  // child session id
    success: bool,
    summary: String,   // one-line; full transcript stays in the popup
}
```

This is the subagent analog of `BackgroundTaskFinished`; the TUI reuses the
same "finalize a `keep_live` card" path. `ToolMeta` keeps updating the card
header (model + tokens) while the subagent runs.

### Cancellation

The in-flight registry stores each child's `cancel_flag: Arc<AtomicBool>`
alongside its run record. Cancelling = setting the flag; the child's agent
loop exits cooperatively at its next checkpoint and `subagent_runs` transitions
to `Cancelled`. A user-facing cancel surface (a `cancel_subagent` tool, or
extending `/cancel` to list/stop background subagents — Claude's `/tasks`,
Codex's `close_agent`) is follow-up; the first cut stores the handle so
cancellation is cheap to add later. Until then the parent can still stop a
runaway child by exiting (orphan repair marks it `Failed`).

### Concurrency (same-turn fan-out)

- **First cut (shared filesystem):** `ResourcePolicy::SharedState { scope: "subagent" }`
  — *sync* subagents serialize with each other (their effects cannot be
  scoped), but may still overlap disjoint file reads. *Async* subagents are
  effectively parallel regardless of wave scheduling: each spawn returns
  immediately and the real work runs in detached tasks. So the first cut
  delivers "parent not blocked + background parallelism", **not** same-wave
  fan-out of blocking subagents.
- **`isolation: "worktree"` (follow-up):** each subagent gets its own worktree
  via `worktree_manager`, so `ResourcePolicy::Independent` becomes safe and N
  *sync* subagents fan out in the same wave.

### `max_turns` and `resume`

- `max_turns` bounds the nested `agent_loop`; on exhaustion, finalize with a
  truncated summary (`… (max_turns reached)`).
- `resume = Some(child_id)` loads the finished session via
  `store.load_session(child_id)` **plus `load_provider_state(child_id)`** (the
  Responses-protocol input-item baseline — required for OpenAI-compatible
  providers), acquires the session lock via `try_lock_session` so two runs
  cannot operate on the same child session, seeds the child context, then runs
  one more turn with `prompt` as the follow-up (Claude's `SendMessage`
  analog). `sessions.ref_id` already links child → parent; expiry policy TBD.

## UI upgrade required

The current UI is single-card + single `Option<SubagentPopup>`. Concurrent
fan-out needs:

- **Multi-card:** one card per dispatched subagent (already natural — each is a
  tool call with its own `tool_id`).
- **Multi-popup:** replace `Option<SubagentPopup>` with a map
  `HashMap<tool_id, SubagentPopup>` (or a stack/switchable set). The popup
  geometry still derives from the main area (AGENTS.md invariant 4).
- **Per-subagent permission prompts:** when several subagents ask for
  permission concurrently, `RequestSelect`/`RequestMultiSelect` already carry a
  `[Subagent]` prefix and go through the shared `ui_responder`; queue/order
  them per subagent rather than blocking on a single global popup.

This is a popup-count upgrade, **not** a new sticky region — consistent with
the doc correction already applied (subagent output is a Log card + popup, not
a sticky tab).

## Interaction and data flow

1. Parent tool loop reaches a `spawn_subagent` wave.
2. Permission snapshot is stamped from the parent manager (current mode +
   allow-list + settings).
3. Each subagent is constructed from that snapshot and its own session store.
4. Sync subagents block; async subagents `tokio::spawn` a detached task, are
   tracked in the agent runtime's in-flight set, and return `async_launched{id}`
   with a `keep_live` card.
5. Progress streams into each subagent's card via tagged `ToolProgress`
   (`ToolMeta` updates the header).
6. On completion the task updates `subagent_runs` to `Completed`/`Failed`,
   emits `AgentUpdate::SubagentFinished` (finalizes the card), and pushes a
   `SubagentResult` into the parent's `pending_subagent_results`.
7. Before the parent's next LLM call, `agent_loop` drains the queue and
   appends a `<subagent-finished id=…>` message through `append_message`; the
   parent may then act on it or `resume` the child for follow-up. On restart,
   orphan `Running` runs are marked `Failed`; finished results are not
   re-delivered — the model re-spawns or resumes as it sees fit.

## Testing

- Permission inheritance unit tests: parent `Default`/`Plan`/`Auto` → child
  same mode; parent allow-list preserved; `consecutive_denials` reset.
- `from_snapshot`/`snapshot` round-trip with and without settings.
- A `Plan`-mode parent produces a read-only subagent (regression for the
  read-only-escape fix).
- Async: `run_in_background: true` returns `async_launched{id}` without
  blocking; the detached task emits `SubagentFinished` exactly once; the
  pending queue is drained before the next LLM call and produces one synthetic
  `<subagent-finished>` message persisted via `append_message`.
- State persistence: `subagent_runs` transitions `Running` → `Completed`/
  `Failed`; orphan repair on startup rewrites `Running` → `Failed`
  ("Process interrupted (agent restarted)") and does **not** re-inject
  finished results (mirror `background`'s
  `marks_stale_running_tasks_on_startup` test).
- `max_turns` bounds a runaway subagent (finishes with a truncated summary).
- `resume` loads an existing child session via `load_session` +
  `load_provider_state`, acquires `try_lock_session`, and continues it.
- Cancellation: setting the stored `cancel_flag` transitions the child to
  `Cancelled` and finalizes its card.
- Concurrency: `ResourcePolicy::SharedState { scope: "subagent" }` serializes
  same-turn subagents but overlaps disjoint file reads (mirror the existing
  `task_tools_serialize_with_each_other` test).
- UI: multi-popup rendering test asserting blank cells carry `theme.bg` (per
  AGENTS.md buffer-level test pattern).

## Documentation

On approval, sync `book/12_chapter_subagent.md` + `_zh.md` (permission
inheritance and async semantics), and append a newest-first Ch 26 issue-log
entry in both languages when the user-visible behavior ships. The async design
plan (`docs/superpowers/plans/2026-08-26-async-subagent.md`) is written before
or with implementation.
