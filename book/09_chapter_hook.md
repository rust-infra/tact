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

## 2. Three Hook Types

Defined in `crates/tact/src/hook/mod.rs`:

| Hook | Registration | Invoked today? | Can mutate | Can veto |
|------|----------------|----------------|------------|----------|
| `SessionStart` | `Agent::session_start` | **No** — registered but not yet called from `agent_loop` | read-only access to `LoopState` (`Agent`) | Yes |
| `PreToolUse` | `Agent::pre_tool` | Yes — before permission check, per tool in order | `ToolUse` input (`name`, `input` JSON) | Yes |
| `PostToolUse` | `Agent::post_tool` | Yes — after each tool finishes, as results stream in | `ToolResult` content | Yes |

`LoopState` is a type alias for `Agent`, so session hooks see the same runtime the loop uses (context, stats, tool routers, etc.).

---

## 3. Control Flow: `HookControl`

Every hook returns one of:

```rust
pub enum HookControl {
    Continue,
    Block(String),
}
```

| Result | Meaning |
|--------|---------|
| `Continue` | Run the next hook of the same type, then proceed with the pipeline. |
| `Block(reason)` | Stop the hook chain immediately; the tool step is treated as failed with `reason`. |

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
        if tool_use.name == "read_file" && tool_result.content.len() > 50_000 {
            tool_result.content.truncate(50_000);
            tool_result.content.push_str("\n… (truncated by hook)");
        }
        Ok(HookControl::Continue)
    })
});
```

Hooks are appended to `Agent.hooks` in registration order and executed in that order for each invocation.

Multiple hooks of the same type compose: all must return `Continue` unless one `Block`s (first block wins).

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

## 10. SessionStart (API Today)

`Agent::session_start` accepts hooks with signature:

```rust
Fn(&LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + '_>>
```

The intended call site is **once per session**, before the first LLM request in `agent_loop` (after `ensure_session`, before the main `loop` body).

As of this writing, **`agent_loop` does not yet invoke `invoke_hooks!(SessionStart, …)`**. You can register session hooks today, but they will not run until that call is wired in. PreToolUse and PostToolUse are fully active.

When wired, session hooks will be the right place for one-time setup: warming caches, validating workspace invariants, or injecting telemetry context.

---

## 11. Design Constraints

| Constraint | Rationale |
|------------|-----------|
| Hooks run on the agent task | They hold `&mut Agent` indirectly via `LoopState`; keep work short or spawn internally. |
| No hooks inside parallel waves | Avoids data races on shared agent state while tools borrow routers immutably. |
| First `Block` wins | Predictable, easy-to-reason-about veto semantics. |
| Errors fail the step | Hook bugs surface as tool failures, not silent no-ops. |
| Registration order = run order | Document hook priority when stacking multiple plugins. |

Do **not** perform permission UI inside hooks — use `PermissionManager` and the existing `RequestSelect` flow instead.

---

## 12. Code Map

| File | Role |
|------|------|
| `crates/tact/src/hook/mod.rs` | Types, traits, `Hook` enum, `invoke_hooks!` macro |
| `crates/tact/src/agent/mod.rs` | `pre_tool`, `post_tool`, `session_start`, `hooks_by_type` |
| `crates/tact/src/agent/tool_dispatch.rs` | PreToolUse / PostToolUse invocation in `execute_tool_call` |
| `crates/tact/src/permission/mod.rs` | Runs after PreToolUse; separate from hooks |
| `docs/state_machines.md` | Hook control enum and pipeline summary |

---

## Related Docs

- [Permission Model](./10_chapter_permission.md) — runs immediately after PreToolUse in the pipeline
- [Tasks and Tool Scheduling](./11_chapter_task.md) — three-phase tool pipeline hooks wrap
- [ARCHITECTURE.md](../ARCHITECTURE.md) — Hook Engine section
- [Tool Rendering](../docs/tool_rendering.md) — how blocked/failed steps appear in the TUI
- [Parallel Tool Execution](../docs/parallel_tool_execution.md) — where hooks do *not* run
