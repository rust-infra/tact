# Tact Desktop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a native cross-platform Tact Desktop application (macOS,
Windows, and Linux) over the existing `tact_host` runtime, with a mixed
execution transcript, project-scoped sessions, typed gates and surfaces, the
complete `tact-ui` entry surface, and GitHub Actions release packaging for every
supported platform.

**Architecture:** A thin GPUI shell talks to a GPUI-free `tact_desktop_core`
controller. `tact_host::HostSession` owns the live session lifecycle. A leaf
`tact_catalog` crate is the single command metadata source. `tact_protocol`
gains kernel-originated run/message identity (start and finish), typed gate
attribution, typed surface snapshots, and background collection/cancel support.

**Tech Stack:** Rust 2024, GPUI (`gpui-pre = 0.3.3`), `gpui-ai = 0.8.0`,
`gpui-component = 0.6.0`, `gpui-base = 0.6.0`, `gpui-kit-assets = 0.6.0`,
Tokio, `tact_host`, `tact_protocol`, `tact_catalog`, `im`, SQLite via the
existing Tact store.

**Spec:** [`docs/superpowers/specs/2026-10-30-tact-desktop-design.md`](../specs/2026-10-30-tact-desktop-design.md)

**Status:** Implemented; verification is tracked by the checked task steps
below and the repository's Rust/book gates.

## Global Constraints

- Do not commit or push unless explicitly requested.
- Work on the existing feature branch; never commit directly to `main`.
- Run one Cargo invocation at a time. Parallel Cargo processes contend on the
  workspace `target/` lock.
- Use focused tests while implementing; widen only after the focused tests
  pass; run the full gate once at the end.
- `tact`, `tact_host`, `tact_protocol`, `tact_catalog`, and
  `tact_desktop_core` must not depend on `gpui`, `gpui-ai`, or
  `gpui-component`.
- `tact-desktop` must not expose `tact`, `tact_host`, `tact_protocol`, Tokio
  channel types, SQLite types, or GPUI entity internals in its public API.
- Keep the documented `vendor/gpui-ai` label/a11y patch when updating the
  pinned `gpui-ai 0.8.0` dependency; the fork is not pristine upstream.
- Pin `gpui-ai = "=0.8.0"`, `gpui-pre = "0.3.3"`,
  `gpui-component = "0.6.0"`, `gpui-base = "0.6.0"`, and
  `gpui-kit-assets = "0.6.0"`.
- One active `HostSession` per desktop controller. Project switching is a
  serialized close-then-open transition.
- `UiResponder::snapshot()` is authoritative for gates. One-shot gate events
  are hints only.
- Do not infer tool-group identity from adjacency or wall-clock time.
- Do not parse TUI Markdown for desktop managers; use typed surface snapshots.
- Keep the TUI's terminal themes separate from the desktop Light/Dark
  appearance.
- Ship macOS, Windows, and Linux builds from the same GPUI shell. Platform-
  specific window chrome or packaging must be isolated behind `cfg` modules;
  never claim a platform that the release matrix did not build.

## Review Focus

- A gate whose hint is lost must still appear from the next `UiResponder`
  snapshot, and a stale response must not answer a newer session.
- A late `ToolProgress` or `StepFinished` must not create a duplicate tool card
  or overwrite another tool.
- A streamed assistant message must update one row without rebuilding the full
  transcript list.
- A session switch must not release the old lock while the old driver is still
  alive.
- Every catalog entry must have a real desktop route; an entry that is visible
  but inert is a product bug.
- Project-local session databases must remain isolated; the desktop project
  registry must not become a second session store.
- The process-global Tact configuration must not be reinstalled on every
  project switch.

---

### Task 0: Freeze the boundary and scaffold the crates

**Files:**

- Create: `crates/catalog/Cargo.toml`
- Create: `crates/catalog/src/lib.rs`
- Create: `crates/desktop_core/Cargo.toml`
- Create: `crates/desktop_core/src/lib.rs`
- Create: `crates/desktop/Cargo.toml`
- Create: `crates/desktop/src/main.rs`
- Modify: `Cargo.toml`
- Create: `scripts/check-desktop-deps.sh`
- Modify: `scripts/check-rust.sh`

**Interfaces:**

- Consumes: existing workspace crates `tact`, `tact_host`, `tact_protocol`.
- Produces: workspace members `tact_catalog`, `tact_desktop_core`, and
  `tact-desktop`; pinned GPUI dependencies; a dependency guard.

- [x] **Step 1: Add the workspace dependency pins and members**

Add the three members to `Cargo.toml` and pin:

```toml
gpui-ai = "=0.8.0"
gpui-pre = "0.3.3"
gpui-pre-platform = "0.3.3"
gpui-component = "0.6.0"
gpui-base = "0.6.0"
gpui-kit-assets = "0.6.0"
gpui-component-macros = "0.6.0"
im = "15"
```

`tact-desktop` uses package aliases so source imports remain `gpui`,
`gpui_platform`, `gpui_component`, `gpui_base`, and
`gpui_component_assets`. Its `[dev-dependencies]` must repeat the `gpui-pre`
pin with `features = ["test-support"]`; `#[gpui::test]` is gated behind that
feature (gpui-ai's own manifest does the same). Without it every GPUI test in
Tasks 7-10 fails to compile.

- [x] **Step 2: Scaffold the three crates with empty public modules**

`tact_catalog` is a leaf crate. `tact_desktop_core` depends on
`tact_host`, `tact_catalog`, `tact_protocol`, `tact` (store types only),
`tokio`, `serde`, `chrono`, `im`, `uuid`, and `url`. `tact-desktop` depends on
`tact_desktop_core`, `tact_catalog`, GPUI, `gpui-ai`, and `gpui-component`.

- [x] **Step 3: Write the dependency guard test script**

`scripts/check-desktop-deps.sh` must fail if `gpui`, `gpui-ai`,
`gpui-component`, or `gpui-base` appears in `cargo tree -p tact`,
`cargo tree -p tact_host`, `cargo tree -p tact_protocol`,
`cargo tree -p tact_catalog`, or `cargo tree -p tact_desktop_core`.

- [x] **Step 4: Run the guard and wire it into the gate**

Run: `scripts/check-desktop-deps.sh`
Expected: PASS with all five packages clean.

Add the guard to `scripts/check-rust.sh` as a separate step that prints its own
failure, so a GPUI leak into the host crates fails the normal pre-push gate.
Do not duplicate the `cargo test --workspace` invocation.

- [x] **Step 5: Check the scaffold**

Run: `cargo check -p tact_catalog -p tact_desktop_core -p tact-desktop`
Expected: PASS after the scaffold from Step 2 exists.

### Task 1: Add the shared command catalog and migrate TUI metadata

**Files:**

- Create: `crates/catalog/src/builtin.rs`
- Create: `crates/catalog/src/cli.rs`
- Create: `crates/catalog/src/settings.rs`
- Create: `crates/catalog/src/localized.rs`
- Create: `crates/catalog/src/fuzzy.rs`
- Create: `crates/catalog/src/catalog.rs`
- Modify: `crates/catalog/src/lib.rs`
- Modify: `crates/tui/src/widgets/state/slash.rs`
- Modify: `crates/tui/src/widgets/state/slash_command.rs`
- Modify: `crates/tui/src/widgets/state/app/config.rs`
- Modify: `crates/tui/src/handlers/mod.rs`
- Modify: `crates/tui/src/handlers/insert.rs`
- Modify: `crates/agent_tui_kit/src/i18n.rs`
- Modify: `crates/agent_tui_kit/Cargo.toml`
- Modify: `crates/tui/Cargo.toml`

**Interfaces:**

- Consumes: the existing 23 `SlashCommand` variants and nested subcommand
  tables from `crates/tui/src/widgets/state/slash.rs`.
- Produces:
  `tact_catalog::{builtins, fuzzy_score, BuiltinCommand, CommandId,
  CommandDescriptor, CommandCatalog, CatalogLocale}`.

- [x] **Step 1: Write the catalog coverage tests first**

Add tests with these names:

```rust
#[test]
fn every_builtin_variant_has_one_catalog_spec() {}

#[test]
fn slash_subcommands_form_a_tree_without_duplicate_siblings() {}

#[test]
fn every_cli_entry_has_one_catalog_spec() {}

#[test]
fn every_setting_has_one_catalog_spec() {}

#[test]
fn fuzzy_search_ranks_prefix_then_word_then_subsequence() {}
```

`every_builtin_variant_has_one_catalog_spec` must assert 23 root commands and
the exact names `model-subagent`, `subagent_cancel`, `view-system-prompt`, and
`tasks-dag`.

- [x] **Step 2: Run the catalog tests to verify they fail**

Run: `cargo test -p tact_catalog --lib`
Expected: FAIL because the catalog types and descriptors do not exist.

- [x] **Step 3: Implement `BuiltinCommand` and the subcommand tree**

Move the exact names, order, subcommands, `needs_args`, and `needs_account`
semantics into `tact_catalog`. Keep `tact_catalog` free of `agent_tui_kit`,
`ratatui`, `tact`, and `tact_host`.

- [x] **Step 4: Implement descriptors, localized text, and fuzzy search**

`CommandDescriptor` carries a stable `CommandId`, parent/children, group,
path, localized label/description, aliases, shortcut, arguments, availability,
and action. `CommandId` includes `Builtin`, `BuiltinPath`, `Cli`, `Setting`,
`Composer`, and `DynamicSkill`.

`fuzzy_score` must match case-insensitive prefixes, word starts, and
acronym-like subsequences. Return `None` when the query cannot match.

- [x] **Step 5: Migrate the TUI to the shared catalog**

Replace the private slash enum and subcommand tables with imports from
`tact_catalog`. Keep the TUI's exhaustive dispatch match. The TUI may keep a
compatibility `desc` adapter while its message catalog is migrated, but it must
not keep a second command-name or subcommand list.

Keep `agent_tui_kit::Language` as the terminal locale vocabulary. Do **not**
write `impl From<agent_tui_kit::Language> for CatalogLocale` in `tui` or in
`tact_catalog`: both the trait and the target type are foreign in `tui`
(E0117), and `tact_catalog` must not depend on `agent_tui_kit`. Add a small
TUI-side function instead:

```rust
fn catalog_locale(language: agent_tui_kit::i18n::Language) -> CatalogLocale {
    match language {
        agent_tui_kit::i18n::Language::English => CatalogLocale::En,
        agent_tui_kit::i18n::Language::Chinese => CatalogLocale::Zh,
    }
}
```

Add `language_maps_to_catalog_locale` so the TUI and desktop cannot drift on
the two supported locales.

- [x] **Step 6: Add the source-sync tests**

Add the first two tests in `crates/tui/src/widgets/state/slash.rs`'s `tests`
module (so the `tui slash` filter actually selects them). Keep the existing
`handlers::tests::every_declared_subcommand_has_a_handler` where it is; do not
create a second test with that name.

```rust
#[test]
fn tui_dispatch_is_exhaustive_over_builtin_catalog() {}

#[test]
fn tui_subcommand_completion_matches_catalog_tree() {}
```

Run: `cargo test -p tact_catalog --lib && cargo test -p tui slash && cargo test -p tui every_declared_subcommand_has_a_handler`
Expected: PASS, and the output must show a non-zero test count for each filter.

### Task 2: Add `HostSession` and kernel identity to the host protocol

**Files:**

- Create: `crates/tact_host/src/host_session.rs`
- Modify: `crates/tact_host/src/lib.rs`
- Modify: `crates/tact_host/src/session_bootstrap.rs`
- Modify: `crates/tact_host/src/driver.rs`
- Modify: `crates/protocol/src/agent.rs`
- Modify: `crates/tact/src/runtime/event_adapter.rs`
- Modify: `crates/tui/src/widgets/state/app/agent.rs`
- Modify: `crates/tui/src/headless_loop.rs`
- Modify: `crates/tact-ui/src/interactive.rs`

**Interfaces:**

- Consumes: `open_session`, `bootstrap_session`,
  `run_command_loop_with_account`, `SessionLockGuard`, `UiResponder`.
- Produces: `tact_host::HostSession`, `HostSessionConfig`, `SessionSelection`,
  `HostEvent`, `HostFailure`, `HostCloseError`, `AgentUpdate::RunStarted`,
  `AgentUpdate::RunFinished`, and `AgentUpdate::MessageStarted`.

- [x] **Step 1: Write the protocol identity tests**

Add tests that drive the real `TactEventBridge`:

```rust
#[test]
fn run_started_precedes_first_turn_stats() {}

#[test]
fn run_finished_closes_the_run_cursor() {}

#[test]
fn compaction_run_does_not_open_an_activation() {}

#[test]
fn message_started_precedes_first_stream_chunk_when_provided() {}
```

Assert the exact order of the `AgentUpdate` values and that no text delta
precedes `MessageStarted` when the kernel emits it. The compaction test drives
`UserCommand::Compact` and asserts `RunStarted`/`RunFinished` are forwarded but
no transcript activation is opened.

- [x] **Step 2: Run the protocol tests to verify they fail**

Run: `cargo test -p tact runtime::event_adapter::tests::run_started`
Expected: FAIL because `AgentUpdate::RunStarted` does not exist.

- [x] **Step 3: Add the additive `AgentUpdate` variants**

Add:

```rust
RunStarted { run_id: String }
RunFinished { run_id: String }
MessageStarted { message_id: String }
```

Emit `RunStarted` for `AgentEvent::RunStarted`, `RunFinished` for both
`AgentEvent::RunFinished` and `AgentEvent::RunFailed`, and `MessageStarted`
for `AgentEvent::MessageStarted`. Update every exhaustive `AgentUpdate` match
that must remain compiling, including:

- `crates/tui/src/widgets/state/app/agent.rs` (`shell_handle`,
  `ends_thinking_region`, and any other non-wildcard match);
- `crates/tact/src/runtime/baseline.rs` (frozen baseline normalizer);
- any other production match that is currently exhaustive without `_ =>`.

Ignore the new variants without changing visible behavior. Prefer a shared
helper such as `AgentUpdate::is_desktop_identity_hint()` only if it keeps the
matches readable; otherwise add explicit no-op arms.
All three variants must travel on the reliable `send` lane so a stream delta
cannot overtake its identity hint.

- [x] **Step 4: Write the `HostSession` lifecycle tests**

Add tests:

```rust
#[tokio::test]
async fn session_switch_cancels_closes_awaits_and_releases_lock() {}

#[tokio::test]
async fn driver_timeout_does_not_release_the_lock_and_close_is_retryable() {}

#[tokio::test]
async fn close_during_startup_drops_bootstrap_and_releases() {}
```

Use `tact_host::test_support` and a fake driver/session store. The first test
asserts the old lock is released only after the old driver is joined. The
second asserts a timed-out close returns `DrainTimedOut`, keeps the lock, and
can be retried on the same handle. The third asserts a close before `Ready`
drops the bootstrap future instead of waiting for an MCP handshake.

- [x] **Step 5: Implement `HostSession`**

`HostSession::start` takes an explicit `SessionSelection` (`New`,
`Resume(id)`, `ResumeLast`) rather than reading `args.session`; `args` carries
only process flags. Add `open_session_selected(selection, args, ...)` as the
typed session-opening helper and keep `open_session(&CliArgs, ...)` as a
compatibility wrapper that converts the existing flags into a selection. It
creates or resumes the row, acquires the lock, creates
the update/account/command channels and `UiResponder`, then spawns a
supervisor. The supervisor performs bootstrap and driver startup and emits
`Ready`. Buffered commands are consumed after the driver starts. Install
`SessionLockRegistry::spawn_exit_listener` in the desktop binary before
`DesktopController::start`.

`HostSession::close(&mut self)` performs the seven-step teardown from the spec,
is retryable, and drops an in-flight bootstrap. On timeout it returns
`HostCloseError::DrainTimedOut`, keeps `self` and the lock, so the actor can
show "Waiting for session to stop" and retry. The controller actor is the only
caller of `next_event`; it selects that future with the shutdown intent.

- [x] **Step 6: Migrate the interactive TUI to `HostSession`**

Replace the manual `open_session` / `bootstrap_session` /
`run_command_loop_with_account` / lock-release sequence in
`crates/tact-ui/src/interactive.rs` with `HostSession::start` and
`HostSession::close`. Keep the headless one-shot path unchanged. Preserve the
quit-during-startup behavior: `close` before `Ready` must drop the bootstrap
future and return promptly, not wait for MCP handshakes.

- [x] **Step 7: Run the focused host tests**

Run: `cargo test -p tact_host host_session`
Expected: PASS, with a non-zero test count.

Run: `cargo test -p tui every_declared_subcommand_has_a_handler`
Expected: PASS, proving the new `AgentUpdate` variants did not change TUI
behavior. (The slash filter does not exercise `AgentUpdate`; use the
`widgets::state::app::agent` tests for that path.)

### Task 3: Add typed gate attribution and runtime surface snapshots

**Files:**

- Modify: `crates/protocol/src/agent.rs`
- Modify: `crates/tact/src/ui_responder.rs`
- Modify: `crates/tact/src/runtime/event_adapter.rs`
- Modify: `crates/tact_host/src/driver.rs`
- Create: `crates/tact_host/src/surface.rs`
- Create: `crates/tact_host/src/extensions.rs`
- Modify: `crates/tact_host/src/lib.rs`
- Modify: `crates/tact/src/background.rs`

**Interfaces:**

- Consumes: `PendingUiRequest`, `UiResponse`, existing `UserCommand` surface
  commands, `BackgroundManager`.
- Produces: `UiRequestPurpose`, `UiAgentRef`, `UiRequestOrigin`,
  `UserCommand::QuerySurface`, `AgentUpdate::SurfaceSnapshot`,
  `AgentUpdate::BackgroundTasksChanged`, `UserCommand::CancelBackgroundTask`,
  `HostExtensionService`, and `ExtensionSnapshot`.

- [x] **Step 1: Write the gate attribution tests**

Add tests:

```rust
#[test]
fn pending_gate_preserves_purpose_and_origin() {}

#[test]
fn subagent_gate_names_the_child_and_tool() {}

#[test]
fn stale_gate_response_is_a_noop() {}
```

The first test asserts `UiRequestPurpose::Question` and `UiAgentRef::Main`.
The second asserts a child id and tool id survive the `UiResponder` snapshot.

- [x] **Step 2: Add `UiRequestPurpose`, `UiAgentRef`, and `UiRequestOrigin`**

Add the fields to `PendingUiRequest`, `RequestSelect`, `RequestMultiSelect`,
`register_select`, and `register_multi`. `from_subagent` stays on the wire and
is derived from `origin` (`matches!(origin.agent, UiAgentRef::Subagent { .. })`)
so the TUI keeps its existing behavior. Do **not** rely on serde defaults to
keep Rust struct literals compiling: `#[serde(default)]` only affects
deserialization. Add `Default` where it is truthful, then run `cargo check`
and migrate every construction site the compiler names. A non-authoritative
starting list from the current tree:

- `crates/tact/src/ui_responder.rs`
- `crates/tact/src/tool/ask_user.rs`
- `crates/tact/src/tool/subagent_ui.rs`
- `crates/tact/src/runtime/event_adapter.rs`
- `crates/tact/src/runtime/gate_adapter/tests.rs`
- `crates/tact/src/runtime/baseline.rs`
- `crates/tact/src/agent/tests/tools.rs`
- `crates/tact-ui/src/headless_log.rs`
- `crates/tact-ui/tests/harness/mod.rs`
- `crates/tui/src/widgets/state/app/agent.rs`
- `crates/tui/src/handlers/select.rs`
- `crates/tui/src/render/scene_tests.rs`
- `crates/tui/src/render/render_gap_tests.rs`

Existing callers use `Main` and `None`; permission, plan, and subagent paths
stamp exact values. The subagent path maps `child_id = ctx.session_id.clone()`
and `tool_id` from the current tool context.

- [x] **Step 3: Write typed surface tests**

Add tests:

```rust
#[test]
fn pending_gate_preserves_purpose_and_origin() {}

#[test]
fn subagent_gate_names_the_child_and_tool() {}

#[test]
fn stale_gate_response_is_a_noop() {}

#[test]
fn typed_surface_query_preserves_request_id() {}

#[test]
fn background_snapshot_replaces_previous_collection() {}

#[test]
fn cancel_background_task_cancels_only_that_task() {}
```

- [x] **Step 4: Implement typed surface queries**

Add `SurfaceQuery` and `SurfaceSnapshot` for MCP servers, MCP prompts, hooks,
stats, system prompt, and history. Keep `PopupMarkdown` for the TUI. The
desktop path uses only typed snapshots.

- [x] **Step 5: Implement background snapshots and cancellation**

Add a full replacement `BackgroundTasksChanged` snapshot. Add
`SyncBackgroundTasks` and `CancelBackgroundTask { task_id }`.

`BackgroundManager` currently has no update sink and no per-task handle. Extend
it before wiring the command:

- add an `UpdateSink` (same shape as the agent bridge) and emit the snapshot at
  session ready, task start, finish, error, and cancellation;
- store `task_id -> CancelToken` at `start` and remove it on terminal status;
  `CancelBackgroundTask` cancels only that token, never the session token;
- project the persisted `BackgroundTaskStatus::Error` as
  `BackgroundTaskStatusSnapshot::Error` (do not rename it `Failed`).

Without both changes the Stop button is not implementable; do not ship it as an
inert control.

- [x] **Step 6: Implement `HostExtensionService`**

Move process-level plugin, marketplace, and skill operations behind
`HostExtensionService`. Add `HostExtensionService::snapshot`,
`reload_skills`, `set_skill_enabled`, and `plugin_action`. Add tests:

```rust
#[test]
fn extension_snapshot_reports_skills_plugins_and_marketplaces() {}

#[test]
fn plugin_action_returns_the_new_extension_snapshot() {}
```

- [x] **Step 7: Run the focused surface tests**

Run: `cargo test -p tact_host surface`
Expected: PASS.

Run: `cargo test -p tact background`
Expected: PASS.

Run: `cargo test -p tact_host extensions`
Expected: PASS.

Run: `cargo test -p tact pending_gate_preserves_purpose_and_origin`
Expected: PASS. (The gate attribution tests live in `tact`, not `tact_host`.)

### Task 4: Add session metadata, filtered listing, and the project registry

**Files:**

- Modify: `crates/tact/src/store/mod.rs`
- Modify: `crates/tact/src/store/session_store/mod.rs`
- Modify: `crates/tact/src/store/session_store/sqlite.rs`
- Modify: `crates/tact_host/src/sessions.rs`
- Modify: `crates/tact_host/src/session_bootstrap.rs`
- Modify: `crates/tact_host/src/host_session.rs`
- Create: `crates/desktop_core/src/project.rs`
- Create: `crates/desktop_core/src/session.rs`
- (Removed) `crates/desktop_core/src/settings.rs`: settings live in
  `config.rs` (persisted desktop settings) and `state.rs` (live/session
  application), so a separate module would split one owner.

**Interfaces:**

- Consumes: `SessionStore`, `SessionSummary`, `TactPath`.
- Produces: `SessionListQuery`, `ArchiveFilter`, `set_session_title`,
  `set_session_archived`, `ProjectRegistry`, `ProjectRecord`,
  `SessionCatalog`, and `SessionSummaryView`.

`SessionListQuery` and `ArchiveFilter` are defined in `tact::store`, next to
`SessionSummary` and the `SessionStore` trait. Defining them in
`tact_desktop_core` would make the store trait depend on a crate that depends
on the store.

- [x] **Step 1: Write the migration tests**

Add in `crates/tact/src/store/session_store/sqlite.rs`:

```rust
#[tokio::test]
async fn migration_adds_title_and_archived_at_to_existing_sessions() {}

#[tokio::test]
async fn migration_is_idempotent_across_two_store_opens() {}

#[tokio::test]
async fn first_real_user_prompt_derives_title() {}

#[tokio::test]
async fn injected_context_does_not_derive_title() {}

#[tokio::test]
async fn tool_result_does_not_derive_title() {}

#[tokio::test]
async fn manual_title_is_not_overwritten() {}

#[tokio::test]
async fn archive_filter_returns_active_archived_and_all() {}

#[tokio::test]
async fn archive_and_restore_are_idempotent() {}
```

Add `resume_last_skips_archived_sessions` in
`crates/tact_host/src/session_bootstrap.rs`: `tact` cannot depend on
`tact_host`, so the resume-selection path is only reachable from `tact_host`.


- [x] **Step 2: Run the migration tests to verify they fail**

Run: `cargo test -p tact store::session_store`
Expected: FAIL because `title` and `archived_at` are absent.

Run: `cargo test -p tact_host resume_last_skips_archived_sessions`
Expected: FAIL because resume selection still sees archived rows.

- [x] **Step 3: Implement the additive schema migration**

Add nullable `title TEXT` and `archived_at TIMESTAMP` columns, the
`idx_sessions_root_archived_updated` index, and the `SessionSummary` fields
`title`, `archived_at`, and derived `locked`. `locked` is projected from
`locked_by` / `lock_epoch`; do not add a `locked` SQL column.

Probe `PRAGMA table_info(sessions)` before each `ALTER` (use the existing
`add_nullable_column` helper). `SqliteSessionStore::new` runs on every open, so
a bare `ALTER` would fail the second launch with `duplicate column name`.

- [x] **Step 4: Implement filtered listing and title derivation**

Keep `list_sessions(Some(root))` as a compatibility wrapper. Add
`list_sessions_filtered(SessionListQuery)`. In `append_agent_message`, derive a
title only for `MessageOrigin::User`, only when `title IS NULL`. Add the
idempotent backfill for legacy rows.

- [x] **Step 5: Write the project registry tests**

Add:

```rust
#[test]
fn project_registry_deduplicates_canonical_roots() {}

#[test]
fn missing_project_remains_visible_unavailable() {}

#[test]
fn project_registry_writes_atomically() {}
```

- [x] **Step 6: Implement `ProjectRegistry` and `SessionCatalog`**

Store only project roots and presentation metadata in the desktop app-data
directory. Open each project's `<root>/.tact/tact.db` lazily. Use
`tact_catalog::fuzzy_score` for session search.

- [x] **Step 7: Run the focused session tests**

Run: `cargo test -p tact store::session_store`
Expected: PASS.

Run: `cargo test -p tact_host resume_last_skips_archived_sessions`
Expected: PASS.

Run: `cargo test -p tact_desktop_core project`
Expected: PASS.

### Task 5: Build the desktop transcript reducer and gate state

**Files:**

- Create: `crates/desktop_core/src/transcript/mod.rs`
- Create: `crates/desktop_core/src/transcript/row.rs`
- Create: `crates/desktop_core/src/transcript/reducer.rs`
- Create: `crates/desktop_core/src/transcript/history.rs` (module doc; history
  seeding is `TranscriptReducer::seed_persisted`)
- Create: `crates/desktop_core/src/transcript/tool_group.rs` (module doc; the
  grouping algorithm lives in `reducer.rs`)
- Create: `crates/desktop_core/src/transcript/output.rs`
- Create: `crates/desktop_core/src/gates.rs`
- Create: `crates/desktop_core/src/state.rs`
- Modify: `crates/desktop_core/src/lib.rs`

**Interfaces:**

- Consumes: `AgentUpdate`, `AgentMessage`, `PendingUiRequest`.
- Produces: `TranscriptReducer`, `TranscriptView`, `TranscriptRow`, `RowId`,
  `RowBody`, `ToolGroupRow`, `GateCard`, `DesktopState`, and `StateCommit`.

- [x] **Step 1: Write the row identity tests**

Add:

```rust
#[test]
fn run_started_and_message_started_keep_stable_rows() {}

#[test]
fn run_finished_closes_the_run_cursor() {}

#[test]
fn compaction_run_does_not_open_an_activation() {}

#[test]
fn turn_stats_allocates_one_tool_group() {}

#[test]
fn stream_chunks_append_to_one_assistant_row() {}

#[test]
fn thinking_lifecycle_creates_one_row_and_finishes() {}

#[test]
fn tool_progress_batches_do_not_duplicate_tool_rows() {}

#[test]
fn tool_group_includes_gate_and_subagent_rows() {}

#[test]
fn standalone_gate_and_diff_have_top_level_rows() {}
```

- [x] **Step 2: Run the reducer tests to verify they fail**

Run: `cargo test -p tact_desktop_core transcript`
Expected: FAIL because the reducer and row types do not exist.

- [x] **Step 3: Implement the row model and reducer**

Implement the `TranscriptRow`/`RowBody` model, `RowId` allocation, `LoopId`
allocation on the first `TurnStats` after `RunStarted`, assistant/thinking
cursors from `MessageStarted`, tool grouping by loop, standalone
`RowBody::Gate`/`RowBody::Diff` rows, and typed notices. A `RunFinished` for a
pending run that never saw `TurnStats` must leave no row.

- [x] **Step 4: Implement streaming coalescing**

Coalesce adjacent `StreamChunk`, `ThinkingChunk::Delta`, and `ToolProgress`
values before the reducer. Store Markdown as `Arc<str>` and tool output as a
bounded preview plus optional full detail. Do not parse Markdown in the
reducer.

- [x] **Step 5: Write the gate reconciliation tests**

Add:

```rust
#[test]
fn stale_gate_hint_is_repaired_from_snapshot() {}

#[test]
fn resolved_gate_collapses_without_losing_history() {}

#[test]
fn session_epoch_rejects_stale_gate_response() {}

#[test]
fn gate_origin_is_not_inferred_from_prompt_text() {}
```

- [x] **Step 6: Implement gate reconciliation**

Apply `UiResponder::snapshot()` by `request_id`. Keep historical rows. Mark
absent pending requests `Withdrawn`, answered requests `Resolved`, and stale
responses `Stale`. Gate rows carry `UiRequestPurpose` and `UiRequestOrigin`,
plus the `session_epoch` they were rendered under. The controller rejects a
response whose card epoch differs from the current epoch; clearing gates on a
session switch is mandatory because request ids restart at zero.

- [x] **Step 7: Write history seeding tests**

Add:

```rust
#[test]
fn persisted_assistant_tool_uses_form_one_group() {}

#[test]
fn persisted_tool_results_pair_by_tool_use_id() {}

#[test]
fn persisted_injected_context_is_not_a_user_turn() {}
```

- [x] **Step 8: Run the focused reducer tests**

Run: `cargo test -p tact_desktop_core transcript`
Expected: PASS.

Run: `cargo test -p tact_desktop_core gate`
Expected: PASS. (The filter is `gate`, not `gates`; the reconciliation tests live
in `transcript/reducer.rs` and every name contains `gate`.)

### Task 6: Build the desktop controller and runtime actor

**Files:**

- Create: `crates/desktop_core/src/controller.rs`
- Create: `crates/desktop_core/src/runtime.rs`
- Create: `crates/desktop_core/src/config.rs`
- Create: `crates/desktop_core/src/snapshot.rs`
- Create: `crates/desktop_core/src/intent.rs`
- Create: `crates/desktop_core/src/effect.rs`
- Create: `crates/desktop_core/src/error.rs`
- Create: `crates/desktop_core/src/commands.rs`
- Modify: `crates/desktop_core/src/lib.rs`

**Interfaces:**

- Consumes: `HostSession`, `HostEvent`, `SessionCatalog`, `ProjectRegistry`,
  `TranscriptReducer`.
- Produces: `DesktopConfig`, `DesktopController`, `DesktopEvents`,
  `DesktopIntent`, `DesktopEvent`, `DesktopSnapshot`, `StateCommit`,
  `DesktopEffect`, `FailureView`, and `ReaderRequest`.

`DesktopConfig` and `DesktopSnapshot` need owners: `config.rs` and
`snapshot.rs` respectively. Task 7 consumes `DesktopSnapshot`, so it cannot be
left implicit.

- [x] **Step 1: Write the controller lifecycle tests**

Add:

```rust
#[tokio::test]
async fn project_switch_closes_before_opening() {}

#[tokio::test]
async fn session_epoch_changes_on_switch() {}

#[tokio::test]
async fn shutdown_joins_host_and_stops_event_pump() {}

#[tokio::test]
async fn close_timeout_keeps_the_handle_retryable() {}

#[tokio::test]
async fn startup_failure_is_a_typed_desktop_failure() {}

#[test]
fn session_filter_and_query_intents_update_session_state() {}

#[test]
fn back_to_bottom_resets_tail_following() {}

#[test]
fn controller_never_reinitializes_process_config() {}

#[test]
fn permission_mode_is_reapplied_after_ready() {}

#[test]
fn startup_global_setting_reports_restart_required() {}

#[test]
fn live_setting_routes_to_user_command() {}
```

- [x] **Step 2: Run the controller tests to verify they fail**

Run: `cargo test -p tact_desktop_core controller`
Expected: FAIL because the controller does not exist.

- [x] **Step 3: Install process-global Tact config once**

Call `DesktopConfig::install_process_config()` in the launcher path
(`crates/desktop/src/app.rs::run_inner`, reached from `main.rs`) before
`DesktopController::start`, and install
`SessionLockRegistry::spawn_exit_listener` there as well. Implement the config
method in `tact_desktop_core`, delegating to the existing host/config facade.
Also implement `DesktopConfig::save` and `persist_subagent_model` (delegating
to `tact::config`). The GPUI shell must not import `tact` or `tact_host`.
`DesktopController` must never call the config installer. Add a test named
`controller_never_reinitializes_process_config` that opens two project
switches and asserts the config installation count remains one.

- [x] **Step 4: Implement the actor loop**

The controller owns one private Tokio runtime and one actor task. The actor
selects over shell intents, `HostEvent`s, gate reconciliation ticks, session
store futures, and shutdown. It never exposes Tokio handles. It is the only
caller of `HostSession::next_event`; a `Shutdown` intent breaks the select and
calls `close` on the same handle. A `DrainTimedOut` keeps the handle and emits
a retryable "Waiting for session to stop" failure.

- [x] **Step 5: Implement session switching**

`OpenProject` and `ResumeSession` first close the active `HostSession`, then
increment `SessionEpoch`, clear `pending_gates`/mark them `Withdrawn`, then open
the new session. Do not open a second host while the old one is alive, and do
not publish the new epoch before the old gates are withdrawn.

- [x] **Step 6: Implement effects and settings**

Map `DesktopIntent` to `UserCommand` or a host/store operation inside the
controller. Emit only domain effects to the shell: copy, open path, reveal,
external URL, reader, save transcript, notification, window close.

Implement `SetSetting` with the two application modes from the spec:
startup-global settings update desktop settings and report "Requires restart";
live settings route to `UserCommand::SetModel`, `SetReasoningEffort`,
`SetThinkingBudget`, `SetPermissionMode`, or shell appearance. After every
`HostEvent::Ready`, send the chosen permission mode before accepting a submit.

- [x] **Step 7: Add a fake-host harness**

Add a test-only `FakeHost` behind `#[cfg(test)]` that can emit
`RunStarted`, `RunFinished`, `MessageStarted`, `TurnStats`, `StepStarted`,
`StreamChunk`, and gate snapshots. The reducer tests must not require a real
model or network.

- [x] **Step 8: Run the focused controller tests**

Run: `cargo test -p tact_desktop_core controller`
Expected: PASS.

### Task 7: Build the GPUI shell and the mixed transcript

**Files:**

- Create: `crates/desktop/src/app.rs`
- Create: `crates/desktop/src/lib.rs`
- Create: `crates/desktop/src/root.rs`
- Create: `crates/desktop/src/theme.rs`
- Create: `crates/desktop/src/ui/shell.rs`
- Create: `crates/desktop/src/ui/title_bar.rs`
- Create: `crates/desktop/src/ui/session_rail.rs`
- Create: `crates/desktop/src/ui/transcript.rs`
- Create: `crates/desktop/src/ui/transcript_row.rs`
- Create: `crates/desktop/src/ui/composer.rs`
- Create: `crates/desktop/src/ui/sticky.rs`
- Create: `crates/desktop/src/keymap.rs`
- Create: `crates/desktop/src/ui/layout.rs`
- Modify: `crates/desktop/src/main.rs`

**Interfaces:**

- Consumes: `DesktopController`, `DesktopSnapshot`, `StateCommit`, `RowId`,
  `TranscriptRow`, `tact_catalog`.
- Produces: the cross-platform shell, virtualized transcript, composer, sticky strip,
  and Light/Dark theme.

`crates/desktop/src/lib.rs` is the testable library root; `main.rs` remains a
thin launcher so `cargo test -p tact-desktop --lib` can exercise the shell.

- [x] **Step 1: Write the shell smoke tests**

Add:

```rust
#[gpui::test]
fn desktop_shell_renders_all_transcript_row_kinds() {}

#[gpui::test]
fn light_and_dark_share_component_geometry() {}

#[gpui::test]
fn reduced_motion_uses_direct_state_changes() {}

#[gpui::test]
fn gate_focus_restores_to_trigger() {}

#[gpui::test]
fn keymap_matches_prd_interaction_table() {}

#[gpui::test]
fn narrow_window_moves_panels_to_scrimmed_overlays() {}

#[gpui::test]
fn text_scale_125_and_150_remain_usable() {}
```

- [x] **Step 2: Run the smoke tests to verify they fail**

Run: `cargo test -p tact-desktop --lib`
Expected: FAIL because the shell does not exist.

- [x] **Step 3: Implement the root and theme**

Call `gpui_ai::init(cx)` once. Root every window in
`gpui_component::Root`. Apply Light/Dark tokens to the shell and use
`gpui-ai` semantic tokens for component text and code.

- [x] **Step 4: Implement the event pump and dirty-row updates**

Poll `DesktopEvents::recv()` with a GPUI foreground task. Apply each
`StateCommit` to the shell. Update only row entities named in `dirty_rows`.
Keep draft text, cursor, focus, selection, and scroll in the shell.

- [x] **Step 5: Implement the mixed transcript**

Use `StreamingText` for assistant Markdown, `Thinking` for reasoning,
`ToolGroup`/`ToolCall` for tool rows, `ApprovalCard`/`QuestionFlow`/`ChoiceGroup`
for gates, `CodeBlock`/`CodeDiff` for evidence, and `ContextMeter` for context.
Tact-owned rows cover user, standalone gate/diff, notice, hook context, hook
status, error, and subagent drawer. Add `Suggestions` and `SelectionActions`
around assistant Markdown, and keep a readable code-block fallback for
Mermaid/ANSI content until the P2 adapter lands.

- [x] **Step 6: Implement the composer and sticky strip**

Compose `PromptBar` with a Tact-owned nested slash overlay. Implement queue
chips, `AttachmentStrip`, model/permission menus, pending permission banner,
Sticky Tasks, and Back to bottom. Do not mount `VoiceControls` in this task:
voice is P2 and has no transcription pipeline yet.

- [x] **Step 6b: Implement the keymap and responsive layout**

Implement `keymap.rs` from PRD section 12 and `ui/layout.rs` for the three-column to
scrimmed-overlay breakpoint. Text scale is applied through the shell's scale
factor, not per-widget hard-coded sizes. Add the three new smoke tests above.

- [x] **Step 7: Run the GPUI smoke tests**

Run: `cargo test -p tact-desktop --lib`
Expected: PASS.

### Task 8: Add project/session manager and parity surfaces

**Files:**

- Create: `crates/desktop/src/ui/session_manager.rs`
- Create: `crates/desktop/src/ui/inspector.rs`
- Create: `crates/desktop/src/ui/goals.rs`
- Create: `crates/desktop/src/ui/subagents.rs`
- Create: `crates/desktop/src/ui/background.rs`
- Create: `crates/desktop/src/ui/extensions.rs`
- Create: `crates/desktop/src/ui/readers.rs`
- Create: `crates/desktop/src/ui/task_dag.rs`
- Create: `crates/desktop/src/ui/balance.rs`
- Create: `crates/desktop/src/ui/system_prompt.rs`
- Create: `crates/desktop/src/ui/subagent_model.rs`
- Create: `crates/desktop/src/ui/overlays.rs`
- Modify: `crates/desktop/src/ui/shell.rs`

**Interfaces:**

- Consumes: `SessionListState`, `GoalView`, `SubagentView`,
  `BackgroundTaskSnapshot`, `SurfaceSnapshot`, `ExtensionsView`.
- Produces: project switcher, fuzzy session manager, Active/Archived/All
  filters, Goal panel, subagent drawer, background list, extension manager, and
  typed readers.

- [x] **Step 1: Write the parity surface tests**

Add:

```rust
#[gpui::test]
fn session_manager_filters_active_and_archived() {}

#[gpui::test]
fn session_manager_fuzzy_search_finds_abbreviations() {}

#[gpui::test]
fn goals_and_subagents_use_the_shared_row_grammar() {}

#[gpui::test]
fn background_stop_targets_one_task() {}

#[gpui::test]
fn extensions_manager_carries_query_into_full_dialog() {}

#[gpui::test]
fn balance_reader_renders_account_snapshot() {}

#[gpui::test]
fn help_and_save_entries_are_executable() {}

#[gpui::test]
fn hook_output_toggle_hides_hook_status_rows() {}
```

- [x] **Step 2: Implement the session manager**

Use `SidebarNav` for the project pane and Tact-owned rows for sessions. The
manager supports project filtering, fuzzy search, Active/Archived/All,
rename, archive/restore, and long-list scrolling.

- [x] **Step 3: Implement the inspector**

Add Run, Goals, Subagents, and Context tabs. Use `ContextMeter` and
`InsightCard` where they fit. Goals and subagents use typed views and existing
`UserCommand` Goal/subagent actions.

- [x] **Step 4: Implement background and extensions**

Background uses `BackgroundTasksChanged` and `CancelBackgroundTask`.
Extensions use `FilterTable`/`Dialog` with typed MCP, hook, skill, and plugin
snapshots. `Manage` carries the current search query into the full dialog.

- [x] **Step 4b: Add the remaining P1 readers and pickers**

Render `/tasks-dag` from `TaskView.blocks` / `blocked_by` as a Tact-owned graph
or dependency-list fallback. Render `/balance` from the snapshot's
`AccountView` through `InsightCard` and `ReaderRef::Balance`. Render
`/view-system-prompt` from `SurfaceSnapshot::SystemPrompt`, and add a
subagent-model picker for `/model-subagent`. Wire `/save` to
`DesktopEffect::SaveTranscript`, `/help` to `ReaderRef::Help`, and
`/hook-output` to a persisted visibility setting that suppresses
`HookStatus` rows.

- [x] **Step 5: Implement readers**

Add tool output, diff, thinking, system prompt, stats, history, help, balance,
subagent, and background readers. Readers do not submit tasks or mutate runtime
state.

- [x] **Step 6: Run the parity surface tests**

Run: `cargo test -p tact-desktop --lib session_manager`
Expected: PASS with a non-zero test count.

Run: `cargo test -p tact-desktop --lib goals_and_subagents_use_the_shared_row_grammar`
Expected: PASS. (`cargo test -p tact-desktop --lib inspector` also selects the
inspector module tests and is a useful secondary filter.)

### Task 9: Add Command Center, slash overlay, and settings

**Files:**

- Create: `crates/desktop/src/ui/command_center.rs`
- Create: `crates/desktop/src/ui/slash_overlay.rs`
- Create: `crates/desktop/src/ui/command_forms.rs`
- Create: `crates/desktop/src/ui/settings.rs`
- Create: `crates/desktop_core/src/command_router.rs`
- Modify: `crates/desktop_core/src/commands.rs`

**Interfaces:**

- Consumes: `tact_catalog::builtins`, `CommandId`, `CommandDescriptor`,
  `ArgumentSpec`, `ChoiceSource`.
- Produces: flat `CommandSearch` items, nested slash navigation, value
  pickers, command forms, and settings surfaces.

- [x] **Step 1: Write the command routing tests**

Add:

```rust
#[test]
fn every_catalog_entry_has_a_desktop_route() {}

#[test]
fn nested_slash_paths_complete_and_execute() {}

#[test]
fn free_form_arguments_open_forms() {}

#[test]
fn command_availability_disables_unavailable_actions() {}

#[test]
fn every_cli_flag_has_a_setting_route() {}

#[test]
fn search_returns_one_hit_per_canonical_action() {}
```

The CLI flag test uses the exception table in the spec: `--session` and
`--resume-last` are navigation, `--notifications`/`--no-notifications` and
`--no-micro-compact` are two spellings of one setting, and `--list-sessions` is
a `CliEntry`.

- [x] **Step 2: Run the command tests to verify they fail**

Run: `cargo test -p tact_desktop_core command`
Expected: FAIL because the router does not exist.

- [x] **Step 3: Implement the desktop command router**

Match every `CommandAction` exhaustively. Route builtins to `UserCommand`,
host operations, or overlays. Route `/mcp auth` and `/mcp login` to
`UserCommand::McpAuth`; do not leave the auth verb unmapped. Route settings to
the settings adapter. Route composer actions to the shell, including the
`SessionActions` entry. Dynamic skills use `CommandId::DynamicSkill`.

- [x] **Step 4: Implement Command Center and slash overlay**

Use `CommandSearch` for the flat Command Center. Empty query shows eight recent
commands. Typing searches the full catalog. `PromptBar` remains the editor;
the Tact overlay owns nested paths, value candidates, Tab completion, Enter
execution, and forms.

Bind `Cmd+K`, `Cmd+B`, `Cmd+Alt+B`, `Cmd+Enter`, `Esc`, `Cmd+.`, `Cmd+1..4`,
`Option+Up/Down`, `Space`, `Enter`, `Tab`, and `Shift+Tab` to the intents in
the spec. Add keyboard-only traversal and focus-restoration tests.

- [x] **Step 5: Implement settings**

Expose config path, provider, model, subagent model, effort, API key, base
URL, max tokens, thinking budget, permission mode, notifications, context
window, snapshot size, micro-compaction, tokio diagnostics, skill-body
injection, hook-output visibility, appearance, and language.

Each row declares its application mode. Startup-global settings report
"Requires restart" and never claim a live effect. Live settings route through
`DesktopIntent::SetSetting` to `UserCommand::SetModel`,
`SetReasoningEffort`, `SetThinkingBudget`, or `SetPermissionMode`. Subagent
model goes through `DesktopConfig::persist_subagent_model` (applies to the next
child), not `SetModel`. A config-path change reports that a restart is
required.

Add Command Center and slash routes for `/balance`, `/view-system-prompt`,
`/tasks-dag`, and `/model-subagent`; each route opens a typed surface or picker
instead of leaving text in the composer. Add the `/save`, `/help`, and
`/hook-output` routes from Task 8.

- [x] **Step 6: Run the command and settings tests**

Run: `cargo test -p tact_desktop_core command`
Expected: PASS with a non-zero test count.

Run: `cargo test -p tact_desktop_core nested_slash_paths_complete_and_execute`
Expected: PASS. (The routing tests live in `tact_desktop_core`; `tact-desktop`
tests the rendered Command Center.)

### Task 10: Finish accessibility, motion, i18n, packaging, and parity

**Files:**

- Modify: `crates/desktop/src/theme.rs`
- Modify: `crates/desktop/src/ui/*`
- Create: `crates/desktop/src/i18n.rs`
- Create: `crates/desktop/src/platform/macos.rs`
- Create: `crates/desktop/tests/parity.rs`
- Modify: `docs/superpowers/specs/2026-10-30-tact-desktop-design.md`
- Modify: `docs/superpowers/plans/2026-10-30-tact-desktop.md`

**Interfaces:**

- Consumes: all previous tasks.
- Produces: accessible, reduced-motion-aware, Light/Dark, i18n-complete
  desktop build with a parity test suite.

- [x] **Step 1: Add the parity test matrix**

Add tests:

```rust
#[test]
fn all_23_builtin_slash_commands_have_desktop_routes() {}

#[test]
fn all_declared_subcommands_have_desktop_routes() {}

#[test]
fn all_cli_management_entries_have_desktop_routes() {}

#[test]
fn all_settings_flags_have_desktop_routes() {}

#[test]
fn all_composer_entries_have_desktop_routes() {}

#[test]
fn cli_tree_and_flags_match_command_catalog() {}
```

The harness must fail on any visible-but-inert entry. `all_declared_subcommands`
cannot use the catalog as its only oracle: after the migration the catalog is
the source of truth for both sides. Compare three sources instead: the catalog,
the TUI subcommand tree, and a Clap `CommandFactory` walk for CLI paths and
top-level flags. The flag walk applies the exception table from Task 9.

- [x] **Step 2: Add accessibility and i18n checks**

Assert every row, gate, tab, search field, and icon has a role/label or is
decorative. Assert no component-owned English string leaks into the Chinese
catalog. Add keyboard-only traversal and focus-restoration tests, and assert
the keymap covers every binding in PRD section 12.

- [x] **Step 3: Add reduced-motion and appearance checks**

Assert `ReducedMotion::Always` skips scripted transitions, Light/Dark share
geometry, and `prefers-contrast`/reduced-transparency variants use stronger
borders or solid surfaces. Include the narrow-window and 125%/150% text-scale
cases in the same matrix.

- [x] **Step 3b: Lock P2 fallback behavior**

Assert Mermaid and ANSI content has a readable code-block fallback before the
P2 adapters. Keep `SearchResults`, `ArtifactPanel`, `DiffTable`, and
`ContextCard` explicitly deferred rather than silently unreachable.

- [x] **Step 4: Add the cross-platform packaging smoke**

Build the release binary on macOS, Windows, and Linux, verify each starts from a
clean app-data directory, and verify the release workflow packages the macOS
`.app`, Linux `tar.gz`, and Windows `zip` assets. The GitHub Actions matrices in
`.github/workflows/rust.yml` and `.github/workflows/release.yml` are the
executable acceptance for all three targets. The off-screen render walk in
`crates/desktop/examples/ui_capture.rs` and the runtime accessibility walk in
`docs/design/tact-desktop/visual-verification.md` cover the visual and
interaction checks on the host platform.

- [x] **Step 5: Run the full verification gate**

Run: `scripts/check-rust.sh`
Expected: PASS.

Run: `python3 scripts/book-check.py book`
Expected: PASS.

- [x] **Step 6: Run the desktop parity suite**

Run: `cargo test -p tact-desktop --test parity`
Expected: PASS with every catalog entry routed.

## Self-Review

- Spec coverage: the plan covers the catalog, host facade, protocol identity
  (start, finish, message), gates, background snapshots and per-task cancel,
  session migration, project registry, transcript, controller, settings
  application, GPUI shell, keymap, responsive layout, parity surfaces, command
  center, accessibility, motion, i18n, macOS/Windows/Linux packaging, and
  verification.
- Type consistency: `HostSession`, `SessionSelection`, `DesktopConfig`,
  `DesktopController`, `DesktopSnapshot`, `StateCommit`, `TranscriptReducer`,
  `CommandCatalog`, `SessionListQuery`, and `ArchiveFilter` are used
  consistently across tasks and have an owning file.
- Verification: every "Expected: PASS" command names a test that exists in the
  same task (or an earlier one), and every new test named in the design's
  testing strategy appears in the plan.
- Review focus: each risk named above has a focused test in the task that owns
  the code.
- Proportion: the plan specifies interfaces and tests; implementation bodies
  are intentionally left to the executor.
