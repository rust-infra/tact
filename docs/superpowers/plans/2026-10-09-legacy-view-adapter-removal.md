# Legacy View Adapter Removal (Task 13 closure)

**Status:** proposed — needs approval before implementation.

**Spec:** `docs/superpowers/specs/2026-10-08-runtime-plugin-architecture-design.md` §2, §6, §Verification and acceptance.
**Parent plan:** `docs/superpowers/plans/2026-10-08-runtime-plugin-architecture.md`, Task 13.

## Goal

Finish Task 13: remove the in-process `AgentUpdate` / `UserCommand` compatibility
adapter so the TUI is a real View adapter — it consumes `RuntimeEvent` and submits
`RuntimeCommand` / `InteractionResponse`, and nothing else.

Everything else in the 2026-10-08 plan is implemented; this is the last item whose
plan checkbox is checked but whose code has not been migrated.

## Why this is not already done

`ARCHITECTURE.md` §15 and `docs/plugin_protocol.md` both state it out loud: "Rich
tool-card lifecycle details and specialized Tact slash commands still use the
in-process `AgentUpdate` / `UserCommand` adapter."

## Inventory (verified 2026-10-09, `50da4312`)

### The protocol side has no gap

`crates/tact_view/src/lib.rs::runtime_events_for` maps **all 25** `AgentUpdate`
variants onto `RuntimeEvent` (23 direct variants plus `RequestSelect` /
`RequestMultiSelect` → `RuntimeEvent::InteractionRequested`). The neutral
protocol already carries everything the legacy type carries, so this migration
does **not** need new `RuntimeEvent` variants — except where a variant is mapped
lossily (see Task 1).

Re-verify with (the second command must print nothing):

```sh
awk '/pub enum AgentUpdate/,/^}/' crates/tact_view/src/lib.rs \
  | grep -oE '^\s{4}[A-Z][A-Za-z]*' | tr -d ' ' | sort > /tmp/variants.txt
awk '/pub fn runtime_events_for/,/^}/' crates/tact_view/src/lib.rs \
  | grep -oE 'AgentUpdate::[A-Za-z]+' | sed 's/AgentUpdate:://' | sort -u > /tmp/arms.txt
comm -23 /tmp/variants.txt /tmp/arms.txt
```

### The direct Agent → TUI channel is already gone in production

- `Agent::with_ui_channel` (`crates/tact_extensions/src/agent/mod.rs:828`) — the
  only writer of `tool_context.ui_tx` — is gated
  `#[cfg(any(test, feature = "test-support"))]` and documented as a compatibility
  adapter for tests.
- All 10 call sites are inside `#[cfg(test)]` modules of the same file.

So the production residue is the **event type**, not the channel.

### Where the legacy type is referenced, by crate

| Crate | refs | note |
|---|---|---|
| `tact_view` | 66 | the type itself, `runtime_events_for`, and the relocated `runtime_event_to_agent_updates` |
| `tact_llm` | **0** | cleared by Task 2 — the crate now depends on `tact_protocol` only |
| `tact_extensions` | 247 | emission side |
| `tui` | 277 | consumption side |
| `tact_ui` | 65 | driver + test support |

Re-verify with:

```sh
for c in tact_view tact_llm tact_extensions tui tact_ui; do
  echo "$(grep -rc 'AgentUpdate' crates/$c/src | awk -F: '{s+=$2} END {print s}')  $c"
done
```

**`tact_llm` is the architecturally loaded one:** the LLM adapter currently
depends on `tact_view` only to name a *view* type, and it emits 7 of the 25
variants (`StreamChunk`, `ThinkingChunk`, `TokenUsage`, `ModelInfo`,
`StepStarted`, `StepFinished`, `StepFailed`). Giving it a protocol-neutral
streaming delta (or `RuntimeEvent`) would drop that dependency.

### Production consumers of `AgentUpdate`

| File | refs |
|---|---|
| `crates/tui/src/widgets/state/app/agent.rs` | 115 |
| `crates/tact_ui/src/driver.rs` | 53 |
| `crates/tui/src/lib.rs` | 38 |
| `crates/tui/src/handlers/mouse.rs` | 14 |
| `crates/tui/src/handlers/normal.rs` | 6 |
| `crates/tui/src/render/task_panel.rs` | 5 |
| `crates/tui/src/render/layout.rs` | 5 |
| `crates/tui/src/widgets/state/app/popups.rs` | 3 |
| `crates/tui/src/widgets/state/mod.rs` | 2 |
| `crates/tui/src/widgets/state/app/construct.rs` | 2 |
| `crates/tui/src/headless_loop.rs` | 2 |
| `crates/tui/src/handlers/skills.rs` | 2 |
| `crates/tui/src/widgets/state/app/extensions.rs` | 1 |
| `crates/tui/src/handlers/select.rs` | 1 |
| `crates/tact_ui/src/session_bootstrap.rs` | 1 |
| `crates/tact_ui/src/headless_session.rs` | 1 |
| `crates/tui/src/test_support.rs` | 4 |
| `crates/tui/src/test_fixtures.rs` | 6 |

Test-only files that also reference it (mechanical churn, no design work):
`render/render_gap_tests.rs` (39), `render/popup_scene_tests.rs` (19),
`render/log_render_tests.rs` (17), `render/scene_tests.rs` (15),
`render/cells/markdown_integration_tests.rs` (5),
`render/cells/code_overlay_tests.rs` (3), `tact_ui/src/test_support.rs` (10).

Re-verify with:

```sh
for f in $(grep -rln 'AgentUpdate' crates/tui/src crates/tact_ui/src); do
  echo "$(grep -c 'AgentUpdate' "$f")  $f"
done | sort -rn
```

## Target end state

- `crates/tact_view` keeps the Rust view-model (`AgentUpdate` is deleted; the
  view-model structs such as `PlanStep`, `ToolPresentationInfo`, `TokenUsageInfo`
  stay — they are payload types `RuntimeEvent` itself references).
- `tact_extensions` emits only through `ViewUpdateEmitter::runtime_sink`; the
  `legacy` variant and `ui_responder`'s `legacy_tx` are deleted.
- The TUI applies `RuntimeEvent`; tests build `RuntimeEvent`s instead of
  `AgentUpdate`s.

## Tasks

Each task must leave the workspace building and the full test suite green, so the
migration can stop after any task without a broken tree.

### Task 1: Close the lossy mappings

`runtime_events_for` maps some variants lossily. Before deleting the legacy type,
make each `RuntimeEvent` carry what the TUI currently reads from `AgentUpdate`.

Audit result (2026-10-09, all 25 variants compared field by field):

- [x] Audit each of the 25 mappings against the fields the TUI reads in
      `widgets/state/app/agent.rs`. **23 of 25 map 1:1.**
- [x] Finding 1 — `RequestSelect.log_confirm` was dropped by a `..` pattern on
      both legs (forward `runtime_events_for`, and reverse
      `runtime_event_to_agent_updates`, which hardcoded `false`). **Closed:**
      `InteractionRequest::Select` now carries `log_confirm` (`#[serde(default)]`),
      both legs pass it through, and regression tests on each leg were verified to
      fail against the old behaviour.
- [x] Finding 2 — **View layer closed; recorder layer still open.** `TaskComplete`
      / `TaskCancelled` carry no run id, and the mapping used to fabricate
      `RunId::from("runtime")` for `RunFinished` / `Cancelled`. Option (a) was
      taken: both variants now carry `Option<RunId>`, so a turn that never
      started a run says so instead of borrowing a shared synthetic identity.
      `tact_view::tests::a_run_less_turn_does_not_invent_a_run_id` pins it and was
      verified to fail when the fabrication is reintroduced.
- [x] Finding 2b — **closed.** The fabrication also lived in
      `TrajectoryService::append`, which substituted `RunId::new("runtime")`
      when neither the caller nor the event supplied one and then derived
      `trajectory_id` from it. Production hits this: the subscriber calls
      `append(None, None, event)` (`tact_ui/src/session_bootstrap.rs`), so every
      run-less fact of a session — plugin lifecycle, notices, a turn cancelled
      before it started — landed in one trajectory literally named `runtime`,
      where a real run of that name would have collided with it.
      `TrajectoryEvent.run_id` is now `Option<RunId>`; run-less facts go to a
      dedicated `unattributed` trajectory and carry no run identity. The SQLite
      column stays `NOT NULL` (an empty string is how "no run" is stored), so
      existing databases keep working. Pinned by
      `tact_trajectory::service::tests::a_fact_outside_any_run_is_not_attributed_to_one`
      and `tact::services::tests::trajectory_append_and_read_through_the_router`,
      both verified to fail against the previous behaviour.
- [x] Non-finding: `AgentUpdate::Error(AgentErrorKind)` looked lossy
      (`to_string()`), but `AgentErrorKind` is a single-variant enum
      (`Other(String)`) today, so nothing is dropped.
- [x] Round-trip test for the projection:
      `tact_view::tests::select_projection_keeps_log_confirm`.

**Why first:** once the legacy type is gone there is no second source to fall
back on, and a dropped field is a silent rendering regression.

### Task 2: Give the LLM adapter a neutral streaming type

`crates/tact_llm` names `tact_view::AgentUpdate` in `LlmClient::stream_message`
(and therefore in Anthropic, both OpenAI adapters, `multi_model`, the mock and
`client.rs`) purely to report streaming progress. It is the only dependency
`tact_llm` has on `tact_view`.

- [x] Replace the sender payload with a protocol-neutral streaming delta (or
      `RuntimeEvent` directly: `Text` / `Thinking` / `TokenUsage`, plus the
      web-search `Step*` facts the Responses adapter emits). `run_id` may be
      `None` here — the emitter that owns the run stamps it.
      **Done:** every provider now sends `tact_protocol::RuntimeEvent` built by
      `crates/tact_llm/src/stream_event.rs` with `run_id: None`, and
      `ViewUpdateEmitter::emit_runtime_event` (+ `stamp_run_id`, 7 arms with an
      explicit pass-through fallback) fills in the identity it owns.
- [x] Drop `tact_view` from `crates/tact_llm/Cargo.toml`; the crate must depend
      on `tact_protocol` only.
- [x] Update `crates/tact_extensions/src/agent/mod.rs:1628` — the local channel +
      forwarder that currently converts `AgentUpdate` → `RuntimeEvent` via
      `runtime_events_for` collapses into a pass-through. The forwarder now feeds
      `emit_runtime_event`; a harness that has not migrated still receives the
      legacy view model, projected back through
      `tact_view::runtime_event_to_agent_updates`.
- [x] `runtime_event_to_agent_updates` moved from `crates/tui` into
      `crates/tact_view` (a pure relocation; `tui` re-exports it), so the
      extension crate can project back without depending on `tui`.
- [x] Update `tact_llm` tests that assert on `AgentUpdate` — they now match
      `RuntimeEvent` patterns.

**Landed 2026-10-09**, verified by `./scripts/check-rust.sh` (fmt + clippy
`-D warnings` + the whole workspace) with the test count unchanged at 2857, i.e.
the round trip through the projection preserved behaviour. `ARCHITECTURE.md` §0
dropped the `llm --> view` edge it no longer has.

**Why here:** it is the most self-contained lane (one crate plus one call site),
it removes a wrong dependency edge, and it does not touch the TUI at all — so it
can land without the human eyeball Task 4 needs.

**Cost re-measured 2026-10-09 — this lane is larger than it first looked:**

- `ViewUpdateEmitter::emit(AgentUpdate)` is the **only** place a run id is
  stamped (it calls `runtime_events_for(&update, run_id)`). An LLM adapter never
  sees a run, so it can only send `run_id: None` and the stamping has to move to
  the agent side.
- `RuntimeEvent` has **41 variants, 33 of which carry `run_id`, and no generic
  setter** — so either write a 41-arm `with_run_id`, or a 7-arm stamp for the
  variants a model stream actually produces plus an explicit pass-through
  fallback. The latter is preferable.
- **The test path needs the reverse mapping.** With no `runtime_event_sink`
  (`agent/mod.rs:1638`) the LLM's output goes straight to `tool_context.ui_tx`,
  which tests read as `AgentUpdate` (e.g. `provider.rs:981` asserts
  `AgentUpdate::TokenUsage`). `runtime_event_to_agent_updates` currently lives in
  the **`tui`** crate, which `tact_extensions` cannot depend on — so either move
  it into `tact_view` or write a cfg-gated 7-arm reverse map in the extension.

Do the `tact_view` move first (it is a pure relocation, compiler-checked, and
`tui` can re-export), then the type swap.

**Verify:** `cargo test -p tact_llm`, then `cargo test -p tact_extensions --lib`.

### Task 3: Give the Agent one emission path

- [x] `ViewUpdateEmitter`: the legacy `AgentUpdate` channel is no longer used by
      any producer. **Landed:** every emission site in the Agent, `tool_dispatch`,
      the tool helpers, `plugin/hooks` and the command driver builds a
      `RuntimeEvent` through `crate::runtime_event` (which leaves `run_id` empty)
      and hands it to the emitter, which attributes it via
      `RuntimeEvent::with_run_id`. `emit_update`,
      `ToolProgressReporter::emit`/`send` and the direct `ViewUpdateEmitter`
      calls take protocol events now; production references drop 247 → 170
      (`tact_extensions`) and 65 → 28 (`tact_ui`), and no `emit(AgentUpdate::…)`
      call site is left.
- [x] The precondition is in place: `RuntimeEvent::with_run_id` matches all 41
      variants exhaustively, so attributing an event can no longer silently drop
      the run id on a variant nobody classified.
- [ ] Still to do: delete the legacy channel itself (`ViewUpdateEmitter::legacy`,
      `UiResponder::legacy_tx`, `Agent::with_ui_channel`, `tool_context.ui_tx`),
      which is what the tests in Task 5 read.

**Verify:** `cargo test -p tact_extensions --lib`.

### Task 4: Switch the TUI to `RuntimeEvent`

- [ ] `App::handle_agent_update(AgentUpdate)` → `handle_runtime_event(RuntimeEvent)`
      in `crates/tui/src/widgets/state/app/agent.rs` (largest single change).
- [ ] Update the remaining production consumers listed above.
- [ ] `crates/tact_ui/src/driver.rs` and `session_bootstrap.rs` push
      `RuntimeEvent`s instead of `AgentUpdate`s.

**Verify:** `cargo test -p tui --lib` then `cargo test -p tact_ui --lib`, plus the
TUI render/integration suites. Behaviour must be byte-identical where the existing
buffer-level tests assert rendering.

### Task 5: Migrate the tests and delete `AgentUpdate` / `UserCommand`

- [ ] Replace `AgentUpdate` construction in the test-only files listed above with
      `RuntimeEvent` construction (mechanical; a helper in `test_fixtures.rs`
      keeps it short).
- [ ] Delete `AgentUpdate`, `UserCommand`, `AgentErrorKind`, and
      `runtime_events_for` from `tact_view`; keep the view-model payload types.
- [ ] Re-check `docs/plugin_protocol.md` / `ARCHITECTURE.md` §15 for the
      "still uses the adapter" sentences and delete them.

**Verify:** `./scripts/check-rust.sh`.

### Task 6: Prove the boundary

- [ ] Add the spec's acceptance test: a View that consumes `RuntimeEvent` and
      submits `RuntimeCommand` with no `AgentUpdate` in scope — i.e. compile-time
      proof that `tui` no longer names the type.
- [ ] `cargo doc --no-deps` and diff the warning count (do not require zero; the
      repo carries ~45 pre-existing rustdoc warnings).

## Risks

- **Rendering regressions are invisible to the agent** — it cannot read the TUI
  output. Mitigation: every stage is gated on the existing buffer-level render
  tests, and Task 1 exists specifically so no field is dropped. A human should
  eyeball the TUI once after Task 4.
- **Driver direction** — `tact_ui/src/driver.rs` is where `UserCommand` and
  `RuntimeCommand` meet; it is the most likely place to discover a command that
  has no `RuntimeCommand` counterpart yet. Budget for it in Task 4.
- **Size** — ~740 `AgentUpdate` references across five crates (roughly half in
  tests). This is a multi-session job; do not start Task 4 without room to finish
  it. Task 2 is still the right first lane (it never touches the TUI) but it is
  **not** a one-file change — see its cost re-measurement.

## Non-goals

- No new runtime capability, no change to what the TUI displays.
- No `Web` / `Desktop` adapters (they do not exist in this branch).
- Not touching the WASM host's Wasmtime-CLI design (separate decision).
