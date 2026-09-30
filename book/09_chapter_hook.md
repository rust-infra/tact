# Agent Lifecycle Hooks
> Language: [English](./09_chapter_hook.md) · [中文](./09_chapter_hook_zh.md)

This chapter explains how Tact lets you inject custom logic around tool execution: inspecting or rewriting tool input before a call, rewriting output after it finishes, and (via the registration API) preparing state before a session begins.

Hooks are the extension point between the **agent loop** and the **tool scheduler**. They run sequentially and can **veto** an operation by returning `HookControl::Block`.

---

## 1. Why Hooks Exist

Not every policy belongs in a tool implementation or in `PermissionManager`:

- **Cross-cutting guards** — block dangerous argument patterns before any tool runs.
- **Input normalisation** — rewrite paths, inject defaults, or strip fields the model often gets wrong.
- **Output shaping** — truncate, redact secrets, or attach metadata to tool results before they enter context.
- **Integrations** — emit metrics, audit logs, or sync external systems without forking every tool.

Hooks keep those concerns out of the core scheduler while still running at predictable points in the pipeline.

---

## 2. Hook Types

Defined in `crates/tact/src/hook/mod.rs`:

| Hook | Registration | Invoked today? | Can mutate | Can veto |
|------|----------------|----------------|------------|----------|
| `SessionStart` | `Agent::session_start` | Yes — once per session, after initialization (`dispatch_session_start_hooks`); what it collects is recorded before the first turn | `&mut SessionStartContext` (append injected context) | Yes |
| `UserPromptSubmit` | `Agent::user_prompt_submit` | Yes — when a user turn message enters `agent_loop` | prompt text (append `additionalContext`) | Yes |
| `PreToolUse` | `Agent::pre_tool` | Yes — before permission check, per tool in order | `ToolUse` input (`name`, `input` JSON) | Yes |
| `PostToolUse` | `Agent::post_tool` | Yes — after each tool finishes, as results stream in | `ToolResult` content | Yes |
| `PostToolUseFailure` | `Agent::post_tool_failure` | Yes — after a tool **fails**, in addition to `PostToolUse` | read-only `LoopState` + `ToolUse` + error text | log-only (tool already failed) |
| `Notification` | `Agent::notification` | Yes — when the agent surfaces a user notification (only `permission_prompt`) | read-only `LoopState` + `NotificationContext` | log-only (observational) |
| `TaskCompleted` | `Agent::task_completed` | Yes — once per completed user task (`dispatch_task_completed_hooks`) | read-only `LoopState` | log-only (task already done) |
| `Stop` | `Agent::stop` | Yes — once at the outer turn boundary (`dispatch_stop_hooks`) | read-only `LoopState` | Yes — `Block(reason)` means *continue* the turn with `reason` as the next prompt |
| `SessionEnd` | `Agent::session_end` | Yes — once at teardown (`dispatch_session_end_hooks`) | read-only `LoopState` | log-only (session is ending) |
| `PreCompact` | `Agent::pre_compact` | Yes — before `compact_history` routes to native/local | read-only `LoopState` + `CompactTrigger` | Yes — `Block` vetoes the compaction |
| `PostCompact` | `Agent::post_compact` | Yes — after a successful compaction, once per route | read-only `LoopState` + `CompactTrigger` | log-only (already committed) |

`SubagentStart` and `SubagentStop` are **standalone** hook traits (`SubagentStartFn` / `SubagentStopFn`), not `Agent.hook` variants: `spawn_subagent` is a tool handler without a parent `Agent` handle, so the closures live on `ToolContext.subagent_start_hooks` / `ToolContext.subagent_stop_hooks` and are invoked by the spawn path. `SubagentStart` mutates the child's system prompt; `SubagentStop` runs after the child finishes and may rewrite the summary fed back to the parent.

`Stop` is the one event where `Block` inverts meaning: the "operation" being blocked is the stop itself, so `Block(reason)` means *continue* the turn (Codex continuation-fragment semantics), not veto. All other hooks use `Block` to veto.

`LoopState` is a type alias for `Agent`, so session hooks see the same runtime the loop uses (context, stats, tool routers, etc.).

---

## 3. Control Flow: `HookControl`

Every hook returns one of:

```rust
pub enum HookControl {
    Continue,
    Allow,
    Block(String),
}
```

| Result | Meaning |
|--------|---------|
| `Continue` | Run the next hook of the same type, then proceed with the pipeline. |
| `Allow` | The hook answered the approval prompt itself (Codex's `permissionDecision: "allow"`). Only `PreToolUse` and `PermissionRequest` act on it; every other event treats it as `Continue`. |
| `Block(reason)` | Stop the hook chain immediately; the tool step is treated as failed with `reason`. |

**A block always wins.** The macro keeps scanning after an `Allow` so a later hook can still refuse, and a refusal is never undone by another hook's approval — the same `deny > allow` precedence the permission rules use. For an event that has no prompt to answer, `Allow` is a no-op rather than an error, so one shared hook file can serve several events.

For `PreToolUse`, a block skips execution and permission prompts — the model still receives a `ToolResult` explaining why the call was blocked.

For `PostToolUse`, a block replaces the successful tool output with a failure message before the result is appended to context.

If a hook returns `Err(...)`, the agent treats it like a block with a generic failure message (`PreToolUse hook failed: …` / `PostToolUse hook failed: …`).

---

## 4. Where Hooks Sit in the Turn Pipeline

Hooks wrap the parallel core described in [Tasks and Tool Scheduling](./11_chapter_task.md):

```text
For each ToolUse in the assistant message (Phase 1 — sequential):
  StepAdded / StepStarted
  ──► PreToolUse hooks (sequential, can mutate input or Block)
  ──► PermissionManager
  ──► mark tool as Run or Resolved (blocked/denied)

Phase 2 — parallel waves (no hooks here)

For each tool that finishes (still sequential per completion):
  ──► PostToolUse hooks (sequential, can mutate content or Block)
  ──► StepFinished UI event
  ──► append ToolResult to context (Phase 3)
```

Important details:

1. **PreToolUse runs before permissions** — hooks can rewrite input that permissions then evaluate.
2. **PreToolUse is strictly ordered** — one tool at a time, in the model's emission order.
3. **PostToolUse runs per completed tool** — as each future in a wave resolves, not after the whole wave joins. Hooks still run one completion at a time on the agent task.
4. **Parallel tools do not share hook state** — each invocation gets its own `ToolUse` / `ToolResult` copies.

---

## 5. Core Types

```rust
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}
```

Hooks are stored on the agent as trait objects:

```rust
pub enum Hook {
    SessionStart(Box<dyn SessionStartFn>),
    PreToolUse(Box<dyn PreToolUseFn>),
    PostToolUse(Box<dyn PostToolUseFn>),
}
```

Closures can be registered directly — the traits are implemented for any `Send + Sync` async closure with the right signature.

---

## 6. Registering Hooks

On `Agent` (`crates/tact/src/agent/mod.rs`):

```rust
agent.pre_tool(|agent, tool_use| {
    Box::pin(async move {
        if tool_use.name == "bash" {
            let cmd = tool_use.input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd.contains("curl") {
                return Ok(HookControl::Block("curl is disabled in this workspace".into()));
            }
        }
        Ok(HookControl::Continue)
    })
});

agent.post_tool(|_agent, tool_use, tool_result| {
    Box::pin(async move {
        if tool_use.name == "bash" && tool_result.content.len() > 50_000 {
            tool_result.content.truncate(50_000);
            tool_result.content.push_str("\n… (truncated by hook)");
        }
        Ok(HookControl::Continue)
    })
});
```

Hooks are appended to `Agent.hooks` in registration order and executed in that order for each invocation.

Multiple hooks of the same type compose: all must return `Continue` unless one `Block`s (first block wins).

### Command hooks

A hook comes from one of six places, and **all matching hooks run** — a higher layer never replaces a lower one, exactly as Codex layers user, project and managed hooks:

| Origin | Path | `${PLUGIN_ROOT}` | `${PLUGIN_DATA}` |
|---|---|---|---|
| user file | `~/.tact/hooks.json` | the file's directory | `~/.tact` |
| project file | `<workdir>/.tact/hooks.json` | the file's directory | `<workdir>/.tact` |
| installed plugin | bundle `hooks/hooks.json`, or the manifest's inline `hooks` map | bundle root | the plugin's data directory |
| user config | `~/.tact/config.toml`, `[hooks]` table | the file's directory | `~/.tact` |
| project config | `<workdir>/config.toml`, `[hooks]` table | the file's directory | `<workdir>/.tact` |
| project config | `<workdir>/.tact/config.toml`, `[hooks]` table | the file's directory | `<workdir>/.tact` |

They are registered in that order — plugins, user file, project file, then the `config.toml` tables — after any Rust closures. The order matters only for which `SessionStart` context is concatenated first, and appending is what keeps every existing order intact, so a new entry point cannot reorder a plugin's or a user file's context.

**A `[hooks]` table is a third spelling, not a third mechanism.** It deserialises into the same `HooksFile` the JSON files use, so the fields, the validation and the review are one implementation:

```toml
# ~/.tact/config.toml
[[hooks.PreToolUse]]
matcher = "bash"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "policy.sh"
timeout = 5
```

Each config file that declares `[hooks]` becomes its **own** source, labelled with its path — never merged, even though the config loaders merge those same files' other values. A hook is reviewed by identity and the review has to name the file it came from; merging would also make "approve this file's hooks" impossible. Because the collector reuses the same loader, every property holds unchanged: an entry starts **unreviewed** and is **never registered** until approved, so a repository-supplied `config.toml` cannot execute anything by being cloned. A file with no `[hooks]` table contributes no source at all, and a malformed one is warned about and skipped, exactly like a malformed `hooks.json`.

The file name is `hooks.json` (Codex's), not `.hooks.json`: a file copied out of `bm hook install --harness codex` works as-is. Tact still never reads `~/.codex/`.

An entry declares one of two **kinds**, and everything downstream is shared: the same output normalization, the same decision contract (`decision` / `hookSpecificOutput` / a bare `exit 2`), the same `additionalContextLimit`.

| `type` | What it runs | Fields |
|---|---|---|
| `"command"` (or absent) | a shell command, with the JSON payload on stdin | `command`, `timeout`, `async`, … |
| `"mcp_tool"` | a tool on a **connected MCP server** — reached through the same `MCPToolRouter` the agent uses | `server`, `tool`, `arguments` |

```json
{ "type": "mcp_tool", "server": "policy", "tool": "gate",
  "arguments": { "path": "secrets/.env" } }
```

The point of the second kind is that a policy can live in the MCP server that already holds an integration's tools, returning the same `{"decision": …}` / `{"hookSpecificOutput": …}` shapes a shell script would — instead of a script that re-implements the payload, the decision contract and `additionalContext` parsing in whatever language it is written in. A tool that answers `{"decision":"block","reason":…}` blocks; one that answers `additionalContext` injects context.

`arguments` is **static**: the hook payload is deliberately not merged into it. A tool call whose input shifted with the event would make the reviewed definition a lie, and the definition is what the user approved.

An unreachable server, an unknown tool or a tool error is reported and **continues**, matching every other hook failure — a hook must not be able to halt the loop by being broken.

Because the identity hash and the review listing both read the definition, an `mcp_tool` entry is identified by `mcp_tool <server>/<tool> <arguments>`. Hashing the (absent) `command` would have given every `mcp_tool` entry in one source the same identity — approving one would approve the rest, editing `tool` or `arguments` would not invalidate an approval, and the review would show a blank line where the definition should be. An entry missing `server` or `tool` is reported as unrunnable rather than silently inert.

### Reviewing a hook before it runs

A hook file is executable configuration, and a repository can ship `.tact/hooks.json` — so cloning a repo must not run its commands. Every hook definition starts **unreviewed**, and an unreviewed hook is **never registered**: the file is read, and then refused. This is Codex's `trusted_hash` model with Tact's own store.

- **Identity** is the definition: source label, event, matcher and command, hashed with SHA-256. Editing the command invalidates the approval.
- **The store** is `~/.tact/hooks-state.json` (`{"version":1,"trusted":{hash:description}}`). It is deliberately not `config.toml`: a hand-edited config must not be able to grant execution. A store that cannot be parsed is an empty one, so everything returns to review.
- **Reviewing** is `tact-ui hooks list` (what is configured, and its status), then `tact-ui hooks trust --all` or `tact-ui hooks trust --source <label>`. `tact-ui hooks forget --all` revokes everything. The same review is reachable where the hooks fire: `/hooks list`, `/hooks trust --all`, `/hooks trust --source <label>`, `/hooks forget --all` in the TUI, with the identical wording, and `/hooks list` is idle-only because the driver serializes non-fast commands behind an in-flight turn. The TUI spelling requires the same explicit `--all` / `--source`; there is no "approve everything" shortcut.
- **Applying** happens when hooks are registered, so approval takes effect from the next session; a running session keeps the decisions it started with.
- **Telling the reader** is never skipped: unreviewed hooks are named on the `AgentUpdate::Info` channel in the TUI and on stderr (`[hooks] …`) in headless mode, using the same two channels the MCP load report uses.

The output contract is **Codex's** (`codex-rs/hooks`): `decision` / `reason`, `hookSpecificOutput.additionalContext`, `suppressOutput`, `continue`, and the `command` handler are what Tact models and honours. Claude-only outputs are deliberately not implemented — there is no `systemPrompt` handling here, because no plugin can legally emit one (Claude's SessionStart documents `additionalContext` / `initialUserMessage` / `watchPaths` / `sessionTitle` / `reloadSkills`, and Codex's schema has exactly `hookEventName` + `additionalContext`).

Installed marketplace plugins can declare command hooks through `.codex-plugin/plugin.json` (`"hooks": "./hooks/hooks.json"`). `apply_plugin_hooks_with_report` (in `crates/tact/src/plugin/hooks.rs`) registers the reviewed ones on the `Agent` builder in `interactive.rs` / `headless.rs` for thirteen of the fifteen mapped events (`SubagentStart` / `SubagentStop` instead come from `plugin_subagent_*_hooks`, which build `ToolContext` closures):

- `SessionStart` — matcher is matched against the real `source` (`startup` / `resume` / `compact`); `additionalContext` (JSON, or plain stdout — the shape the reference `basic-memory` plugin prints its briefing in) is recorded as a synthetic `<hook-context>` user message before the first turn; `continue: false` skips the turn, since that schema has no `decision`.
- `UserPromptSubmit` — matcher against the prompt text; `additionalContext` output is appended to the user prompt.
- `PreToolUse` — matcher against the tool name; `additionalContext` is recorded as conversation context before the next request; `block` prevents execution, and `permissionDecision: "allow"` runs the call without asking.
- `PermissionRequest` — runs only when Tact was **about to ask for approval** (matcher against the tool name), so a policy hook that approves does not pay for calls that need no approval: `allow` skips the prompt, `block` denies with its reason, anything else leaves the decision to the user.
- `Interrupt` — observational, matcher against the literal `interrupt`; fires once per turn when the user cancels (`/cancel`), for logging or flushing.
- `PostToolUse` — matcher against the tool name; `additionalContext` is recorded as conversation context before the next request; `suppressOutput` clears the result; `block` turns it into a failure.
- `PostToolUseFailure` — matcher against the tool name; observational (`tool_name`, `tool_input`, `tool_use_id`, `error`).
- `Notification` — matcher against the notification type (`permission_prompt`); observational (`notification_type`, `title`, `message`).
- `TaskCompleted` — matcher ignored (parity); observational (`task_description` = last assistant message).
- `SubagentStart` — `plugin_subagent_start_hooks` builds `ToolContext` closures; `additionalContext` is appended to the child system prompt.
- `SubagentStop` — `plugin_subagent_stop_hooks` builds `ToolContext` closures; runs after the child finishes; observational (a `block` cannot resume a finished child).
- `Stop` — matcher ignored (Codex parity); a `block` continues the turn with the `reason` as the next prompt.
- `SessionEnd` — matcher against `reason` (`"other"`); observational.
- `PreCompact` — matcher against the trigger string (`auto`/`manual`/`recovery`/`command`); a `block` vetoes the compaction.
- `PostCompact` — matcher against the trigger string; observational.

Each hook entry is a shell command (`sh -c` on Unix, `commandWindows` ignored for now) run with `CLAUDE_PLUGIN_ROOT` / `CLAUDE_PROJECT_DIR` env vars, the Claude input JSON on stdin (`session_id`, `transcript_path`, `cwd`, `hook_event_name`, event fields), and stdout JSON parsed in both the newer `decision` / `reason` / `additionalContext` format and the legacy `hookSpecificOutput` format. `timeout` defaults to 60s, `async: true` fire-and-forgets, and `additionalContextLimit` bounds what one hook may inject (in tokens), applied where the context is produced rather than at whatever the session allows.

**`exit 2` is the simple block contract.** The reason is the hook's **stderr** text, and what it blocks is per event, as Codex defines it:

| Event | `exit 2` means |
|---|---|
| `PreToolUse`, `PermissionRequest` | block, `stderr` is the reason |
| `PostToolUse` | the tool result becomes a failure carrying `stderr` |
| `Stop`, `SubagentStop`, `UserPromptSubmit` | continue with `stderr` as the next prompt |
| everything else | fail-open, but reported |

A JSON decision always wins over a bare `exit 2`: a hook that printed one already said what it meant, and only a hook that decided *nothing* falls back to its exit status. `additionalContext` and `systemMessage` count as additions, not decisions, so they can ride along with `exit 2`.

Every other failure — a timeout, a spawn error, a non-zero exit that is not `2`, JSON that looks like JSON but does not parse — still logs a warning, surfaces `[plugin hook <Event> failed] …`, and **continues** (fail-open, matching Claude Code).

Hooks execute in declaration order after any Rust closures registered earlier; a `Block` short-circuits.

Stacking several sources makes that order concrete. Plugins are visited in `<marketplace>/<plugin>` key order — the order of `installed.json`'s `BTreeMap`, i.e. lexicographic, **not** installation time — and within one plugin in the order its hooks file (or inline manifest map) declares matchers for that event. There is no priority field: cross-plugin order is fixed by that key, while the order you control is the declaration order inside one plugin.

---

## 7. The `invoke_hooks!` Macro

Defined in `crates/tact/src/hook/mod.rs` and exported at the crate root:

```rust
invoke_hooks!(PreToolUse, self, &mut tool_use)
invoke_hooks!(PostToolUse, self, &tool_use, &mut tool_result)
```

Behaviour:

1. Start with `HookControl::Continue`.
2. Filter `self.hooks` to the requested `HookTypes` variant.
3. Await each hook in registration order.
4. On the first `Block`, stop and return that control value.
5. Propagate errors with `?`.

Call sites live in `crates/tact/src/agent/tool_dispatch.rs` inside `Agent::execute_tool_call`.

---

## 8. PreToolUse in Detail

**When:** Phase 1 of `execute_tool_call`, once per `ContentBlock::ToolUse`.

**Order relative to other pre-flight work:**

```text
stats.tool_counts += 1
cancel check
StepAdded / StepStarted
PreToolUse  ◄── hooks
PermissionManager::check
PreparedState::Run | Resolved(blocked message)
```

**Mutating input:** Because `tool_use` is `&mut ToolUse`, a hook can change `input` before permissions and execution see it. The mutated JSON is what gets scheduled and logged.

**Blocking:** On `Block`, the agent sets `PreparedState::Resolved(msg)` — the tool never enters the scheduler. The model still gets a matching `ToolResult` for protocol correctness.

---

## 9. PostToolUse in Detail

**When:** Inside the wave execution loop, immediately after a native or MCP tool returns and before `StepFinished` is emitted.

**Typical uses:**

- Redact API keys or tokens from command output.
- Normalise error strings for the model.
- Attach structured prefixes (`[cached]`, `[retry 2/3]`, etc.).

**Blocking after success:** If the tool returned `StepStatus::Success` but a hook blocks, the UI and context see a failed step with the hook's reason.

---

## 10. SessionStart

`Agent::session_start` accepts hooks with signature:

```rust
Fn(&LoopState, &mut SessionStartContext) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + '_>>
```

They run **once per session, on the first turn** — not at startup. A plugin hook is a subprocess that can take seconds (the reference `basic-memory` hook measured ~8s warm and ~100s on a cold `uv` cache), and waiting for it before the first frame delayed every launch. `AgentRuntime::session_start_hooks_pending` gates the dispatch so the hooks run exactly once, and the hooks themselves run **concurrently**, with registration order preserved in what they collect.

Context a hook collects lands on `AgentRuntime::pending_session_context` rather than going straight into the conversation: `dispatch_session_start_hooks` runs before `ensure_session`, and `push_message` there would leave the context non-empty and suppress the history restore. `agent_loop` drains it **after any pre-turn compaction** and before the turn's user message, recording each chunk as its own synthetic `<hook-context>` user message carrying `MessageKind::HookContext`. It has to be after the compaction: `build_compacted_history` keeps only real user turns, so a cell injected earlier would be dropped by the very compaction it arrived with.

A chunk over 2,500 approximate tokens is written in full under `<temp_dir>/hook_outputs/<session>/` and replaced with a head/tail preview plus `Full hook output saved to: <path>` (Codex's `HookOutputSpiller` and its default limit); the TUI still shows the hook's full text.

The plugin path also fills in the two fields Codex's `session-start.command.input` schema requires beyond Claude's base set — `model` and `permission_mode` (the latter in the Claude Code vocabulary Tact maps onto: `default` / `plan` / `acceptEdits`) — and surfaces the plugin's own `statusMessage` through `AgentUpdate::Info`, so a hook that takes seconds is visible rather than silent.

That placement and the one-message-per-chunk rule match Codex, whose `SessionStart` handler records each `additionalContext` as its own `developer` role message and whose start hooks run after `run_pre_sampling_compact`. Tact's message model has only user/assistant, so the `<hook-context>` markers carry the provenance instead — and unlike the in-memory kind, they survive a reload. Stdout that looks like JSON but does not parse is treated as a failed hook rather than injected, matching Codex's `looks_like_json` check.

The matcher is matched against the **real** `source`: `startup` for a fresh session, `resume` when `ensure_session` restored history, and `compact` when a compaction re-queued the hooks. That last one is how a plugin re-orients after its context was summarized away — the reference `basic-memory` plugin asks for an authored checkpoint this way — and it costs a hook run per compaction, exactly as it does in Codex.

A hook that returns `continue: false` on `SessionStart` **skips the turn**: `agent_loop` returns before the user message is pushed, matching Codex's `return Ok(None)`. Its `stopReason` is what the reader sees. `decision: block` is not part of that schema, so it is not a way to stop a session.

### The hook payload

Every event's stdin payload carries the fields Codex's schemas require, so a plugin written against them does not read `null`:

| Field | Value |
|---|---|
| `session_id` | The live session id |
| `cwd`, `hook_event_name` | As before |
| `model` | `Agent::model()` — the current model, following `/model` |
| `permission_mode` | Claude Code's vocabulary: `default` / `plan` / `acceptEdits` |
| `turn_id` | `Agent::turns_taken` |
| `transcript_path` | **`null`** — Tact keeps a session in SQLite and writes a transcript only when compaction runs, so there is no single live file (Codex names its rollout file). It used to report the transcripts *directory*, which a plugin would try to open. |
| `tool_use_id` | On `PreToolUse` / `PostToolUse`: the call being annotated |

A hook that fails (non-zero exit, timeout, unspawnable command) is fail-open and, since this work, **visible**: the agent emits `[plugin hook <Event> failed] <error>`, because the `tracing::warn!` alone only reached a log file that a default session never writes.

### Which events carry `additionalContext` into the conversation

Codex has exactly four such channels — `SessionStart` / `SubagentStart` (they share one outcome type), `UserPromptSubmit`, `PreToolUse`, and `PostToolUse` — and Tact now covers the same set:

| Event | Where the context goes |
|---|---|
| `SessionStart` | A `<hook-context>` message before the first turn's user message |
| `PreToolUse` / `PostToolUse` | A `<hook-context>` message before the next request, so it sits with the tool call it annotates |
| `SubagentStart` | Appended to the child's system prompt (Claude Code semantics) |
| `UserPromptSubmit` | Appended to the prompt text itself |

The remaining events are control/observational only; their `additionalContext` is not a channel Codex defines either. In particular, `PreToolUse` context is **not** written into the tool arguments — an earlier `tool_use.input["_hook_context"]` key had no reader, so the context vanished and the field leaked into what the permission check and the tool itself saw.

Hooks are also the right place for one-time setup: warming caches, validating workspace invariants, or injecting telemetry context.

---

## 11. Design Constraints

| Constraint | Rationale |
|------------|-----------|
| Hooks run on the agent task | They hold `&mut Agent` indirectly via `LoopState`; keep work short or spawn internally. |
| No hooks inside parallel waves | Avoids data races on shared agent state while tools borrow routers immutably. |
| First `Block` wins | Predictable, easy-to-reason-about veto semantics. |
| Errors fail the step | Hook bugs surface as tool failures, not silent no-ops. |
| Registration order = run order | Hook priority when stacking plugins is `<marketplace>/<plugin>` key order (see §6); there is no priority field to set. |

Do **not** perform permission UI inside hooks — use `PermissionManager` and the existing `RequestSelect` flow instead.

---

## 12. Code Map

| File | Role |
|------|------|
| `crates/tact/src/hook/mod.rs` | Types, traits, `Hook` enum, `invoke_hooks!` macro |
| `crates/tact/src/agent/mod.rs` | `pre_tool`, `post_tool`, `session_start`, `hooks_by_type` |
| `crates/tact/src/agent/tool_dispatch.rs` | PreToolUse / PostToolUse invocation in `execute_tool_call` |
| `crates/tact/src/permission/mod.rs` | Runs after PreToolUse; separate from hooks |
| `crates/tact/src/plugin/hooks.rs` | `collect_hook_sources` / `config_hook_paths` (six origins), `HooksFile::{from_file, from_toml_file}`, `admit_trusted`, `HookTrust`, `survey_hooks`, `trust_hooks`, `run_hook` (dispatches by `HookCommand::kind`), `run_command_hook`, `run_mcp_tool_hook`, `definition_text`, `build_payload` |
| `crates/tact-ui/src/hooks_cli.rs` | `tact-ui hooks list` / `trust` / `forget`, and their renderers |
| `crates/tui/src/handlers/hooks.rs` | `/hooks list` / `trust` / `forget` — parsing and the idle gate; the driver runs the work |
| `crates/tact-ui/src/driver.rs` | `UserCommand::Hooks{List,Trust,Forget}` → `survey_hooks` / `trust_hooks` / `forget_hook_trust`, reported on the `Info` / `MdInfo` channels |
| `docs/state_machines.md` | Hook control enum and pipeline summary |

---

## 13. Deliberate gaps

| Gap | Why |
|-----|-----|
| `SessionStart` sources `clear` and `fork` | Codex reports them, but Tact has no history-clear command and no session fork, so the variants would be unreachable. The vocabulary is `startup` / `resume` / `compact` — the three Tact actually distinguishes. |
| Managed / enterprise hooks, `bypass_trust` | An admin-managed hook bundle and a switch that disables review are both trust-model decisions with no consumer here yet. `tact-ui hooks trust --all` is the scriptable equivalent. |

---

## Related Docs

- [Permission Model](./10_chapter_permission.md) — runs immediately after PreToolUse in the pipeline
- [Tasks and Tool Scheduling](./11_chapter_task.md) — three-phase tool pipeline hooks wrap
- [ARCHITECTURE.md](../ARCHITECTURE.md) — Hook Engine section
- [Tool Rendering](../docs/tool_rendering.md) — how blocked/failed steps appear in the TUI
- [Parallel Tool Execution](../docs/parallel_tool_execution.md) — where hooks do *not* run
