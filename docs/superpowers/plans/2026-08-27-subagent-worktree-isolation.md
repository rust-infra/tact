# Subagent Worktree Isolation Implementation Plan

> **Goal:** Give `spawn_subagent` an optional git-worktree isolation mode. When
> enabled, the subagent runs with `work_dir` = a fresh `git worktree` lane,
> so parallel/background subagents can edit the repo without racing the parent
> or each other. This is the "worktree follow-up" named in the 2026-08-26
> async-subagent design review (C2): *"Same-wave fan-out of blocking subagents
> is deferred to the worktree follow-up, where `ResourcePolicy::Independent`
> becomes safe."*

**Design source:** `docs/superpowers/specs/2026-08-26-async-subagent-design.md`
(mapping tier: `Adapt | isolation: "worktree" | Later: reuse worktree_manager
for parallel edits`) and `docs/superpowers/specs/2026-08-26-async-subagent-design-review.md`
(Revision paragraph). Claude Code reference: `isolation: "worktree"` via
`EnterWorktree`/`ExitWorktree`.

## Scope

- **Add** `SubagentInput.worktree: Option<bool>`.
- **Create/reuse** a git worktree named `subagent-<child_id>` and run the child
  with `ToolContext.work_dir` pointed at the worktree.
- **Surface** the worktree name/path in both sync and async results.
- **Relax scheduling** for isolated spawns: per-invocation
  `ToolResources::independent()` (no barrier) so worktree-isolated subagents
  may fan out in the same wave — exactly the case the review says becomes safe.
- **Keep** `ResourcePolicy::Barrier` as the static metadata default for
  non-isolated spawns; no change to permission model; no OS sandbox.

## Non-goals

- No worktree **removal** surface yet (worktrees persist for inspection via
  `worktree_status` / `worktree_run`; manual `git worktree remove` remains).
- No per-agent declarative `.tact/agents/*.md`.
- No nested worktree base-ref resolution (base is always repo-root `HEAD`);
  a subagent spawned *from* a worktree still branches from the main repo HEAD.

## Design decisions

1. **Field semantics:** `worktree: true` requests isolation. `git worktree add`
   fails on a non-git `work_dir` → spawn errors with a clear message; no
   silent fallback to the shared filesystem.
2. **Naming:** worktree name `subagent-<child_id>` (child_id is a UUID →
   unique, git-ref-safe), branch `wt/subagent-<child_id>`, path
   `<repo_root>/.worktrees/subagent-<child_id>`. Worktree row `session_id` =
   child session id (the lane belongs to the child).
3. **Creation timing:** synchronously inside the handler — before the sync
   `agent_loop`, or before returning `async_launched { id }` for background
   spawns — so failures surface immediately instead of in a detached task.
4. **Resume:** `resume + worktree` reuses the existing lane for the same
   child id instead of failing on the unique-name constraint. Requires a
   public `WorktreeManager::get(name)`.
5. **Result:** append `(worktree: <name> at <path>)` to the summary in both
   sync and async paths so the parent LLM knows where changes landed.
6. **Scheduling:** in `execute_tool_call`, resolve resources per invocation
   (mirroring `make_presentation_for`): `spawn_subagent` with
   `input.worktree == true` → `ToolResources::independent()`; otherwise the
   static metadata (`Barrier`) stands. Two isolated subagents may therefore
   share a wave (concurrent sync fan-out); an isolated subagent no longer
   blocks a same-turn `write_file`/`edit_file`/`bash` in the main tree.
7. **Permissions:** worktree creation is part of the already-approved High
   spawn op; no separate `worktree_create`-style prompt.

## Files

- **Modify:** `crates/tact/src/tool/subagent.rs` — field + metadata
  description, worktree wiring (sync + async), resume reuse, tests.
- **Modify:** `crates/tact/src/worktree/mod.rs` — public `get(name)` on
  `WorktreeManager` + `SharedWorktreeManager` (reuse on resume).
- **Modify:** `crates/tact/src/agent/tool_dispatch.rs` — extract
  `tool_resources_for(prep, work_dir)` with the per-invocation override; unit
  tests.
- **Docs:** `book/12_chapter_subagent.md` + `_zh.md`; Ch 26 both languages.

## Verification

- `cargo test -p tact --lib subagent::` (input deserialization + worktree
  helper) with proxy unset.
- `cargo test -p tact --lib tool_dispatch::` (resource override).
- `cargo clippy -p tact --all-targets` with proxy unset.
- Manual smoke: `spawn_subagent` with `worktree: true` on a git repo creates
  the lane and the child works inside it; resume reuses it; non-git dir errors.
