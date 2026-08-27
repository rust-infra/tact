# Async / Multi-Subagent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend Tact's subagent from a single synchronous, blocking call into a multi-subagent model with (1) Claude-style permission inheritance, (2) `run_in_background` async dispatch with result re-injection, (3) `max_turns` / `resume` / `check_subagent`, and (4) multi-card / multi-popup / concurrent-permission UI — while keeping the first cut safe (no worktree isolation, `spawn_subagent` stays `Barrier`).

**Design docs:** `docs/superpowers/specs/2026-08-26-async-subagent-design.md` (design) and `docs/superpowers/specs/2026-08-26-async-subagent-design-review.md` (this plan's source of truth for the corrections below).

**Architecture:** Four layers, implemented in dependency order — (A) permission inheritance, (B) persisted subagent-run state, (C) async mechanics + re-injection + resume/cancel/check, (D) TUI. Reuse `background_run`'s three-piece pattern (keep-live card → `ToolProgress` streaming → `BackgroundTaskFinished` finalize) and `background_store`'s store shape; a subagent summary differs from a background command in that it **must flow back into the parent LLM's context**, which `background_run` never does.

**Tech Stack:** Rust 2024, tokio, sqlx/SQLite, ratatui, protocol enums, TUI unit tests, Markdown book chapters (bilingual).

## Review corrections baked into this plan

| ID | Correction | Where implemented |
|----|-----------|-------------------|
| C1 | Wake-up is a **new** `UserCommand` + TUI→driver relay, not the existing `ui_tx` | Task 10, 16 |
| C2 | `spawn_subagent` **stays `Barrier`**; background parallelism comes from `tokio::spawn`, not wave scheduling | Task 6, 8 (no resource-policy change) |
| M1 | Stamp snapshot once, after phase-1, before `waves_grouped`; it is per-**turn**, not per-invocation | Task 4 |
| M2 | `SubagentFinished` must carry the popup transcript; keep-live path skips it today | Task 14 |
| M3 | Add `check_subagent`; fix the persistence narrative (drained message **is** persisted) | Task 12, 15 |
| M4 | `SubagentFinished` must go on the **parent** `ui_tx`, not the child tagged forwarder | Task 10 |
| M5 | Concurrent permission prompts need a **queue/map**, not a single `select` slot | Task 17 |
| M6 | Define `ToolContext.subagent_results: None` degradation (async spawn errors) | Task 9 |
| M7 | `resume` relies on `ensure_session` auto-loading provider state; no manual `load_provider_state` | Task 11 |

## Global Constraints

- Do **not** change `spawn_subagent`'s `ResourcePolicy::Barrier`. Async decoupling is via `tokio::spawn`, not wave scheduling. (C2)
- Do **not** introduce per-agent `permissionMode` override, `bypassPermissions`, or OS sandbox. Out of scope.
- Do **not** introduce automatic cross-restart re-delivery of finished results; the drained `<subagent-finished>` message is persisted, so it is *already* in the transcript after a restart.
- Async tests that wait on channels must use timeouts (see `background.rs` tests); no unbounded `recv().await`.
- Run Cargo commands sequentially; unset `http_proxy`, `https_proxy`, and `all_proxy` for local tests (`env -u http_proxy -u https_proxy -u all_proxy cargo test ...`).
- Do not commit at `main`. Bilingual book chapters must stay structurally aligned; append Ch 26 issue-log entries in both languages when user-visible behavior ships.

---

## File Map

- **Modify:** `crates/tact/src/permission/mod.rs` — `PermissionSnapshot`, `snapshot()` / `from_snapshot()`.
- **Modify:** `crates/tact/src/tool/mod.rs` — `ToolContext` gains `permission_snapshot`, `subagent_results`, `subagent_manager`.
- **Modify:** `crates/tact/src/agent/tool_dispatch.rs` — stamp snapshot; input-aware keep-live for `spawn_subagent`.
- **Modify:** `crates/tact/src/tool/subagent.rs` — new input fields, body split into sync/async helpers, snapshot-based child manager, resume, max_turns.
- **Modify:** `crates/tact/src/agent/mod.rs` — `AgentRuntime.pending_subagent_results`, `max_turns`/`turns_taken`, queue drain in `agent_loop`.
- **Modify:** `crates/protocol/src/agent.rs` — `AgentUpdate::SubagentFinished`, `UserCommand::SubagentFinishedNotification`.
- **New:** `crates/tact/src/store/subagent_store/mod.rs` + `sqlite.rs` — `subagent_runs` table + store trait.
- **Modify:** `crates/tact/src/store/mod.rs` — export `subagent_store`.
- **New:** `crates/tact/src/subagent.rs` — `SubagentManager` / `SharedSubagentManager` / `SubagentRun` / `SubagentStatus` (orphan repair).
- **Modify:** `crates/tact-ui/src/driver.rs` — handle `SubagentFinishedNotification` (idle → wake-up turn).
- **Modify:** `crates/tui/src/widgets/state/app/agent.rs` — relay `SubagentFinished` to driver; concurrent-select queue.
- **Modify:** `crates/agent_tui_kit/src/components/tool.rs` — `SubagentFinished` transcript carry-over + finalize.
- **Modify:** `crates/tui/src/widgets/state/mod.rs` — `subagent_popup` → multi-popup map.

---

## Phase A — Permission inheritance (independent, can land first)

### Task 1: Add `PermissionSnapshot` + `snapshot`/`from_snapshot`

**Files:**
- Modify: `crates/tact/src/permission/mod.rs`

**Interfaces:**
- Produces: `PermissionSnapshot { mode: PermissionMode, always_allowed_tools: Vec<String>, settings: Option<settings::PermissionSettings> }` (derive `Clone`).
- Produces: `PermissionManager::snapshot(&self) -> PermissionSnapshot` and `PermissionManager::from_snapshot(s) -> Self`.

- [ ] **Step 1: Define `PermissionSnapshot`**

`PermissionSettings` is `Clone` (`settings.rs:337`); `PermissionManager` is not (`mod.rs:88`). Add after the `PermissionManager` struct:

```rust
#[derive(Clone)]
pub struct PermissionSnapshot {
    pub mode: PermissionMode,
    pub always_allowed_tools: Vec<String>,
    pub settings: Option<settings::PermissionSettings>,
}
```

- [ ] **Step 2: Implement `snapshot` / `from_snapshot`**

`from_snapshot` must reset `consecutive_denials` to 0 and keep `max_consecutive_denials = 3`, mirroring `try_new_with_settings`:

```rust
impl PermissionManager {
    pub fn snapshot(&self) -> PermissionSnapshot {
        PermissionSnapshot {
            mode: self.mode,
            always_allowed_tools: self.always_allowed_tools.clone(),
            settings: self.settings.clone(),
        }
    }

    pub fn from_snapshot(s: PermissionSnapshot) -> Self {
        Self {
            mode: s.mode,
            always_allowed_tools: s.always_allowed_tools,
            consecutive_denials: 0,
            max_consecutive_denials: 3,
            settings: s.settings,
        }
    }
}
```

- [ ] **Step 3: Unit tests**

Round-trip with and without settings; `consecutive_denials` resets after round-trip; mode + allow-list preserved.

---

### Task 2: Add `permission_snapshot` to `ToolContext`

**Files:**
- Modify: `crates/tact/src/tool/mod.rs`

- [ ] **Step 1: Add the field** (near `session_store`, line ~135)

```rust
/// Parent permission state stamped at dispatch time; `spawn_subagent` clones
/// it to inherit the parent's mode / allow-list / settings.
pub permission_snapshot: Option<crate::permission::PermissionSnapshot>,
```

- [ ] **Step 2: Update `for_invocation`**

The `for_invocation` clone (`tool/mod.rs:130-134`) already copies all fields via `self.clone()`; no change beyond the derive being satisfied (field is `Clone`).

- [ ] **Step 3: Fix all `ToolContext { .. }` literal constructions**

`grep -rn "ToolContext {" crates/` — add `permission_snapshot: None` (and later `subagent_results: None`, `subagent_manager` where applicable) to every literal, or use `..Default`/builder where available.

---

### Task 3: Stamp the snapshot in `execute_tool_call` (M1)

**Files:**
- Modify: `crates/tact/src/agent/tool_dispatch.rs`

**Interfaces:**
- Consumes: `self.runtime.permission_manager`; the existing `prepare` loop and `waves_grouped`.
- Produces: a single per-turn stamp, after phase-1 pre-flight, before the wave loop.

- [ ] **Step 1: Locate the stamp point**

After the `prepare` loop pushes the last `PreparedTool` (which is where a same-turn "always allow this tool" via `allow_tool_with_input` lands), and **before** `for wave in super::tool_schedule::waves_grouped(&resources)` (`tool_dispatch.rs:599`). The wave loop borrows `self.tool_context` (`let ctx = &self.tool_context;` at `:614`), so the stamp must precede it or the borrow checker rejects a mutable access.

- [ ] **Step 2: Stamp**

```rust
self.tool_context.permission_snapshot =
    Some(self.runtime.permission_manager.snapshot());
```

- [ ] **Step 3: Add a test** asserting the stamped snapshot reflects a mode change made by `SetPermissionMode` earlier in the same turn, and that `waves_grouped` futures see the stamped value (not a launch-time stale one).

---

### Task 4: Build the child manager from the snapshot (M1)

**Files:**
- Modify: `crates/tact/src/tool/subagent.rs`

- [ ] **Step 1: Replace the hardcoded `PermissionMode::Default`**

Today `subagent.rs:92` builds `PermissionManager::try_new_with_settings(PermissionMode::Default, settings)`. Replace with the snapshot fallback:

```rust
let pm = match ctx.permission_snapshot.clone() {
    Some(s) => PermissionManager::from_snapshot(s),
    None => PermissionManager::try_new_with_settings(
        PermissionMode::Default,
        PermissionSettings::load(&TactPath::new(&ctx.work_dir)),
    )?,
};
```

`PermissionSettings::load` is still needed only for the orphan/test fallback.

- [ ] **Step 2: Tests** — `Default`/`Plan`/`Auto` parent → child same mode; parent allow-list preserved; `Plan` parent yields a read-only child (regression for the read-only-escape fix).

---

## Phase B — Persisted subagent-run state

### Task 5: `SubagentStatus` + `SubagentRun` types

**Files:**
- New: `crates/tact/src/subagent.rs`

- [ ] **Step 1: Define types** (mirror `BackgroundTaskStatus` / `BackgroundTaskRecord`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus { Running, Completed, Failed, Cancelled }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentRun {
    pub child_id: String,
    pub status: SubagentStatus,
    pub summary: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}
```

---

### Task 6: `subagent_store` (trait + SQLite)

**Files:**
- New: `crates/tact/src/store/subagent_store/mod.rs`
- New: `crates/tact/src/store/subagent_store/sqlite.rs`
- Modify: `crates/tact/src/store/mod.rs` (`pub mod subagent_store;`)

- [ ] **Step 1: Store trait** — clone `BackgroundStore` (`background_store/mod.rs`): `upsert`, `get(id)`, `list()`, `list_running()` (for orphan repair).

- [ ] **Step 2: SQLite schema** — clone `SqliteBackgroundStore` (`background_store/sqlite.rs`), with:

```sql
CREATE TABLE IF NOT EXISTS subagent_runs (
    child_id    TEXT    PRIMARY KEY,
    status      TEXT    NOT NULL
                CHECK (status IN ('running','completed','failed','cancelled')),
    summary     TEXT,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_status ON subagent_runs(status);
```

Epoch-millis timestamps, `status_to_str` / `str_to_status` / `row_to_run` mappers — copy the background store's shape exactly.

- [ ] **Step 3: Store tests** — round-trip all four statuses, unknown id → `None`, persistence across reopen, `list_running` filters.

---

### Task 7: `SubagentManager` with orphan repair

**Files:**
- Modify: `crates/tact/src/subagent.rs` (extend Task 5 file)

- [ ] **Step 1: Manager + shared wrapper** — clone `BackgroundManager` / `SharedBackgroundManager` (`background.rs:65-180`).

- [ ] **Step 2: Orphan repair in `new()`** — any `Running` row → `Failed`, `finished_at = now`, `summary = Some("Process interrupted (agent restarted)")`. Mirror `BackgroundManager::new` (`background.rs:100-112`).

- [ ] **Step 3: Tests** — `marks_stale_running_subagents_on_startup` mirrors `marks_stale_running_tasks_on_startup` (`background.rs` test).

---

### Task 8: Mount the manager on `ToolContext`

**Files:**
- Modify: `crates/tact/src/tool/mod.rs` — add `pub subagent_manager: SharedSubagentManager`.
- Modify: every `ToolContext { .. }` literal and the construction site(s) in `crates/tact-ui/src/{interactive,headless}.rs` — construct the manager the same way `background_manager` is constructed today.

- [ ] **Step 1: Add field + construction**; keep `spawn_subagent` on `Barrier` (no `ResourcePolicy` change).

---

## Phase C — Async mechanics + re-injection + resume/cancel/check

### Task 9: `SubagentInput` fields + `pending_subagent_results` queue + `subagent_results` (M6)

**Files:**
- Modify: `crates/tact/src/tool/subagent.rs` (input)
- Modify: `crates/tact/src/agent/mod.rs` (`AgentRuntime.pending_subagent_results`, `SubagentResult`)
- Modify: `crates/tact/src/tool/mod.rs` (`ToolContext.subagent_results`)

- [ ] **Step 1: Input fields**

```rust
#[serde(default)]
pub run_in_background: Option<bool>,
#[serde(default)]
pub max_turns: Option<u32>,
#[serde(default)]
pub resume: Option<String>,
```

- [ ] **Step 2: `SubagentResult` + runtime queue**

```rust
// crates/tact/src/agent/mod.rs
pub struct SubagentResult { pub child_id: String, pub summary: String, pub success: bool }
// AgentRuntime gains:
pub pending_subagent_results: Arc<Mutex<VecDeque<SubagentResult>>>,
```

- [ ] **Step 3: `ToolContext.subagent_results`**

```rust
pub subagent_results: Option<Arc<Mutex<VecDeque<SubagentResult>>>>,
```

- [ ] **Step 4: Define the `None` degradation (M6)**

In the async branch of `spawn_subagent`: if `ctx.subagent_results.is_none()`, return an error `"run_in_background requires a parent agent runtime"` (the child would have nowhere to push). Do not leave it unwrapable.

- [ ] **Step 5: Stamp the queue** — in `execute_tool_call`, alongside Task 3's snapshot stamp:

```rust
self.tool_context.subagent_results = Some(self.runtime.pending_subagent_results.clone());
```

---

### Task 10: Protocol — `SubagentFinished` + `SubagentFinishedNotification` (C1, M4)

**Files:**
- Modify: `crates/protocol/src/agent.rs`

- [ ] **Step 1: `AgentUpdate::SubagentFinished`** (subagent analog of `BackgroundTaskFinished`, `agent.rs:290-300`):

```rust
SubagentFinished {
    tool_id: String,   // parent tool-card to finalize
    child_id: String,
    success: bool,
    summary: String,   // one-line; full transcript stays in the popup
},
```

- [ ] **Step 2: `UserCommand::SubagentFinishedNotification`** (C1 — there is no existing wake-up path):

```rust
SubagentFinishedNotification {
    child_id: String,
    summary: String,
    success: bool,
},
```

- [ ] **Step 3: Doc comments** — note the completion task must send `SubagentFinished` on the **parent** `ctx.ui_tx` captured at spawn (M4: the child's `tagged_ui_channel_with_progress` forwarder drops unknown variants at `subagent_ui.rs:125`).

---

### Task 11: Split `spawn_subagent` body into sync/async + resume + max_turns

**Files:**
- Modify: `crates/tact/src/tool/subagent.rs`

- [ ] **Step 1: Extract `build_child_agent(...) -> Result<Agent>`** — the client/overrides/system-prompt/permission-manager/store wiring currently at `subagent.rs:75-120`, parameterized by `child_id` (resume reuses an existing id) and the snapshot-built `PermissionManager`.

- [ ] **Step 2: Sync path** — unchanged behavior: await `agent_loop`, extract last assistant summary (`subagent.rs:129-135`).

- [ ] **Step 3: Async path** — when `run_in_background == Some(true)`:
  - insert `subagent_runs` `Running`;
  - `tokio::spawn` a detached task that runs `agent_loop`, extracts the summary, transitions the run to `Completed`/`Failed`, sends `AgentUpdate::SubagentFinished` on the **parent** `ctx.ui_tx`, and pushes a `SubagentResult` into `ctx.subagent_results` (which must be `Some` — see Task 9 Step 4);
  - return `async_launched { child_id }` immediately.

- [ ] **Step 4: `max_turns`** — add `Agent.max_turns: Option<u32>` + `Agent::with_max_turns(Option<u32>)` and `Agent.turns_taken: u32` (default 0). In `agent_loop`, increment `turns_taken` at the top of each loop iteration and, when `max_turns` is reached, break (emit an info update). After `agent_loop` returns, `spawn_subagent` appends `(max_turns reached)` to the summary when the cap was hit. (The existing `tool_use_counter` is a tool counter reset per `SubmitTask`, not a turn counter — do not reuse it.)

- [ ] **Step 5: `resume`** — when `resume = Some(child_id)`: `try_lock_session(child_id, pid)` first (paired with `release_session_lock`), build the child `Agent` with `with_session(child_id, store)`, then `agent_loop(Some(Message::new_text(Role::User, input.prompt)))`. Provider state is rehydrated by the child's `ensure_session` on entry (`agent/mod.rs:427-465`) — **no** manual `load_provider_state` (M7). The child's context is seeded from `load_session` automatically because `ensure_session` runs before the loop.

- [ ] **Step 6: `check_subagent`** — add a read-only tool (`PermissionPolicy::Read`, `ResourcePolicy::Independent`) reading `ctx.subagent_manager.check(child_id)`, returning pretty JSON of the `SubagentRun` (clone of `check_background`, `background_run.rs`). This closes M3's "no status-query tool after restart" gap.

- [ ] **Step 7: Cancellation** — store each async child's `cancel_flag: Arc<AtomicBool>` in the manager/in-flight registry; `subagent_runs` transitions to `Cancelled` when the flag is set and the loop exits. (User-facing cancel surface is follow-up; first cut just stores the handle.)

---

### Task 12: Drain the queue in `agent_loop`

**Files:**
- Modify: `crates/tact/src/agent/mod.rs`

- [ ] **Step 1: Drain before building the next request** — after the micro/auto-compact block and before `let conversation_messages = self.runtime.context.clone()` (`agent/mod.rs:700`):

```rust
let pending: Vec<SubagentResult> = {
    let mut q = self.runtime.pending_subagent_results.lock().expect("queue lock poisoned");
    q.drain(..).collect()
};
for result in pending {
    let text = format!(
        "<subagent-finished id=\"{}\" success=\"{}\">\n{}\n</subagent-finished>",
        result.child_id, result.success, result.summary
    );
    self.push_message(Message::new_text(Role::User, text)).await?;
}
```

Use `push_message` (not `runtime.context.push()`) so the synthetic message persists via `append_message` / `replace_persisted_context_and_state`. This is the durability mechanism; after a restart the message is **already in the transcript**, so no re-delivery is needed (M3).

- [ ] **Step 2: Tests** — drain produces exactly one persisted `<subagent-finished>` message before the next LLM call; drain before compact or re-queue after (pick one and assert it).

---

## Phase D — TUI

### Task 13: `SubagentFinished` dispatch → transcript carry-over (M2)

**Files:**
- Modify: `crates/agent_tui_kit/src/components/tool.rs`

- [ ] **Step 1: Add a `SubagentFinished` match arm** in the dispatch (`tool.rs:432-510`) and a handler that, **before** finalizing, carries the subagent transcript out of the live card — the exact logic `on_step_finished` performs at `tool.rs:257-272` (`take_full_detail()` → `detail_full`, `subagent_model` → `output.subagent_model`, `subagent_tokens` → `output.subagent_tokens`).

The existing `on_background_task_finished` (`tool.rs:324-360`) rebuilds from `message`/`output` only and would **drop** the transcript — do not reuse it verbatim.

- [ ] **Step 2: Test** — a keep-live subagent card finalized by `SubagentFinished` still exposes the full transcript in `detail_full`.

---

### Task 14: Keep-live presentation must be input-aware (both call sites)

**Files:**
- Modify: `crates/tact/src/agent/tool_dispatch.rs`

- [ ] **Step 1:** `make_presentation` (`tool_dispatch.rs:250-263`) is static. At **both** call sites (`:400` `StepStarted` and `:754` `StepFinished`), when `name == "spawn_subagent"` and `input.get("run_in_background").and_then(|v| v.as_bool()) == Some(true)`, override `presentation.keep_live = true` (keep `popup = SubagentTranscript`). Otherwise the finished event's `keep_live` falls back to static `false`.

- [ ] **Step 2: Test** — `StepFinished` for a `run_in_background` subagent carries `keep_live == true`.

---

### Task 15: TUI relay to driver (C1)

**Files:**
- Modify: `crates/tui/src/widgets/state/app/agent.rs`

- [ ] **Step 1: Handle `AgentUpdate::SubagentFinished`** in `handle_agent_update` — update local card state, then relay a wake-up command:

```rust
if let Some(tx) = self.user_cmd_tx.clone() {
    let _ = tx.send(UserCommand::SubagentFinishedNotification { child_id, summary, success });
}
```

`App` already holds `user_cmd_tx` (`state/mod.rs:159`).

- [ ] **Step 2: Driver handling** in `crates/tact-ui/src/driver.rs` (`match cmd` at `driver.rs:55`):

```rust
UserCommand::SubagentFinishedNotification { .. } => {
    reap_finished_task(&mut agent, &mut active).await;
    if active.is_none() {
        // idle: submit a turn carrying only the notification.
        let task = /* the same "<subagent-finished …>" text as Task 12 */;
        // reuse the UserCommand::SubmitTask dispatch path.
    }
    // else: parent mid-turn — the agent_loop drain handles it.
}
```

- [ ] **Step 3: Test** — parent idle → notification produces a `SubmitTask`-shaped turn; parent busy → dropped (queue drain covers it).

---

### Task 16: Multi-popup

**Files:**
- Modify: `crates/tui/src/widgets/state/mod.rs` — replace `subagent_popup: Option<SubagentPopup>` (`:215`) with `subagent_popups: HashMap<String, SubagentPopup>` keyed by `tool_id`.
- Modify: the popup render/geometry site(s) — keep geometry derived from the main area (AGENTS.md invariant 4).

- [ ] **Step 1: Map + render loop**; active popup selection (e.g. most-recent `tool_id`).
- [ ] **Step 2: Buffer-level test** — popup stays inside the main area; blank cells carry `theme.bg`.

---

### Task 17: Concurrent permission-prompt queue (M5)

**Files:**
- Modify: `crates/tui/src/widgets/state/app/agent.rs` (the `RequestSelect`/`RequestMultiSelect` arms at `:279-296`) and the select-state type in `state/mod.rs`.

- [ ] **Step 1:** Today a single `self.select` is overwritten by a second concurrent request, hanging the first waiter (`ui_responder` oneshot). Keep a queue (or `HashMap<request_id, …>`) of pending selects; resolve each waiter's oneshot as its answer arrives, in arrival order.

- [ ] **Step 2: Test** — two concurrent `RequestSelect`s: answering the second does not drop the first; both oneshots resolve.

---

## Documentation & close-out

### Task 18: Docs + issue log

- [ ] **Step 1:** Sync `book/12_chapter_subagent.md` + `_zh.md` (permission inheritance + async semantics + `check_subagent`).
- [ ] **Step 2:** Append newest-first Ch 26 entries (`book/26_chapter_issue.md` + `_zh.md`) for each user-visible change (permission inheritance, async subagent, resume/cancel/check).
- [ ] **Step 3:** Run the full verification loop once, sequentially, with proxy unset:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tui --lib
env -u http_proxy -u https_proxy -u all_proxy cargo clippy --workspace --all-targets
```

---

## Verification checklist (before completion claims)

- [ ] `snapshot`/`from_snapshot` round-trip; `Plan` parent → read-only child (regression).
- [ ] `run_in_background: true` returns `async_launched{id}` without blocking; `SubagentFinished` emitted exactly once on the **parent** `ui_tx`.
- [ ] Queue drained before the next LLM call → one persisted `<subagent-finished>` message.
- [ ] `subagent_runs` `Running` → `Completed`/`Failed`; orphan repair on startup → `Failed` ("Process interrupted (agent restarted)").
- [ ] `max_turns` bounds a runaway child (truncated summary).
- [ ] `resume` loads an existing child via `ensure_session` (no manual `load_provider_state`), acquires `try_lock_session`.
- [ ] `check_subagent` reads `subagent_runs`.
- [ ] Concurrent permission prompts resolve both waiters (no hang).
- [ ] Multi-popup stays inside the main area; blank cells carry `theme.bg`.
- [ ] `spawn_subagent` remains `Barrier` (no file-race regression).
