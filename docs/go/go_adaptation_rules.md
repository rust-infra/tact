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

---

## 5. Storage and Schema Rules

SQLite persistence semantics must be preserved.

- Keep message/token usage linkage via message IDs.
- Persist tool scheduling metadata with token usage rows for analysis.
- **Do not use SQLite foreign keys** (`FOREIGN KEY`, `REFERENCES`, `ON DELETE/UPDATE`).
- Integrity is managed in application code via explicit cleanup/update flows and indexed IDs.
- Every schema change requires backward-compatible migration logic.

---

## 6. LLM Provider Rules

- Keep provider abstraction stable (`anthropic`, `openai`, `deepseek` behavior equivalents).
- Preserve tool-call handling semantics and stop-reason logic.
- Preserve request/response observability fields used for debugging.
- Streaming behavior must be backpressure-safe and cancellation-safe.

---

## 7. TUI Migration Rules

When TUI migration starts:

- Prioritize semantic parity over pixel parity.
- Keep state-machine transitions equivalent to Rust docs (`docs/state_machines.md`).
- Preserve scroll correctness, long-block rendering, and popup interaction behavior.
- Add regression tests/golden snapshots for known historical issues.

---

## 8. Testing and Verification Rules

For each migrated subsystem:

- Add unit tests for core logic (especially scheduler and store).
- Add integration tests for loop + tools + persistence interactions.
- Keep compatibility checks for:
  - tool result ordering
  - cancellation behavior
  - token usage + tool schedule persistence linkage
- No subsystem is marked complete without executable verification.

---

## 9. Performance Rules

- Optimize only after parity and correctness are verified.
- Measure:
  - LLM call latency
  - tool execution latency
  - scheduler parallelism effectiveness
  - DB write overhead
- Any optimization that changes behavior must be explicitly reviewed.

---

## 10. Change Control Rules

- Document major decisions and divergences in the migration plan.
- Keep incremental PR-sized changes.
- Avoid "big bang" rewrites.
- New behavior defaults to safe/serial when uncertainty exists.
