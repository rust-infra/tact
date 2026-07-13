# Parallel Tool Execution

How `tact` runs the tool calls in a single assistant turn concurrently while
keeping conflicting operations correctly ordered.

## Background: where dependencies actually live

The LLM may return several tool calls in one response. Two facts shape the design:

1. **Within one response, tool-call arguments are fixed.** The model emits all
   of a turn's `ToolUse` blocks at once; none of them can wait for another's
   output. So tools in the *same* turn carry no data dependency on each other.
2. **Real dependencies are cross-turn.** The agent loop (`agent_loop` in
   `crates/tact/src/agent/mod.rs`) feeds each turn's tool results back into the next
   request. If the model needs `search_code`'s result to decide what to
   `read_file`, it issues `search_code` in turn *N* and `read_file` in turn
   *N+1*. The loop already serialises turns, so cross-turn dependencies are
   handled for free.

The only intra-turn ordering we must preserve is a **resource conflict**: a
read/write or write/write on the same workspace file. Everything else in a turn
can overlap.

## The three-phase pipeline (`Agent::execute_tool_call`)

```
content (assistant ToolUse blocks)
        │
        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 1 — pre-flight (sequential, &mut self)                   │
│   stats · StepAdded/StepStarted · PreToolUse hook · permission │
│   (permission prompts are interactive → must stay ordered)     │
│   → Vec<PreparedTool> { Run | Resolved(blocked output) }       │
└──────────────────────────────────────────────────────────────┘
        │
        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 2 — execution (parallel by wave)                         │
│   schedule cleared tools into conflict-free waves;             │
│   run each wave concurrently (join_all over shared borrows);   │
│   barrier waves (bash/MCP) run solo                            │
└──────────────────────────────────────────────────────────────┘
        │
        ▼
┌──────────────────────────────────────────────────────────────┐
│ Phase 3 — post-processing (sequential, &mut self)              │
│   PostToolUse hook · StepFinished · bookkeeping (recent files, │
│   compact) — replayed in the model's original tool order       │
└──────────────────────────────────────────────────────────────┘
```

Only **Phase 2** is parallel. The framework around each call (stats, UI step
events, hooks, permissions) needs `&mut self` and/or interactive ordering, so it
stays sequential; the actual tool I/O (`ToolRouter::call`, `&self`) is the slow
part worth overlapping.

## Conflict model & wave scheduling (`crates/tact/src/agent/tool_schedule.rs`)

Each cleared tool is mapped to the workspace files it touches:

| Tool | Resource | Mode |
|------|----------|------|
| `read_file` | `input.path` | read |
| `batch_read` | `input.files[].path` | read |
| `search_code` | `input.path` or workspace root | read (directory scope) |
| `write_file` | `input.path` | write |
| `batch_edit` | `input.edits[].file_path` | write |
| `web_search`, `web_fetch`, `lsp`, `sleep` | — | independent (never conflicts) |
| **everything else** (`bash`, `apply_patch`, `task`/subagent, MCP, state mutations, unknown) | — | **barrier** (conflicts with all) |

Paths are normalised to absolute (lexically, rooted at `work_dir`). Two paths
**overlap** when they are equal or one is an ancestor of the other (so a write
to `src/foo.rs` conflicts with a search scoped to `src/`).

**Conflict:** two tools conflict if either is a barrier, or one writes a path
the other reads or writes. Two pure reads never conflict.

**Wave assignment** preserves the model's order for conflicting pairs while
overlapping independent calls:

```
wave[i] = max( wave[j] + 1  for every j < i that conflicts with i ),  else 0
```

Tools sharing a wave run concurrently; waves run in ascending order. A barrier
always lands alone in its own wave (so `bash` / MCP / subagents never run
concurrently with anything), which is also why MCP's stateful `&mut self`
router fits cleanly.

### Example

Model returns, in order: `read A`, `read B`, `write A`, `read C`, `read A`.

| Wave | Tools | Notes |
|------|-------|-------|
| 0 | `read A`, `read B`, `read C` | run together |
| 1 | `write A` | waits for `read A` |
| 2 | `read A` | waits for `write A` |

`read B` / `read C` are unaffected by the write to `A` and stay in wave 0.

### Safety stance: barrier-by-default

Unknown tools default to a **barrier** (run solo), so adding a new tool never
parallelises unsafely. Opting a tool into parallelism is an explicit edit to
`tool_resources` in `tool_schedule.rs`. `bash` is intentionally a barrier.

## Comparison with codex-cli

Both gate parallelism behind an allowlist and run a turn's eligible calls
concurrently, but the safety model differs:

| | codex-cli | tact |
|---|-----------|------|
| Trigger | model batches via `multi_tool_use.parallel`; request sets `parallel_tool_calls: true` | runtime schedules a turn's `ToolUse` blocks |
| Eligibility | per-tool / per-MCP-server `supports_parallel_tool_calls` flag; serial by default | barrier-by-default + known-tool allowlist |
| Conflict detection | **none** — trusts the model not to batch dependent ops | path conflict graph + wave scheduling |
| `bash`/shell | marked parallel-capable | **barrier** (runs solo) |

codex-cli's lack of temporal/conflict awareness is a known issue: it has been
observed issuing `git add` + `git commit` in parallel and racing
(openai/codex#13963, #14485). In `tact`, both are `bash` → separate barrier
waves, so that race cannot occur — at the cost of not (yet) parallelising
`bash`/subagent calls.

## Persistence for analysis

After scheduling, `execute_tool_call` records a `ToolScheduleSummary` (tool
count, wave count, max parallelism, per-wave tool names + barrier flag) into the
**same `token_usages` row** as that LLM call's token usage, keyed by
`last_message_id` on that row (set at `persist_llm_call` from
`llm_call_last_message_id`). This links scheduling strategy to token cost for later
performance/troubleshooting analysis. See the
[`tool_schedule` column](token_usage_schema.md#tool_schedule-column) for the
JSON shape and example queries.

## Code locations

| File | Role |
|------|------|
| `crates/tact/src/agent/tool_schedule.rs` | Resource model, conflict detection, wave scheduler, `ToolScheduleSummary`. |
| `crates/tact/src/agent/tool_dispatch.rs` | `Agent::execute_tool_call` (three phases), native/MCP dispatch, `persist_tool_schedule`. |
| `crates/tact/src/store/session_store/` | `record_tool_schedule` — UPDATE the call's `token_usages` row. |
