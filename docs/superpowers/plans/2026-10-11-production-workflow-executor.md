# Production Workflow executor — plan

**Spec:** `docs/superpowers/specs/2026-10-11-production-workflow-executor-design.md`
**Status:** implemented 2026-10-11.

- [x] Add `ChatTurnWorkflow` to
  `crates/tact_extensions/src/extensions/workflow.rs`: a `WorkflowExecutor`
  holding a `RuntimeContext`, parsing `{"steps": [{"prompt", "run_id"?}]}` and
  invoking `chat.submit` once per step on the serving router.
- [x] Per step: fresh invocation from the stored runtime, carrying the caller's
  plugin id and actor and its own request id; a generated run id when the step
  omits one, so the output can report every run.
- [x] Fail closed: `deny_unknown_fields`; the first failing step stops the
  sequence and its error is re-origined as `workflow` with the step index in the
  message, every other field preserved.
- [x] Register `WorkflowExtension` over `ChatTurnWorkflow` in
  `crates/tact_ui/src/session_bootstrap.rs::register_host_extensions`, so both
  hosts serve `workflow.run`.
- [x] Allow `workflow.run` in `crates/tact_ui/src/permission.rs`
  `HostControlPlanePermission`, for the host identities only (it chains
  `chat.submit`, so it grants no new power).
- [x] Tests: three in `workflow.rs` (order + output, stop-and-name on failure,
  unknown field rejected) and one in `crates/tact_ui/src/driver.rs`
  (`workflow_run_sequences_turns_through_the_serving_router`: two steps, two
  turns, `workflow.run` → `chat.submit` → `runs.start` all on the Router).
- [x] Sync `ARCHITECTURE.md` §15, the 2026-10-08 spec status rows, that plan's
  status note, and `book/25_chapter_issue_zh.md`.

**Verification:** `./scripts/check-rust.sh` ends with `Rust checks passed.`;
`cargo test --workspace` **2893 → 2897 passed / 0 failed / 22 ignored** (46
suites, +4 tests).

**Not done (deliberate):** no production caller. `workflow.run` is registered
and permissioned on the same host control plane as the other capabilities, so a
`/workflow` command (or a plugin orchestrator) can be added without touching
this design.
