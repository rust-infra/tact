# TUI Component Library Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract Tact's TUI crate into a reusable **agent-TUI kit** (`agent_tui_kit`) — thinking card, tool card, streaming markdown log, popup family, task/plan panels, input box, status/bottom bar — consumable by any project via one contract: *feed an `AgentUpdate`-shaped stream in, receive `UserCommand`-shaped commands out*. Tact's own specifics (plugins, voice, skills, balance/quota) remain an app layer in `crates/tui` on top of the kit.

**Architecture:** Introduce a `Component` trait + shared `Ctx`; decompose the monolithic `App` and its giant `handle_agent_update` match into a priority-ordered component registry. The shared log becomes `LogCoordinator` (priority 0). The kit depends only on `tact_protocol` (the existing types-only wire contract) + ratatui; `crates/tui` becomes the Tact app layer. All moves of rendering code are **verbatim** (`&App` → `&Ctx` only) — zero visual change, verified by the existing test suite at every phase gate.

**Tech Stack:** Rust (edition 2024), ratatui + crossterm, `tact_protocol` (unchanged in Phase 1), existing `Renderable` trait as the model for `Component`, headless harness (`headless_loop.rs`, `test_support.rs`) as the verification backbone.

**Design:** `docs/superpowers/specs/2026-08-18-tui-component-library-design.md`.

## Options

- **A (target):** full extraction into `agent_tui_kit` with the `Component` trait; `crates/tui` becomes the app layer.
- **B (fallback / de-risk):** publish the existing `tui` crate as-is (documented channel-wiring contract). Chosen only if the Phase 0 gate fails (e.g. `Component` trait cannot cover some update path without behavior change).

## Global Constraints

- Cargo commands run **serially** — never two `cargo test`/`build`/`clippy` processes at once (they contend on the `target/` lock).
- Unset `http_proxy`, `https_proxy`, `all_proxy` for `cargo test` and `git push` (wiremock tests break through the proxy).
- Every phase ends with a gate: `cargo fmt --all` + `cargo clippy` + `env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib` **all green with no reduction in test count** + `cargo test -p tact-ui` green. No phase advances past a failing gate.
- The AGENTS.md TUI shadow/residue invariants (six rules) carry over verbatim into the kit; every moved render unit keeps its buffer-level tests.
- Pure refactor: **no behavior change, no visual change** until the kit crate exists. Any visual change is a separate, reviewed diff.
- One independently revertable commit per phase.
- Update `book/23_chapter_tui{,_zh}.md` and `book/26_chapter_issue{,_zh}.md` in the same change when the architecture ships (Phase 5).

## File Map

- New: `crates/agent_tui_kit/**` (lib.rs, protocol.rs, theme.rs, i18n.rs, shell.rs, components/, render/, handlers/, bridge.rs, examples/mock_agent.rs), `docs/superpowers/specs/2026-08-18-tui-component-library-design.md`, `docs/superpowers/plans/2026-08-18-tui-component-library.md` (this plan).
- Modify: root `Cargo.toml` (workspace member `agent_tui_kit`), `crates/tui/Cargo.toml` (depend on kit; keep tact/tact_llm only for the app layer), `crates/tui/**` (remove moved code; App becomes shell assembly + extension impl).
- Untouched (Phase 1 guarantee): `crates/protocol/**`, `crates/tact/**`, `crates/tact_llm/**` (Phase 4 only removes *TUI-side* imports of tact/tact_llm; the crates themselves stay put).
- Book/docs sync (Phase 5): `book/23_chapter_tui{,_zh}.md`, `book/26_chapter_issue{,_zh}.md`.

---

## Phase 0 — Baseline freeze and contract draft (no production change)

- [x] **T0.1 Baseline.** Record current test counts: `cargo test -p tui --lib` and `cargo test -p tact-ui` (env-unset). Confirm the `test-support` feature and headless harness build. Record numbers in the Phase 0 results section below.
- [x] **T0.2 Kit skeleton.** Create `crates/agent_tui_kit` (empty lib) with only `tact_protocol` + ratatui-family deps. Draft `Component<U, E>` trait, `Ctx`, `AgentBridge`, `BridgeExtension` signatures (compile-only, no call sites). Add to root workspace members.
- [x] **T0.3 Protocol audit.** Enumerate every `AgentUpdate` (19 variants) and `UserCommand` variant, its TUI consumer file/function, and classify **core** vs **extension**. Fill in the design doc's §8 appendix table.
- [x] **T0.4 GO/NO-GO.** Proceed only if every update path classifies cleanly (no variant needs cross-component mutation that a coordinator pre-pass cannot express). Otherwise adopt Option B and close this plan.

## Phase 1 — Extract LogCoordinator (highest-coupling node first)

- [x] **T1.1 Move log ownership.** Move `App::log_items` + placeholder/separator/system-message helpers (`add_*_separator`, `remove_loading_placeholder`, `append_system_markdown`, splice helpers) into a `LogCoordinator` struct. `App` keeps equivalent delegating methods as a compatibility layer (no call-site churn yet).
- [x] **T1.2 Rewire direct writes.** Change `handle_agent_update` paths that mutate `log_items` directly to go through coordinator accessors (`push_item`, `remove_placeholder`, `splice_stream_lines`, `flush`).
- [x] **T1.3 Gate.** Full test suite; zero test edits allowed except field-access renames inside the crate.

## Phase 2 — Decompose `handle_agent_update` into component dispatch

- [x] **T2.1 Registry.** Add the priority-ordered component registry (`Vec<(u8, Box<dyn Component>)>`) to the shell; `handle_agent_update` becomes: coordinator pre-pass (thinking-close safety net, placeholder removal) → sequential `on_update` dispatch.
- [x] **T2.2 Migrate update arms.**
  - `StepAdded/StepStarted/StepFinished/StepFailed` → `ToolComponent`
  - `ThinkingChunk::{Started,Delta,Finished}` → `ThinkingComponent`
  - `StreamChunk/StreamDone` → `StreamComponent` (flush + splice via coordinator)
  - `TaskComplete/TaskCancelled/Error/Info/MdInfo` → coordinator + status transitions (Status kept in the shell for now)
  - `TokenUsage/ModelInfo` → `StatusBarComponent`
  - `TasksChanged/BackgroundTaskFinished` → `TaskPanelComponent`
- [x] **T2.3 Extension events.** Convert plugin/account variants into `ExtensionEvent` values, handled in the tui layer exactly where they are today (no relocation of Tact logic yet).
- [x] **T2.4 Gate.** Full test suite; this phase must show **zero visual diffs** in scene/render tests.

## Phase 3 — Create `agent_tui_kit`: verbatim relocation

- [x] **T3.1 Move modules.** Move `theme.rs`, `i18n.rs`, `render/` (cells, markdown, mermaid_sequence, bar, input, layout, popups), component states, and widgets into the kit. Mechanical-only change: `&App` → `&Ctx`, `App`-field reads → accessor calls. No logic edits.
- [x] **T3.2 Move tests.** Relocate the moved modules' unit/render tests (cells, popup scenes, render gaps, mermaid, bar) into the kit unchanged (fixtures excepted where they reference App-only test helpers — then use the kit's `test_harness` equivalent).
- [x] **T3.3 Rewire `crates/tui`.** `tui` depends on the kit; delete duplicated code; `App` assembles the component registry and injects host data (palette command list, slash skills, model tiers); `tui` keeps `run_tui`, `theme_detection`, `system_prompt`, and the extension implementation.
- [x] **T3.4 Gate.** `cargo test -p tui --lib` + `cargo test -p tact-ui` (the cross-crate bridge via `test_support` must stay green); render/scene tests green with no expectation edits.

## Phase 4 — Cut `tact` / `tact_llm` coupling

- [x] **T4.1 Extensions.** Move plugin / voice / skill handling behind `BridgeExtension` implemented in `crates/tui` (feature-gated where sensible: `voice`, `plugins`).
- [x] **T4.2 Chat model.** Define kit `ChatItem` (replacing `tact_llm::content::{Message, ContentBlock, Role}` in TUI state); migrate `messages.rs`.
- [x] **T4.3 Model tiers.** Inject model/budget/effort tiers as `Vec<ModelChoice>`; delete `OpenAiReasoningEffort` and `ProviderKind` references from the kit.
- [x] **T4.4 Command split.** Keep generic `UserCommand` variants in protocol; move `QueryBalance` (and any Tact-only commands) to the extension command channel.
- [x] **T4.5 Dependency gate.** `cargo tree -p agent_tui_kit` must show **no** `tact` / `tact_llm`; then the full test gate.

## Phase 5 — External-consumer validation, docs, ship

- [x] **T5.1 Mock consumer.** Write `crates/agent_tui_kit/examples/mock_agent.rs`: a mock agent emits a full `AgentUpdate` sequence (thinking → step started → tool progress → stream chunk → task complete) and receives `Command`s; run it headless end-to-end.
- [x] **T5.2 Book/docs sync.** `book/23_chapter_tui{,_zh}.md` (architecture: kit + app layer), `book/26_chapter_issue{,_zh}.md` newest-first entry (type `optimization`; symptom/motivation, decision + observable behavior, code/spec/plan pointers, link this plan and the design spec), kit README (contract, component inventory, mock example).
- [x] **T5.3 Full verification.** `cargo fmt --all`; `cargo clippy`; `env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib`; `cargo test -p tact-ui`; run the example; eyeball `cargo tree -p agent_tui_kit`.

## Acceptance criteria

1. `cargo tree -p agent_tui_kit` contains no `tact` / `tact_llm`.
2. All test gates green with no reduction in test count; existing render/scene tests pass **without expectation edits** (proof of no visual regression).
3. `mock_agent` example runs headless through the full flow.
4. Design spec and this plan are checked in; book chapters and Ch 26 entries are synced.

## Risk Register

- **R1 Decomposition regressions.** Mitigate: gates run the unchanged suite every phase; headless harness + tact-ui bridge tests cover the wiring.
- **R2 Shared-log entanglement via Ctx.** Mitigate: coordinator owns insertion order; AGENTS.md render invariants carry over; accessor names encode intent.
- **R3 Protocol split breaks agent↔TUI.** Mitigate: Phase 1 leaves `tact_protocol` untouched; only re-exports and extension events are added.
- **R4 ratatui-markdown fork blocks external reuse.** Mitigate: workspace path dep stays; fork must be pushed to a remote before any external consumer (tracked from the 2026-08-15 migration).
- **R5 Kit API absorbs Tact quirks.** Mitigate: `on_update` consumes protocol types only; Tact specifics live in extensions; the mock consumer in T5.1 forces the API to stand without Tact.
- **R6 Scope creep in moved code.** Mitigate: verbatim-move rule; visual changes are separate diffs; Phase gates diff the render tests.

---

# Phase 0 results (2026-08-18)

## T0.1 — baseline

- `cargo test -p tui --lib`: **604 passed, 0 failed**
- `cargo test -p tact-ui`: **45 passed, 0 failed** (26 lib unittests + 4 `app_bridge_integration` + 9 `driver_integration` + 6 `harness_advanced`; the earlier "12" was a truncated `tail` capture — corrected after a full run)
- test-support feature / headless harness: **verified** — tact-ui dev-dep enables `tui/test-support`; `app_bridge_integration` exercises `tui::test_support::TestApp` + `headless_loop` (`drain_agent_updates` / `auto_confirm_select`).

## T0.2 — kit skeleton

- `crates/agent_tui_kit` added to workspace members; deps: `tact_protocol` (path), `crossterm`, `ratatui` (workspace). `cargo check -p agent_tui_kit` passes.
- Drafts committed: `Component<U=AgentUpdate>` trait (on_update / on_key / render / height / priority + `Box<dyn Component>` impl), `Ctx` (log coordinator + input mode + pending queue; theme/messages/scroll marked TODO(Phase 3)), `LogCoordinator` / `PendingQueue` / `InputMode` placeholders, `AgentBridge` / `BridgeExtension` / `ExtensionEvent` (uninhabited, TODO(T2.3)), `protocol.rs` generic re-exports (biz deliberately excluded).

## T0.3 — protocol audit table (core / extension)

See design doc §8 (filled). Summary:

- `AgentUpdate` — **20 variants, all generic/core**; no Tact-specific variant exists. Consumers map cleanly to ToolComponent / ThinkingComponent / StreamComponent / coordinator / StatusBar / TaskPanel / SelectPopup.
- `UserCommand` — 9 core + `QueryBalance` (extension).
- Separate channels/workers — `AccountUpdate` (balance/quota), `PluginEvent`/`PluginRequest`, voice worker: all **extension**.
- Discrepancy vs design assumption: this branch wires `ratatui-markdown` to the **upstream git rev 3a8bcbe** (celestia-island), not the local fork path. The kit inherits `{ workspace = true }`; publish/fork question is deferred to Phase 3.

## T0.4 — decision: **GO** (Option A)

Every update path classifies cleanly; the extension seam is exactly the three side channels + `QueryBalance`. Proceed to Phase 1.

---

# Phase 1 results (2026-08-18)

- **T1.1** — New `crates/tui/src/widgets/state/log_coordinator.rs`: `LogCoordinator { items: Vec<LogItem> }` + primitive ops (`append_msg`, `append_markdown`, `append_blank`, `extend_msgs`, `insert_msg`, `splice_msgs`, `drain_msgs`, `remove_msg`, `push_placeholder_rows`). `App.log_items: Vec<LogItem>` → `App.log: LogCoordinator`; 148 refs renamed mechanically (`.log_items` → `.log.items`). `App` keeps equivalent delegating methods (compat layer, no helper-name churn).
- **T1.2** — Verified zero direct mutations of `log.items` outside the coordinator: every mutation path already used the helper API, which now delegates to coordinator primitives. No extra rewiring needed.
- **T1.3** — Gate green: `cargo fmt` clean, `cargo clippy -p tui -p agent_tui_kit` zero warnings, `cargo test -p tui --lib` **604 passed** (same as baseline, zero test-semantics edits), `cargo test -p tact-ui` **45 passed** (26 lib + 4 app_bridge + 9 driver + 6 harness_advanced).

---

# Phase 2 results (2026-08-18)

**Plan refinement (critical review):** the `Vec<Box<dyn Component>>` registry over `&mut App` is infeasible while component state still lives on `App` — a component that is a field of `App` cannot borrow `App` mutably through a trait object without disjoint-field gymnastics. The registry becomes meaningful only in Phase 3, when components own their state. Phase 2 therefore delivers the feasible decomposition: a thin `prepass → match → tail` router, explicit coordinator pre-pass, and extension-channel separation; the per-component **file shuffle** is deferred to Phase 3 (where it is the verbatim move anyway).

- **T2.1** — `handle_agent_update` is now a thin router: `dirty = true → coordinator_prepass(&update) → match(20 arms) → refresh_tail_scroll()`. The two inline safety-net blocks (thinking-close + loading-placeholder removal) were extracted into `coordinator_prepass`; the tail scroll refresh into `refresh_tail_scroll`.
- **T2.2** — Update arms already delegated to per-component `on_*` handlers (`on_step_*`/`on_tool_*`/`apply_stream_chunk`/`on_tasks_changed`); no behavior change. Full method-file-shuffle into `components/` is deferred to Phase 3's verbatim move.
- **T2.3** — Extension channels separated: `handle_account_update`, `show_plugin_list`, `show_marketplace_list`, `handle_plugin_event` + their 4 helper fns + `MAX_PLUGIN_FAILURE_DETAIL_CHARS` moved to `widgets/state/app/extensions.rs`. Account/plugin were already on separate channels; this makes the extension seam an explicit module (the future `BridgeExtension` impl site).
- **T2.4** — Gate green: `cargo fmt` clean, `cargo clippy -p tui` zero warnings, `cargo test -p tui --lib` **604 passed**, `cargo test -p tact-ui` **45 passed**.

---

# Phase 3 results (in progress — 2026-08-18)

**Test-count note:** as modules move from `tui` to `agent_tui_kit`, their tests move with them. The gate therefore tracks **total** `tui --lib` + `agent_tui_kit --lib` = 604 (not `-p tui` alone).

- **T3.1a (done) — theme + i18n.** `theme.rs` (456 lines) and `i18n.rs` (830 lines) moved verbatim into the kit (pure modules: no `App` dependency). Visibility fixes: `Language::all()` `pub(crate)`→`pub`, `ThemeName::next()` `pub(super)`→`pub`. `crates/tui` now re-exports them via inline `pub(crate) mod theme { pub(crate) use agent_tui_kit::theme::*; }` (same for `i18n`), preserving all `crate::theme::*` / `crate::i18n::*` paths. `crates/tui` gains `agent_tui_kit = { path = "../agent_tui_kit" }`.
- **Gate:** `cargo test -p tui --lib` **601** + `cargo test -p agent_tui_kit --lib` **3** = 604 (3 theme/i18n tests relocated); `tact-ui` **45**; clippy zero warnings.

**Remaining Phase 3 slices (in dependency order):**
1. Cell renderers (`render/cells/*`) + `LogItem`/`LogItemKind`/`SystemMsgStyle` → kit (unify with kit's draft `LogCoordinator`).
2. `render/log.rs`, `layout.rs`, `bar.rs`, `input.rs`, `render/popups/*` — these take `&App`; require the `Ctx` abstraction (`&App` → `&Ctx`).
3. Component state extraction out of `App` into `Component` impls; `tui` becomes shell + extension assembly.

- **T3.1b (done) — render/util + renderable.** `render/util.rs` (365 lines: 3 indent consts + wrap/split/truncate helpers) and `render/renderable.rs` (15 lines: `Renderable` trait) moved to `agent_tui_kit::render`. Both pure (ratatui + unicode-width only). Visibility `pub(crate)`→`pub`; kit gains `unicode-width` dep; tui re-exports via inline `mod util { pub(crate) use … }` / `mod renderable { … }`.
- **T3.1c (done) — cells/text + cells/separator.** Pure per-row cells (`TextCell`, task-end `MessageSeparator`/`TaskEndSeparator` + `is_task_end_separator`/`task_end_separator_raw`/`task_end_elapsed_secs`) moved to `agent_tui_kit::render::cells`. Their internal `crate::render::{renderable,util}` paths resolve identically inside the kit (no path edits). tui re-exports via inline `mod text`/`mod separator`.
- **T3.1d (done) — thinking cluster.** `PopupTextSelection` (→ `state/selection.rs`), `ActiveThinkingBlock`/`ThinkingBlock`/`ThinkingPopup`/`ThinkingState` (→ `state/thinking.rs`), and `ThinkingCell`/`thinking_visual_rows` (→ `render/cells/thinking.rs`) moved to the kit. Kit gains a `state` namespace re-exporting the types. `tool_state.rs` now imports `PopupTextSelection` via `super`; tui re-exports the whole cluster. Kit-side cell tests inline a local `buffer_text` helper (replaces the tui-only `test_harness::buffer_text`).
- **T3.1e (done) — markdown render core.** `render_md.rs` (1853), `pulldown.rs` (519), `mermaid_sequence.rs` (802) moved to `agent_tui_kit::render`. Their `super::` cross-references (`render_md ↔ pulldown` circular, `render_md → mermaid_sequence`) resolve identically in the kit's `render` module. Kit gains `pulldown-cmark` + `ratatui-markdown` deps; tui re-exports only `render_md` (pulldown/mermaid_sequence are now kit-internal).
- **T3.1f (done) — cells/markdown (`MarkdownCell`).** Non-test cell + `RenderedMarkdown` + the pure `mod tests` moved to `agent_tui_kit::render::cells::markdown` (kit-side `buffer_text` inlined). The 6 App-integration tests (`make_app` / `handle_agent_update` / log rendering) stay in tui at `render/cells/markdown_integration_tests.rs` (they exercise `App`, not the cell). tui's `LogItem.markdown_cell` field type now resolves to the kit's `MarkdownCell` via the `cells::markdown` re-export — no source change.
- **T3.1g (done) — LogItem model + LogCoordinator unification.** `LogItem`/`LogItemKind`/`SystemMsgStyle` (+ impls, 4 tests) and the real `LogCoordinator` moved to `agent_tui_kit::state::log`, replacing the kit's draft `rows: Vec<String>` placeholder. tui re-exports `LogCoordinator`/`LogItemKind`/`SystemMsgStyle` (the `LogItem` type itself is no longer named in tui code, only via field access). tui's `log_messages.rs` / `log_coordinator.rs` deleted. Note: an orphaned `#[derive(Clone, Copy, …)]` (formerly on `SystemMsgStyle`) briefly attached to `App` and was removed — the kind/style enums' derives now live in the kit.
- **T3.1h (done) — tool cluster.** `widgets/tool_widget.rs` (1232: `ToolWidget` builder, `ToolPhase`, `ToolRenderOutput`, spinner, meta-text/elapsed helpers) → `agent_tui_kit::widgets::tool_widget`; `cells/tool.rs` (735: `ToolCell`) → `agent_tui_kit::render::cells::tool`. Both pure (theme + i18n + protocol only). tui re-exports both. Kit gains a `widgets` namespace.
- **Gate:** `tui --lib` **474** + `kit --lib` **130** = 604 (38 tool-cluster tests relocated); `tact-ui` **45**; clippy zero warnings.

**Phase 3 mechanical moves complete.** The kit is now a 130-test library: `bridge / i18n / protocol / theme / widgets::tool_widget / render{util, renderable, render_md, pulldown, mermaid_sequence, cells{markdown, separator, text, thinking, tool}} / state{log, selection, thinking}`.

---

# Ctx design + implementation (Phase 3 hard part)

**Design:** `docs/superpowers/specs/2026-08-18-tui-component-library-ctx-design.md` — `RenderCtx<'a>` (theme/messages/log/scroll/sub-states + explicit `Vec<RenderCommand>` write channel), derived methods become pure `&RenderCtx` free fns, migration order (leaf helpers → free fns → sub-state types → code → log → bar/input/layout → popups → rewire).

- **Ctx-step 1 (done) — `render/log_column.rs`.** `LogColumnRenderer` (150, pure: `Renderable` + ratatui only) → `agent_tui_kit::render::log_column`. Public visibility added a `Default` impl (clippy `new_without_default`). tui re-exports.
- **Ctx-step 3a (done) — pure sub-states.** `StreamState`, `StatusBarState`, `LogScroll`, `MouseState`(+`LogSelection`/`TextPosition`/`PopupHitRow`/`PopupTextHit`), `PlanPanel`, `SelectPopup` → `agent_tui_kit::state`. Kit gains `tokio{features=["sync"]}` (SelectPopup holds `oneshot`). `PlanPanel`'s `super::PlanStep` → `crate::protocol::PlanStep`. Public `LogScroll::new()` gained a `Default` impl (clippy). tui re-exports all six; tui's `PlanStep` re-export dropped (now kit-internal).
- **Ctx-step 3b (done) — remaining pure sub-states.** `TaskPanelState`, `ToolState`(+`ActiveToolBlock`/`ToolBlock`/`DiffPopup`/`SubagentPopup`), `selectable_text::PopupLayoutCache`, and the inline UI types (`InputMode`, `FocusedPanel`, `SkillEntry`, `HistoryEntry`, `CodeBlock`, `MermaidBlock`, `CodePopup`, `MermaidPopup`, `SystemPromptPopup`, `Status`) → kit (`state/ui_types.rs` + `render/selectable_text.rs`). Kit's duplicate `InputMode` draft removed (now `state::InputMode`). `ToolState`/`selectable_text` re-pathed to kit. App-layer types (`SelectKind` — tact_llm dep, `PALETTE_COMMANDS`) stay in tui.
- **Gate:** `tui --lib` **457** + `kit --lib` **147** = 604; `tact-ui` **45**; clippy zero warnings.

**All pure state extracted.** tui's `widgets/state` now holds only `App` (shell), `SelectKind`, `PALETTE_COMMANDS`, and app-layer/handler-coupled state (`AccountState`, `VoiceState`, `FilePicker`, `SlashCommandState`, `InputHistory`, `TaskDagPopup`).

- **Ctx-step 4b (done) — RenderCtx expansion + free fns.** Expanded `RenderCtx` with the `log.rs` fields (`log: &LogCoordinator`, `mermaid_blocks`, `tools`, `thinking`, `stream`, `mouse`, `skills_data`, `loading_idx`, `spinner_frame: u8`). Added kit free fns `log_indent_at` (state/log) and `find_thinking_at_logical` (state/thinking). App methods (`is_message_visible`/`nested_log_indent`/`table_layout_width`/…) stay for the non-migrated App/handler code.
- **Gate:** `tui --lib` **457** + `kit --lib` **147** = 604; `tact-ui` **45**; clippy zero warnings.

**Design finding (blocks the naive `log.rs` migration):** `render_log_panel` **mutates** `app.log_scroll` heavily every frame (rebuilds `height`/`width`/`visible_indices`/`phys_to_logical_cache`/`visible_indices_ver`/`visual_cache`/`visual_start_cache`/`visual_cache_width`/`visual_cache_ver`/`visual_top`/`visual_start`). The design doc's "render is a pure function of state" (§6) does not hold for the log panel. Two resolutions:
1. `RenderCtx.log_scroll: &mut LogScroll` (pragmatic; render owns scroll-cache rebuild).
2. Split into a mutable `prepare_frame(&mut LogScroll, &LogCoordinator, …)` cache-build pre-pass + a pure render that only reads the built caches.

**Decision made: Option 2** (mutable prepare + pure render). Two commits:
- `e03ad292` — split `render_log_panel_with_borders` into `prepare_log_frame` (mutable cache rebuild, stays in tui) + `render_log_panel_pure` (`&RenderCtx`, read-only).
- `97959115` — move `render_log_panel_pure` + `restamp_log_left_border` + `render_loading_spinner` into `agent_tui_kit::render::log`; tui keeps only `prepare_log_frame` + the two `&mut App` wrappers. Dropped the now-unused `cells::{code,text,tool}` re-exports in tui (their consumers moved with the pure phase).

Rationale for Option 2 over Option 1: the prepare phase depends on app-layer skill styling (`slash_style::skill_name_set`, `log_style::restyle_log_line_with_skills`), so it must stay in tui; splitting makes the pure Phase 3 genuinely reusable without a `&mut LogScroll` leaking into `RenderCtx`.
- **Gate:** `tui --lib` **457** + `kit --lib` **147** = 604; `tact-ui` **45**; clippy zero warnings.

**Remaining Ctx steps (same RenderCtx pattern, one panel at a time):**
- ~~`render/log.rs`~~ → **done** (Option 2: prepare/pure split, pure phase moved to kit).
- ~~`render/bar.rs`~~ → **done** (below).
- ~~`render/input.rs`~~ → **done** (below).
- ~~`popups/*` (14 popups)~~ → **done** (below; 4 app-layer popups stay in tui).
- ~~`render/task_panel.rs`~~ → **done** (below).
- `layout.rs` → **stays in tui by decision** (pure orchestration over app-layer
  state: `app.mouse` hit areas, `app.log_scroll.height`, task-panel geometry,
  popup dispatch; no independent render logic to reuse — the shell assembles
  the kit's panels, which is the Phase 3+4 target shape).
- 9. handlers move + App state extraction → `Component` impls (Phase 4/5 follow).

**Ctx slice results (all gates green; `tui --lib` + `kit --lib` = 606):**

- **bar.rs** (`c37d906b`) — `render_bottom_bar` / `render_status_bar` +
  helpers + 22 pure-fn tests → `agent_tui_kit::render::bar` (`&RenderCtx`).
  `App::render_ctx()` introduced as the single ctx construction site
  (`widgets/state/app/config.rs`); `AccountState` moved to
  `agent_tui_kit::state::account` (tui re-exports); `format_task_elapsed`
  became a kit free fn; tui keeps the 8 App-integration tests.
- **input.rs** (`f882a8a3`) — `render_input_box` / `render_command_line` /
  `wrap_line` / `caret_display_line` / `truncate_to_width` → kit
  (`&RenderCtx` + explicit `skill_names` param). Pending-block `[Cancel]` hit
  area is *returned* by the pure render; the tui wrapper keeps the mutable
  prepare phase (caret-scroll clamp, voice button area). `PendingMessage`
  upgraded to the real `{display, agent_task}` type in the kit; `voice_title`
  became an `App` method injected via `RenderCtx::input_voice_title`.
  `slash_style` split: pure fns (incl. parameterized `skill_name_set`) → kit,
  tui keeps a thin wrapper injecting `PALETTE_COMMANDS` builtins.
- **popups** (`cd6dcdc6`) — chrome helpers (`FooterHint`, `centered_popup_*`,
  `render_popup_chrome`, `PopupMouseSurface`) + 8 pure renderers →
  `agent_tui_kit::render::popups`: code / mermaid / history / select /
  system_prompt / thinking / diff / subagent. Mouse hit areas returned via
  `PopupMouseSurface`; diff/subagent lazy caches (git diff / file read /
  markdown layout) run in kit `prepare_*` fns called by the tui wrappers;
  thinking `selection_text` write-back returned to the wrapper. Widgets
  `help_widget` / `popup_widget` / `select_popup_widget` moved to kit.
  App-layer popups stay in tui (own Tact state): command_palette, file_picker,
  slash_command, task_dag_popup, help (voice keybind leak).
- **task_panel** (`f0abbaf8`) — sticky strip render → `agent_tui_kit::render::task_panel`
  (`&RenderCtx`); tui wrapper keeps `app.mouse.task_panel_area` + geometry
  helpers (`sticky_host_*`).

**tui now holds:** `lib.rs` / `run_tui`, `App` shell + handlers, `prepare_*`
phases, app-layer popups (palette/file-picker/slash/task-dag/help), `log_style`
(skill restyle), `layout.rs` (orchestration), `test_harness` + scene tests.
**kit renders:** log column, log panel (pure), bar, input, popups (8), task
panel, markdown/mermaid, cells, widgets — all via `RenderCtx`.

**Next-slice findings (cells are coupled to state, not standalone):**
- `cells/text.rs`, `separator.rs` — pure (depend only on renderable + util). Movable immediately.
- `cells/thinking.rs` — coupled to `ActiveThinkingBlock`/`ThinkingBlock` (thinking_state) + `PopupTextSelection` (tool_state). Moves as a "thinking cluster" (cell + state + selection).
- `cells/markdown.rs` — `MarkdownCell` is referenced by `LogItem`; pulls `render_md.rs` (ratatui-markdown) + `mermaid_sequence.rs`.
- `cells/tool.rs` — coupled to `widgets/tool_widget.rs` (`ToolPhase`/`ToolRenderOutput`/…).
- `cells/code.rs` — **reads `&App` in its render path** (`app.code_blocks`, `app.log_scroll`, `app.phys_to_logical_fast`); the first true `Ctx` blocker.


---

# Phase 4 results (2026-08-22)

- **T4.5 (verified first — it drove the rest):** `cargo tree -p agent_tui_kit` shows
  **no `tact` / `tact_llm`** — only `tact_protocol` (+ ratatui family, chrono,
  syntect, tokio, unicode-*, pulldown-cmark, ratatui-markdown). Phase 3's
  verbatim-move discipline already isolated every `tact`/`tact_llm` reference
  into `crates/tui`.
- **T4.1 (contract, not full wiring) + T4.4 (Command split):** `bridge.rs`
  rewritten (`e2a66cc2`):
  - `Command` is now the kit's own 9-variant generic enum (no `QueryBalance`),
    with `From<Command> for UserCommand` + `TryFrom<UserCommand> for Command`
    (QueryBalance → `Err`, extension-only). Round-trip tests cover all 9.
  - `ExtensionCommand::QueryBalance` added; `BridgeExtension::on_command`
    routes it (default no-op).
  - `ExtensionEvent` filled from the T0.3 audit: `AccountUpdate` is the only
    event whose payload type is protocol-level; plugin/voice events stay
    host-internal (payloads live in `tact`, not `tact_protocol`) and remain in
    `crates/tui` (`widgets/state/app/extensions.rs` is the future impl site).
- **T4.2 (already satisfied):** the kit's chat model *is* `LogItem`/`LogCoordinator`
  (protocol-level rows); `crates/tui/src/widgets/state/app/messages.rs`
  `load_history` is an app-layer adapter (`tact_llm::Message` → log rows) and
  legitimately keeps its `tact_llm` import — the kit never sees those types.
- **T4.3 (already satisfied):** zero `OpenAiReasoningEffort` / `ProviderKind`
  references in the kit (grep-verified); model/budget/effort tiers live in
  `crates/tui` (`handlers/select.rs`, `SelectKind`), injected at the app layer.
- **Deferred (step 9 — handlers + `Component` registry):** wiring the kit's
  `AgentBridge`/`BridgeExtension` into `App`'s channels and extracting
  component state out of `App` into `Component` impls is the Phase 5 +
  follow-up slice; the current `App` + handlers structure stays until then
  (Phase 2 analysis: registry over `&mut App` is infeasible pre-extraction).
- **Gate:** `tui --lib` **413** + `kit --lib` **194** = 607; `tact-ui` **105**;
  clippy zero warnings; `cargo fmt` clean. (Numbers corrected after removing
  duplicate `#[test]` attributes and duplicate pure-fn tests — see Phase 5.)


---

# Phase 5 results (2026-08-23)

- **T5.1 — mock consumer** (`crates/agent_tui_kit/examples/mock_agent.rs`,
  commit `5c675a85`): a self-contained `MockShell` (kit state only) applies a
  full `AgentUpdate` sequence — thinking started/delta/finished → step added →
  step started (tool card via `ToolWidget` builder) → step finished → stream
  chunks → token usage → task complete — then renders headless frames (status
  bar, bordered log panel, input box, bottom bar) through `TestBackend` using
  only `agent_tui_kit` + ratatui. The example builds its own minimal
  `prepare_log_frame` (visible indices + wrap cache + bottom clamp) since the
  Tact app's real prepare applies app-layer skill styling. Outbound contract
  exercised via `Command::SubmitTask`. Run: `cargo run -p agent_tui_kit
  --example mock_agent` (prints the rendered frame; asserts model name,
  workspace path, thinking card).
- **Test-hygiene fix (same commit):** the diff-popup's 11 `selectable_text`
  hit-map tests were lost during the popups slice (they lived in
  `diff_popup.rs`); restored in the kit. Dropped 4 duplicated pure-fn tests in
  `crates/tui` (input wrap/caret/truncate ×3, slash_style ×1) that duplicated
  kit copies; removed 9 duplicate `#[test]` attributes in kit `bar.rs` (they
  double-registered tests). Net: `tui --lib` **413** + `kit --lib` **194** =
  **607** (≥ baseline 604; +2 bridge contract tests, +1 tui slash-wrapper
  test), `tact-ui` **105**, clippy zero warnings, fmt clean.
- **T5.2 — docs sync (partial):** plan updated with Phase 3/4/5 results;
  `book/23_chapter_tui{,_zh}.md` + `book/26_chapter_issue{,_zh}.md` +
  kit README remain TODO for the final ship (component-registry slice
  included), so they are deferred to the step-9 follow-up rather than written
  twice.

# Remaining (post-Phase-5 follow-ups)

- Step 9 — `Component` registry + handlers migration (the kit's
  `Component<U>` trait is still a compile-only draft; `App` remains the shell).
- T5.2 full book/docs sync (`book/23`, `book/26` newest-first entry, kit README
  with contract + component inventory + mock example pointer).
- External-consumer validation of the ratatui-markdown fork (R4) before any
  non-Tact consumer publishes the kit.


# Step 9 progress (2026-08-23)

- **`ThinkingComponent`** (`crates/agent_tui_kit/src/components/thinking.rs`,
  commit `24cb74c1`): the kit's `Component<U>` trait is no longer a
  compile-only draft — it now has a real, tested implementation. The
  component owns its `ThinkingState`, feeds the shared log through `Ctx`
  (placeholder anchor row), renders active/completed reasoning cards into a
  plain `Buffer`, and returns `true` from `on_update` only when the frame
  must repaint. 3 tests cover the started/delta/finished lifecycle, the
  no-repaint path for unrelated updates, and buffer rendering.
- **`ComponentRegistry`** (`components/registry.rs`, commit `dfaa5df6`):
  priority-ordered `Vec<(u8, Box<dyn Component<U>>)>` shell — `push` keeps
  insertion-stable priority order; `dispatch_update` / `dispatch_key` stop at
  the first claiming component; `render_all` is a sequential pass returning
  used height. 2 tests cover sort+claim semantics and key bubbling.
- **Pattern for the remaining components:** state in the component, shared
  surfaces (log, input mode, pending queue) through `Ctx`, theme/messages
  owned by the component, `render` into `Buffer` (hosts route in
  `priority()` order). Full migration of `App` fields/handlers into a
  registry stays TODO (largest slice — the Tact app's thinking/tool state is
  coupled into the log render path, so each component migration needs its own
  render-path change; kit-side pattern + infra are now complete).
- **Gate after step-9 infra:** `tui --lib` **413** + `kit --lib` **199** =
  612; `tact-ui` **105**; clippy zero warnings; fmt clean.


# Step 9 progress II (2026-08-23)

- **Stream parse extraction** (`3bc5843c`): the streaming-text state machine
  (fence detection, table/paragraph buffering, code-block lifecycle) moved
  from `App::apply_stream_chunk` into the kit as
  `StreamState::push_chunk(&mut self, text) -> Vec<StreamEvent>`
  (`state/stream_parser.rs`). The host loop now applies `StreamEvent`s
  (MarkdownParagraph / Table / Blank / OpenCodeBlock / CodeLine /
  CloseCodeBlock) with app-layer rendering; `finish_stream_code_block` takes
  `is_mermaid` explicitly. 8 parser tests; the unchanged 413 tui tests prove
  behavior parity (incl. streaming indicators, tables, mermaid splice).
- **Component downcast** (`d0668f8d`): `Component` gained `as_any` /
  `as_any_mut` (+ `'static` bound) and `ComponentRegistry::get::<T>()` — the
  shell can now read component state for rendering.
- **`StreamComponent`** (`d0668f8d`): second real component — owns
  `StreamState`, parses `StreamChunk` into `Ctx::stream_events` (new outbox
  field on `Ctx`), renders the in-flight line. 4 tests incl. a registry
  downcast round-trip.
- **Pattern now complete:** state in component → `on_update` parses/mutates +
  pushes events to `Ctx` outboxes → shell applies events to log/UI →
  shell reads state via `registry.get::<T>()` for render. Remaining
  components (TaskPanel / StatusBar / Tool / Plan / Thinking) follow the same
  shape, each preceded by the equivalent state-machine extraction where the
  logic is still entangled in `App` (e.g. tool-card building, task-snapshot
  formatting).
- **Gate:** `tui --lib` **413** + `kit --lib` **211** = 624; `tact-ui` **105**;
  clippy zero warnings; fmt clean; mock example runs.


# Step 9 progress III (2026-08-23) — kit component inventory complete

- **`StatusBarComponent`** (`42d4bb8a`): `TokenUsage` → cache stats,
  `ModelInfo` → model metadata. 3 tests.
- **`TaskPanelComponent`** (`42d4bb8a`): `TasksChanged` → `apply_snapshot`
  (visibility/expand logic already in the kit). 2 tests.
- **`PlanComponent`** (`42d4bb8a`): `StepAdded` → step record; `StepFinished` /
  `StepFailed` → step output sync. 2 tests.
- **`ToolComponent`** (`865d6e3a`): `ToolProgress` → live output buffer,
  `ToolMeta` → subagent model/tokens. The `Step*` placeholder-row lifecycle
  is entangled with the host log (`phys_idx` allocation, `finalize`/`resize`,
  scroll) and stays in the shell — noted as the next extraction boundary
  (mirrors `StreamComponent`'s event outbox pattern). 3 tests.
- **Component inventory (kit):** Thinking, Stream, StatusBar, TaskPanel,
  Plan, Tool + `ComponentRegistry` (priority dispatch) + `as_any`/`get<T>`
  downcast + `Ctx` outboxes (`stream_events`).

**tui switch-out is a separate, larger slice**: `App` still owns the fields
and `handle_agent_update`'s match + `coordinator_prepass`; switching to the
registry means removing those fields, delegating dispatch, and reading state
via `registry.get::<T>()` for render — plus adapting the 413 app tests. The
kit side is ready for it; the app side is deliberately not done in this
slice (it is a whole-App refactor, not a component addition).

- **Gate:** `tui --lib` **413** + `kit --lib` **221** = 634; `tact-ui` **105**;
  clippy zero warnings; fmt clean; mock example runs.

# Step 9 completion (2026-08-23) — whole-App switch to `ComponentRegistry` (task #42)

Two commits land the switch, each gated (413 + 221 = 634; tact-ui 105;
clippy zero warnings; fmt clean; zero visual change — no scene/render test
edits):

- **`b0de20e6` — fields → registry components.** `App` drops `plan` /
  `thinking` / `stream` / `tools` / `task_panel` / `status_bar`; adds
  `registry: ComponentRegistry` owning the six components. New
  `app/registry.rs` typed accessors (`plan()` / `plan_mut()`, …); components
  gained `Deref`/`DerefMut` to their state so `app.<field>.<state>` call
  sites survive with minimal churn. `LogCoordinator` stays shell-owned.
  `ComponentRegistry::get_mut` added (typed mutable downcast); `Component`
  gained a `Send` supertrait (the registry-holding shell moves onto the
  tokio task in `tact-ui`). render_ctx/prepare phases read state via
  downcast; disjoint field-borrow patterns replace whole-App method borrows
  where they conflicted (prepare_log_frame, subagent/diff popup prepare,
  overlay_scroll_mut, copy_diff_popup).
- **`7f1ba78b` — dispatch.** `handle_agent_update` =
  `coordinator_prepass` → `dispatch_components` (registry dispatch; `Ctx`
  borrows shell log / input mode / pending queue via field-split borrows;
  returns the `StreamEvent` outbox) → `apply_stream_events` (StreamChunk
  only — the gap checks append rows and must not fire for other update
  types; regression caught by
  `progress_keeps_bottom_pinned_log_on_live_output_growth`) →
  `shell_handle` (status/log effects, tool-card lifecycle, select popups,
  thinking card) → `refresh_tail_scroll`.
- Component claims: `TokenUsage`/`ModelInfo` → StatusBar, `ToolProgress`/
  `ToolMeta` → Tool, `StepAdded` → Plan, `TasksChanged` → TaskPanel,
  `StreamChunk` → Stream (parse only). `ThinkingChunk` and
  `StepFinished`/`StepFailed` stay in the shell (log-anchored thinking card
  + resolved-step tool lifecycle would double-process if dispatched).
- Shell writes removed from handlers: `on_step_added` no longer pushes the
  plan step; `on_tool_progress` no longer pushes live-output chunks;
  `on_tool_meta` deleted; `on_tasks_changed` became a dag-sync tail after
  the component applied the snapshot.

**Task #42 is now complete.** T5.2 book/docs sync done in this commit
(`book/23_chapter_tui{,_zh}.md` §1 registry paragraph; `book/26` newest
entry). Remaining known deferrals: the `Step*` placeholder-row lifecycle is
still shell code (extraction boundary noted in Step 9 progress III), and
the ratatui-markdown fork (R4) still needs external-consumer validation
before the kit is published for non-Tact hosts.
