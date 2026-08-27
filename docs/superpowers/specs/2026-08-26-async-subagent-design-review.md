# Review: 2026-08-26 Async / Multi-Subagent Design

- **Date:** 2026-08-26
- **Status:** Review complete — 2 critical, 7 major findings with
  patch-level revision proposals
- **Reviewed doc:** `docs/superpowers/specs/2026-08-26-async-subagent-design.md`
- **Method:** every finding verified against code, not against the design
  doc's own self-description. Line numbers refer to the reviewed revision.

---

This is a strict, code-grounded review of the async/multi-subagent design.
The permission-inheritance core (snapshot + `from_snapshot`) is technically
sound given `PermissionSettings: Clone` and `ToolContext: Clone`. The async
mechanics have **two core assumptions that contradict the code** and **seven
design gaps that would silently break behavior**. Each finding below states
the doc's claim, the code truth, and a concrete revision.

---

## Critical

### C1 — "wake-up turn via existing `ui_tx`" is an invented channel

**Doc claims** (§Async / multi-subagent mechanics, "Result re-injection"):

> "If the parent is idle … the driver submits a lightweight wake-up turn …
> the completion task signals the driver through the existing `ui_tx` channel."

**Code truth:**

- `ui_tx` is `UnboundedSender<AgentUpdate>`, wired **one-way to the TUI only**
  (`crates/tact-ui/src/interactive.rs:102` `agent_tx/agent_rx` → `App`;
  `:123` `ui_tx: Some(agent_tx.clone())`). The driver never reads it.
- The driver consumes **only** `user_cmd_rx`
  (`crates/tact-ui/src/driver.rs:52` `while let Some(cmd) = user_cmd_rx.recv()`).
- `UserCommand` (`crates/protocol/src/agent.rs:349-388`) has **no** wake-up /
  notification variant — only `SubmitTask / Cancel / Compact / QueryBalance /
  QueryStats / QueryBackground / SetPermissionMode / SetThinkingBudget /
  SetReasoningEffort / SetModel / UiResponse`.
- The TUI never sends a command back to the driver in reaction to an
  `AgentUpdate` (`crates/tui/src/widgets/state/app/agent.rs:220-238`
  `handle_agent_update` only mutates local state).

**Impact:** the core UX scenario — parent finishes its turn, a background
subagent completes later, and the parent automatically resumes to consume the
result — has **no implementation basis**. Written as-is, it would never fire.

**Revision (patch-level):**

Add one variant and one TUI→driver relay; the driver is the only component
that knows whether the agent is busy (`active: Option<JoinHandle>`):

```rust
// crates/protocol/src/agent.rs — UserCommand gains:
/// A background subagent finished while the parent may be idle. The driver
/// decides whether to (a) drop it (parent is mid-turn — the result is already
/// in `pending_subagent_results`) or (b) submit a synthetic wake-up turn.
SubagentFinishedNotification {
    child_id: String,
    summary: String,
    success: bool,
},
```

```rust
// crates/tui/src/widgets/state/app/agent.rs — handle_agent_update,
// AgentUpdate::SubagentFinished arm: relay to the driver (TUI already holds
// the user_cmd_tx sender passed in at interactive.rs:186).
if let Some(tx) = self.user_cmd_tx.clone() {
    let _ = tx.send(UserCommand::SubagentFinishedNotification { .. });
}
```

```rust
// crates/tact-ui/src/driver.rs — match arm:
UserCommand::SubagentFinishedNotification { child_id, summary, success } => {
    reap_finished_task(&mut agent, &mut active).await;
    if active.is_none() {
        // idle: submit a turn that carries only the notification.
        let task = format!("<subagent-finished id=\"{child_id}\" success=\"{success}\">\n{summary}\n</subagent-finished>");
        // ... same SubmitTask dispatch path as UserCommand::SubmitTask ...
    }
    // else: parent is mid-turn; the queue drain in agent_loop handles it.
}
```

Alternatively (smaller surface): the completion task pushes into
`pending_subagent_results` **and** sends the TUI a plain
`AgentUpdate::SubagentFinished`; the TUI unconditionally relays a
`SubmitTask`-shaped notification to the driver, and the driver drops it when
`active.is_some()`. Either way, **a new channel/relay is required** — do not
claim `ui_tx` already covers it.

---

### C2 — `SharedState { scope: "subagent" }` overlaps disjoint *writes*, not just reads

**Doc claims** (§Concurrency):

> "sync subagents serialize with each other … but may still overlap disjoint
> file **reads**."

**Code truth** (`crates/tact/src/tool/metadata.rs:120-130` +
`crates/tact/src/agent/tool_schedule.rs:95-99`):

`SharedState { scope }` resolves to a single **synthetic write**
`__tact_{scope}__`. `conflicts()` returns conflict only for (barrier) / (real
path overlap) / (same synthetic scope). It does **not** conflict with real
writes to other paths.

Therefore a sync subagent — which internally holds `bash` + `edit_file` and
whose file effects **cannot be scoped** — would run in the **same wave** as a
`write_file` / `edit_file` / `apply_patch` writing any *other* path. The doc's
"only reads" is wrong: disjoint **writes** also overlap. Subagent A's `bash`
could write path `B` while a same-wave `write_file` also writes `B`.

**Impact:** moving `spawn_subagent` from `Barrier` (`subagent.rs:39`) to
`SharedState` is a **much wider** relaxation than the doc acknowledges — a
real file-race. This is exactly what `Barrier` currently prevents.

**Revision:** keep `ResourcePolicy::Barrier` for the first cut. Background
parallelism does **not** need a resource-policy change — `run_in_background`
decouples the parent *turn* via `tokio::spawn`, not via wave scheduling. The
"overlap disjoint reads" claim (which itself is impossible with barrier) can
only be delivered after `isolation: "worktree"` gives each subagent a scoped
filesystem. Rewrite the Concurrency section as:

> First cut: `spawn_subagent` stays `Barrier`. Async subagents achieve
> "parent not blocked" by spawning detached tasks outside the wave scheduler.
> Same-wave fan-out of *blocking* subagents is deferred to the worktree
> follow-up, where `ResourcePolicy::Independent` becomes safe.

---

## Major

### M1 — `permission_snapshot` stamp position is underspecified and "per-invocation" is wrong

**Doc** says "stamp per-invocation" and sketches
`self.tool_context.permission_snapshot = Some(self.runtime.permission_manager.snapshot());`
with no anchor point.

**Code truth** (`crates/tact/src/agent/tool_dispatch.rs`):

- `ToolContext` is `#[derive(Clone)]` (`crates/tact/src/tool/mod.rs:101`); each
  invocation clones it via `for_invocation` (`tool_dispatch.rs:183`). The doc's
  "one clone per invocation" assumption holds, but the stamp is a **single
  write to the shared `&self.tool_context`**, therefore **per-turn**, not
  per-invocation. (This is fine for the stated semantics — a subagent should
  see the parent's *current* mode, identical across one turn's wave — but the
  wording should not promise per-invocation granularity the structure can't
  give.)
- The stamp **must** land after phase 1 pre-flight (where the user may have
  just clicked "always allow this tool" → `allow_tool_with_input`) and
  **before** `let ctx = &self.tool_context;` inside the wave loop
  (`tool_dispatch.rs:614`) — after that point `ctx` is borrowed by the wave
  futures and the borrow checker rejects a mutable access to
  `self.tool_context`.

**Revision:** state the invariant explicitly:

> Stamp `permission_snapshot` once in `execute_tool_call`, after the phase-1
> prepare loop and before entering `waves_grouped` — i.e. after the last
> `PreparedTool` is pushed and before `for wave in waves_grouped(...)`. This
> captures the parent's post-authorization mode (same-turn "always allow"
> included) and is done before the wave futures borrow `self.tool_context`.

---

### M2 — keep-live finalize path drops the subagent popup transcript

**Doc** (§UI / Return) claims both:

> "the card stays live (`keep_live`)" … and "full transcript stays in the
> popup".

**Code truth** (`crates/agent_tui_kit/src/components/tool.rs`):

- `on_step_finished` returns **immediately** when
  `result.presentation.keep_live` is true (`tool.rs:246-253`).
- The subagent transcript carry-over (`take_full_detail()` +
  `subagent_model` + `subagent_tokens` → `output.detail_full`) lives **only**
  in the non-keep-live branch (`tool.rs:257-272`).
- `on_background_task_finished` (`tool.rs:317-360`) rebuilds the card from
  `message`/`output` and does **not** carry over any popup transcript.

**Impact:** an async subagent that follows "keep_live + `SubagentFinished`
finalize" will **lose the full conversation** — the popup shows only the
one-line summary.

**Revision:** add a dedicated `AgentUpdate::SubagentFinished` handler in
`agent_tui_kit` that, before finalizing, performs the same carry-over the
`on_step_finished` subagent branch does today (read `active.live_output.
take_full_detail()` and the `subagent_model`/`subagent_tokens` fields into
`detail_full`). Do not reuse `on_background_task_finished` verbatim — it
cannot see the transcript.

---

### M3 — persistence narrative self-contradicts; no status-query tool after restart

**Doc** says (§Result re-injection) the synthetic `<subagent-finished>` goes
through `append_message` (persisted), **and** that "After a restart … the
parent transcript already contains the `async_launched { id }` tool result"
(implying the finished summary is *not* persisted).

**Code truth** (`crates/tact/src/agent/mod.rs:493-535` `persist_message`):
once the queue is drained, the `<subagent-finished>` message **is** in the
parent session's `messages` table and is read back by
`ensure_session → load_session`. The doc's justification ("transcript only has
`async_launched {id}`") is false in the drained case. The "no automatic
re-delivery" *conclusion* is still correct — but for a different reason
(the message is already in the transcript, so re-delivery would duplicate it).

Second gap: if the process dies **before** drain, the result is lost, and the
scope provides **no tool for the model to discover child state**. `resume` is
a follow-up turn, not a status query; `subagent_runs` is never exposed to the
model.

**Revision:**

1. Rewrite the durability paragraph: the `<subagent-finished>` message **is**
   persisted (that is the durability mechanism); "not re-delivered after
   restart" means "not re-injected a second time," because the transcript
   already carries it.
2. Add a status-query tool (e.g. `check_subagent { child_id }` reading
   `subagent_runs`, returning `Running / Completed / Failed / Cancelled` +
   `summary`), or explicitly accept that the model guesses blindly after a
   crash. Given `background_run` already ships a `check_background` tool, the
   symmetric `check_subagent` is the natural, cheap fix.

---

### M4 — `SubagentFinished` must go through the **parent** `ui_tx`, not the child's tagged forwarder

**Doc** (§New protocol event) only says "emits `AgentUpdate::SubagentFinished`"
without specifying the channel.

**Code truth** (`crates/tact/src/tool/subagent_ui.rs:125`): the child's
`tagged_ui_channel_with_progress` forwarder matches known variants and has
`_ => {}` — a `SubagentFinished` sent down the **child's** tagged channel is
**silently dropped**. The completion task must send on the **parent**
`ctx.ui_tx` captured at spawn time.

**Revision:** state it: "the detached completion task sends
`SubagentFinished` on the **parent** `ToolContext.ui_tx` captured at spawn;
it must not route it through the child's tagged progress forwarder (which
drops unknown variants)."

---

### M5 — concurrent permission prompts are a hard blocker, not a UI polish item

**Doc** (§UI upgrade) lists per-subagent permission-prompt queuing as a popup
upgrade.

**Code truth** (`crates/tui/src/widgets/state/app/agent.rs:279-294`): the
`RequestSelect` / `RequestMultiSelect` handler does
`self.select.set(...)` — a **single** popup slot. Two subagents asking
concurrently: the second overwrites the first; the first's waiter (a
`ui_responder` oneshot, `crates/tact/src/ui_responder.rs:61-63`) blocks
**forever**. This hangs a subagent, not merely reorders UI.

**Revision:** promote this from "UI upgrade" to a **blocking prerequisite**.
The responder waiters already carry unique `request_id`s; the fix is to keep a
queue (or map keyed by `request_id`) of pending selects instead of one
`self.select`, resolving each oneshot as its answer arrives.

---

### M6 — `ToolContext.subagent_results: None` has no defined fallback

**Doc** (§Result re-injection) gives the permission snapshot a `None` fallback
but defines **none** for `subagent_results`.

**Code truth:** any path that constructs a `ToolContext` and runs the subagent
tool **without** going through `execute_tool_call` (headless direct tool runs,
`test_support::run_tool`, future callers) would spawn an async child whose
completion callback has nowhere to push.

**Revision:** define the degradation. Options: (a) async spawn returns an
error when `subagent_results.is_none()` ("run_in_background requires a parent
agent runtime"); or (b) fall back to UI-event-only finalize with no
re-injection. Pick one and state it; do not leave it as an unwrapable `None`.

---

### M7 — `load_provider_state` is not "required for OpenAI-compatible providers"; manual call is redundant

**Doc** (§`max_turns` and `resume`) says
"`load_provider_state(child_id)` … required for OpenAI-compatible providers"
and lists it as a manual resume step.

**Code truth** (`crates/tact/src/store/session_store/sqlite.rs:389`):

- `load_provider_state` reads the `responses_states` table — provider state
  exists **only** for the Responses protocol. OpenAI-compatible providers can
  run `ChatCompletions` (the default `OpenAiProtocol`), where it is `None` and
  resume still works; so is Anthropic.
- Resume does **not** need a manual call: constructing the child `Agent` and
  entering `agent_loop` triggers `ensure_session`, which already does
  `load_session` + `load_provider_state`.

**Revision:** drop the manual call and the "required" wording. Say: "provider
state is rehydrated by the child's `ensure_session` on the next
`agent_loop`; it is only relevant for Responses-protocol providers."

---

## Minor (wording / implementation details)

- "`PermissionPolicy::High` → always `Ask` in Default mode" is not absolute:
  settings deny/allow rules (steps 4-6 in `permission/mod.rs:228` `check`)
  run before the high-risk ask (step 7). Say "asks, absent a matching
  settings rule."
- "tokens forwarded as `AgentUpdate::ToolProgress`" — tokens/model go via
  `AgentUpdate::ToolMeta`; steps/thinking/stream via `ToolProgress`.
- "the last assistant text block" — it is the last `Role::Assistant` message's
  `extract_text` (`subagent.rs:129-135`).
- `max_turns` needs a **new** counter; `tool_use_counter` is a tool counter
  reset by the driver on every `SubmitTask` (`driver.rs:147`) and is not a
  turn count.
- `try_lock_session` takes `pid: u32` and returns `lock_epoch`; it must be
  paired with `release_session_lock` — the resume section should mention the
  pid/release pairing.
- Dynamic keep-live presentation must be recomputed at **both**
  `make_presentation` call sites — `StepStarted` (`tool_dispatch.rs:400`) and
  `StepFinished` (`:754`) — or the finished event's `keep_live` will fall
  back to the static `false`.
- Switching `spawn_subagent` off `Barrier` removes its
  `WaveSummary.barrier` audit marker (see `tool_schedule.rs::summarize`).
- Parent-exit queue accumulation is bounded (task death releases the
  `Arc<Mutex<VecDeque>>`); worth one sentence, not a leak.

## Unverifiable (external)

- Claude Code / Codex reference behaviors (`<task-notification>`,
  `async_launched{id}`, 24h expiry, Claude issue #39335) are external sources
  and cannot be confirmed from this repository.

---

## Bottom line

Permission inheritance is sound. Before writing the implementation plan, fix
C1 (invent a wake-up relay), C2 (keep `Barrier` for the first cut), and the
seven majors — especially M2 (transcript carry-over), M4 (parent `ui_tx`), M5
(concurrent prompt queue), and M6 (`None` fallback) — or the first
implementation will silently drop transcripts, hang subagents, or never wake
the parent.
