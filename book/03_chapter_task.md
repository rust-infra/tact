# Tasks and Tool Scheduling

This chapter walks through what happens after the LLM decides to act: how Tact turns a set of `ToolUse` blocks into executed commands, results, and the next conversation turn.

---

## 1. A Task Is a Turn of the Agent Loop

In Tact, a **task** is the work performed in one iteration of `Agent::agent_loop` (`crates/tact/src/lib.rs`):

```text
┌─────────────┐    LLM call    ┌─────────────────────┐
│ User prompt │ ─────────────► │ assistant response  │
└─────────────┘                │ (text + ToolUses)   │
                               └─────────────────────┘
                                         │
                                         ▼
                               ┌─────────────────────┐
│                              │ execute_tool_call() │
│                              └─────────────────────┘
│                                         │
│          ┌──────────────────────────────┼──────────────────────────────┐
│          ▼                              ▼                              ▼
│    pre-flight                    parallel execution              post-processing
│    (sequential)                  (waves)                          (sequential)
│          │                              │                              │
│          ▼                              ▼                              ▼
│   permission + hooks            tool calls run                results + hooks
│                                 concurrently where safe       appended to context
└────────────────────────────────────────────────────────────────────────────┘
                                         │
                                         ▼
                               next LLM call
```

The loop keeps running until the model stops, asks the user, or hits a completion condition.

---

## 2. The Three-Phase Pipeline

`Agent::execute_tool_call` (`crates/tact/src/lib.rs`) splits every turn into three phases.

### Phase 1 — Pre-flight (sequential)

Run once per tool, in the order the model emitted them:

1. Emit `StepAdded` / `StepStarted` UI events.
2. Run the `PreToolUse` hook.
3. Check permissions via `PermissionManager`.
4. If permission is denied, produce a blocked result without running the tool.

This phase must stay sequential because permission prompts can be interactive and hooks need `&mut self`.

### Phase 2 — Execution (parallel by wave)

All tools that cleared pre-flight are handed to the scheduler in `crates/tact/src/tool_schedule.rs`:

- Independent reads run together.
- Conflicting reads/writes or writes/writes are serialized.
- `bash`, MCP, subagents, and unknown tools are **barriers** — they run alone.

The scheduler assigns each tool a **wave number**:

```text
wave[i] = max( wave[j] + 1  for every j < i that conflicts with i ), else 0
```

Waves execute in order; tools inside the same wave run concurrently.

### Phase 3 — Post-processing (sequential)

After all waves finish:

1. Run the `PostToolUse` hook.
2. Emit `StepFinished` UI events in the model's original order.
3. Update bookkeeping: recent files, stats, compaction triggers.
4. Append tool results to `runtime.context`.

---

## 3. Conflict Model and Safety

`tool_schedule.rs` decides which tools can overlap. Each known tool declares the workspace resources it touches:

| Tool | Resource | Mode |
|------|----------|------|
| `read_file` | `input.path` | read |
| `batch_read` | `input.files[].path` | read |
| `search_code` | directory scope | read |
| `write_file`, `edit_file` | `input.path` | write |
| `batch_edit` | `input.edits[].file_path` | write |
| `web_search`, `web_fetch`, `lsp`, `sleep` | — | independent |
| `bash`, `apply_patch`, subagent, MCP, unknown | — | barrier |

Paths are normalised to absolute and rooted at `work_dir`. Two paths overlap if they are equal or one is an ancestor of the other, so a write to `src/foo.rs` conflicts with a search scoped to `src/`.

### Example

Model returns, in order:

1. `read A`
2. `read B`
3. `write A`
4. `read C`
5. `read A`

| Wave | Tools | Notes |
|------|-------|-------|
| 0 | `read A`, `read B`, `read C` | run together |
| 1 | `write A` | waits for the first `read A` |
| 2 | `read A` | waits for the write |

`read B` and `read C` are unaffected and stay in wave 0.

### Barrier-by-default

Unknown tools are treated as barriers. Adding a new tool can never accidentally introduce unsafe parallelism; you must explicitly opt it in by updating `tool_resources` in `tool_schedule.rs`.

---

## 4. Permissions and Hooks

Before a tool enters scheduling, `PermissionManager` classifies its intent:

- **Read-only**: generally allowed.
- **High-risk write**: asks the user (or checks the allowlist in `plan`/`auto` modes).
- **Unknown / stateful**: treated conservatively.

Hooks (`PreToolUse`, `PostToolUse`) live in `crates/tact/src/hooks.rs` and can inspect or modify tool input/output. They run sequentially around the parallel core.

---

## 5. What Goes Back to the LLM

Each finished tool produces a `ToolResult` with JSON content. These are appended to `runtime.context` as `Role::User` messages, preserving the model's original tool-call order. The agent loop then sends the updated context to the LLM for the next turn.

---

## 6. Observability: Tool Schedule Summary

After execution, `persist_tool_schedule` records a `ToolScheduleSummary` into the same `token_usages` row as the LLM call:

```json
{
  "tool_count": 5,
  "wave_count": 3,
  "max_parallelism": 3,
  "waves": [
    { "wave": 0, "tools": ["read_file", "read_file", "read_file"], "barrier": false },
    { "wave": 1, "tools": ["write_file"], "barrier": false },
    { "wave": 2, "tools": ["read_file"], "barrier": false }
  ]
}
```

This links scheduling strategy to token cost for later analysis.

---

## 7. Customizing Scheduling

To make a new native tool parallel-safe:

1. Add its resource pattern to `tool_resources()` in `crates/tact/src/tool_schedule.rs`.
2. Return the correct `ToolResourceMode` (`Read`, `Write`, or `Independent`).
3. Avoid side effects outside the declared resources.

If a tool has global side effects (shell commands, subagents, MCP state), leave it as a barrier.

---

## 8. Code Map

| File | Role |
|------|------|
| `crates/tact/src/lib.rs` | `Agent::agent_loop`, `Agent::execute_tool_call`, three-phase orchestration |
| `crates/tact/src/tool_schedule.rs` | Resource model, conflict detection, wave scheduler, `ToolScheduleSummary` |
| `crates/tact/src/permission.rs` | Intent classification and permission decisions |
| `crates/tact/src/hooks.rs` | `PreToolUse` / `PostToolUse` hooks |
| `crates/tact/src/tool/mod.rs` | `ToolRouter`, tool registration, native tool dispatch |
| `crates/tact/src/store/session_store/` | `record_tool_schedule` — persists schedule summary |

---

## Related Docs

- [Parallel Tool Execution](../docs/parallel_tool_execution.md)
- [Batch Tools Flow](../docs/batch_tools_flow.md)
- [Tool Rendering](../docs/tool_rendering.md)
- [Token Usage Schema](../docs/token_usage_schema.md)
