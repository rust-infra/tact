# TUI Open Affordance Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Finish the recorded Open affordance follow-ups while preserving single-click behavior and legacy double-click compatibility.

**Architecture:** Add end-to-end mouse tests against rendered `OpenAction` hitboxes, migrate code and Mermaid popups from positional indexes to stable block IDs, then align naming and documentation with the public single-click affordance. Keep physical-index wrappers only where existing callers still need them.

**Tech Stack:** Rust, ratatui TestBackend, Cargo workspace tests.

## Global Constraints

- Single-click `[Open]` is the primary affordance; double-click remains compatibility behavior.
- Stable IDs must survive list reordering and stale popup data must close or no-op safely.
- Existing TUI rendering and popup tests must remain green.

## Review Focus

- A rendered button hitbox must route to the correct popup through `handle_mouse_event`.
- Reordering code or Mermaid blocks must not retarget an open popup.
- A stale or missing popup ID must not panic or show unrelated content.
- Legacy physical-index callers must remain behaviorally compatible during migration.
- Documentation and comments must describe single-click Open as primary.

### Task 1: Single-click integration coverage

**Files:**
- Modify: `crates/tui/src/handlers/mouse.rs` tests
- Modify: `crates/tui/src/render/*` tests only if shared render helpers are needed

- [x] Add tests for collapsed tool, subagent, and thinking buttons that render the app, read the recorded hitbox, send one left-button click, and assert the matching popup opens.
- [x] Run the focused handler/render tests and confirm the tests fail before any production change if a missing path is exposed.

### Task 2: Stable Code/Mermaid popup IDs

**Files:**
- Modify: `crates/agent_tui_kit/src/state/ui_types.rs`
- Modify: `crates/tui/src/widgets/state/app/agent.rs`
- Modify: `crates/tui/src/widgets/state/app/popups.rs`
- Modify: `crates/tui/src/handlers/mouse.rs`
- Modify: affected render and fixture tests

- [x] Add stable `block_id` fields to `CodeBlock` and `MermaidBlock`, generate IDs when blocks are finalized, and make popup state store IDs.
- [x] Resolve popup content by ID, closing or returning when the ID is stale.
- [x] Update mouse and test fixtures to open by ID while preserving physical-index compatibility wrappers.
- [x] Add a regression test that reorders blocks and verifies the popup still targets the original block.

### Task 3: Naming and documentation cleanup

**Files:**
- Modify: `docs/tui_rendering.md`
- Modify: `crates/agent_tui_kit/src/i18n.rs`
- Modify: `crates/agent_tui_kit/src/state/mouse_state.rs`
- Modify: `crates/agent_tui_kit/src/state/ui_types.rs`
- Modify: `crates/tui/src/widgets/state/app/messages.rs`
- Modify: `crates/tui/src/widgets/state/mod.rs`
- Modify: `crates/tui/src/handlers/mouse.rs`

- [x] Describe single-click Open as primary and double-click as compatibility in docs/comments.
- [x] Rename physical-index compatibility methods to physical-index names and update callers.
- [x] Label `last_click_*` fields explicitly as legacy double-click state.

### Task 4: Verification

- [x] Run focused `cargo test -p tui` targets sequentially.
- [x] Run `cargo check --workspace` and `cargo test --workspace` sequentially.
- [x] Review the diff and confirm the requested test, stable-ID, docs, naming, compatibility, and StickyTab data-driven follow-ups are addressed.
