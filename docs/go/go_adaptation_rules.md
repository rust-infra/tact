# Go Adaptation Rules

This document defines the non-negotiable adaptation rules when implementing a Go version of `tact`.

---

## 1. Scope and Compatibility Goal

The Go implementation must preserve **behavioral compatibility** with the Rust codebase first, and optimize second.

- Keep user-visible behavior stable:
  - agent loop semantics
  - tool execution ordering and output shape
  - TUI interaction contracts (once implemented)
  - persistence schema semantics
- Any intentional behavior change must be documented in `docs/go/go_migration_plan.md`.

---

## 2. Architectural Mapping Rules

Maintain module boundaries similar to Rust to reduce migration risk:

- `tact_protocol` -> `internal/protocol`
- `tact` runtime -> `internal/agent`, `internal/tool`, `internal/store`, `internal/mcp`, `internal/permission`, `internal/hook`, `internal/compact`
- `tact_llm` -> `internal/llm`
- `tui` -> `internal/tui`
- `tools` (Sandbox only) -> `internal/sandbox` or equivalent secure I/O wrapper

Binary entry (must match Rust):

- Rust ships a **single** CLI binary: `tact-ui` (`crates/tact/src/bin/tui.rs`).
- Default mode: interactive TUI.
- Non-interactive runs use a **`headless` subcommand** (not a separate binary).
- Go equivalent: `cmd/tact-ui` with the same default/subcommand split. Do **not** introduce a second headless-only binary unless explicitly documented as a divergence.

Rules:

- Preserve clear boundaries between:
  - protocol types
  - runtime orchestration
  - tool implementations
  - storage layer
  - UI layer
- Avoid introducing cyclic package imports.
- Keep tool implementations side-effect scoped and testable.

---

## 3. Concurrency and Ordering Rules

Use `context.Context` + goroutines/channels as the baseline model.

- Agent loop stays multi-turn and sequential across turns.
- Same-turn tool calls may run in parallel **only** under conflict-aware scheduling.
- Preserve the current three-phase tool execution model:
  1. pre-flight sequential (permissions/hooks/step lifecycle)
  2. conflict-aware wave execution
  3. post-processing sequential in original model order
- Cancellation must be cooperative and explicit via context propagation.

---

## 4. Tool Scheduling Rules (Must Match Rust)

Migration must preserve the scheduler semantics from `crates/tact/src/tool_schedule.rs`.

- Read/write conflicts on the same path cannot run concurrently.
- Unknown tools default to **barrier** (serial safety first).
- Path overlap checks must preserve ancestor/descendant conflict behavior.
- MCP/stateful tool calls remain serialized unless explicitly proven safe.
- Record scheduling summary (`tool_schedule`) for observability.

Phase 1 lifecycle events (`StepAdded`, `StepStarted`, `StepFinished`) must stay aligned with Rust:

- Emit `StepAdded` then `StepStarted` per tool during pre-flight (before parallel execution).
- `StepAdded(PlanStep).description` = `tool (arg_summary)` when args exist; full untruncated args live in `PlanStep.args`.
- `StepStarted` carries truncated `arg_summary` (same extraction as Rust `tool_arg_summary`, max **120** runes/chars).
- `StepFinished` / `StepResult` must include optional `arg_full` for popup/detail views.
- `StepAdded` updates the plan panel only — it must **not** insert a separate log line; the log block appears on `StepStarted`.

See `docs/parallel_tool_execution.md` for the three-phase pipeline and `docs/tool_rendering.md` for TUI consumption rules.

---

## 5. Protocol Wire-Type Rules

Port `tact_protocol` types faithfully before UI work:

| Rust type / variant | Go must preserve |
|---|---|
| `AgentUpdate::StepAdded(PlanStep)` | Plan append only; `description` + `args` shape |
| `AgentUpdate::StepStarted(idx, tool_id, tool_name, arg_summary)` | Running tool block trigger |
| `AgentUpdate::StepFinished(idx, tool_id, StepResult)` | Includes `arg_full`, `permission_label`, `duration_us` |
| `StepResult.arg_summary` | Truncated display string (≤120 chars) |
| `StepResult.arg_full` | Optional full path/command/JSON for popups |
| `PlanStep.args` | Full argument map (string values) |

Helper parity (runtime):

- `tool_arg_full(name, input)` — untruncated extraction (path, command, or JSON)
- `tool_arg_summary(name, input)` — same extraction, truncated to 120 chars
- Truncation uses character count, not byte count, when feasible in Go.

Any new field on Rust wire types must be mirrored in Go or documented as an intentional divergence in `docs/go/go_migration_plan.md`.

---

## 6. Storage and Schema Rules

SQLite persistence semantics must be preserved.

- Keep message/token usage linkage via message IDs.
- Persist tool scheduling metadata with token usage rows for analysis.
- **Do not use SQLite foreign keys** (`FOREIGN KEY`, `REFERENCES`, `ON DELETE/UPDATE`).
- Integrity is managed in application code via explicit cleanup/update flows and indexed IDs.
- Every schema change requires backward-compatible migration logic.

---

## 7. LLM Provider Rules

- Keep provider abstraction stable (`anthropic`, `openai`, `deepseek` behavior equivalents).
- Preserve tool-call handling semantics and stop-reason logic.
- Preserve request/response observability fields used for debugging.
- Streaming behavior must be backpressure-safe and cancellation-safe.

---

## 8. TUI Migration Rules

When TUI migration starts:

- Prioritize semantic parity over pixel parity.
- Keep state-machine transitions equivalent to Rust docs (`docs/state_machines.md`).
- Preserve scroll correctness, long-block rendering, and popup interaction behavior.
- Add regression tests/golden snapshots for known historical issues.

Tool-block parity checklist (must match `docs/tool_rendering.md`):

| Behavior | Rust reference |
|---|---|
| 3-tier layout (title + meta + detail card) | `ToolWidget` / `ToolCell` |
| Concurrent running tools | `ToolState.active: Vec<ActiveToolBlock>` |
| Title: command tools | `{step}. {tool} ({arg_summary})` e.g. `2. bash (git status)` |
| Title: other tools | `{step}. {label}  {arg_summary}` (double space) |
| Arg truncation in title/meta | 120 chars via `tool_arg_summary` |
| Full args in popup | `StepResult.arg_full`; bash popup prefixes `$ <full command>` |
| Spacing before tool blocks | One blank line when following normal content |
| Spacing after thinking | One trailing blank line; next tool reuses it |
| Popups | Centered modal, **no drop shadow** |
| Code cards | Plain language label in title; no side emoji icons |
| Permission select | `log_confirm = false` — no duplicate approval text in log |

Canonical Rust docs to re-read when syncing Go work:

- `docs/tool_rendering.md`, `docs/tui_rendering.md`, `docs/parallel_tool_execution.md`
- `ARCHITECTURE.md` (AgentUpdate table, §14 changelog)

---

## 9. Testing and Verification Rules

For each migrated subsystem:

- Add unit tests for core logic (especially scheduler and store).
- Add integration tests for loop + tools + persistence interactions.
- Keep compatibility checks for:
  - tool result ordering
  - cancellation behavior
  - token usage + tool schedule persistence linkage
- No subsystem is marked complete without executable verification.

---

## 10. Performance Rules

- Optimize only after parity and correctness are verified.
- Measure:
  - LLM call latency
  - tool execution latency
  - scheduler parallelism effectiveness
  - DB write overhead
- Any optimization that changes behavior must be explicitly reviewed.

---

## 11. Change Control Rules

- Document major decisions and divergences in the migration plan.
- Keep incremental PR-sized changes.
- Avoid "big bang" rewrites.
- New behavior defaults to safe/serial when uncertainty exists.
