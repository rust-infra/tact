# Production Workflow executor — design

**Status:** approved 2026-10-11 (the user chose option A: steps are chat turns).
**Context:** `docs/superpowers/specs/2026-10-08-runtime-plugin-architecture-design.md` §5
("Workflow owns multi-step orchestration and future task graphs") and its
`Implementation status` table, which listed `workflow.run` as "registered only in
tests" and a production Workflow executor under "Not planned for now".

## Goal

Give the official Workflow extension a real, production-registered executor so
`workflow.run` is no longer a test-only stub, without inventing a second run
entry: a workflow is a **sequence of conversational turns**, and each step is
executed by invoking `chat.submit` on the session's serving router.

## Non-goals

- A `/workflow` host command or any other production caller. This change makes
  the capability real and registered; who invokes it is a separate product
  decision. (Registering it puts it on the same router and permission path as
  the other host control-plane capabilities, so a caller can be added without
  touching this design.)
- A general capability-chaining primitive (arbitrary `capability` + `input` per
  step). Rejected as option B: it hands a plugin a composable invocation
  primitive whose permission surface needs its own design.
- Task graphs, parallelism, conditionals, retries. First cut is strictly
  sequential.

## Contract

`workflow.run` input:

```json
{ "steps": [ { "prompt": "…", "run_id": "optional-id" } ] }
```

- `steps` is required and must be a non-empty-free array; each element is an
  object with a `prompt` string and an optional `run_id`.
- `deny_unknown_fields` on every level, so a typo is rejected rather than
  silently ignored — the same convention as `chat.submit` / `chat.compact`.

`workflow.run` output (success):

```json
{ "steps": 2, "runs": ["…", "…"] }
```

- `runs[i]` is the run id of step `i`. When a step omits `run_id`, the executor
  generates one and passes it to `chat.submit`, so the caller always learns
  which run each step produced *without* changing `chat.submit`'s own contract
  (it still answers `{"accepted": true}`, asserted by the headless tests).

Failure: the first failing step stops the sequence and the step's error is
returned, with `origin` set to `workflow` and the message prefixed
`workflow step {i} failed: ` — every other `KernelError` field (category,
retryability, request/plugin id, details) is preserved by mutating the protocol
error rather than rebuilding it.

## Design

- `crates/tact_extensions/src/extensions/workflow.rs` gains
  `ChatTurnWorkflow`, a `WorkflowExecutor` holding a clone of the session's
  `RuntimeContext`. A registered handler only receives an `InvocationContext`,
  which carries the request but not the router, so the runtime is stored at
  construction — exactly the reason `ChatExtension` stores one.
- Per step the executor builds a **fresh** invocation from the stored runtime
  (`self.runtime.invocation(fresh_request_id, caller_plugin_id, caller_actor)`),
  copying the *caller's* plugin id and actor and giving the step its own request
  id and cancellation token. This is the same hop `chat.submit` already makes
  towards `runs.start`, and for the same reason: reusing the caller's invocation
  would give every step one request id and couple the caller's cancellation to a
  step it never named.
- The executor calls `self.runtime.router().invoke("chat.submit", …)`, so a
  workflow step gets the full turn semantics (message assembly, Stop-hook
  continuation loop, `TaskComplete`, TaskCompleted hooks) and every hop stays
  permission-checked and trajectory-recorded. Workflow is a choreographer, not a
  run entry.
- Workflow keeps no compile-time dependency on `chat`: the step input is
  `{"prompt", "run_id"}`, the contract `chat.submit` documents, invoked by name
  through the router.

## Registration and permission

- `crates/tact_ui/src/session_bootstrap.rs::register_host_extensions` registers
  `WorkflowExtension::new(Arc::new(ChatTurnWorkflow::new(serving.clone())))`
  after Chat and the Agent, so both hosts get it on the same session serving
  context.
- `HostControlPlanePermission` (crates/tact_ui/src/permission.rs) gains
  `workflow.run` in its allowed-name set, gated on the same host identities as
  the rest. A step's `chat.submit` passes because the executor propagates the
  caller's plugin id (a host identity) — no new trusted identity is added, and a
  plugin holding the router still cannot inject a turn under its own name.
- Invoking `workflow.run` chains `chat.submit`, so it grants no power the host
  control plane did not already have; it is listed with the other host
  capabilities for exactly that reason.

## Tests

- `workflow.rs` unit tests (stub `chat.submit` on a runtime with an allow-all
  policy): steps are invoked in order with the right prompts and run ids; the
  output reports `steps`/`runs`; a failing middle step stops the sequence and
  the error names the step; an unknown field is `InvalidRequest`.
- `crates/tact_ui/src/driver.rs`: one end-to-end test that registers the host
  extensions on a serving context, invokes `workflow.run` with two steps as the
  host identity, and asserts the shared Agent completed two turns and the
  recorder saw `workflow.run` then `chat.submit` then `runs.start` twice.

## Docs

`ARCHITECTURE.md` §15 (the `WorkflowExtension::register` row and the
"what is still not wired" prose), the 2026-10-08 spec's `Implementation status`
rows, the plan's status note, and a `book/25_chapter_issue_zh.md` entry.
