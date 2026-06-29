# Go Migration Plan

This plan describes a staged migration path for implementing a Go version of `tact` with controlled risk and observable progress.

---

## 0. Principles

- Deliver in vertical slices.
- Preserve behavior first, optimize second.
- Keep each milestone independently verifiable.
- Prefer fallback-safe behavior (serial over unsafe parallel).

---

## 1. Phase A - Foundation and Skeleton

Goal: establish a compilable Go project with clear package boundaries and shared types.

Deliverables:

- Repository skeleton and package layout (`cmd/`, `internal/`, optional `pkg/`).
- Core protocol types in `internal/protocol` — including `StepResult.arg_full`, `PlanStep.args`, full `AgentUpdate` enum.
- Configuration, logging, and error conventions.
- Minimal CLI entrypoint (`cmd/tact-ui`) with default TUI bootstrap and `headless` subcommand stub.

Exit criteria:

- Project builds cleanly.
- `tact-ui headless` (or equivalent) starts without TUI dependencies.
- CI runs unit tests.

---

## 2. Phase B - Headless Agent Loop (No TUI)

Goal: run multi-turn LLM loop with tool-use support in headless mode.

Deliverables:

- Agent runtime loop (`internal/agent`):
  - message/context handling
  - stop-reason handling
  - continuation/retry hooks where needed
  - pre-flight `StepAdded` / `StepStarted` and post-flight `StepFinished` with `arg_full`
  - `tool_arg_summary` / `tool_arg_full` helpers (120-char truncation)
- Provider interface (`internal/llm`) and at least one concrete provider.
- Tool router + 3-5 core tools:
  - `read_file`, `write_file`, `search_code`, `shell`/`bash`, `apply_patch` equivalent

Exit criteria:

- A full prompt -> tool-use -> tool-result -> follow-up turn cycle runs end-to-end.
- `StepAdded.description` matches `tool (arg_summary)`; `StepResult.arg_full` populated on finish.
- Cancellation and error propagation are verified.

---

## 3. Phase C - Conflict-Aware Tool Scheduler

Goal: port `tool_schedule` semantics and parallel wave execution.

Deliverables:

- Port scheduler model from Rust:
  - resource extraction
  - conflict detection
  - wave grouping
  - barrier-default policy
- Integrate three-stage execution:
  - pre-flight sequential
  - execution by waves
  - post-processing sequential in original order

Exit criteria:

- Scheduler unit tests cover representative conflict cases.
- Same-file write/read conflict is always serialized.
- Unknown tools default to barrier behavior.

---

## 4. Phase D - Persistence Layer (SQLite)

Goal: migrate session + token usage persistence with analysis linkage.

Deliverables:

- Session store interfaces + SQLite implementation.
- Message persistence and load/replay.
- Token usage persistence.
- `tool_schedule` persistence linked to the same call window.
- Backward-compatible migration scripts.

Constraints:

- No SQLite foreign keys. Integrity is application-managed.

Exit criteria:

- CRUD and lifecycle tests pass.
- Token usage and tool schedule can be queried together.

---

## 5. Phase E - Permissions, Hooks, MCP, Subagent

Goal: migrate operational controls and extension capabilities.

Deliverables:

- Permission manager with allow/deny/ask flows.
- Pre/post tool hooks.
- MCP client/router baseline.
- Subagent orchestration baseline (if required by scope).

Exit criteria:

- Approval flow is functionally correct.
- Hook execution order and blocking semantics are verified.

---

## 6. Phase F - TUI Migration

Goal: restore interactive terminal experience.

Deliverables:

- TUI state model equivalent to Rust state machines.
- Log rendering pipeline and popup interactions per `docs/tui_rendering.md`.
- Tool card rendering per `docs/tool_rendering.md`:
  - 3-tier blocks, concurrent active tools, truncated title args, `arg_full` in popups
  - spacing gaps (content→tool, thinking→tool)
  - popups without drop shadow; code cards without emoji icons

Exit criteria:

- Key workflows operate end-to-end from TUI (`tact-ui` default mode).
- Historical regressions (scroll/render issues) have tests or reproducible checks.
- Tool title/popup arg behavior matches Rust golden cases (120-char truncation, full args on double-click).

---

## 7. Phase G - Compatibility and Hardening

Goal: close parity gaps and harden production behavior.

Deliverables:

- Behavior comparison checklist against Rust implementation.
- Observability dashboards/log exports for:
  - token usage
  - tool schedule
  - latency and failure distributions
- Performance tuning pass after correctness lock.

Exit criteria:

- Compatibility checklist is green for agreed critical flows.
- Release candidate passes integration and smoke suites.

---

## 8. Work Breakdown and Milestones

Recommended milestone order:

1. A1: skeleton + protocol (`StepResult`, `AgentUpdate`) + `cmd/tact-ui` bootstrap
2. B1: minimal loop + one provider + two tools
3. B2: tool-use multi-turn support + step lifecycle events (`StepAdded`/`StepStarted`/`StepFinished`)
4. C1: scheduler port + tests
5. D1: session/message persistence
6. D2: token usage + tool schedule persistence
7. E1: permission + hooks
8. E2: MCP baseline
9. F1: TUI core panels + tool blocks (title/meta/detail, arg truncation)
10. F2: popups (`arg_full`, bash `$` prefix) + advanced interactions
11. G1: compatibility hardening

---

## 9. Iteration Task Breakdown (Suggested)

Assumption: 2-week iterations, 1 small team.

| Iteration | Focus | Key tasks | Acceptance criteria |
|---|---|---|---|
| Iteration 1 | Foundation | repo skeleton, protocol types (`arg_full`, `AgentUpdate`), logger/config, `cmd/tact-ui`, CI pipeline | `go test ./...` green; `tact-ui headless` starts |
| Iteration 2 | Headless loop | provider interface, first provider adapter, basic tool router + 3 tools, step lifecycle events | multi-turn tool-use cycle works; `StepAdded`/`StepFinished` shape matches Rust |
| Iteration 3 | Scheduler + store | conflict-aware scheduler, SQLite session/message/token usage, `tool_schedule` persistence | same-file write/read serialized; schedule+token rows queryable together |
| Iteration 4 | Control plane | permissions, pre/post hooks, cancellation hardening, retry/backoff normalization | approval flow and hook blocking semantics verified |
| Iteration 5 | MCP + subagent baseline | MCP router, minimal subagent orchestration, observability fields | MCP tools callable; failures observable and recoverable |
| Iteration 6 | TUI parity + hardening | state machine parity, tool blocks + popups (`arg_full`), spacing/render rules, regression checks | critical TUI workflows pass; compatibility checklist mostly green |

Notes:

- If team capacity is limited, split Iteration 6 into two iterations.
- Keep each iteration mergeable and independently releasable.

---

## 10. Risk Register and Mitigations

- TUI parity complexity:
  - Mitigation: migrate headless first; add focused visual regression checks.
- Concurrency races:
  - Mitigation: preserve sequential boundaries; add deterministic scheduler tests.
- Provider behavior divergence:
  - Mitigation: normalize provider adapters and assert stop-reason/tool-call invariants.
- Migration fatigue from large scope:
  - Mitigation: milestone gates, vertical slices, and strict DoD per phase.

---

## 11. Definition of Done (Per Phase)

A phase is done only when all are true:

- Code merged with tests.
- Compatibility notes updated.
- Known deviations documented.
- Operational observability fields emitted.
- No open blocker for next phase.

---

## 12. Rust Reference Sync Checklist

Re-read these Rust docs whenever the Go port lags behind main:

| Topic | Rust doc / code |
|---|---|
| CLI entry | `tact-ui`, `crates/tact/src/bin/tui.rs`, `headless` subcommand |
| Agent loop + step events | `crates/tact/src/lib.rs` (`execute_tool_call`, `tool_arg_summary`) |
| Wire types | `crates/protocol/src/lib.rs` (`StepResult`, `AgentUpdate`) |
| Tool scheduling | `docs/parallel_tool_execution.md`, `crates/tact/src/tool_schedule.rs` |
| Tool / TUI rendering | `docs/tool_rendering.md`, `docs/tui_rendering.md` |
| Architecture overview | `ARCHITECTURE.md` §2 (AgentUpdate table), §14 (changelog) |

Compatibility checklist items for Phase G:

- [ ] Single `tact-ui` binary; headless via subcommand
- [ ] `StepAdded` plan-only (no duplicate log line)
- [ ] Tool title truncation at 120 chars; `arg_full` in popups
- [ ] Content→tool and thinking→tool spacing
- [ ] Popups render without drop shadow
- [ ] Code block titles use plain language labels
