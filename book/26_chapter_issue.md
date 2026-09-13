# Engineering Issue Log

> Language: [English](./26_chapter_issue.md) · [中文](./26_chapter_issue_zh.md)

This chapter is a **chronological log of optimizations and bug fixes** that changed user-visible or API-visible behavior. It is not a tutorial: each entry records the problem, the decision, and where the code / design docs live so future work does not rediscover the same trade-offs.

Related process docs: `AGENTS.md` (when to append here), `docs/superpowers/specs/` (design), `docs/superpowers/plans/` (implementation plans).

---

## 0. Purpose

| Goal | Detail |
|------|--------|
| Continuity | Capture *why* a change landed, not only *what* files moved |
| Cross-link | Point at design specs, PRs, and book chapters that teach the subsystem |
| Avoid churn | Prefer one entry per shipped behavior change; do not log pure refactors or test-only edits |

### Entry template

Newest entries first. Each entry should include:

1. **Date / ID** — `YYYY-MM-DD` and optional PR number  
2. **Type** — `optimization` · `bugfix` · `removal` · `docs`  
3. **Symptom / motivation** — what was wrong or expensive before  
4. **Decision** — the chosen contract (not discarded alternatives in full)  
5. **Behavior after** — observable rules agents and users rely on  
6. **Pointers** — code paths, specs, related book chapters  

---

---


## 1. 2026-09-13 — Explicit config beats the built-in model→window table, and a subagent section can no longer be dropped in silence

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/resolve.rs` (`resolve_config`, `resolve_subagent`); `config.example.toml`; [Ch 21](./21_chapter_config.md) §3 |

**Symptom / motivation:** Two instances of one class of bug — configuration that *looks* applied but never reaches a request.

1. `model_context_window` resolved as mapping > CLI > file, so the built-in model→window table **overrode both** the CLI flag and `[agent].model_context_window`. A user who deliberately set a window for a model with a built-in mapping had their value ignored. The documented rationale was safety (a stale manual window cannot under-report a well-known model), but the effect contradicted the project's "config wins" rule, and the same file could set `max_tokens` (honored) and `model_context_window` (ignored) side by side.
2. `resolve_subagent` returned `Ok(None)` as soon as `provider` was absent, so `[agent.subagent] max_tokens = 64000` with `provider` commented out silently did nothing. A `tracing::warn!` would not have helped here: logging is only installed when `RUST_LOG`/tokio-console is set, and config resolution runs earlier (`init()` in `crates/tact-ui/src/main.rs`), so the warning would never be printed.

**Decision:** (1) `model_context_window` now resolves CLI > `[agent]` > mapping > default `200_000`: the built-in table is a **fallback for unconfigured models**, not an override. The safety trade-off is documented instead of enforced — a stale manual window can under-report a long-context model and trigger premature auto-compaction, so the key should be deleted rather than left outdated. An explicit `0` still means "disabled/unknown window" and is *not* a fallback to the mapping. (2) A `[agent.subagent]` section that sets `model` / `max_tokens` / `thinking_budget` / `reasoning_effort` without `provider` is now a **hard resolve error** naming the fix, because `provider` is documented as required and the alternative is silently ignoring the overrides. A section with no overrides at all (a leftover header whose keys are all commented out) stays a silent no-op, so the guard does not fire on harmless templates.

**Behavior after:** Setting `[agent] model_context_window = 128000` for `deepseek-v4-pro` (built-in 1M) now yields 128,000, and `--model-context-window` wins over both. A config carrying subagent overrides without `provider` fails to start with `[agent.subagent] sets overrides but \`provider\` is missing, so the whole section is ignored. … Set \`provider\` to a key from [llm.providers.*], or remove the section.` — surfacing a class of error that previously required reading the resolve source to notice.

**Pointers:** `resolve_config` + `resolve_subagent` in `crates/tact/src/config/resolve.rs`; tests `resolve_model_context_window_toml_overrides_mapping`, `resolve_model_context_window_cli_overrides_toml_and_mapping`, `resolve_model_context_window_mapping_is_the_fallback`, `subagent_overrides_without_provider_errors`, `subagent_empty_section_without_provider_is_ignored`; [Ch 21](./21_chapter_config.md) §3.

---

---


## 1. 2026-09-13 — The bar's `out` budget no longer changes on the first prompt of a session

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/agent_tui_kit/src/render/bar.rs` (`format_max_out_tokens`); `crates/tact/src/agent/mod.rs` (`emit_model_status`, in-turn `ModelInfo`); [Ch 23](./23_chapter_tui.md) §6.6; `docs/token_usage_schema.md` |

**Symptom / motivation:** The bottom bar's `out` segment showed one value at startup and a larger one after the first prompt was sent, on an unchanged configuration. `out` is the *effective* text-output budget: for effort-semantic models (openai / deepseek / kimi k3) reasoning shares the `max_tokens` envelope, so the reasoning share is subtracted (`max_tokens × 100/(100+pct)`), while a budget-semantic model (Anthropic-style `thinking_budget`) keeps a separate envelope and shows the full value. The discriminator was `thinking_budget.is_some()`, but the two producers encode "thinking off" differently: the `/model` path emits `None` (`(budget > 0).then_some(..)`) while the in-turn request path emits `Some(0)` — it maps the always-present `Thinking` struct from `with_thinking`. So the first prompt of a session flipped the renderer from "shared envelope" to "separate envelope" and `out` went from the subtracted value to the full `max_tokens`, e.g. `36.6K` → `64K` on a 64000 envelope at `high`.

**Decision:** The discriminator becomes a **non-zero** budget — `thinking_budget.is_some_and(|b| b > 0)`. `Some(0)` and `None` both mean "thinking off", i.e. shared-envelope semantics, so both subtract and render identically. Fixing it in the renderer (rather than aligning the in-turn emitter with `emit_model_status`) also covers the compaction-summary emitter, which sends `thinking_budget: None` with a small `max_tokens` and would otherwise still be able to flip the segment mid-session. The `think` segment already filtered on `> 0`, which is why only `out` moved.

**Behavior after:** `out` stays put across a session boundary for a given config: on a 64000 envelope at `high` effort it reads `36.6K` before and after the first prompt, instead of `36.6K` → `64K`. A genuinely budget-semantic model (`thinking_budget > 0`) still shows the full `max_tokens`.

**Pointers:** `format_max_out_tokens` in `crates/agent_tui_kit/src/render/bar.rs`; test `format_max_out_tokens_zero_budget_subtracts_effort_share`; [Ch 23](./23_chapter_tui.md) §6.6; `docs/token_usage_schema.md`.

---

---


## 1. 2026-09-13 — `[agent].max_tokens` becomes a real level in the output-budget chain

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/types.rs` (`AgentTomlConfig::max_tokens`); `crates/tact/src/config/resolve.rs` (`resolve_config`); `config.example.toml`; [Ch 21](./21_chapter_config.md) §3 |

**Symptom / motivation:** `[agent] max_tokens = 64000` did nothing. `AgentTomlConfig` had no such field, and the struct is `#[serde(default)]` **without** `deny_unknown_fields`, so serde discarded the key silently — no error, no warning. The chain was `--max-tokens` > `[llm.providers.<active>].max_tokens` > `[llm].max_tokens` > default 8000, so a config that only set `[agent] max_tokens` ran at 8000 while *looking* configured. Observed live: a config carrying `[agent] max_tokens = 64000` (file mtime 14:32) whose requests at 14:34 went out with `"max_tokens": 8000`, and no `token_usages.request_body` row in the project DB had ever held 64000. The same file's `[agent.subagent] max_tokens = 64000` was inert for a second reason — `resolve_subagent` returns `Ok(None)` when the section has no `provider`.

**Decision:** `[agent].max_tokens` becomes a supported key and takes the slot **between the provider entry and the `[llm]` global**: `--max-tokens` > `[llm.providers.<active>].max_tokens` > `[agent].max_tokens` > `[llm].max_tokens` > default (8000; 32000 for Kimi K2.x). It sits *above* the `[llm]` global so it stays live for users who also set the global — the reverse order would have reproduced the exact "configured but ignored" trap this fix removes. The `[llm]` global is retained so existing configs keep resolving. The `model_context_window` validator no longer names `llm.max_tokens` in its message, because the value can now originate from three places.

**Behavior after:** A config that only sets `[agent] max_tokens = 64000` runs at 64000 on every provider whose entry omits `max_tokens`; a provider entry still wins over it, and `--max-tokens` still wins over all. A subagent without `[agent.subagent].max_tokens` inherits the resolved main value, `[agent]` level included. The window validator now reports `invalid token limits: max_tokens (N) must be less than agent.model_context_window (M)`.

**Pointers:** `resolve_config` in `crates/tact/src/config/resolve.rs`; tests `agent_max_tokens_overrides_global`, `per_provider_max_tokens_overrides_agent`, `cli_max_tokens_overrides_agent`, `subagent_inherits_agent_max_tokens`; [Ch 21](./21_chapter_config.md) §3 precedence table + §4 schema.

---

---


## 1. 2026-09-13 — Compaction summarizer uses an effort bucket + staged ladder instead of a fixed reserve

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`, `compact_effort_reserve_tokens`, `compact_summary_server_default_effort`, `compact_summary_effort`, `next_compaction_reserve`); `crates/tact/src/recovery.rs` (`MAX_COMPACT_SUMMARY_ATTEMPTS`, `MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS`) |

**Symptom / motivation:** The summarizer's reasoning reserve was a percentage of the summary **text** budget (capped at 2,000 tokens), so `high` effort reserved only 1,500 tokens even though an effort tier names an absolute thinking allowance. The truncation recovery was also a fixed 3 continuations that could not converge on reasoning-heavy providers: DeepSeek does not replay historical `reasoning_content` and every call regenerates thinking from scratch, so each continuation repeated the same overrun until the loop accepted a partial — or bailed on empty text. Codex's own summarizer is single-shot, runs at the sampling effort, and tolerates a truncated summary, which shaped the fix below.

**Decision:** (1) The initial reserve is the absolute token **bucket** of the effective effort — `none` 0 / `minimal|low` 2,000 / `medium` 4,000 / `high` 8,000 / `xhigh|max` 16,000 — not a percentage of the text budget; with no effort configured, DeepSeek / Kimi K3 (which reason at effort high by server default) take the `high` bucket and every other provider takes 0. (2) The single continuation loop becomes a **staged ladder**: stage 0 inherits the session effort, stage 1 minimizes it (`low` for DeepSeek / Kimi K3, `none` for OpenAI reasoning models, omitted elsewhere), and stage 2+ size the reserve from the previous attempt's `usage.reasoning_tokens` via `clamp(observed × 1.25, floor, cap)` with `floor = max(previous reserve, effort bucket, text/4)` and `cap = 2 × floor`, each capped so the request still fits the window. (3) `MAX_COMPACT_SUMMARY_ATTEMPTS` (new, 5) bounds the ladder independently of the main loop's `MAX_CONTINUATION_ATTEMPTS` (3), and the transport-retry budget `MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS` went 3 → 5. Empty summary text still fails.

**Behavior after:** `max_tokens` = 2,000 text + the effort bucket — 10,000 at `high`, and the same for DeepSeek / Kimi K3 with no explicit effort (server-default high). A truncated summary emits `[compact continue n/5]` with the escalating budget, and `[compact fallback]` once the ladder is exhausted, accepting the partial summary instead of failing; compaction no longer bails just because a reasoning model consumed the previous envelope.

**Design notes (vs Codex):** The summarizer *synthesizes* one tool-less `create_message` — instructions + optional focus + recent-file list + the recent message slice serialized as JSON text — instead of replaying the real history. Codex replays: it appends `SUMMARIZATION_PROMPT` to the native items and trims the oldest item on `ContextWindowExceeded`. Synthesis buys a request that is guaranteed to fit the input budget and is always structurally valid (no orphan `tool_use`/`tool_result`, no tools on the wire), and it is where `focus`, recent files, and oversized-media downgrades are injected; the cost is that tool-call nuance survives only as JSON. On the rebuild side both keep recent real user messages plus one summary cell under a 20k estimated-token cap, but Tact strips `ToolResult` blocks before retaining a user message (its tool results live inside user messages, unlike Codex's separate `FunctionCallOutput` items) and runs an outer fit-loop that shrinks the retained budget until `system prompt + tool specs + rebuilt + max_tokens + headroom` fits the window, while Codex keeps the flat 20k and leaves that to the caller. Tact also fails on an empty summary; Codex accepts `SUMMARY_PREFIX` alone. The prompt itself was tightened in the same change: it states that the conversation is appended as a JSON message array, grounds the summary in that content, and requires a compact, structured handoff that never paraphrases identifiers.

**Pointers:** `compact_history_local_with_mode` + helpers in `crates/tact/src/agent/mod.rs`; constants in `crates/tact/src/recovery.rs`; tests `compact_effort_reserve_bucket_tiers`, `compact_summary_server_default_effort_tiers`, `compact_summary_effort_ladder_per_provider`, `next_compaction_reserve_*`, `local_compact_inherits_session_effort`, `local_compact_keeps_server_default_reasoning_reserve`, `local_compact_accepts_partial_summary_when_continuations_exhausted`; [Ch 5](./05_chapter_compact.md) §5 step 3.

---

## 1. 2026-09-12 — One `/model` flow instead of two, and the TUI stops forking git on the UI thread

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tui/src/widgets/state/mod.rs` (`ModelTarget`); `crates/tui/src/handlers/select.rs`; `crates/tui/src/widgets/state/app/background.rs` (new) |

**Symptom / motivation:** Two independent problems.

(1) `SelectKind` carried the `/model` flow twice: five `Subagent*` variants mirroring the main-agent ones (`SubagentModelProfileEffortPick` vs `ModelProfileEffortPick`, and so on), re-matched by three large parallel blocks in `handlers/select.rs`. One of those blocks ended in a 12-variant OR-pattern placed inside a match that also had a wildcard arm — so forgetting a variant when adding a new one would have silently fallen through rather than failing to compile.

(2) Two blocking operations ran on the event loop: `App::maybe_refresh_git_branch` forked `git branch --show-current` (throttled to 5 s, but still a process spawn on the UI thread), and `refresh_skills` took a `std::sync::Mutex` and walked the filesystem while holding it. Neither task was tracked, so shutdown could not stop either one.

**Decision:** (1) `ModelTarget { Main, Subagent }` parameterizes the model/effort/budget flow, and the five duplicate variants collapse into the shared ones via a `target` field — `SelectKind` goes 13 → 8 variants, the five duplicate `*_subagent_*` helpers are gone, and the three parallel matches become one dispatch. The remaining OR-pattern now covers every variant of a match with no wildcard arm, so a new variant is a compile error instead of a silent fallthrough.

(2) Both operations moved to `tokio::task::spawn_blocking` in a new `app/background.rs`, each storing a `JoinHandle` plus a `oneshot::Receiver` in `App` (`git_branch_task` / `skills_task`). `poll_background_tasks` applies results each loop iteration and `abort_background_tasks` runs on shutdown. The git refresh keeps its 5 s throttle and is in-flight-gated (never a new task per frame). `reload_skills` is synchronous and lock-scoped, so no lock is held across an `.await`. When no tokio runtime is present both fall back to running inline, which keeps the existing tests working.

**Behavior after:** `/model` and `/model-subagent` behave exactly as before — same popups, order, labels and resulting state; two tests pin the target-specific behavior the duplicate families used to encode (the subagent budget flow uses subagent persist templates, and the subagent effort pick writes `agent.subagent.reasoning_effort`). The status bar's git branch and the skill list now refresh off the UI thread. Known asymmetry, preserved and now commented: the subagent **effort** flow shares the main-agent persist/session-only templates, unlike the budget flow, which has dedicated subagent strings.

**Pointers:** `crates/tui/src/widgets/state/mod.rs`; `crates/tui/src/handlers/select.rs`; `crates/tui/src/widgets/state/app/background.rs`.

---

## 1. 2026-09-12 — Responses stream events are classified by the SDK enum, not a hand-written list

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact_llm/src/openai/responses/mod.rs` (`sdk_event_types`, `parse_stream_event_with_raw`) |

**Symptom / motivation:** `parse_stream_event_with_raw` decided whether to consume an SSE event by string-matching its `type` against a hardcoded allowlist of 23 `"response.*"` literals, *before* deserializing. The vendored SDK's `ResponseStreamEvent` enum models 48 types, so the list was a hand-maintained subset that had to stay in sync with two other places by hand (`stream.rs`'s match arms, and `wire.rs`'s output-item table). An event the SDK learned about in a later version — or one that was simply forgotten when the list was written — was dropped before the state machine ever saw it, with no log and no error.

**Decision:** Ask serde. For an internally tagged enum, the unknown-variant error enumerates every valid tag, so `sdk_event_types()` derives the complete, authoritative set at runtime from the enum itself (`LazyLock`, one probe deserialization of a bogus tag). It follows SDK bumps automatically instead of needing a mirror. The allowlist is gone; the decision is now `sdk_knows_event(type)`.

The derivation is deliberately used only to separate "a type this build does not model" (drop it — forward compatibility with a newer server) from "a malformed payload for a type the SDK *does* model" (still a hard error). It does **not** decide which events Tact acts on: that remains `stream.rs`'s `ResponsesStreamState::apply`, the single source of truth for stream semantics.

Normalization (`normalize_stream_event_json`) still runs before deserialization, because it repairs wire shapes the typed parser would otherwise reject; it is now keyed on the two event categories that actually need repair (`output_item.{added,done}`, and the terminal `completed`/`incomplete`/`failed`).

**Behavior after:** Every event the SDK models reaches the state machine; `stream.rs` ignores the ones Tact does not use, exactly as before. An event type outside the SDK's enum is still dropped rather than failing the stream. A malformed known event still errors. Since the derived set is parsed out of an error message, `sdk_event_types_are_derived_from_the_enum` pins that the derivation still works — otherwise a serde wording change would silently empty the set and kill every stream.

**Pointers:** `crates/tact_llm/src/openai/responses/mod.rs`; `crates/tact_llm/src/openai/responses/stream.rs`; `crates/tact_llm/src/openai/responses/wire.rs`.

---

## 1. 2026-09-12 — One config orchestrator, and permission settings load in one place

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/resolve.rs` (`resolve_non_llm`, `NonLlmSettings`); `crates/tact/src/permission/settings.rs` (`PermissionSettings::load`) |

**Symptom / motivation:** Two duplications, one of them with a crash. (1) `resolve_non_llm_settings` and `resolve_config` each resolved the same ~45 lines of non-LLM settings (notifications, snapshots, micro-compaction, skill dirs, instruction sources, theme, vision, bash timeout/nice, RTK filter, permission mode) with their own copy of the precedence chain — so a change to precedence had to be made twice or the two paths silently disagreed. (2) `PermissionSettings::load` and `load_from` had byte-identical merge blocks.

Additionally the non-LLM path resolved `[agent].instruction_sources` with `.expect("invalid instruction_sources in config")`. That is library code reached from `main`, so a typo in a config key aborted the process with a panic instead of naming the offending key.

**Decision:** Extracted `NonLlmSettings` + `resolve_non_llm(args, toml_cfg) -> Result<NonLlmSettings>`, used by both paths, with the precedence order documented once at the resolution site. The `.expect` became a `Err`, and `resolve_non_llm_settings` now returns `Result`; its caller in `config/mod.rs` propagates with `?`. `PermissionSettings::load` became a one-line delegate to `load_from`.

One difference was deliberately **not** merged: a malformed `[voice]` is fatal on the full path (`resolve_voice(...)?`) but warns-and-degrades on the non-LLM path (which serves subcommands that never record audio). Collapsing it would have changed behavior, so `voice` stays resolved at each call site with a comment saying why.

**Behavior after:** Precedence is unchanged (`CLI flag > TOML > built-in default`; `--no-notifications` / `--no-micro-compact` are absolute and skip the TOML value). Both paths now share one implementation, so they cannot drift. A bad `[agent].instruction_sources` reports `invalid [agent].instruction_sources: …` instead of panicking. Permission rule merge semantics are unchanged and now pinned by `load_from_unions_global_then_project_deduplicating`: global rules come first, project rules are appended, duplicates dropped — there is no per-layer override, because precedence is decided at match time (`deny > ask > allow`).

**Pointers:** `crates/tact/src/config/resolve.rs`; `crates/tact/src/permission/settings.rs`; `crates/tact/src/config/mod.rs`.

---

## 1. 2026-09-12 — MCP handshake and tool calls are bounded by timeouts

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/mod.rs`; `crates/tact/src/mcp/remote.rs` |

**Symptom / motivation:** The MCP loader awaits *every* server connection before returning, and neither the initialize handshake nor `tools/list` nor `tools/call` had a deadline. One hung third-party server therefore wedged `tact` startup forever, with no timeout, no error, and no way to tell which server was at fault — remote servers were worse, since only the OAuth legs (`OAUTH_CALLBACK_TIMEOUT`, `OAUTH_TOKEN_EXCHANGE_TIMEOUT`) were bounded.

**Decision:** Added three named deadlines and applied them at the blocking awaits: `MCP_INIT_TIMEOUT` (60 s) for the initialize handshake on both the stdio and remote transports, `MCP_LIST_TOOLS_TIMEOUT` (30 s) for `tools/list`, and `MCP_CALL_TOOL_TIMEOUT` (600 s) for `tools/call`. A tool call is bounded generously rather than tightly: a long-running server-side tool is legitimate work, whereas an unbounded wait is not.

**Behavior after:** A server that never completes its handshake is reported as a timeout failure and removed from the router instead of blocking startup; the same applies per call. Every timeout names its own duration in the error message.

**Pointers:** `crates/tact/src/mcp/mod.rs` (`MCP_INIT_TIMEOUT`, `MCP_LIST_TOOLS_TIMEOUT`, `MCP_CALL_TOOL_TIMEOUT`); `crates/tact/src/mcp/remote.rs` (`REMOTE_INIT_TIMEOUT`).

---

## 1. 2026-09-12 — A hook subprocess is killed when its timeout expires

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/plugin/hooks.rs` |

**Symptom / motivation:** Plugin command hooks spawn `sh -c` with a 60 s timeout. On expiry the `wait_with_output` future was dropped, which detaches the child rather than killing it: the hook was reported as timed out while its process kept running and kept holding the stdout/stderr pipes the parent had already moved on from. Leaked hook processes accumulated across a session, and a hook that outlived its timeout could still mutate the worktree after Tact had decided it had failed.

**Decision:** Set `.kill_on_drop(true)` on the spawned command so dropping the future terminates the child. `tool/bash.rs` and `tool/background.rs` already did this; the hook path was the outlier.

**Behavior after:** A hook that exceeds its timeout is terminated, not orphaned. No process outlives the tool result that reports it.

**Pointers:** `crates/tact/src/plugin/hooks.rs`.

---

## 1. 2026-09-12 — A poisoned lock no longer aborts the process

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/utils/lock.rs`; `crates/tact_llm/src/lock.rs` |

**Symptom / motivation:** 42 production sites took a lock with `.lock().expect("… lock poisoned")` or `.write().unwrap()`. Every one of those locks guards a cache, a counter, a registry or a config snapshot — none guards an invariant that a panic could leave torn. So an unrelated panic while a lock happened to be held escalated a recoverable state glitch into a process abort; in the TUI that tears down the entire session and loses the in-flight turn, rather than degrading the one subsystem that misbehaved.

**Decision:** Added `LockExt::lock_recover` and `RwLockExt::{read_recover, write_recover}` (`utils/lock.rs`; mirrored as `tact_llm::lock`, since the two crates cannot share a module), which use `PoisonError::into_inner` instead of panicking. Safety argument: Rust guarantees the guarded data is still memory-safe after a poisoning panic; at worst it is logically stale, and for a counter/cache/registry that is precisely the case the next read already tolerates. All 42 sites were migrated across `agent/mod.rs`, `agent/tool_dispatch.rs`, `config/mod.rs`, `ui_responder.rs`, `store/sqlite.rs`, `voice/recorder.rs`, `prompt/mod.rs`, `tact_llm/provider.rs`, `tact_llm/models.rs` and `tact-ui/driver.rs`.

**Behavior after:** A poisoned lock degrades to "the guarded value may be one update stale", never to an abort. The remaining intentional panics are uninitialized-global invariants (`LLM provider not initialized; call tact_llm::init_provider first`), which are not lock state and are left alone.

**Pointers:** `crates/tact/src/utils/lock.rs`; `crates/tact_llm/src/lock.rs`.

---

## 1. 2026-09-12 — A subagent inherits its provider's compaction routing

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/agent/mod.rs` (`Agent::provider_kind`, `Agent::new`); `crates/tact_llm/src/provider.rs` (`current_provider_kind`) |

**Symptom / motivation:** `Agent::provider_kind` was initialised to a hardcoded `ProviderKind::OpenAi` and only corrected when a caller chained `.with_provider_kind(…)`. `tact-ui` does that for the top-level agent; `tool/subagent.rs` does not. A subagent therefore claimed to be OpenAI regardless of the real provider, and since the local `compact` tool is stripped from Responses tool sets, a DeepSeek subagent on a Responses-protocol endpoint would route compaction to `POST /responses/compact` — an endpoint DeepSeek does not implement.

**Decision:** Made the field `Option<ProviderKind>`: `None` (the default) means "inherit from the live provider", and `Agent::provider_kind()` resolves it lazily, falling back to OpenAI only when no provider has been installed. Added `tact_llm::current_provider_kind()` for this — a non-panicking sibling of `read_provider`, which would have aborted on the pre-`init_provider` paths. It reports a generic OpenAI-compatible endpoint pointed at DeepSeek as `DeepSeek`, matching `is_deepseek`.

**Behavior after:** Routing follows the actually-configured provider for every agent, including subagents. An explicit `.with_provider_kind(…)` still wins. Nothing changes when no provider is installed (tests).

**Pointers:** `crates/tact/src/agent/mod.rs`; `crates/tact_llm/src/provider.rs`.

---

## 1. 2026-09-12 — Retries are decided by HTTP status, not by error prose

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/recovery.rs` (`FailureKind`, `classify_error`, `classify_llm_error`); `crates/tact/src/agent/mod.rs` (`stream_message`, `retry_compaction_call`) |

**Symptom / motivation:** Two compounding defects. (1) The retry decider matched on `error.to_string()` against English substrings (`timeout`, `rate limit`, …), while `LlmError::HttpError { status, .. }` — the only variant carrying a status code — was ignored. A 429 and a 400 were indistinguishable, so a permanently malformed request could be retried with back-off until the budget ran out. (2) `Agent::stream_message` wrapped the typed error with `anyhow::anyhow!("{e}")`, stringifying it *before* the recovery loop ever saw it, and the loop's exit path re-wrapped again with `anyhow::anyhow!(error)`. The type was unrecoverable by the time the decision was made, and the cause chain was flattened for every caller up the stack.

**Decision:** Added `is_transient_http_status` (408/429/5xx are retryable; other 4xx are not), `FailureKind { PromptTooLong, Transient, Permanent }`, and two classifiers: `classify_llm_error(&LlmError)` and `classify_error(&anyhow::Error)`, the latter downcasting to `LlmError` when the typed cause survived. `stream_message` now preserves the error via `anyhow::Error::from`, and the loop propagates it unchanged. A non-transient status is still checked for an over-long prompt, because that is normally reported as a 400 and *is* recoverable — by compacting, not by retrying verbatim. The duplicated retry/back-off/emit block in the two compaction paths became `Agent::retry_compaction_call`, which now classifies instead of substring-matching.

**Behavior after:** 429/408/5xx back off and retry; 400/401/403/404 fail fast without burning quota; a 400 reporting an over-long context triggers compaction. Errors reaching callers keep their type and chain.

**Pointers:** `crates/tact/src/recovery.rs`; `crates/tact/src/agent/mod.rs`.

---

## 1. 2026-09-12 — Worktree names are validated before becoming paths and refs

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/worktree/mod.rs` (`validate_worktree_name`) |

**Symptom / motivation:** A worktree name is used twice: joined onto `<repo>/.worktrees` and interpolated into the `wt/<name>` branch ref. Nothing validated it. The only thing preventing escape was that `git worktree add` happens to reject ref-invalid names — git's rule, not Tact's, and not applied to the path.

**Decision:** Added `validate_worktree_name`, called from `WorktreeManager::create` before the store is touched: ASCII alphanumerics plus `-`, `_`, `.` and `/`; ≤ 100 characters; must start with an alphanumeric; no `..`, no `//`, no `.lock` path component, no trailing `/` or `.`. The "starts with an alphanumeric" rule came from the test suite, not from theory: the first draft allowed `/abs`, and `Path::join` with an absolute path *replaces* the base, so the name would have escaped `.worktrees` entirely.

**Behavior after:** A hazardous name is rejected with a specific message naming the offending character or pattern, before any path or ref is constructed. `subagent-<id>` and `feat/thing`-style names are unaffected.

**Pointers:** `crates/tact/src/worktree/mod.rs`.

---

## 1. 2026-09-12 — SQLite runs in WAL, and a message insert is atomic

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/store/sqlite.rs` (`connect_with_pragmas`); `crates/tact/src/store/session_store/sqlite.rs` (`append_message`) |

**Symptom / motivation:** Every domain store (sessions, tasks, background, team, worktrees) shares one `<workdir>/.tact/tact.db`, opened with the default rollback journal. Readers and writers therefore blocked each other, which the TUI feels directly: it reads session history while the agent appends to it. Separately, `append_message` issued the `messages` insert and the `sessions.updated_at` bump as two independent statements, so a failure between them left a persisted message whose session looked stale.

**Decision:** Configure the pool through `SqliteConnectOptions` rather than a one-shot `PRAGMA`: `journal_mode = WAL`, `synchronous = NORMAL` (corruption-safe in WAL and avoids an fsync per commit; kept at `FULL` under the rollback journal), and `busy_timeout = 5 s`. Falling back rather than failing: switching a database *into* WAL needs an exclusive lock that `busy_timeout` cannot wait on, so a concurrent opener can legitimately fail the switch — that case logs a warning and retries with the default journal instead of refusing to start. `append_message` now runs both statements in one transaction.

**Behavior after:** Readers proceed while a writer holds the lock. WAL is persisted in the database file, so existing databases are converted on the next open. A failed message write leaves no partial state.

**Pointers:** `crates/tact/src/store/sqlite.rs`; `crates/tact/src/store/session_store/sqlite.rs`.

---

## 1. 2026-09-12 — Anthropic token counters saturate instead of truncating

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/anthropic/mod.rs` (`usage_from_json`, `clamp_token_count`) |

**Symptom / motivation:** Eight sites converted wire token counts with `as u32`, which wraps. A provider (or a proxy, or a corrupted body) reporting more than `u32::MAX` prompt tokens would silently become a small plausible number, under-reporting usage in the bottom bar and in the persisted accounting. `total: prompt + completion` could also overflow and panic in a debug build. The Chat Completions path already saturated and the Responses path already validated with `try_from`; Anthropic was the outlier.

**Decision:** Added `clamp_token_count` (saturating) plus `usage_field` / `usage_reasoning_tokens` / `usage_from_json` helpers, replacing all eight conversions and the duplicated inline extraction in both the streaming and non-streaming paths.

**Behavior after:** An out-of-range counter saturates at `u32::MAX`; `total` saturates rather than overflowing. The three adapters now agree on out-of-range behaviour.

**Pointers:** `crates/tact_llm/src/anthropic/mod.rs`.

---

## 1. 2026-09-12 — The ctx percentage leads the bottom bar; the gauge is gone

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/render/bar.rs`; `crates/tui/src/render/bar.rs`; `docs/token_usage_schema.md`; Ch 23 §6.6 |

**Symptom / motivation:** The same-day row-2 compaction (entry below) had removed the ctx meter's `pct%` and kept the `■`/`·` gauge, leaving `ctx [▍···] 45K/1M`. Answering the question the segment exists for — "how close am I to auto-compact?" — required doing the division mentally, and the gauge only restated that same percentage as glyphs.

**Decision:** Reverse the ctx part of that compaction, keeping the *one value, one rendering* rule but picking the other encoding:
1. **Dropped the gauge entirely** — `render_usage_bar`, `partial_block_char`, the `■`/`·`/partial-block glyph constants and `USAGE_BAR_WIDTH` were deleted. The top status bar's step progress (`render_progress_bar`, `█`/`░`) is a different widget and is untouched.
2. **Restored the percentage, first** — `format_context_meter` now renders `ctx 4% 45K/1M`. The absolute `used/window` stays, because a ratio cannot replace the two counts it came from; it is also what disambiguates small values (590/200K renders as `0%`).
3. **Moved the `▣` cache segment to sit directly after `ctx`**, before the turn counters — both are session-wide ratios, so they now read together. Push order became `model → out → think → ctx → cache → turns → timing`; `fit_row_spans` drops from the end, so survival became `ctx > cache > turns > timing` (cache and the turn counters swapped).

**Behavior after:** Row 2 renders `deepseek-v4  out 73.1K  think high  ctx 4% 45K/1M  ▣ 30%  ⟳ 12  ⇅ 3  ⏱ 02:05 avg 01:45` — **86 columns** (was 90). Order and the 100-column budget are pinned by `bottom_bar_orders_cache_before_turn_counters` and `bottom_bar_fits_every_segment_in_100_columns`; `format_context_meter_leads_with_the_percentage` asserts the gauge glyphs never come back.

**Pointers:** `crates/agent_tui_kit/src/render/bar.rs` (`format_context_meter`, `context_usage_pct`, row-2 `DropGroup` push order); `crates/tui/src/render/bar.rs`; `docs/token_usage_schema.md`; Ch 23 §6.6.

---

## 1. 2026-09-12 — A drained subagent result no longer spawns an empty wake-up turn

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-ui/src/driver.rs` (`spawn_wakeup_task`); `crates/tact/src/agent/mod.rs` (`Agent::has_pending_subagent_results`); Ch 12 |

**Symptom:** a background subagent's `SubagentFinishedNotification` whose summary had *already* been drained and injected by the in-flight turn still spawned a wake-up turn. The queue was empty, so nothing was injected, and the parent received only the bare prompt `A background subagent finished. Review its result below.` — with no result below it. The user saw an extra turn whose only content was a notification with no payload, and the model had to answer a prompt that promised content it never got.

**Decision:** gate the wake-up on the queue. `Agent::has_pending_subagent_results()` exposes whether `pending_subagent_results` is non-empty, and `spawn_wakeup_task` returns early when it is empty — a wake-up turn exists only to deliver queued results into the parent's context, so an empty queue means a previous turn already delivered the summary. The notification prompt also points at `check_subagent` as the fallback retrieval path instead of promising a result "below". The driver's retention logic is unchanged: a notification arriving mid-turn is still retained until that turn's `JoinHandle` completes, and a result enqueued after the turn's final drain still wakes the parent.

**Behavior after:** a completion whose summary is still queued wakes the parent and is delivered in that turn; a completion whose summary was already drained is a no-op (no extra turn, no wasted LLM request); a poisoned queue lock falls back to the previous always-wake behavior rather than swallowing a completion.

**Pointers:** `crates/tact-ui/src/driver.rs` (`spawn_wakeup_task`, `run_command_loop_with_account`); `crates/tact/src/agent/mod.rs` (`has_pending_subagent_results`, `agent_loop` drain); driver tests `subagent_finished_notification_is_not_lost_when_parent_finishes` and `subagent_notification_with_empty_queue_does_not_wake_parent`; Ch 12.

---

## 1. 2026-09-12 — Bottom-bar row 2 compacted to a 90-column budget

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/render/bar.rs`; `crates/agent_tui_kit/src/i18n.rs`; `crates/tui/src/render/bar.rs`; `docs/token_usage_schema.md`; Ch 23 §6.6 |

**Symptom / motivation:** Adding the turn counters and turn timing pushed row 2 to ~138 columns, so on ordinary terminals `fit_row_spans` began silently dropping segments — the row was "full". Auditing the content found the same ratio or value rendered two or three times over: `∑ₜₒₖ {total}` and the `ctx` meter's `used` both read `StatusBarState.token_total`, and the ctx meter separately encoded its ratio as a gauge, a `pct%`, *and* `used/window`.

**Decision:** Cut row 2 to **90 columns with no loss of distinct information**, by the rule *one value, one rendering*:
1. **Deleted the `∑ₜₒₖ {total}` segment** — redundant with the `ctx` meter's `used`. The exact integer is still available in the task-stats block after each turn and in `/stats`, so only its duplicate rendering is lost. `ICON_TOKENS` / `format_token_total` were removed with it.
2. **Deleted the ctx meter's `pct%`** (same rule) — it is a pure function of the `used/window` rendered immediately beside it — and **narrowed the gauge 10 → 6 cells**, since the gauge's exact value is also given twice over and it only needs to convey an at-a-glance sense. That segment went **24 → 17 columns (−29%)** while still distinguishing near-limit usage (`[■■■▍]` at 85% vs `[▍···]` at 4%). `format_context_meter` was left rendering `ctx [▍···] 45K/1M` — **superseded the same day** (see the newest entry): the gauge was dropped and the percentage restored, giving `ctx 4% 45K/1M`.
3. **`max_out_token` → `out`** in both languages (ZH `输出`), matching the shorthand level of the neighbouring `ctx` / `think` labels.
4. **`▣ cache% 30%` → `▣ 30%`** — the glyph plus `%` already identify the number.
5. **`⟳ 12 turns ⇅ 3 turns` → `⟳ 12 ⇅ 3`** — the word was dropped from both counters; the adjacent glyph pair reads as one "turns" figure.
6. **`⏱ 02:05 · avg 01:45` → `⏱ 02:05 avg 01:45`** — dropped the separator.

The now-unused i18n fields (`bottom_cache_pct`, `bottom_turns`, `bottom_llm_turns`) were removed rather than left stale. The `render_usage_bar` unit tests were rewritten to derive expected widths from `USAGE_BAR_WIDTH` instead of hard-coding 8 inner cells, so the next width change cannot break them.

**Behavior after:** With every segment populated the full row renders in 90 columns (`deepseek-v4  out 73.1K  think high  ctx [▍···] 45K/1M  ⟳ 12  ⇅ 3  ▣ 30%  ⏱ 02:05 avg 01:45`), so ordinary terminals no longer drop segments. The drop order is `ctx > turns > cache > timing`. A width-budget test (`bottom_bar_fits_every_segment_in_100_columns`) fails if a future segment pushes the row back over ~100 columns — the exact failure mode this change fixed. (Row width, drop order, and the ctx gauge helpers named below were all changed later the same day; see the newest entry.)

**Pointers:** `crates/agent_tui_kit/src/render/bar.rs` (`USAGE_BAR_WIDTH`, `format_context_meter`, `format_cache_pct`, `format_turn_user`, `format_turn_llm`, `format_turn_timing`, row-2 group push order; `ICON_TOKENS`/`format_token_total` removed); `crates/agent_tui_kit/src/i18n.rs` (`bottom_out`, `bottom_avg`; three fields removed); `crates/tui/src/render/bar.rs` (`bottom_bar_fits_every_segment_in_100_columns`); `docs/token_usage_schema.md`; Ch 23 §6.6.

---

## 1. 2026-09-12 — TUI bottom bar shows turn counts and turn timing

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/protocol/src/agent.rs` (`AgentUpdate::TurnStats`); `crates/tact/src/agent/mod.rs` (`agent_loop`); `crates/agent_tui_kit/src/state/status_bar_state.rs`; `crates/agent_tui_kit/src/components/status_bar.rs`; `crates/agent_tui_kit/src/render/bar.rs`; `crates/tui/src/handlers/skills.rs`; `crates/tui/src/widgets/state/app/popups.rs`; `docs/token_usage_schema.md`; Ch 23 §6.6 |

**Symptom / motivation:** The bottom bar reported tokens, cache rate and context usage but nothing about *turns*. The only timing surface was the frozen `⏱ mm:ss` on the task-end separator and the post-task stats block — historical log rows, never a live counter. There was no way to see how many turns a session had run, how many agent-loop iterations the current task was taking, or how long turns were taking on average.

**Decision:** Add three counters to bottom-bar row 2, all derived from data already in memory (no new persistence, no schema change):
1. `AgentUpdate::TurnStats { turns_taken, max_turns }` is emitted once per agent-loop iteration from `agent_loop`, right after `self.turns_taken += 1` — same cadence as `TokenUsage`, so no new event volume. The kit's `StatusBarComponent` claims it; the shell resets `turn_llm` at dispatch.
2. Session user turns (`⟳`) are counted at the single dispatch choke point, `handlers/skills.rs::dispatch_user_task` (which also serves queued flushes and skill dispatch), and seeded on resume by counting persisted user messages in `load_history`.
3. Turn timing accumulates in `add_task_end_separator`, the **only** place that actually freezes `task_start_time` (`freeze_last_prompt_cost` runs after it and always sees `None`), so a turn cannot be counted twice. Cancelled turns count — their wall time is real; synthetic separators (no start time) do not.
4. `TurnStats` is registered as **per-call metadata** in `coordinator_prepass`, alongside `TokenUsage`/`ModelInfo`. It fires between turns while the loading spinner is up, so without this it would run the content gates and make the spinner vanish the moment the loop starts (regression test: `turn_stats_is_metadata_and_keeps_the_loading_placeholder`).
5. `max_turns` is plumbed into `StatusBarState.turn_llm_cap` but deliberately **not rendered**: only `spawn_subagent` ever sets a cap and there is no CLI flag or TUI wiring for it, so a main-agent bar could never show `/cap`. Bare `⇅ {n}` renders instead; the field is kept so the segment is ready if a main-agent cap is ever added.

**Behavior after:** Row 2 shows `⟳ 12 turns ⇅ 3 turns  ∑ₜₒₖ …  ▣ cache% 5%  ⏱ 02:05 · avg 01:45` (`⟳ 12 轮 ⇅ 3 轮次 … ⏱ 02:05 · 均 01:45`). The `⇅` segment is hidden until the task's first LLM call; `avg` is hidden until a turn completes; the whole timing group is hidden while no turn has finished. Live in-flight elapsed stays on the top status bar — the bottom bar shows only frozen values. On narrow terminals the new segments are droppable with survival order `ctx > turns > ∑ₜₒₖ > cache > timing`, preserving the pre-existing `ctx > ∑ > cache` priority. *(Superseded later the same day — this entry records the state when turn stats first shipped; see the row-2 compaction entry above for the current 90-column row.)*

**Pointers:** `crates/protocol/src/agent.rs` (`AgentUpdate::TurnStats`); `crates/tact/src/agent/mod.rs` (`agent_loop` emit); `crates/agent_tui_kit/src/state/status_bar_state.rs` (`turn_user`, `turn_llm`, `turn_llm_cap`, `turn_last_secs`, `turn_done`, `turn_total_secs`); `crates/agent_tui_kit/src/render/bar.rs` (`ICON_TURNS`/`ICON_LLM_TURNS`/`ICON_ELAPSED`, `format_turn_user`, `format_turn_llm`, `format_turn_timing`); `crates/tui/src/widgets/state/app/popups.rs` (`add_task_end_separator`); `crates/tui/src/widgets/state/app/messages.rs` (`load_history` seeding); spec `docs/superpowers/specs/2026-09-12-turn-stats-bottom-bar-design.md`; Ch 23 §6.6; `docs/token_usage_schema.md`.

---

## 1. 2026-09-11 — `bash` accepts a per-call `timeout`

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/tool/bash.rs` (`BashInput::timeout`, `resolve_timeout_secs`); Ch 7 §8 |

**Symptom / motivation:** The bash wall-clock limit was config-only (`[tools].bash_timeout_secs`, default 1,800 s). One long build could not extend it, an agent that wanted a *tighter* bound had no way to ask, and because unknown JSON fields deserialize away, a `timeout` the model invented was silently ignored instead of applied.

**Decision:** Add an optional `timeout` field (seconds; serde alias `timeout_secs`) to `BashInput`, resolved by `resolve_timeout_secs(input_timeout, ctx.bash_timeout_secs)`. The per-call value wins: `Some(0)` disables the limit for that call, `None` inherits the configured value (including a configured `0` disable).

**Behavior after:** `{"command": "cargo build", "timeout": 600}` caps that invocation at 10 minutes regardless of config; `"timeout": 0` runs without a wall-clock limit (user cancellation still applies). The failure text reports the effective limit, `Timeout (<n>s)`.

**Pointers:** `crates/tact/src/tool/bash.rs` (`BashInput`, `resolve_timeout_secs`, `bash`); tests `tool::bash::tests::{resolve_timeout_prefers_input_then_config,bash_input_timeout_overrides_configured,bash_input_timeout_zero_disables_configured_timeout}`; Ch 7 §8.

---


## 1. 2026-09-11 — `/mcp list` gives the TUI a live MCP server view without reconnecting

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/protocol/src/agent.rs` (`UserCommand::McpList`); `crates/tact/src/mcp/mod.rs` (`McpLiveStatus`, `McpServerView`, `describe_servers`); `crates/tact-ui/src/mcp_cli.rs` (`render_live_listing`); `crates/tact-ui/src/driver.rs`; `crates/tui/src/handlers/mcp.rs`; Ch 8 §Step 1c |

**Symptom / motivation:** MCP servers could only be listed from the shell — `tact-ui mcp list` — and that command dials every configured server. Inside a running TUI there was no way to see which servers the agent actually had, so a server that failed at startup, or a remote one waiting on OAuth, had no on-demand answer short of restarting.

**Decision:** Add `/mcp list` as a second subcommand of the existing `/mcp` slash entry, answered by the driver.
1. `UserCommand::McpList` carries the request. `tui::handlers::mcp` sends it **only when idle**; while `Planning`/`Executing` it flashes the busy hint, because the driver serializes ordinary commands behind an in-flight turn — queueing would show the table only after the turn ended.
2. `tact::mcp::describe_servers(connected)` classifies every configured server against the **live** connection set without dialling: `Connected { tools }` when the router holds it, `NeedsAuthorization` for a remote OAuth server with no usable credential, otherwise `NotConnected`. The connected check runs first, so a working server is never mislabelled by the credential heuristic.
3. The driver renders the views via `render_live_listing` into `AgentUpdate::MdInfo`, so the log shows a Markdown table (server / transport / source / status) through the same `MarkdownCell` as `/skills`. Cell values escape `|` and newlines, since a source path is user-controlled.

**Behavior after:** `/mcp list` prints a table of every configured server with its live status and tool count. It **never** opens a connection, so it cannot duplicate a remote dial or contend with a live stdio child — unlike `tact-ui mcp list`, which still connects and reports fresh status. An empty configuration explains where to declare servers.

**Pointers:** `crates/protocol/src/agent.rs` (`UserCommand::McpList`); `crates/tact/src/mcp/mod.rs` (`transport_kind`, `McpLiveStatus`, `McpServerView`, `describe_servers`, `describe_resolved`); `crates/tact-ui/src/mcp_cli.rs` (`render_live_listing`); `crates/tact-ui/src/driver.rs` (`UserCommand::McpList` arm); `crates/tui/src/handlers/mcp.rs`; `crates/agent_tui_kit/src/bridge.rs` (`TryFrom<UserCommand>`). Tests: `mcp::tests::{describe_resolved_classifies_against_the_live_connection_set,describe_resolved_lists_a_connected_oauth_server_as_connected}`; `mcp_cli::tests::{live_listing_has_a_row_per_server_with_its_status,live_listing_explains_how_to_configure_when_empty,live_listing_escapes_pipes_so_a_source_path_cannot_break_the_table}`; `driver::tests::mcp_list_emits_the_live_listing_without_reconnecting`; `handlers::mcp::tests::{mcp_list_queues_a_listing_request_when_idle,mcp_list_flashes_busy_instead_of_queueing_while_a_task_runs}`.

---


## 1. 2026-09-11 — The Mermaid popup shows the rendered diagram, and says when it cannot render

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/agent_tui_kit/src/render/popups/mermaid_popup.rs`; `crates/agent_tui_kit/src/state/ui_types.rs` (`MermaidPopupView`, `MermaidPopup::new`); `crates/tui/src/handlers/overlay.rs`; Ch 23 §6.7 |

**Symptom / motivation:** The log panel renders Mermaid diagrams, but only at the log column's width, and the double-click popup existed solely to **copy the source** — it never showed the diagram. Dense flowcharts (the common case for agent-authored diagrams) were cramped and hard to read in the main area, and the popup offered no better view. Worse, a fence using Mermaid `style` / `classDef` / `linkStyle` silently failed: the upstream `ratatui-markdown` grammar only accepts `chain` / `nodedef` / `comment` statements, so the whole block fell back to raw code with no indication of why.

**Decision:** Make the popup the wide view of the diagram rather than a source viewer.
1. `MermaidPopupView { Diagram, Source }` is added to the popup state, defaulting to `Diagram`; the existing `scroll` field is reused.
2. The popup re-renders the fence body through `render_mermaid_block` at the popup's own width (`centered_popup_area`, ~80% of the frame) instead of showing raw lines — that width, not the log panel's, is the point of the popup.
3. `Tab` toggles between the two views (theme-agnostic, not currently bound inside overlay popups) and re-anchors `scroll` to 0, since the views have different heights. `y` still copies the source in both views.
4. When the diagram view cannot render, the popup downgrades to the source view **and** prints an explicit header note, so an unsupported-syntax fence is diagnosable instead of looking like an empty diagram.

**Behavior after:** Double-clicking a diagram opens it rendered at ~80% of the frame width; `Tab` shows the source; `y` copies it; `Esc` closes. Unrenderable Mermaid shows its source plus `⚠ this diagram does not render (unsupported syntax) — showing source`. Main-area rendering is unchanged.

**Pointers:** `crates/agent_tui_kit/src/render/popups/mermaid_popup.rs`; `crates/agent_tui_kit/src/state/ui_types.rs` (`MermaidPopupView`, `MermaidPopup::new`); `crates/tui/src/widgets/state/app/popups.rs` (`open_mermaid_popup`, `toggle_mermaid_popup_view`); `crates/tui/src/handlers/overlay.rs` (`Tab`). Tests: `render_gap_tests::mermaid_popup_opens_on_rendered_diagram_not_source`, `mermaid_popup_tab_switches_to_source_and_back`, `mermaid_popup_falls_back_to_source_and_labels_unsupported_syntax`, `mermaid_popup_renders_diagram_at_wider_width_than_log`, `mermaid_popup_paints_theme_bg_across_its_area`; `handlers::overlay::mermaid_view_tests::*`.

---


## 1. 2026-09-11 — Compaction no longer emits orphaned `role: tool` messages

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/compact/mod.rs` (`build_compacted_history`, `without_tool_results`); `crates/tact_llm/src/convert.rs` (`drop_orphaned_tool_messages`); Ch 5 |

**Symptom / motivation:** After an auto-compact, the next OpenAI-compatible (chat-completions) request failed with a provider 400: `Messages with role 'tool' must be a response to a preceding message with 'tool_calls'`. Harness-style user turns mix a tool result with an image (`[ToolResult, Image]` — e.g. a screenshot tool that reads a PNG). `is_real_user_message` classifies those as real user turns because of the image, so the Codex-style rebuild retained them **verbatim** while dropping the assistant `tool_use` turn that produced the result. The retained block then converted to a wire `role: tool` message with no parent `tool_calls` list, and OpenAI-compatible providers reject the whole request. Replaying the triggering transcript through the old rebuild produced exactly 6 such orphans; the request failed until the session was restarted.

**Decision:** Two layers, mirroring the existing forward-direction guard (`sanitize_assistant_messages`, which strips tool calls whose results are missing):
1. **Rebuild strips tool results** — `build_compacted_history` runs each retained user message through `without_tool_results`, which removes `ToolResult` blocks and keeps the rest (text/image). A message that held *only* tool results is skipped entirely rather than becoming an empty turn.
2. **Wire conversion is the last line of defense** — `drop_orphaned_tool_messages` deletes any `role: tool` message whose parent assistant `tool_calls` is absent, walking back over a run of consecutive tool messages first so parallel tool calls (one assistant turn → N results) are not falsely dropped.

**Behavior after:** A compacted context never carries a tool result without its producing assistant turn. Retained harness turns keep their images and text; pure tool-result turns vanish with their parent. If some future path produces an orphan anyway, the request is still sent (with a `tracing::warn` naming the dropped `tool_call_id`) instead of failing with a 400.

**Pointers:** `crates/tact/src/compact/mod.rs` (`without_tool_results`, `build_compacted_history`); `crates/tact_llm/src/convert.rs` (`drop_orphaned_tool_messages`). Tests: `compact::tests::build_compacted_history_drops_tool_results_from_retained_harness_turns`, `compact::tests::build_compacted_history_skips_pure_tool_result_turns`, `convert::tests::orphan_tool_messages_without_preceding_tool_calls_are_dropped`, `convert::tests::tool_message_with_preceding_tool_calls_is_kept`.

---


## 1. 2026-09-11 — `deepseek-v4-*` experiment variants get the 1M window, not the 200K default

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/config/resolve.rs` (`model_context_window_for_model`); `config.example.toml`; Ch 21 §5, Ch 5 settings tables |

**Symptom / motivation:** The model→context-window mapping matched DeepSeek V4 by exact id (`deepseek-v4-pro`, `deepseek-v4-flash`, `deepseek-reasoner`). Any suffixed variant — `deepseek-v4-flash-version-exp`, `deepseek-v4-flash-vision-exp` — missed every arm and fell through to the `200_000` default. The bottom-bar `ctx` meter then showed `…/200K` for a real 1M model, and the derived auto-compaction threshold (80% of the window) fired around ~160k instead of ~800k, compacting sessions that had plenty of room left.

**Decision:** Match the DeepSeek V4 family by prefix (`model.starts_with("deepseek-v4-")`) instead of a fixed id list, so experiment/vision/respin suffixes inherit the family window. The unversioned gateway alias `deepseek-flash` and `deepseek-reasoner` keep explicit 1M arms. The mapping keeps its highest-priority position over CLI/TOML.

**Behavior after:** Every `deepseek-v4-*` id resolves to a `1_000_000`-token window, including `deepseek-v4-flash-version-exp` and `deepseek-v4-flash-vision-exp`; `deepseek-flash` (OpenAI-compatible gateway alias) and `deepseek-reasoner` also resolve to 1M. The `ctx` meter and 80% auto-compact threshold follow. Manual `model_context_window` still only applies to models without a built-in mapping.

**Pointers:** `crates/tact/src/config/resolve.rs` (`model_context_window_for_model`). Test: `config::resolve::tests::resolve_model_context_window_maps_deepseek_v4_variants`.

---


## 1. 2026-09-11 — `/mcp auth` survives stray loopback traffic, and the token exchange is bounded

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/remote.rs` (`await_oauth_callback`, `handle_callback_request`, `read_request_head`, `percent_decode`, `hex_nibble`, `redact_query_value`, `OAUTH_TOKEN_EXCHANGE_TIMEOUT`, `OAUTH_CALLBACK_PATH`, `MAX_REQUEST_BYTES`); Ch 8 §Step 1c |

**Symptom / motivation:** The loopback callback listener accepted exactly one connection and treated any request that did not carry `code` + `state` as fatal. A browser prefetch, a `favicon.ico` probe, a manual visit to `http://127.0.0.1:<port>/`, or a local port scanner was therefore enough to win the single `accept()`; the flow bailed with "authorization callback did not carry a code and state", and the real redirect — arriving moments later — was never read, leaving the user to blame the provider for a flow that was actually broken locally. Three further defects sat on the same path: the request head was read with a single `read()`, so a head split across TCP segments produced a body-less parse and the same bogus error; `percent_decode` sliced the raw `&str` at byte offsets, so a malformed escape followed by a multi-byte UTF-8 character (`?code=%aé`) panicked with "byte index N is not a char boundary" inside the future the TUI polls, aborting the driver task; and `session.handle_callback` — the token POST — was awaited with no timeout at all, so a provider that accepted the connection and never answered hung `/mcp auth` forever. Separately, the authorization URL was logged at `info` with its query intact, durably recording the one-time CSRF `state`.

**Decision:** Make the callback listener tolerant and bounded, and stop logging the secret half of the URL. `await_oauth_callback` now loops over `accept()`: only a request on `OAUTH_CALLBACK_PATH` carrying `code` + `state` completes the flow, a request carrying `error` fails it (including `error_description`), and anything else gets a short response and is ignored while the overall 300 s `OAUTH_CALLBACK_TIMEOUT` keeps running. The head is reassembled with `read_request_head` until the terminating blank line, EOF, or the 8 KiB `MAX_REQUEST_BYTES` cap. `percent_decode` works on bytes via `hex_nibble`, so a malformed escape degrades to the literal character instead of panicking. The exchange is wrapped in `tokio::time::timeout(OAUTH_TOKEN_EXCHANGE_TIMEOUT, …)` — a new, local 60 s bound — so a stalled token endpoint surfaces as an error the user can act on. `redact_query_value(&url, "state")` replaces only the `state` value with `[redacted]` and leaves the rest of the URL (endpoint, `client_id`) readable for troubleshooting.

**Behavior after:** A stray or malformed localhost request no longer aborts `/mcp auth`; the listener keeps waiting for the real redirect inside the same 300 s window. A denial reports the provider's `error_description`, not just `error=access_denied`. A malformed percent escape cannot panic the driver. A hung token exchange fails after 60 s instead of hanging. `RUST_LOG=tact=info` shows the authorization URL with `state=[redacted]`.

**Pointers:** `crates/tact/src/mcp/remote.rs`. Tests: `mcp::remote::tests::{a_stray_connection_before_the_callback_is_ignored,a_fragmented_request_head_is_reassembled,callback_listener_surfaces_denied_authorization,a_denial_reports_the_error_description,a_malformed_escape_on_the_callback_path_is_not_fatal,malformed_percent_escapes_degrade_instead_of_panicking,the_csrf_state_is_redacted_before_logging,callback_listener_times_out_without_a_request}`.

---

## 1. 2026-09-11 — Editing `mcp.json` keeps the file's permissions, uses a unique temp file, and tolerates a BOM

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/edit.rs` (`read_document`, `write_document`); `crates/tact/src/mcp/remote.rs` (`write_private`, `create_dir_private`); `crates/tact-ui/src/mcp_cli.rs` (`add`, `remove`) |

**Symptom / motivation:** `mcp.json` legitimately stores secrets (`headers` — commonly `Authorization: Bearer …` — and `env`), so a user may harden it with `chmod 600`. The rewrite wrote a temporary sibling with `fs::write`, which creates it at the umask default (usually 0644), and `rename`d that inode over the original — silently widening a 0600 file to 0644 and exposing those header/env values to other local users. The temp name was a fixed `mcp.json.tmp`, so two concurrent `mcp add` invocations in the same scope interleaved and truncated each other's bytes and one addition was lost. A `mcp.json` saved with a UTF-8 BOM (common on Windows) failed to parse, blocking every edit with "fix or remove it". The OAuth credential store had the same class of issue: the token file was written first and `chmod 0600`'d afterwards, leaving a window at 0644, and its parent directory was created at the umask default, exposing the set of authorized server names.

**Decision:** Write through a private handle or restore the mode before the file becomes visible, and make the temp name collision-proof. `write_document` creates a unique sibling (`mcp.json.<pid>.<nanos>.tmp`), copies the original file's `Permissions` onto it before `rename` via `set_permissions`, and removes the temp when the rename fails. `read_document` strips a leading `\u{feff}` before parsing. In `remote.rs`, `create_dir_private` creates the credential directory with `DirBuilder::mode(0o700)` and `write_private` opens the token with `OpenOptions::mode(0o600)`, so it is never reachable at a looser mode.

**Behavior after:** A `chmod 600 ~/.tact/mcp.json` stays 0600 across `tact-ui mcp add …`/`remove`. Concurrent edits in one scope no longer clobber each other's temp file, and no `.tmp` sibling is left behind on failure. A BOM'd config is editable instead of being reported unparseable. `~/.tact/mcp/oauth` is 0700 and its token files are created 0600.

**Pointers:** `crates/tact/src/mcp/edit.rs` (`read_document`, `write_document`), `crates/tact/src/mcp/remote.rs` (`create_dir_private`, `write_private`). Tests: `mcp::edit::tests::{rewriting_preserves_the_file_mode,the_written_file_has_no_leftover_temp_sibling,a_leading_bom_does_not_block_editing}`; `mcp::remote::tests::file_credential_store_round_trips_and_clears`.

---

## 1. 2026-09-11 — `mcp add`/`mcp list` respect scope precedence, and repeated credential flags are rejected

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/mod.rs` (`resolve_servers`); `crates/tact-ui/src/mcp_cli.rs` (`add`, `parse_pairs`, `scope_hint`); Ch 8 §`tact-ui mcp` |

**Symptom / motivation:** Three scope-and-input bugs. (a) `tact-ui mcp add foo --user` when the project file already declared `foo` printed "Added MCP server 'foo' in ~/.tact/mcp.json" — but the loader resolves project over user, so the effective server was unchanged and the user got a success message for a no-op. (b) A server name with no usable `command`/`url` in one scope and a valid declaration in another was both pushed to `skipped_remote` and resolved into `order`, so `mcp list` listed it twice and counted it twice ("2 MCP server(s) configured" for one server). (c) `--header A:1 --header A:2` (and likewise `--env`) silently kept only the last value — exactly the quiet surprise a credential flag should not have.

**Decision:** Report the shadowing instead of pretending success, de-duplicate the skip list against the servers that actually resolved, and reject a repeated flag name. `add` compares the written path against `mcp::resolved_server_for(name)` and, when another source wins, prints a warning naming that source and stating the change will not take effect until it is removed. `resolve_servers` calls `skipped_remote.retain(|name| !index_of.contains_key(name))` before sorting/deduplicating, so a name is either skipped or configured, never both. `parse_pairs` errors when a name is inserted twice, without echoing either value.

**Behavior after:** `mcp add` never claims success for a declaration the loader will not reach; it names the winning file. `mcp list` lists and counts each server once. Duplicate `--header`/`--env` names fail with `--header NAME was given more than once` rather than dropping a value.

**Pointers:** `crates/tact/src/mcp/mod.rs` (`resolve_servers`), `crates/tact-ui/src/mcp_cli.rs` (`add`, `parse_pairs`). Tests: `mcp::tests::a_name_configured_in_another_scope_is_not_also_reported_as_skipped`; `mcp_cli::tests::{a_repeated_pair_name_is_rejected,malformed_pairs_fail_without_echoing_the_value,pair_parsing_trims_the_name_and_value}`.

---

## 1. 2026-09-11 — `/mcp auth` now shows the OAuth URL while it waits for the browser

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-ui/src/driver.rs` (`stream_auth_progress`, the `UserCommand::McpAuth` arm); `crates/tact/src/mcp/remote.rs` (`authorize_remote_server`, `OAUTH_CALLBACK_TIMEOUT`); Ch 8 §Step 1c |

**Symptom / motivation:** `/mcp auth figma` printed only `Starting MCP authorization for figma (watch for the URL below)...` and then nothing — the URL the message promises never appeared, so there was nothing to click and the command looked hung. The flow was not slow: `authorize_remote_server` reports the URL through `notify` *before* it blocks on the loopback callback, but the driver collected those lines in a `Vec` and flushed them only after the future resolved. That resolution happens at best after the user has already authorized (too late to be useful) and at worst after the 300 s `OAUTH_CALLBACK_TIMEOUT`, so the one line that matters was structurally unreachable — the CLI path (`tact-ui mcp login`) was unaffected only because it prints inside its own `notify`.

**Decision:** Stream progress lines to the UI as they arrive instead of buffering them. `authorize_server` requires a `Send` notify closure, which rules out capturing `&Agent` inside it, so an unbounded channel decouples the two: the closure sends each line, and `stream_auth_progress` selects between the authorization future and the line receiver, emitting each line into `AgentUpdate::Info` the moment it arrives. Lines still queued when the flow resolves are drained after the `select!`, so a URL produced in the same poll that completes the flow cannot be lost to the race.

**Behavior after:** The authorization URL is emitted as an `Info` update as soon as `authorize_server` produces it — while the flow is still blocked on the redirect — so the user has something to click without waiting for the command to finish. Authorization semantics are unchanged, including the 300 s window during which the callback listener stays open and the command keeps the driver's user-command loop busy. Because the URL travels as a normal `Info` update, it appears in the transcript rather than as a one-off overlay.

**Pointers:** `crates/tact-ui/src/driver.rs` (`stream_auth_progress`, `UserCommand::McpAuth`). Tests: `crates/tact-ui/src/driver.rs` unit tests `driver::tests::{auth_progress_reaches_the_user_before_the_flow_finishes,auth_progress_drains_lines_sent_at_completion}`; end-to-end regression `crates/tact-ui/tests/mcp_auth_url_progress.rs`, which drives `handle_user_command` against a `wiremock` OAuth provider and fails against the buffering implementation. Related: Ch 8 §Step 1c (the documented `/mcp auth` behavior this change restores).

---


## 1. 2026-09-11 — `mcp.oauth_client_name`: OAuth registration identity, default `Codex`

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/config/{types,resolve}.rs` (`McpTomlConfig`, `McpSettings`, `resolve_mcp`), `crates/tact/src/mcp/remote.rs` (`OauthRequestParameters`, `oauth_parameters`, `registration_message`, `authorize_remote_server`), `config.example.toml`; Ch 8 §Step 1c + FAQ (EN+ZH) |

**Symptom / motivation:** With the cause identified (Figma gates dynamic client registration on an exact `client_name`, admitting `"Codex"` and refusing `"Tact"`), `tact-ui mcp login figma` was still unusable: the name was a hard-coded constant with no way to change it, so a user who knew the answer had no way to act on it. Figma's remote server — the recommended one, with the fullest feature set — was unreachable from Tact.

**Decision:** Make the registration name configuration — `[mcp] oauth_client_name` in `config.toml`, defaulting to `"Codex"`, with a per-server `auth.clientName` override in `mcp.json` for the common case where one provider needs a different answer than the rest. The default is chosen so OAuth works out of the box against providers that admit only known clients; `McpSettings::TACT_OAUTH_CLIENT_NAME` documents the honest alternative (`"Tact"`), and both `config.example.toml` and the book state the trade-off plainly: the provider, and the consent screen shown to the user, sees `"Codex"` rather than `"Tact"`. Tact does not hide which name it used — it is logged with the other OAuth progress lines at `info` (`client_name=…`) and printed in any registration failure, alongside both override points. `oauth_parameters` returned a 3-tuple that grew to 4; it now returns a named `OauthRequestParameters` with `effective_client_name()`, which resolves per-server override → configured default → built-in default and treats a blank override as absent (every provider rejects an empty `client_name`). `McpSettings` has a hand-written `Default` rather than a derived one, since an empty name would be the wrong fallback. `resolve_mcp` trims the configured value and falls back on whitespace-only input.

**Behavior after:** `tact-ui mcp login figma` reaches the authorization URL with no configuration (verified against the live Figma endpoint). Any provider that admits a known client name can be reached by setting it. `mcp.oauth_client_name = "Tact"` restores honest identification, and allowlisting providers then refuse with the actionable message. A per-server `clientName` wins over the global default (verified: global `"Tact"` + per-server `"Codex"` succeeds). The name used is visible in `RUST_LOG=tact=debug`/`info` logs.

**Pointers:** `crates/tact/src/config/types.rs` (`McpTomlConfig`, `McpSettings::{DEFAULT_OAUTH_CLIENT_NAME,TACT_OAUTH_CLIENT_NAME,Default}`), `crates/tact/src/config/resolve.rs` (`resolve_mcp`), `crates/tact/src/mcp/remote.rs` (`OauthRequestParameters::effective_client_name`, `oauth_parameters`, `registration_message`, `authorize_remote_server`); `crates/tact-ui/src/test_support.rs`, `crates/tui/src/handlers/select.rs`, `crates/tact/src/{agent/mod.rs,tool/read_image.rs}`, `crates/tact-ui/tests/recovery_compaction.rs` (new required `mcp` field in test config literals). Tests: `config::resolve::tests::resolve_mcp_oauth_client_name_defaults_to_codex_and_is_overridable`, `mcp::remote::tests::{oauth_parameters_default_when_auth_is_not_declared,the_registration_name_falls_back_to_the_configured_default,refused_registration_explains_the_options_not_just_the_status}`. Docs: Ch 8 §Step 1c + FAQ table + a `[mcp]` block in `config.example.toml`; `README.md`.

---

## 1. 2026-09-11 — Figma's OAuth block is a client-name allowlist, not a bug

| Field | Value |
|-------|-------|
| **Type** | docs + bugfix (error message) |
| **Related** | `crates/tact/src/mcp/remote.rs` (`OAUTH_CLIENT_NAME`, `registration_message`); Ch 8 §Step 1c + FAQ (EN+ZH) |

**Symptom / motivation:** `codex mcp add figma --url https://mcp.figma.com/mcp` authenticates successfully, while `tact-ui mcp login figma` fails with `HTTP 403 Forbidden` at dynamic client registration. That contrast was unexplainable from the previous entry's wording ("providers only admit clients they already know about"), which did not say what the provider actually keys on — and Codex demonstrably gets through.

**Investigation:** Codex ships a `figma@codex-marketplace-global` plugin whose `.mcp.json` is an ordinary remote entry (no `client_id`), and each `codex mcp add` run produces a *different* `client_id` — so Codex registers dynamically like Tact does. The difference is the request body: `AuthorizationSession::new(..., Some("Tact"), None)` sends `client_name: "Tact"`, whereas Codex sends `"Codex"`. Posting otherwise byte-identical registration bodies to `https://api.figma.com/v1/oauth/mcp/register` gives a clean, reproducible split: `Codex` → `200` (three consecutive runs, fresh `client_id` + `client_secret`, `token_endpoint_auth_method: "none"`), `Claude Code` → `200`, while `Tact`, `Cursor`, `Visual Studio Code` and lowercase `codex` → `403`. Figma therefore allowlists dynamic registration by an **exact client-name string**, matching its documented MCP catalog. Discovery and everything else in the flow are fine.

**Decision:** Tact keeps registering under its own name and reports the refusal honestly. Sending `"Codex"` would make the registration succeed, but it misrepresents the client to the provider and depends on another product's entitlement, so it is not done — not even silently behind a config flag. Instead: the constant `OAUTH_CLIENT_NAME` names the client once, the error message now states that registration is commonly gated on the client name and that Tact registers as `"Tact"`, and it offers three routes instead of two (a self-registered `auth.clientId`, a static token in `headers`, or the provider's own local server — for Figma, `http://127.0.0.1:3845/mcp`, which needs no OAuth). The measured table is recorded in the Ch 8 FAQ so this is not re-investigated later.

**Behavior after:** Unchanged for every provider that accepts Tact's registration; the next `mcp login` after this fails with an explanation that names the client-name gate, so the user can act (self-register, use a token, or use the local server) instead of retrying. No code path attempts impersonation. Remaining known gap, unchanged and now documented next to the cause: a confidential client's `client_secret` still cannot be supplied, since rmcp's `StoredCredentials` persists only `client_id`.

**Pointers:** `crates/tact/src/mcp/remote.rs` (`OAUTH_CLIENT_NAME`, `registration_message`, `authorize_remote_server`). Tests: `mcp::remote::tests::{refused_registration_explains_the_options_not_just_the_status,a_missing_registration_endpoint_does_not_blame_the_client_name}`. Docs: Ch 8 §Step 1c bullet + FAQ table (EN+ZH). Method: compared `codex mcp` against the live Figma endpoints and identified the `client_name` split by controlled registration requests.

---

## 1. 2026-09-11 — Loopback MCP servers are no longer sent through the environment proxy

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/remote.rs` (`http_client_for`, `is_loopback_url`, `serve_remote`), `crates/tact/Cargo.toml` (`reqwest13`); Ch 8 §Step 1c + gaps (EN+ZH) |

**Symptom / motivation:** With `http_proxy` / `all_proxy` exported, Tact routed **loopback** MCP requests through the proxy. reqwest honours those variables for every host unless told otherwise, so `http://127.0.0.1:3845/mcp` (Figma's desktop server) never reached the local server: the proxy answered instead, and because a proxy error page carries no `Content-Type`, the failure surfaced as the deeply misleading `Unexpected content type: None` rather than "connection refused". Confirmed by contrasting the same server with and without the proxy exported (`Unexpected content type: None` vs `error sending request`), and by running a local listener that then reported `Some("text/plain")` — its own content type — proving the request had finally arrived locally. This matters because the documented workaround for providers that refuse OAuth registration (Figma) is a loopback URL, and the development setup for this repository exports exactly those variables.

**Decision:** Build the transport's HTTP client explicitly and disable proxies when the endpoint is loopback. `reqwest13::Client::builder().no_proxy().build()` is passed through `StreamableHttpClientTransport::with_client`, which produces the same `StreamableHttpClientTransport<reqwest::Client>` type as the previous `from_config`, so nothing else changes. Non-loopback endpoints keep `Client::default()` and therefore the environment proxy, because a proxy is precisely what makes a remote MCP server reachable in a restricted network. Loopback detection is deliberately string-based (`127.0.0.0/8`, `localhost`, `::1`, with ports, userinfo, paths and queries handled): the URL comes from user config, and pulling in a parser or DNS just to recognise `localhost` would add failure modes where a simple check suffices. `localhost.evil.com` and `127.0.0.1.evil.com` are correctly *not* loopback. Failing to build the proxy-free client falls back to the default with a warning, so the connection attempt survives and the log says why it may now fail. One limitation is documented rather than hidden: the OAuth manager builds its own client, so discovery/registration against a loopback server would still use the environment proxy — irrelevant for the Figma desktop server, which needs no OAuth.

**Behavior after:** A local MCP server works with a proxy exported, and a genuinely absent one reports a connection failure instead of a proxy status. Remote servers are unaffected and still use the proxy. `crates/tact` now depends on `reqwest13` (the same version rmcp 0.17 uses; the 0.12 crate it already used would not satisfy `StreamableHttpClient for reqwest::Client`).

**Pointers:** `crates/tact/src/mcp/remote.rs` (`http_client_for`, `is_loopback_url`, `serve_remote`). Tests: `mcp::remote::tests::{loopback_urls_are_recognized_so_they_can_bypass_a_proxy,remote_urls_keep_using_the_environment_proxy}`. Docs: Ch 8 §Step 1c bullet, gaps table (EN+ZH), `README.md`. Live: `cargo test -p tact --test live_remote_mcp -- --ignored` still passes with the proxy exported.

---

## 1. 2026-09-11 — A refused OAuth client registration now says what to do

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/mcp/remote.rs` (`registration_error`, `registration_message`, `registration_reason`, `authorize_remote_server`); Ch 8 §Step 1c + FAQ (EN+ZH) |

**Symptom / motivation:** Authorizing a provider that refuses dynamic client registration produced a message that named no cause and no remedy:

```
Error: MCP authorization failed for figma: OAuth client registration failed for figma:
Registration failed: Dynamic registration failed: Registration failed: HTTP 403 Forbidden: Forbidden
```

The provider here is Figma: its remote server (`https://mcp.figma.com/mcp`) only admits clients listed in its MCP catalog (VS Code, Cursor, Claude Code), so `https://api.figma.com/v1/oauth/mcp/register` answers `403` to any other client. Discovery succeeds — the authorization server is `https://api.figma.com`, whose metadata advertises `client_secret_basic`/`client_secret_post` and *not* the public-client `none` — so the flow fails at its last step with a status that looks like a transient network problem. Verified against the live endpoint with and without a proxy, and with an `Authorization: Bearer`, a form-encoded body, and an empty body: `403` every time, i.e. a server-side policy, not a request-shape bug. `mcp list` correctly showed the server as `needs authorization`, so the dead end was only in the error text.

**Decision:** Classify `AuthError::RegistrationFailed` at the one place the flow still holds the metadata, and replace the raw chain with an explanation plus the two real escape hatches. The guidance distinguishes "advertised a registration endpoint and refused us" from "never advertised one" (rmcp reports `Dynamic client registration not supported` in the latter case), and prints a concrete redirect URI — the pinned `http://127.0.0.1:<callbackPort>/callback` when `callbackPort` is set, since that is exactly what a provider asks for when a client is registered by hand. rmcp wraps its own message twice, so `registration_reason` strips the repeated `Registration failed:` / `Dynamic registration failed:` prefixes to leave the informative tail; if rmcp changes its wording the prefix stops matching and the full text is shown, so this degrades rather than misreports. The failure is logged with the server name, provider URL and whether a registration endpoint was advertised (never tokens), matching the troubleshooting convention for this subsystem. This intentionally does **not** claim to fix Figma: no client-side change can, and the docs now say so and point at the desktop server (`http://127.0.0.1:3845/mcp`, no OAuth) instead.

**Behavior after:** `tact-ui mcp login figma` (and `/mcp auth figma`, which shares the code path) prints `OAuth client registration failed for figma: HTTP 403 Forbidden: Forbidden` followed by the reason, a warning that retrying will not help, and the options: register your own OAuth client and set `auth.clientId` with a pinned `callbackPort`, or supply a provider-issued static token via `headers`. DCR-capable providers are unaffected — the new branch is only reached on `RegistrationFailed`. A confidential client's *secret* still cannot be supplied (rmcp's `StoredCredentials` carries only `client_id`, so refresh would lose it), which is now documented as a known gap rather than a silent limitation.

**Pointers:** `crates/tact/src/mcp/remote.rs` (`registration_error`, `registration_message`, `registration_reason`, `has_registration_endpoint` capture in `authorize_remote_server`, OAuth metadata `debug` log). Tests: `mcp::remote::tests::{registration_reason_unwraps_rmcps_nested_wrapping,refused_registration_explains_the_options_not_just_the_status,missing_registration_endpoint_reads_differently_from_a_refusal}`. Docs: Ch 8 §Step 1c bullet + FAQ entry + gaps table row (EN+ZH). Live check: `cargo test -p tact --test live_remote_mcp -- --ignored` still passes (DCR providers unaffected).

---

## 1. 2026-09-11 — `tact-ui mcp`: full MCP server management from the CLI

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/mcp/edit.rs` (new), `crates/tact/src/mcp/{mod,remote}.rs`, `crates/tact/src/config/cli.rs` (`McpSubcommand`), `crates/tact-ui/src/mcp_cli.rs`, `crates/tui/src/handlers/mcp.rs`; plan `docs/superpowers/plans/2026-09-11-remote-mcp.md` |

**Symptom / motivation:** The CLI could inspect (`mcp list`) and authorize (`mcp auth`) servers but not create, remove or de-authorize one — declaring a server still meant hand-writing `mcp.json`, whose shape is easy to get wrong (remote vs stdio keys, `auth` spelling, quoting), and a headless user has no UI that could help. Credentials additionally had no deletion path at all: once authorized, a server stayed authorized forever, and `~/.tact/mcp/oauth/<server>.json` could only be removed by hand. `mcp list` also connected to *every* server, so inspecting one entry spawned and dialed the rest of the configuration.

**Decision:** Ship the full surface as six subcommands split by the side effect they own, so no command silently does two things: `list` (all servers, connects), `get <name>` (one server, connects to that one only), `add` (writes config, never connects), `remove` (writes config, keeps credentials), `login` (OAuth flow, writes credentials; `auth` kept as a visible alias), `logout` (deletes credentials, never connects). `tact-ui mcp add <name> --url <URL> [--oauth] [--header N:V]…` / `--command <CMD> [--arg A]… [--env N=V]…` covers both transports; clap enforces the `--url` XOR `--command` split and that `--arg`/`--env`/`--header`/`--oauth` require their transport. `--user` selects the home file over the project file; `add --force` replaces, and a `remove` of an unknown name is an error rather than a silent no-op — the message says which file the server *is* declared in (`retry with --user`) or that a plugin contributes it. Validation (name charset, URL scheme, HTTP header names *and* values) happens before anything is written. Config writes **edit the raw JSON document** rather than round-tripping through `McpConfigFile`, so unknown keys and unrelated servers survive; they are atomic (temp file + rename), and an unparseable file becomes an error instead of a clobber target. Removing the last server leaves an empty `mcpServers` object (a predictable edit that still reads back as "no servers"). Server names are additionally refused if they contain whitespace, control characters or a path separator: a name is also the `<server>` segment of `mcp__<server>__<tool>` and the credential file name, so `mcp logout <name>` must never be pointable at an arbitrary file — `oauth_credential_path` now refuses unsafe names outright, which also hardens `login`/`auth` against a hostile `mcp.json` key. The TUI accepts `/mcp login <server>` as an alias for `/mcp auth <server>`, and both views share one status vocabulary via the extracted `connected_text`/`needs_auth_text`/`failed_text` helpers. `mcp list` additionally prints an **Overridden declarations** section naming the losing and winning files, since an override decides what removing a declaration actually changes. Header/env *values* are never echoed or logged.

**Behavior after:** `tact-ui mcp add figma --url https://mcp.figma.com/mcp` creates or extends `.tact/mcp.json` and prints the path plus the new entry's transport; `--oauth` prints the follow-up `tact-ui mcp login figma` hint. `mcp get figma` prints transport, source, status and the tool names as the agent must call them (`mcp__figma__<tool>`), connecting only to that server. `mcp remove` deletes exactly one declaration and reports where to look when the name lives elsewhere; `mcp logout` deletes the credential file, is idempotent when nothing is stored, and works even after the declaration is gone. Re-adding an existing name errors unless `--force` is given. Keys Tact does not model survive the round trip; keys are re-serialized as sorted pretty JSON, so a hand-formatted file is reformatted once on first write.

**Pointers:** `crates/tact/src/mcp/edit.rs` (`McpConfigScope`, `McpDraftTransport`, `McpServerDraft::{new,transport_kind}`, `add_mcp_server`, `RemovedMcpServer`, `remove_mcp_server`, `read_document`, `write_document`, `validate_remote_url`); `crates/tact/src/mcp/mod.rs` (`is_safe_server_name`, `validate_server_name`, `McpServerStatus`, `McpServerInspection`, `ConnectOutcome`, `connect_server`, `resolved_server_for`, `inspect_server`, refactored `load_mcp_router_with_report_inner`); `crates/tact/src/mcp/remote.rs` (`oauth_credential_path` hardening, `forget_credentials`); `crates/tact/src/config/cli.rs` (`McpSubcommand::{List,Get,Add,Remove,Login,Logout}`); `crates/tact-ui/src/mcp_cli.rs` (`get_server`, `render_server_detail`, `status_text`, `remove`, `scope_hint`, `scope_of`, `add`, `draft_from_args`, `parse_pairs`, `logout`); `crates/tui/src/handlers/mcp.rs`. Tests: `mcp::edit::tests::{creates_a_project_file_for_a_remote_server,writes_oauth_declaration_for_remote_server,writes_stdio_entry_with_args_and_env,adding_a_second_server_keeps_unknown_keys_and_the_first_server,refuses_to_replace_without_force_and_replaces_with_it,an_unparseable_file_is_never_overwritten,a_flat_mcp_servers_value_is_rejected,written_entries_load_back_through_the_reader,the_written_file_has_no_leftover_temp_sibling,invalid_names_and_transports_are_rejected_before_writing,project_scope_targets_the_workdir_and_user_scope_the_home_dir,remove_deletes_only_that_entry_and_keeps_everything_else,removing_the_last_server_leaves_an_empty_but_valid_config,remove_reports_an_absent_name_instead_of_succeeding_silently,an_unsafe_server_name_is_rejected_by_both_add_and_remove}`, `mcp::remote::tests::{an_unsafe_server_name_never_derives_a_credential_path,forgetting_credentials_rejects_an_unsafe_name_before_touching_disk,forgetting_a_missing_credential_is_not_an_error}`, `mcp_cli::tests::{overridden_declarations_name_the_file_that_wins,a_report_without_overrides_has_no_override_section,detail_view_shows_transport_source_status_and_qualified_tool_names,detail_view_reuses_the_list_wording_for_pending_and_failed_servers,detail_view_never_prints_an_empty_tool_list_as_success,scope_of_maps_the_user_flag,a_url_becomes_a_remote_draft_with_headers_and_oauth,a_command_becomes_a_stdio_draft_with_args_and_env,neither_or_both_transports_are_rejected,malformed_pairs_fail_without_echoing_the_value,pair_parsing_trims_the_name_and_value}`, `tui::handlers::mcp::tests::mcp_login_is_an_alias_for_auth`. Docs: Ch 8 Step 1 + Step 1c (EN+ZH), Ch 21 plugin paragraph (EN+ZH), `README.md`.

---

## 1. 2026-09-11 — Remote MCP servers: Streamable HTTP + OAuth 2.0

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/mcp/{mod,remote}.rs`, `crates/tact/src/consts.rs`, `crates/tact/src/agent/mod.rs` (`reload_mcp_router`), `crates/protocol/src/agent.rs` (`UserCommand::McpAuth`), `crates/tact-ui/src/driver.rs`, `crates/tui/src/handlers/mcp.rs`, `crates/agent_tui_kit/src/i18n.rs`; design `docs/superpowers/specs/2026-09-11-remote-mcp-design.md`; plan `docs/superpowers/plans/2026-09-11-remote-mcp.md` |

**Symptom / motivation:** Tact's MCP client spoke only stdio. `McpProjectConfig` already parsed `type: "http" | "sse"` and `url`, but `resolve_servers` pushed every remote entry into `skipped_remote` and dropped it, so a `{ "url": … }` server produced no connection and only a terse "skipped" notice. The 2026-09-10 `openai-curated` catalog made this concrete: plugins whose MCP servers are remote could be installed but never used. Remote MCP endpoints also generally require OAuth (MCP 2025-06-18 / SEP-985), so transport alone would not have made them usable.

**Decision:** Support remote entries end to end on top of `rmcp`'s Streamable HTTP client plus its OAuth support, keeping the existing `McpService` abstraction so routing, naming, and permissions are untouched. An entry is `command` (stdio) or `url` (remote), with optional `headers` (static auth) and `auth: { "type": "oauth", … }`. `command` wins when both are present; an entry with neither is reported as skipped. OAuth is the authorization-code + PKCE flow with metadata discovery, dynamic client registration (unless `clientId` is given), and a loopback redirect on `127.0.0.1`; tokens are persisted per server at `~/.tact/mcp/oauth/<server>.json` (`0600`) and refreshed automatically. Startup never blocks on the browser: a server with OAuth but no usable credential is reported as `pending_auth` (`MCP server <name> needs authorization — run /mcp auth <name>`) instead of being connected or failed. `/mcp auth <server>` runs the flow, prints the authorization URL, and then hot-reloads the MCP router (`Agent::reload_mcp_router`) so no restart is needed. Every step logs server names, URLs, and header *names* only — token values are never logged.

Declaring `auth` is deliberately **not required** even for a server that needs OAuth. Live testing showed a bare `url` against Linear (which requires OAuth) surfaced as an opaque `Auth required` connection failure, so the 401 is now detected and upgraded to `pending_auth`. rmcp models this as `StreamableHttpError::AuthRequired`, but that variant holds a type implementing neither `Display` nor `Error` and `ClientInitializeError::TransportError` does not chain it via `#[source]`, so it cannot be downcast; what *is* reachable is `ClientInitializeError` itself (verified against rmcp 0.17), so `is_auth_required_error` matches that transport variant and checks rmcp's `"Auth required"` message — an unrecognised error degrades to a plain failure rather than misreporting. The complementary half is required for the notice not to be a dead end: `/mcp auth` works without an `auth` declaration (defaulting to dynamic registration, no scopes, ephemeral port), and a stored credential is honoured regardless of whether `auth` was declared.

The interactive TUI reaches this through `/mcp auth <server>`; headless users have no TUI, so the same pair is exposed as CLI subcommands: `tact-ui mcp list` resolves and connects exactly like startup then prints every server with its transport, source, and status (connected / needs authorization / failed / skipped), and `tact-ui mcp auth <server>` runs the flow, prints the URL, and re-lists afterwards so the result is immediately visible. Both are registered as non-LLM commands, so they never require provider config or an API key. `mcp list` also covers the `/mcp status` surface originally deferred in this work; the TUI itself still learns status from the startup notices.

**Behavior after:** `{ "url": … }` / `type: "http"` servers are connected instead of skipped; `skipped_remote` now means an unsupported/incomplete transport only. Installed plugins may contribute remote servers. An OAuth server appears once as a pending-authorization notice — whether it declared `auth` or was caught by its 401 — and after `/mcp auth <server>` its tools become available in the same session; later sessions connect without prompting. Expired, non-refreshable tokens return to `pending_auth` rather than surfacing as connection failures. A connection failure is still never fatal.

**Pointers:** `crates/tact/src/mcp/remote.rs` (`McpRemoteConfig`, `McpAuthConfig`, `serve_remote`, `resolve_remote_auth`, `stored_access_token_at`, `oauth_parameters`, `is_auth_required_error`, `authorize_remote_server`, `FileCredentialStore`, `await_oauth_callback`, `percent_decode`); `crates/tact/src/mcp/mod.rs` (`McpTransportConfig`, `McpTransportKind`, `ConfiguredServer`, `to_transport`, `resolve_servers`, `ResolvedServers::configured`, `load_mcp_router_with_report`, `remote_config_for`, `authorize_server`, `McpLoadReport::{configured,pending_auth,notice_lines}`); `crates/tact-ui/src/mcp_cli.rs` (`run_mcp_cli`, `render_report`, `status_for`); `crates/tact/src/config/cli.rs` (`McpSubcommand`); `crates/tact/src/agent/mod.rs` (`rebuild_cached_tool_specs`, `reload_mcp_router`); `crates/tact/src/consts.rs` (`home_mcp_oauth_dir`). Tests: `remote_config_parses_url_headers_and_oauth`, `invalid_header_names_are_dropped_from_the_transport_config`, `oauth_token_becomes_the_bearer_auth_header`, `file_credential_store_round_trips_and_clears`, `callback_listener_{extracts_code_and_state,surfaces_denied_authorization,times_out_without_a_request}`, `commandless_entries_are_skipped_not_fatal_while_remote_entries_connect`, `remote_entry_with_oauth_needs_authorization_without_credentials`, `load_report_renders_pending_authorization`, `auth_required_detection_ignores_unrelated_errors`, `undeclared_auth_still_uses_a_stored_credential`, `oauth_parameters_default_when_auth_is_not_declared`, `mcp_cli::tests::{empty_report_explains_how_to_configure,renders_each_server_with_its_status,skipped_servers_are_listed_even_though_they_are_not_configured,a_server_with_no_recorded_outcome_is_not_reported_as_healthy}`. Live (opt-in) end-to-end checks against public remote servers: `crates/tact/tests/live_remote_mcp.rs` (`cargo test -p tact --test live_remote_mcp -- --ignored --nocapture`) covers DeepWiki + Cloudflare Docs connecting with `mcp__<key>__*` tools, Linear's 401 being upgraded to `pending_auth`, and the authorization URL being produced both with and without a declared `auth` (verifying discovery, dynamic registration, and PKCE S256 against a real provider). Docs: Ch 8 §3.2/Step 1b/1c/FAQ/Gaps (EN+ZH), Ch 21 plugin paragraph (EN+ZH).

---

## 1. 2026-09-10 — `default-features = false` on `tracing-subscriber` finally takes effect

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `Cargo.toml`, `crates/tact-ui/Cargo.toml`, `crates/tact-ui/src/main.rs` (`init_logging`); supersedes part of `24150b08` |

**Symptom / motivation:** The 2026-09-08 fix that routed MCP logs into `tracing` and kept them out of the TUI also tried to drop `tracing-subscriber`'s default features, writing `tracing-subscriber = { workspace = true, default-features = false }` in `crates/tact-ui/Cargo.toml`. Cargo **ignores `default-features` when a dependency is inherited from the workspace** — it only emits a warning — so `ansi` and `tracing-log` stayed enabled. `tracing-log` being on makes `registry().init()` install the global `log` bridge, so every `log::` record from a dependency (rmcp's MCP handshake, reqwest, …) was forwarded into Tact's daily log file as a field-less line: exactly the noise the earlier fix was meant to remove. `ansi` was equally useless here, since `init_logging` writes to a file with `.with_ansi(false)`.

**Decision:** Put the feature selection in the one place Cargo honours — the workspace dependency — and let the member inherit it verbatim: `tracing-subscriber = { version = "0.3", default-features = false, features = ["fmt", "env-filter"] }` in the root `Cargo.toml`, with `crates/tact-ui/Cargo.toml` reduced to `{ workspace = true }`. `fmt` and `env-filter` are the only features `init_logging` actually uses; `ansi` and `tracing-log` are now absent from the resolved graph.

**Behavior after:** Dependency `log` records no longer reach Tact's log file — `.tact/logs/tact-<date>.log` carries only events emitted through `tracing` by Tact crates. `RUST_LOG` filtering is unchanged (`EnvFilter` is still built from the environment), and the interactive TUI still never installs a terminal fmt layer. Verified with `cargo tree -e features -i tracing-subscriber`, which no longer lists `ansi` or `tracing-log`, plus `cargo test -p tact --lib` (751 passed) and `cargo check -p tact-ui --all-targets`.

**Pointers:** `Cargo.toml` (`[workspace.dependencies] tracing-subscriber`); `crates/tact-ui/Cargo.toml`; `crates/tact-ui/src/main.rs` (`init_logging`). Superseded commit: `24150b08`.

---

## 1. 2026-09-10 — MCP gains a native config file; plugin state and skill roots converge

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/mcp/mod.rs`, `crates/tact/src/consts.rs`, `crates/tact/src/plugin/store.rs`, `crates/tact-ui/src/{interactive,headless}.rs`; design `docs/superpowers/specs/2026-09-10-path-convergence-design.md`; plan `docs/superpowers/plans/2026-09-10-path-convergence.md` |

**Symptom / motivation:** MCP configuration was spread across three ecosystems with no Tact-native home. A project's servers could only come from a cwd-scoped `.codex-plugin/plugin.json` manifest, and global scope required a marketplace → install → cache round trip; there was no `~/.tact/mcp.json` and no project-scoped MCP file at all. Three further defects compounded it: the cwd manifest used a *different* naming scheme (`{plugin}__{server}`) from plugin-contributed servers, so the same concept had two answers; a plugin-root `.mcp.json` was read while the same filename at the working directory was silently ignored; a failed server connection logged only `tracing::debug!` so a typo'd `command` produced no user-visible signal at all; and `~/.tact/plugins/` mixed a few KB of state with hundreds of MB of cache. Separately, `skill_search_dirs` returned `[workdir/.tact/skills, ~/.tact/skills, ~/.agents/skills]` with later-wins semantics, so the Codex compatibility root overrode Tact's own root.

**Decision:** Give MCP a native `mcp.json` at both scopes — `~/.tact/mcp.json` (user) and `<workdir>/.tact/mcp.json` (project) — using the same `mcpServers` shape every MCP client accepts, with project overriding user by server name. A server declared there is named by its map key verbatim, so its tools are exactly `mcp__<key>__<tool>` with no manifest prefix. Project scope gets **exactly one** filename: Tact now reads neither a cwd `.mcp.json` nor a cwd `.codex-plugin/plugin.json`, so "where does this project declare its servers?" has a single answer and no directory needs to be interpreted as a "plugin". `PluginLoader` was deleted with that source — it had no other caller. Only installed marketplace plugins still contribute servers, keeping their `plugin__<plugin>__<server>` names, because a plugin is a distributable bundle rather than a config convention. Resolution order: `~/.tact/mcp.json` → `<workdir>/.tact/mcp.json` → installed plugins. It returns an `McpLoadReport` (connected / failures / shadowed / skipped_remote) instead of discarding failures, and plugin state moves to `~/.tact/plugins/state/` with a legacy fallback read plus best-effort one-time migration. Skill roots are reordered to `[~/.agents/skills, ~/.tact/skills, <workdir>/.tact/skills]` so the project always wins and Tact always beats Codex. `PluginHome` now stores `home` and `state` explicitly instead of deriving `$HOME` via `root.parent().parent()`.

**Behavior after:** Adding an MCP server is one file, at project or user scope; no marketplace round trip, and no ambiguity about which project file to use. Only two places need to be checked when a server is missing: `~/.tact/mcp.json` and `.tact/mcp.json`. A broken server produces a visible startup notice (`AgentUpdate::Info` in the TUI, stderr in headless) naming the server and error, while a clean load stays silent; connection failures remain non-fatal so one bad server cannot stop the agent from starting. Overrides between sources are reported rather than silent. Remote (`http`/`sse`) and command-less entries are reported as skipped instead of aborting resolution. A malformed cwd manifest can no longer abort startup, because it is no longer read. Plugin state is written to `state/`, with legacy files read and left in place so an older binary sharing the home keeps working. A user-level skill present in both `~/.tact/skills` and `~/.agents/skills` now resolves to the `~/.tact/skills` body — an intentional precedence flip.

**Pointers:** `crates/tact/src/mcp/mod.rs` (`McpConfigFile`, `McpLoadReport`, `collect_sourced_servers`, `resolve_servers`, `load_mcp_router_with_report`); `crates/tact/src/consts.rs` (`TactPath::{mcp_config_path, home_mcp_config_path}`, `skill_search_dirs`, `PluginHome::{home, state}`); `crates/tact/src/plugin/store.rs` (`state_file`, `read_state`); `crates/tact-ui/src/{interactive,headless}.rs`. Tests: `mcp_config_file_reads_servers_and_missing_is_none`, `mcp_config_file_parse_error_names_the_path`, `later_source_overrides_earlier_by_server_name`, `non_conflicting_sources_merge`, `remote_and_commandless_entries_are_skipped_not_fatal`, `native_config_key_is_the_server_name_without_a_prefix`, `project_mcp_json_is_read_and_a_cwd_dot_mcp_json_is_not`, `load_report_*`, `plugin_home_exposes_explicit_home_state_and_cache_paths`, `new_state_location_wins_when_both_exist`, `legacy_state_is_read_and_migrated_to_state_dir`, `state_is_written_to_the_state_directory`, `tact_skill_root_outranks_the_agents_compatibility_root`.

---

## 1. 2026-09-10 — Permission prompts reconcile from a shared pending-UI broker

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/ui_responder.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/handlers/select.rs`, `crates/tact-ui/src/interactive.rs`; Ch 25 §4.3; `docs/state_machines.md` §2/§8 |

**Symptom / motivation:** A permission prompt could remain `Running` forever when its single `AgentUpdate::RequestSelect` event was lost or handled while another update reset `input_mode`. The previous `restore_pending_select_mode` fix covered the known `SessionStats` desync, but the event itself was still the only source of truth: if it was dropped, or the TUI did not process it, the `UiResponder` waiter had nothing that could answer it. The same failure class hit interactive `edit_file` prompts; one observed step sat in Running for roughly 30 minutes without writing the file or producing a `tool_result`.

**Decision:** Keep the protocol types unchanged and make the in-process `UiResponder` the authoritative pending-request registry. `register_select` / `register_multi` record `PendingUiRequest` metadata before emitting the existing `RequestSelect` hint; `snapshot()` returns the ordered pending set; `respond()` atomically removes and wakes the waiter; `withdraw()` handles cancellation/drop. The TUI reconciles `InputMode::Select` from `snapshot()` after every agent update and on poll ticks, treating `RequestSelect` as a wake-up hint and answering directly through the broker when interactive. `RequestSelect` remains authoritative for headless/tests when no broker is attached. `PendingRequestGuard` withdraws a request if its waiter future is abandoned, so an aborted tool cannot leave a ghost popup.

**Behavior after:** A lost or duplicated `RequestSelect` no longer leaves a tool stuck in Running: the TUI pulls the pending snapshot and surfaces the popup. Multiple prompts (concurrent subagents / `ask_user`) queue by request id in the broker rather than a separate `VecDeque`. Enter/Esc answer the broker directly; `/cancel` answers the active prompt with `None` before sending `UserCommand::Cancel`; aborting a waiter removes its pending entry. The `tact_protocol` enum and wire shape are unchanged; this is an in-process reconciliation layer intended to be promoted to a versioned snapshot protocol before a server transport is added.

**Pointers:** `crates/tact/src/ui_responder.rs` (`PendingUiRequest`, `snapshot`, `respond`, `withdraw`, `PendingRequestGuard`); `crates/tui/src/widgets/state/app/agent.rs` (`reconcile_pending_ui`); `crates/tui/src/handlers/select.rs`; `crates/tact-ui/src/interactive.rs`; `crates/tui/src/lib.rs`; Ch 25 §4.3; `docs/state_machines.md` §2/§8. Tests: `ui_responder::tests::*`, `broker_snapshot_*`, `broker_mode_enter_wakes_registered_waiter`, `broker_mode_cancel_answers_pending_select_with_none`.

---

## 1. 2026-09-10 — Discover Codex local marketplaces for plugin install/list

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/plugin/{model,store,marketplace,install,mod}.rs`, `crates/tact-ui/src/plugin_cli.rs`; design `docs/superpowers/specs/2026-09-10-codex-marketplace-design.md`; plan `docs/superpowers/plans/2026-09-10-codex-marketplace.md` |

**Symptom / motivation:** Tact adopted Codex plugin manifests/hooks/MCP, but marketplace discovery still exposed only the hardcoded Claude official Git marketplace. A Codex personal marketplace at `~/.agents/plugins/marketplace.json` was invisible to `tact-ui plugin marketplace list`; Codex `source: "local"` catalog entries could not be parsed; and a bare `plugin install <name>` defaulted to `claude-plugins-official`.

**Decision:** Discover Codex local marketplaces from `$HOME/.agents/plugins/marketplace.json` and the nearest ancestor `.agents/plugins/marketplace.json`, store their roots as non-persisted `MarketplaceSource::LocalPath` records, and resolve `source: "local"` plugin paths relative to the marketplace root. Bare installs now scan discovered Codex marketplaces first and fall back to `claude-plugins-official`; local marketplace update re-reads the catalog instead of fetching.

**Behavior after:** `tact-ui plugin marketplace list` shows Codex local marketplaces before the legacy official marketplace; `tact-ui plugin install build-ios-apps` can install from the user's Codex marketplace without an explicit `@marketplace`; `plugin marketplace update <codex-local-name>` refreshes from disk. The Claude official marketplace remains available as fallback.

**Pointers:** `crates/tact/src/plugin/model.rs` (`LocalPath`, discovered state), `crates/tact/src/plugin/store.rs` (Codex marketplace discovery), `crates/tact/src/plugin/marketplace.rs` (`source: "local"`, catalog path), `crates/tact/src/plugin/install.rs` (local source root), `crates/tact/src/plugin/mod.rs` (default install resolution); tests `parses_codex_local_plugin_source`, `load_marketplaces_discovers_codex_personal_marketplace`, `install_from_codex_local_marketplace_resolves_relative_to_home`, `install_without_marketplace_prefers_discovered_codex_marketplace`.

---

## 1. 2026-09-10 — Seed the OpenAI Codex marketplace as a built-in

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/plugin/{model,install,hooks}.rs`; tests `codex_manifest_accepts_string_mcp_servers_and_inline_hooks`, `parses_inline_manifest_hooks`; Ch 21 §plugin |

**Symptom / motivation:** Tact only had one built-in marketplace, `claude-plugins-official`. OpenAI's official Codex catalog `github.com/openai/plugins` (catalog `openai-curated`) was not available out of the box, and its plugins could not install because the Codex manifests do not inline `mcpServers`/`hooks`: `mcpServers` is a relative file path `"./.mcp.json"` and `hooks` may be an inline object, both of which the installer rejected.

**Decision:** Register `openai-curated` (source `https://github.com/openai/plugins.git`) as a second built-in marketplace, protected like `claude-plugins-official` (cannot be replaced/removed) and restored on load/deserialize. The installer and hooks manifest parsers now accept both inline values and relative file paths for `mcpServers`/`hooks`; a missing declared file does not count as a feature. `mcp` features remain install-time validation only — runtime still connects stdio servers, while remote (`http`/`url`) MCP and `apps`-connector manifests that Tact does not interpret are not install blockers.

**Behavior after:** `plugin marketplace list` shows `claude-plugins-official` and `openai-curated` by default; `plugin install linear@openai-curated` (and bare installs that fall back to a discovered Codex marketplace) can install 57 of the OpenAI catalog's plugins (the rest are pure `apps` connectors with no `mcpServers`/`skills`). Remote-HTTP-MCP plugins from `openai/plugins` install but their MCP is skipped at runtime.

**Pointers:** `crates/tact/src/plugin/model.rs` (`OPENAI_MARKETPLACE`, `BUILTIN_MARKETPLACES`, `is_builtin_marketplace`, `builtin_record`), `crates/tact/src/plugin/install.rs` (`PluginManifest`, `manifest_declares_file_or_inline`), `crates/tact/src/plugin/hooks.rs` (`inline_hooks`, `load_installed_hooks`).

---

## 1. 2026-09-09 — Subagent sticky no longer stays open after the last subagent finishes

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/agent_tui_kit/src/state/subagent_panel.rs` (`SubagentPanelState::apply_snapshot`); host render `crates/tui/src/render/task_panel.rs` |

**Symptom / motivation:** After the last running subagent completed, the sticky subagent strip stayed **expanded** (`[Subagent] 0/1` + a `— Completed —` row with the summary) instead of closing. It only hid if the user manually collapsed it first. The Tasks sticky, by contrast, hides as soon as no open item remains.

**Decision:** Mirror the Tasks rule — the subagent sticky is visible only while at least one subagent is **Running**. When the last one finishes, hide the whole strip (`visible = false`, `expanded = false`). The finished run's summary/detail is not lost: it stays on the parent `spawn_subagent` tool card / subagent popup, not the sticky.

**Behavior after:** A running subagent pops the strip (expanded); once the final subagent reaches a terminal state the strip closes automatically. If several run concurrently, the strip stays until the last one finishes. Sticky show/timing no longer depends on the user's expand/collapse state.

**Pointers:** `crates/agent_tui_kit/src/state/subagent_panel.rs`; sibling rule `crates/agent_tui_kit/src/state/task_panel.rs::apply_snapshot`; Ch 9 hook / agent-loop chapters reference the subagent overview. Tests: `agent_tui_kit` state `subagent_panel` (`hides_when_all_done`, `stays_visible_while_other_runs_are_running`) and `crates/tui` `sticky_host` render tests.

---

## 1. 2026-09-09 — Adopt the Codex plugin/ecosystem; remove Claude-directory compatibility

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | `crates/tact/src/plugin/{install,hooks,marketplace,store,model}.rs`, `crates/tact/src/mcp/mod.rs`, `crates/tact/src/consts.rs`, `crates/tact/src/skill/mod.rs`, `crates/tact/src/config/instruction_sources.rs`, `crates/tact/src/prompt/{mod.rs,system_prompt_template.md,responses_system_prompt_template.md}`, `crates/tact/src/agent/mod.rs`; design `docs/superpowers/plans/2026-09-09-codex-plugin-compat-and-memory.md`; Ch 2, 3, 4, 8, 9, 12, 18 |

**Symptom / motivation:** Tact maintained two plugin ecosystems (Claude `.claude-plugin` and the Codex family). Both share the same command-hook kernel (subprocess + stdin JSON + `CLAUDE_PLUGIN_ROOT`), but maintaining two manifest/discovery systems is not worth it; Codex's layout is the cleaner, actively-maintained spec, and agentmemory ships `.codex-plugin`, so Codex-only still consumes it. Claude-directory compat (`.claude-plugin`, `.claude/`, `CLAUDE.md`, `.claude/skills`) was legacy surface.

**Decision:** Standardise on the Codex plugin system and remove Claude-directory compatibility outright (not gated): plugin manifest dir is `.codex-plugin/plugin.json`; skill roots are `.tact/skills` → `~/.tact/skills` → `~/.agents/skills` (`.claude/skills` gone); `home_claude_dir()` / `claude_dir()` removed; `CLAUDE.md` instruction injection removed so `[agent].instruction_sources` only accepts `agents_md`; system-prompt `# Additional context` no longer carries a claude_md branch. `CLAUDE_PLUGIN_ROOT` env name is **kept** (Codex's own hook engine injects it); Claude/Anthropic as an **LLM provider and `claude-*` model names are untouched**; `~/.agents` is kept.

**Behavior after:** Plugins are discovered only through `.codex-plugin/plugin.json` (+ default `hooks/hooks.json`). Project skills load from `.tact/skills` (plus `~/.tact/skills`, `~/.agents/skills`); a `.claude/skills` dir under the workdir is ignored. Only `AGENTS.md` is injected as an instruction file; a `claude_md*` value in `instruction_sources` is rejected. Users migrating from `.claude-plugin` must re-point to codex layouts.

---

## 1. 2026-09-08 — Remove permission-prompt timeout; fix the "missed popup" desync instead

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tui/src/widgets/state/app/agent.rs` (`restore_pending_select_mode`), `crates/tact/src/agent/tool_dispatch.rs` (Ask-path `request_select` wait) |

**Symptom / motivation:** The intermediate timeout fix (`PERMISSION_PROMPT_TIMEOUT_SECS`, 300 s) was wrong on both counts. It only bounded the hang — the original desync that hid a pending popup remained. And its side effect was too large: any prompt left unattended for 300 s was silently auto-`deny`ed, dropping a tool the user may simply have stepped away from, with no way to distinguish "I said no" from "I wasn't there". A permission decision must come from the user, never from a clock.

**Root cause:** A pending `RequestSelect` must keep `input_mode == Select` for the popup to render, but `AgentUpdate::SessionStats` (and any future update) reset `input_mode = Normal` unconditionally. When that happened while a permission prompt was outstanding, the popup vanished but `select.request_id` stayed set, so the waiter blocked forever (the original 14-minute `save_memory` hang).

**Decision:** Two-part fix. (1) Remove the timeout entirely — the Ask-path wait is again an unbounded `request_select().await`, whose only terminations are a real user answer or a UI close (both already route through `UiResponder`: Esc → `choice: None`, UI close / dead channel → `Err(Closed)`, both deny). (2) Remove the desync that made a popup missable: after every `handle_agent_update`, if `select.request_id` is set but `input_mode` is no longer `Select`, restore `Select` (`restore_pending_select_mode`). A pending request can therefore never be rendered invisible, so the user always has a popup to answer and the ACK paths fire.

**Behavior after:** No timeout exists for permission prompts. A prompt stays on screen and waits until the user answers (Allow once / Always allow / Deny) or the UI closes — never auto-denied by elapsed time. The "missed popup" desync that caused the original 14-minute hang is gone.

---

## 1. 2026-09-08 — Three more hooks complete agentmemory integration (PostToolUseFailure · Notification · TaskCompleted)

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/tact/src/hook/mod.rs`, `crates/tact/src/agent/{mod,tool_dispatch}.rs`, `crates/tact/src/plugin/hooks.rs`, `crates/tact-ui/src/driver.rs` |

**Symptom / motivation:** agentmemory's auto-capture plugin declares twelve Claude-Code hook events (`plugin/hooks/hooks.json`), but tact still lacked `PostToolUseFailure`, `Notification`, and `TaskCompleted` — failed tool calls, permission prompts, and task completion were invisible to memory capture.

**Decision:** Add the three remaining events following Claude Code semantics. `PostToolUseFailure` fires after a tool *fails* (in addition to the success-oriented `PostToolUse`), carrying `tool_name` / `tool_input` / `tool_use_id` / `error`. `Notification` fires when the agent surfaces a user notification — only `permission_prompt` today — carrying `notification_type` / `title` / `message`. `TaskCompleted` fires once per completed user task at the driver's `SubmitTask` boundary, carrying `task_description` (the last assistant message). All three are observational (a `Block` is logged and ignored) and wired through `apply_plugin_hooks`.

**Behavior after:** Tact now covers all twelve agentmemory hook events (plus `PostCompact`, which agentmemory does not consume). Failed tool calls, permission prompts, and completed tasks flow to plugin command hooks.

---

## 1. 2026-09-08 — Five more lifecycle hooks (SubagentStop · Stop · SessionEnd · PreCompact · PostCompact)

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/tact/src/hook/mod.rs`, `crates/tact/src/agent/mod.rs`, `crates/tact/src/compact/mod.rs`, `crates/tact/src/tool/{mod,subagent}.rs`, `crates/tact/src/plugin/hooks.rs`, `crates/tact-ui/src/{interactive,headless,driver}.rs` |

**Symptom / motivation:** Tact mapped only five Claude-Code-style hook events (`SessionStart`, `UserPromptSubmit`, `SubagentStart`, `PreToolUse`, `PostToolUse`), whereas Codex exposes twelve. Users porting Codex/Claude plugins that rely on `SubagentStop`, `Stop`, `SessionEnd`, `PreCompact`, or `PostCompact` found no loop point to attach to.

**Decision:** Add the five events Codex has that tact lacked, following Codex's semantics (source: `codex-rs/hooks/src/events/{stop,compact,session_end}.rs`). `Stop` fires once at the outer turn boundary and a `Block(reason)` *continues* the turn with `reason` as the next prompt (Codex continuation fragment) — the one event where `Block` inverts to "continue". `PreCompact` fires before compaction and a `Block` vetoes it; `PostCompact` after success; both matched against a `CompactTrigger { Auto|Manual|Recovery|Command }` string. `SessionEnd` fires at teardown (observational). `SubagentStop` is a standalone `ToolContext` trait (like `SubagentStart`) that may rewrite the child summary; all events are wired through the plugin command layer (`apply_plugin_hooks`) and the `Hook` enum.

**Behavior after:** Plugins can declare the ten events; `Stop` blocks loop the agent once more (bounded to 4 continuations per task); `PreCompact` blocks skip compaction; the other three are observational. Compaction call sites pass an explicit `CompactTrigger` so plugin matchers can distinguish auto/manual/recovery/command.

---

## 1. 2026-09-08 — Thinking-mode on a reasoning model forces reasoning replay on compatible base URLs

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/openai/responses/mod.rs` (`reasoning_replay_required`, `build_wire_request` / compact reasoning-replay policy); DeepSeek (OpenCode) provider support |

**Symptom / motivation:** On an OpenAI-compatible base URL that is **not** `api.openai.com` (e.g. the OpenCode Go endpoint `opencode.ai/zen/go/v1` or `api.deepseek.com`), the adapter's base-URL heuristic defaults to *dropping* historical `reasoning` replay to save input tokens. But DeepSeek-style Reasoning models in **thinking mode** (`thinking` or `reasoning_effort` set) require the prior `reasoning_text` to be passed back on the next turn; dropping it made the provider return HTTP 400 (`reasoning_text in the thinking mode must be passed back to the API`).

**Decision:** `reasoning_replay_required(request)` is true when the request is thinking-mode **and** the model id looks reasoning-capable (currently a `deepseek` substring on the lowercased model). The effective policy becomes `replay_prior_reasoning || reasoning_replay_required(...)`, so the explicit `with_replay_prior_reasoning` override is still honored. The base-URL-only heuristic still drops replay for ordinary (non-thinking) requests to save tokens.

**Behavior after:** A thinking-mode request on a DeepSeek-style model over a compatible base URL replays the historical `reasoning` item, so the provider no longer 400s. Non-thinking requests and thinking-mode on models the heuristic does not recognize keep the prior token-saving default.

---

## 1. 2026-09-08 — Permission prompts time out instead of hanging a tool in "Running" (superseded — see newest entry)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/agent/tool_dispatch.rs` (Ask-path `request_select` wait) |

**Symptom / motivation:** A `save_memory` call sat in the TUI as `Running · 829s` (14 minutes) with no visible approval popup. `save_memory` is `PermissionPolicy::Write`, so in Default mode it raised an interactive `PermissionBehavior::Ask`, which dispatched `RequestSelect` to the TUI and then awaited the `UiResponder` oneshot. If the popup was missed, dismissed, or dropped while the UI was busy, the tool future blocked forever and the card's live elapsed counter kept climbing.

**Decision (superseded):** This first attempt bounded every interactive permission `request_select` wait with `PERMISSION_PROMPT_TIMEOUT_SECS` (300 s), auto-denying on timeout. **Reverted** the same day: the timeout auto-denied prompts the user never answered (dropping tools they may have stepped away from), and it masked — rather than fixed — the desync that hid the popup. See the newest entry ("Remove permission-prompt timeout; fix the 'missed popup' desync instead") for the final root-cause fix.

**Behavior after:** Interim state only; no timeout shipped in a release. The Ask-path wait is again unbounded (terminates on a real user answer or a UI close), and the popup-hiding desync is fixed at its source.

---

## 1. 2026-09-07 — Plugin hook stdin broken pipe no longer swallows hook stdout

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact/src/plugin/hooks.rs` (`run_process` stdin payload delivery; regression test `run_command_hook_stdin_broken_pipe_still_honors_stdout`); Claude Code plugin-hook compatibility (v1.1.26) |

**Symptom / motivation:** Command hooks that finish before the parent delivers the JSON payload on stdin — fast `printf`-style hooks that never read stdin, on a loaded machine or under parallel test load — intermittently closed the pipe first, so the payload write failed with `Broken pipe (os error 32)`. `run_process` treated *any* stdin-write failure as fatal, returning the default `Continue` output and silently discarding the hook's stdout: a `block` decision, `suppressOutput`, or `additionalContext` was lost (hooks fail open). This surfaced as flaky `plugin::hooks` tests (`run_command_hook_expands_plugin_root_env`, `run_command_hook_suppress_output_new_format`, …) failing roughly one run in ten.

**Decision:** The stdin payload is a best-effort delivery; a broken pipe (the hook already exited or closed stdin) is not a run failure — the hook's exit status and stdout remain authoritative. Only genuine non-`BrokenPipe` I/O errors still fail the hook. The deterministic regression test inflates the payload past the OS pipe buffer (64 KiB) so the write is guaranteed to break once a non-reading hook exits, independent of scheduling.

**Behavior after:** A hook that exits without reading stdin still has its stdout parsed (`decision` / `reason` / `additionalContext` / `suppressOutput` / legacy shape); only spawn failures, timeouts, non-zero exits, invalid JSON, and real I/O errors fall back to `Continue` with a warning.

---

## 1. 2026-09-07 — Subagent sticky tab: overview strip under the Log

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/protocol/src/agent.rs` (`SubagentRunSnapshot`, `SubagentStatusSnapshot`, `AgentUpdate::SubagentsChanged`), `crates/tact/src/subagent.rs` (`SubagentManager.known`, `note_started`, `ui_snapshot`, `MAX_SUBAGENT_SNAPSHOT`, `emit_subagents_changed`), `crates/tact/src/tool/subagent.rs` + `crates/tact-ui/src/driver.rs` (emit points), `crates/agent_tui_kit/src/{state,components,render}/subagent_panel.rs`, `crates/agent_tui_kit/src/render/sticky_host.rs` (two-domain host), `crates/tui/src/render/task_panel.rs` + `handlers/{mouse,normal}.rs`; design `docs/superpowers/specs/2026-09-07-subagent-sticky-tab-design.md`, plan `docs/superpowers/plans/2026-09-07-subagent-sticky-tab.md`; Ch 12, 23 |

**Symptom / motivation:** Background `run_in_background` subagent fan-out had no persistent status overview: each child's live stream renders in its own parent `spawn_subagent` tool card and the Log only shows the card of the invocation that returned, so "which children are still running / just finished / what did they say" was scattered across tool cards and `check_subagent`. The 2026-07-26 removal (`98a133f`) of the old Subagent sticky pane left a gap for a *status-level* surface; this re-implements it on the current component architecture **without** restoring the old `AgentUpdate::Subagent` wrapper or routing live detail into the sticky.

**Decision:** Mirror the Tasks pattern with a new `AgentUpdate::SubagentsChanged { runs }` full-snapshot event. `SubagentManager` keeps an in-memory `known` set of children started by the current process (not the whole `subagent_runs` table, which accumulates across sessions and orphan-repair noise), and emits after spawn start / sync+async finish / `cancel_subagent` tool / driver `CancelSubagent`. The TUI gains a `SubagentPanelComponent`/`SubagentPanelState` (kit) and a two-domain sticky host under the Log showing `[Tasks] [Subagent]` tab segments; each domain keeps its own visible/expanded/scroll state. The Subagent body groups runs Running → Completed → Failed → Cancelled with `{marker} {short-id} {summary-first-line} ⏱ {duration}`; rows are capped (`MAX_SUBAGENT_SNAPSHOT = 20` total, all Running preserved). Live detail still lives on the tool card / SubagentPopup.

**Behavior after:** Spawning or finishing any subagent in the current process updates the sticky (hidden when no domain is active; first appearance defaults expanded; collapses to one row and hides once collapsed with nothing running). Clicking a visible inactive tab switches the active domain and expands it; wheel/`jk` scroll the active domain. Subagents are never added to the main Log (one row = the tool card) and never duplicate into Tasks.

---

## 1. 2026-09-07 — OpenCode Go `x-opencode-session` header re-wired (session-bound)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/opencode.rs` (re-created; `endpoint_headers(base_url, session)`), `crates/tact_llm/src/openai/responses/mod.rs` (`OpenAiResponsesAdapter::set_session_id`, `ResponsesCompatConfig.opencode_session`, compact POST), `crates/tact_llm/src/openai/compatible/mod.rs` (`OpenAiAdapter::set_session_id`, `request_headers`, `CompatibleConfig::headers`), `crates/tact_llm/src/openai/compatible/multi_model.rs` (`ChatCompletionsAdapter::set_user_id` → forwards session), `crates/tact_llm/src/models.rs` (`fetch_model_ids`), `crates/tact_llm/src/client.rs` (`LlmProvider::set_user_id`); Ch 21 |

**Symptom / motivation:** OpenCode's published client requirements for OpenCode Go (`https://opencode.ai/zen/go/v1`) again ask every coding-agent client to identify itself and send a **stable session id** in `x-opencode-session` on each request (routing + prompt caching); clients without session support are listed as "Known Problematic". The whole mechanism that satisfied this (session-bound header + `tact/<version>` User-Agent, ed480ec `revert(llm)` on 09-05) had been removed while the header was optional, so OpenCode Go requests went out bare again.

**Decision:** Re-create the `opencode` helper module and re-attach the headers: the Responses SDK config (ordinary `/responses` via `create_byot` / `create_stream_byot`), the direct `/responses/compact` POST, the **Chat Completions transport** (`OpenAiAdapter::request_headers`, now session-aware), and the `/v1/models` picker fetch. The value is filled from the **Tact session id** only: `Agent::with_session` → `LlmProvider::set_user_id` → each adapter stores it (`OpenAiResponsesAdapter::set_session_id` for Responses; `ChatCompletionsAdapter::set_user_id` forwards to `OpenAiAdapter::set_session_id` for Chat Completions), so one Tact conversation (including a resumed session and each subagent's own child session) maps to exactly one OpenCode session/cache on either protocol. Unlike the 09-02 design there is **no per-`base_url` fallback token and no `TACT_OPENCODE_SESSION` env override** — the header is `session_id` verbatim or absent. Requests without a session (e.g. the `/v1/models` picker fetch, which predates `with_session`) omit `x-opencode-session` and send only the identifying `tact/<version>` `User-Agent`. Endpoint detection is unchanged from the pre-removal build (`opencode.ai` or a subdomain host).

**Behavior after:** conversation requests to an OpenCode Go endpoint over either the Responses or Chat Completions protocol carry `x-opencode-session` equal to the Tact session id plus a `tact/<version>` `User-Agent`; sessionless requests (models picker) omit the session header but still identify via the User-Agent; non-OpenCode endpoints carry no additional headers. This is a re-add of the design removed on 09-05 — the entry below stays for history.

---

## 1. 2026-09-06 — Subagent skill cards: isolated role injection via `skill`

| Field | Value |
|-------|-------|
| **Type** | feat |
| **Related** | `crates/tact/src/tool/subagent.rs` (`SubagentInput.skill`, `parse_skill_card_frontmatter`, `read_skill_card`, `list_skill_cards`, `format_skill_card_line`, `apply_skill_card`, `annotate_spawn_subagent_skill_catalog`), `crates/tact/src/tool/mod.rs` (`ToolRouter::set_tool_description` description overrides), `crates/tact-ui/src/{interactive,headless}.rs`; design `docs/superpowers/specs/2026-09-06-subagent-skill-cards-design.md`, plan `docs/superpowers/plans/2026-09-06-subagent-skill-cards.md`; Ch 12 §2.1 |

**Symptom / motivation:** Removing declarative agent definitions (`8c74f4e`) left subagents with no reusable way to attach a stable role/working method — every specialized worker persona had to be restated inline in `prompt`. Reusing the main-agent `SkillRegistry` was rejected because it would pollute the main agent's visible skill list and entangle the two systems.

**Decision:** Add an isolated, opt-in `skill: <name>` field to `spawn_subagent`. The handler reads `~/.tact/subagent/<name>.md` (key = file stem; frontmatter `description` is used for error listings and the discovery catalog) and appends its body as a `<skill>` block to the child's static system prompt, before `SubagentStart` hooks run. Names must be plain file stems (no separators / `.`/`..` / NUL), so a `skill` value cannot read outside the card directory. Unknown names fail the spawn and list available cards. No `ToolContext` state, no `SkillRegistry` involvement, and no tools/model/permission frontmatter semantics.

**Behavior after:** `spawn_subagent { prompt, skill: "reviewer" }` gives the child a stable role carried in its system prompt every turn (compaction-safe); omitting `skill` keeps the exact prior behavior (generic template + five-tool set). The card directory is physically isolated from the main agent's skills. At session start the `spawn_subagent` tool description is annotated with the available card list (single-line, 60-char-capped descriptions, capped at 30 cards) so the main agent can discover valid `skill:` names; an absent/empty card directory leaves the description unchanged. Symlinked cards are listed and readable consistently.

---

## 1. 2026-09-06 — Declarative subagent definitions removed

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | deleted `crates/tact/src/agent_def.rs`; `crates/tact/src/tool/subagent.rs` (`SubagentInput.agent`, `resolve_agent_model`, spawn prompt/permission/model/toolset overrides), `crates/tact/src/tool/registry.rs` (`subagent_toolset_for`, `allowed_tool_names`, filtered toolset builder), `crates/tact/src/tool/mod.rs` (`ToolContext.agent_registry`), `crates/tact/src/consts.rs` (`TactPath::agents_dir`), plugin feature bookkeeping (`InstalledPlugin.agent_count`, `PluginFeatures.agent_count` in `crates/tact/src/plugin/{model,install}.rs`), `crates/tact-ui/src/plugin_cli.rs`, `crates/tui/src/widgets/state/app/extensions.rs`, `crates/agent_tui_kit/src/i18n.rs` (`plugin_list_header`); Ch 7, 12, 21 |

**Symptom / motivation:** `spawn_subagent` carried a declarative "agent definition" path — Markdown+YAML-frontmatter files under `<workdir>/.tact/agents/*.md` and installed-plugin `agents/*.md` (namespaced `plugin:<name>`) — that replaced the child system prompt and could override its tool set, model, and permission mode. The surface was large for the value: a frontmatter parser, a shared registry (`Arc<Mutex>` with local-name ambiguity resolution), a second toolset builder (`subagent_toolset_for` + Claude-name mapping) guarded by a fail-closed empty-router check, a third model-override layer on top of `[agent.subagent]`, install-time `agent_count` bookkeeping, and two plugin-list UIs — all to run a worker equally well served by a plain prompt on the fixed five-tool set.

**Decision:** Remove the feature end to end. `spawn_subagent` always uses the generic static system prompt ("You are a coding subagent at …") and `subagent_toolset()`; `SubagentInput.agent`, `resolve_agent_model`, the whole `agent_def` module, the `ToolContext.agent_registry` field, and the `subagent_toolset_for`/`allowed_tool_names`/filtered-builder trio are deleted. Plugins no longer count or advertise `agents/*.md`: `agent_count` is dropped from `InstalledPlugin`/`PluginFeatures`, `tact plugin list`, and the `/plugin` table (column removed from the localized `plugin_list_header`); plugin `SubagentStart` hooks and the skill/command/hook/MCP plugin features are unaffected. An `agents/`-only plugin now has no installable feature and is rejected at install time.

**Behavior after:** `spawn_subagent` accepts only `prompt`/`description`/`run_in_background`/`max_turns`/`resume`/`worktree`; every subagent runs on the fixed five-tool set with the generic prompt. Per-worker definition overrides are gone; the `[agent.subagent]` config block and `/model-subagent` picker still set the subagent's provider/model globally. Plugin feature summaries show `skills/commands/hooks/mcp` only.

---

## 1. 2026-09-05 — OpenCode `x-opencode-session` header wiring removed

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | removed `crates/tact_llm/src/opencode.rs`; `crates/tact_llm/src/openai/responses/mod.rs` (`OpenAiResponsesAdapter`, `ResponsesCompatConfig`), `crates/tact_llm/src/openai/compatible/mod.rs` (`CompatibleConfig::headers`), `crates/tact_llm/src/models.rs` (`fetch_model_ids`), `crates/tact_llm/src/client.rs` (`LlmProvider::set_user_id`); Ch 21 |

**Symptom / motivation:** OpenCode's hosted endpoints no longer require an `x-opencode-session` header (nor a custom `tact/<version>` User-Agent), so the whole mechanism that detected `opencode.ai` base URLs and attached the header to every request was dead weight — and it forced plumbing the Tact session id (`Agent::with_session` → `LlmProvider::set_user_id` → `OpenAiResponsesAdapter::set_session_id`) purely to feed one header value.

**Decision:** Remove the OpenCode session logic entirely: delete the `opencode` helper module and stop attaching `x-opencode-session`/`tact/<version>` on the Responses SDK config, the direct `/responses/compact` POST, the Chat Completions config, and the `/v1/models` picker fetch. Drop the `session_id`/`opencode_session` fields and `set_session_id` from the Responses adapter, remove the `OpenAiResponses` arm of `LlmProvider::set_user_id`, and delete the `TACT_OPENCODE_SESSION` env override.

**Behavior after:** requests carry no `x-opencode-session` header (and no custom OpenCode User-Agent) on any endpoint; `opencode.ai` endpoints are treated like any other OpenAI-compatible base URL. DeepSeek `user_id` KV-cache isolation through the Chat Completions adapter is unaffected.

---

## 1. 2026-09-05 — Compatible `/responses` endpoints stop replaying historical reasoning items

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact_llm/src/openai/responses/convert.rs` (`ResponsesRequestPolicy`, `create_response_with_policy`), `crates/tact_llm/src/openai/responses/mod.rs` (`OpenAiResponsesAdapter::with_replay_prior_reasoning`, `is_official_openai_base_url`, compact POST); task #58; Ch 22 §6.2/§6.2.3 |

**Symptom / motivation:** Every `/responses` request replayed the full historical `reasoning` items (persisted signatures with opaque encrypted payloads) into `input`. Official OpenAI needs that for turn continuation, but compatible endpoints (OpenCode Go, custom OpenAI-compatible proxies) regenerate reasoning each turn, so replaying every previous chain of thought was pure input-token waste — measured at ~17% of request bytes (up to ~47% of tokens on DeepSeek-style endpoints).

**Decision:** Add a per-adapter `replay_prior_reasoning` policy. `OpenAiResponsesAdapter::new` derives the default from the base URL (`is_official_openai_base_url`: `api.openai.com` / Azure OpenAI → replay; everything else → drop); `with_replay_prior_reasoning` overrides it. `create_response` became `create_response_with_policy(request, provider_state, compact_threshold, ResponsesRequestPolicy { native_web_search, replay_prior_reasoning })`. The reasoning signature is still decoded on every path so `fc_*` function-call item ids stay attached to their `function_call` items; only the standalone `reasoning` payload is omitted. When replay is disabled the persisted state baseline is filtered of stale `reasoning` items before the body is built, and the returned `input_items` exclude reasoning, so states persisted by an older build self-heal on the first request.

**Behavior after:** official OpenAI (and Azure OpenAI) Responses requests replay prior reasoning exactly as before; any other base URL drops historical reasoning items from ordinary requests, explicit `/responses/compact` bodies, and the persisted state baseline, cutting ~17% of input bytes while preserving function-call identity. An explicit override exists for endpoints whose semantics differ from their host name.

---

## 1. 2026-09-02 — OpenCode `x-opencode-session` is bound to the Tact session id (cache isolation)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/opencode.rs` (`endpoint_headers(base_url, session)`), `crates/tact_llm/src/openai/responses/mod.rs` (`OpenAiResponsesAdapter::set_session_id`, `ResponsesCompatConfig.opencode_session`, compact POST), `crates/tact_llm/src/client.rs` (`LlmProvider::set_user_id`), `crates/tact/src/agent/mod.rs` (`Agent::with_session`); Ch 21 |

**Symptom / motivation:** The first OpenCode fix sent a per-process, per-`base_url` token as `x-opencode-session`. OpenCode uses that header as the **session key that distinguishes its per-conversation caches**, so two different Tact sessions sharing one token would share (and pollute) each other's OpenCode cache, while resuming a Tact session would not resume the same OpenCode cache.

**Decision:** Bind the header to the Tact session id. `Agent::with_session` already forwards the session id to the client via `LlmProvider::set_user_id` (the hook DeepSeek uses for KV-cache isolation); the OpenAI Responses adapter now stores it (`set_session_id`) and the SDK config / compact POST emit `x-opencode-session = <session id>`. Requests without a session (the `/v1/models` picker fetch) still fall back to the per-`base_url` token; `TACT_OPENCODE_SESSION` pins that fallback only.

**Behavior after:** one Tact session (including a resumed one, which reuses the same session id) maps to exactly one OpenCode session/cache; different Tact sessions — main agent, each subagent with its own child session id — get distinct `x-opencode-session` values, so OpenCode caches are isolated per conversation.

---

## 1. 2026-09-02 — OpenCode Go endpoints send `x-opencode-session` + a real User-Agent

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact_llm/src/opencode.rs` (new), `crates/tact_llm/src/openai/responses/mod.rs` (`ResponsesCompatConfig::headers`, compact POST), `crates/tact_llm/src/openai/compatible/mod.rs` (`CompatibleConfig::headers`), `crates/tact_llm/src/models.rs` (`fetch_model_ids`); Ch 21 |

**Symptom / motivation:** Requests to OpenCode Go (`https://opencode.ai/zen/go/v1`) carried no `x-opencode-session` header and only reqwest's generic `User-Agent`, so OpenCode could not correlate/optimize the session and reported them as "Unknown client"; starting 09/06 requests missing the header may error.

**Decision:** Add an `opencode` helper module that detects OpenCode endpoints (`opencode.ai` or a subdomain) and returns an `x-opencode-session` value that is stable per process and per `base_url`, plus a `tact/<version>` `User-Agent`. The headers are attached on every path that talks to the endpoint: the Responses SDK config (`create_byot` / `create_stream_byot`), the direct `/responses/compact` POST, the Chat Completions config (defensive), and the `/v1/models` picker fetch. `TACT_OPENCODE_SESSION` pins the session value when set.

**Behavior after:** every request to an OpenCode Go endpoint carries a stable `x-opencode-session` header and an identifying `tact/<version>` `User-Agent`; other endpoints are unaffected (empty header map).

---

## 1. 2026-09-02 — Remove nested subagent spawns (depth-0 restored)

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | `crates/tact/src/tool/registry.rs` (`subagent_toolset` back to 5 tools), `crates/tact/src/tool/mod.rs` (`ToolContext.subagent_depth` removed), `crates/tact/src/tool/subagent.rs` (`MAX_SUBAGENT_DEPTH` removed); revert of `6c32665e`; Ch 12 |

**Symptom / motivation:** The nested-spawn feature (commit `6c32665e`, depth-limited to `MAX_SUBAGENT_DEPTH = 3`, 9-tool subagent set) was shipped on the same day, but the design proved too complex for the value: subagents now carried `subagent_depth` state through `ToolContext`, the router had to map Claude `Task` names, and each child needed depth propagation — all to let a worker spawn a worker.

**Decision:** Revert the nested-spawn commit. `subagent_toolset()` is back to exactly five tools (`bash`, `read_file`, `write_file`, `edit_file`, `sleep`), `spawn_subagent`/`check_subagent`/`wait_subagent`/`cancel_subagent` are main-toolset-only again, `ToolContext.subagent_depth` and `MAX_SUBAGENT_DEPTH` are deleted, and `allowed_tool_names` drops the Task/Check/Wait/Cancel mappings. The resume 24h expiry and the worktree improvements (branch cleanup, run validation/audit, index reconciliation) are unaffected.

**Behavior after:** subagents cannot spawn nested subagents (same depth-0 contract as before 2026-09-02); the main agent keeps full subagent tool surface; declarative `tools:` lists only narrow the five-tool set.

---

## 1. 2026-09-02 — Close async-subagent leftovers: resume expiry, worktree branch cleanup, index reconciliation, run validation/audit

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/tool/subagent.rs` (`RESUME_EXPIRY_HOURS`), `crates/tact/src/worktree/mod.rs` (`new` orphan repair, `remove` branch cleanup, `run` validation + audit), `crates/tact/src/tool/worktree.rs`; Ch 12, 15 |

**Symptom / motivation:** Four design-deferred leftovers remained after the async-subagent follow-ups: (1) `resume` had no expiry policy ("TBD" in the 2026-08-26 design), so a weeks-old session could be resumed with stale context; (2) `worktree_remove` always left the backing `wt/<name>` branch, requiring a manual merge or `git branch -D`; (3) manual `git worktree remove`/`prune` left stale DB records forever (index drift); (4) `worktree_run` bypassed `validate_shell_command` and was not audit-logged.

**Decision:** (1) `resume` rejects a target whose `finished_at` is older than `RESUME_EXPIRY_HOURS = 24` (Claude's 24h expiry). (2) `WorktreeManager::remove` now runs `git branch -d wt/<name>` after `git worktree remove` — only a fully-merged branch is deleted; unmerged branches are kept and the outcome is reported/audited. (3) `WorktreeManager::new` repairs orphans: any tracked lane whose path is missing is dropped from the table and logged `worktree.stale-removed`. (4) `WorktreeManager::run` calls `validate_shell_command` (same gate as `bash`) and appends `worktree.run <name> <command>` to the audit log.

**Behavior after:** resume expires after 24h; `worktree_remove` auto-deletes only merged branches; the worktree index reconciles with git on startup; `worktree_run` blocks high-risk commands and every invocation is audit-logged.

---

## 1. 2026-09-02 — Async-subagent P1 reliability fixes (wake-up race, cancellation status, resume validation, sync lifecycle, headless semantics)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tact-ui/src/driver.rs` (`run_command_loop_with_account` + `spawn_wakeup_task`), `crates/tact/src/tool/subagent.rs` (`spawn_subagent`, `terminal_success`); Ch 12 |

**Symptom / motivation:** Five P1 reliability gaps in the async-subagent path. (1) **Wake-up race** — the driver dropped `SubagentFinishedNotification` whenever a turn was in flight, so a result that landed in the gap between the final queue drain and turn exit was never re-injected (the parent stayed silent until the next manual turn). (2) **Cancellation status** — the async completion task emitted `AgentUpdate::SubagentFinished` with the raw `agent_loop` result, so a cancelled child that exited cleanly was reported `success: true`. (3) **Resume validation** — `resume` blindly reused any id: resuming a still-`Running` child raced the session, and resuming an unknown id silently minted a fresh child instead of a follow-up. (4) **Synchronous child lifecycle** — sync children registered a cancel handle but never wrote a `subagent_runs` row, so `check_subagent`/`cancel_subagent` were blind to them and a failed sync spawn left a stale `Running` row. (5) **Headless semantics** — `run_in_background` in headless spawned a detached child that was then cancelled at exit, silently discarding its work.

**Decision:** (1) the driver loop now `select!`s on the in-flight `JoinHandle` and `user_cmd_rx`, retaining a `pending_subagent_wakeup` flag and submitting the wake-up turn as soon as the active turn completes (new `spawn_wakeup_task` helper; `SubmitTask` clears the flag since the new turn drains the queue itself). (2) a `terminal_success(success, cancelled) = success && !cancelled` helper is used for both the queued `SubagentResult` and the `SubagentFinished` event. (3) resume validates via `SubagentManager::get`: a still-`Running` target or an unknown id bails before any spawn work. (4) `manager.start` moved above the sync/async split and the sync path now records `Completed`/`Failed`/`Cancelled` on exit (also unregisters the handle and releases the lock before propagating an error). (5) `run_async` requires `ctx.ui_tx.is_some()`; without an interactive channel (headless) `run_in_background` degrades to synchronous with a `warn!`, so the summary still reaches the parent.

**Behavior after:** a subagent result is never lost to the wake-up gap; cancelled children read as unsuccessful in both the queue and the card; resume rejects running/unknown targets; sync and async children are uniformly visible to `check_subagent`/`cancel_subagent`/`wait_subagent`; headless `run_in_background` completes synchronously instead of being cancelled at exit.

---

## 1. 2026-09-02 — Async-subagent follow-ups: `wait_subagent` + `worktree_remove` + concurrent popups

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/subagent.rs` (`SharedSubagentManager::wait` / `get`), `crates/tact/src/tool/subagent.rs` (`WaitSubagentTool`), `crates/tact/src/tool/worktree.rs` (`WorktreeRemoveTool`), `crates/tact/src/worktree/mod.rs` (`WorktreeManager::remove`), `crates/tact/src/store/worktree_store/` (`remove_worktree`), `crates/tact/src/tool/registry.rs`, `crates/tui/src/widgets/state/{mod.rs,app/popups.rs,app/config.rs,app/construct.rs}`, `crates/tui/src/{handlers,render}`; Ch 12 |

**Symptom / motivation:** The 2026-08-26 async-subagent design shipped `run_in_background`, `check_subagent`, `resume`, and cancel, but three follow-ups remained open: (1) the parent could only learn a running child's outcome by calling `check_subagent` across multiple LLM turns — wasteful; (2) isolated worktree lanes (`subagent-<child_id>`) had no removal surface and leaked until a manual `git worktree remove`; (3) the TUI held a single `Option<SubagentPopup>` slot, so opening a second concurrent subagent's transcript dropped the first one's scroll/selection.

**Decision:** (1) `wait_subagent { child_id, timeout_ms? }` — a new Read tool that polls `subagent_runs` (250 ms interval) until the child reaches `Completed`/`Failed`/`Cancelled` or times out (default 60 s) and returns the summary; the Codex `wait_agent` analog (`SubagentManager::wait` / `get` added). (2) `worktree_remove { name }` — runs `git worktree remove` (no `--force`, so a dirty tree fails), deletes the tracking row, appends an audit event, and leaves the backing `wt/<name>` branch recoverable; it refuses a `subagent-<id>` lane whose run is still `Running` (`WorktreeStore::remove_worktree` + `WorktreeManager::remove` added). (3) TUI multi-popup — `App.subagent_popup: Option<_>` became `subagent_popups: HashMap<tool_id, _>` + `active_subagent_popup: Option<tool_id>`; `open_subagent_popup` inserts-or-reuses each card's entry so switching between concurrent subagents preserves scroll/selection/cached layout.

**Behavior after:** the parent can spawn N background subagents and `wait_subagent` each (no more turn-burning `check_subagent` polling); isolated lanes are cleanable through a tool; concurrent subagent transcripts no longer clobber each other's popup state.

---

## 1. 2026-08-30 — 子代理取消（`cancel_subagent` 工具 + `/subagent_cancel` + tool 卡片 [Cancel] 按钮）

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/subagent.rs`（`register_cancel_handle` / `request_cancel` / `unregister_cancel_handle`）、`crates/tact/src/tool/subagent.rs`（`CancelSubagentTool`、spawn 注册/注销 handle、取消感知结束路径）、`crates/tact/src/tool/registry.rs`、`crates/protocol/src/agent.rs`（`UserCommand::CancelSubagent`）、`crates/tact-ui/src/driver.rs`、`crates/tui/src/handlers/{mod,mouse}.rs`（`/subagent_cancel`、按钮点击）、`crates/agent_tui_kit/src/{components/tool.rs,render/log.rs,state/tool_state.rs,state/mouse_state.rs,i18n.rs}`（`parse_async_launched`、`SubagentCancelButton`、`subagent_child_id`）；Ch 12 |

**Symptom / motivation:** 运行中的后台子代理没有任何取消入口：父级 `/cancel` 只取消主任务（每次 `Agent::new` 为子代理新建独立 cancel_flag，父进程拿不到句柄）；`SubagentManager::cancel` 只能事后改 DB 状态，无法中止运行中的子代理；异步子代理 detach 后只能等 `max_turns` 或自然结束。

**Decision:** 建立协作取消链路：`SubagentManager` 增加内存 cancel-handle 注册表（`child_id → Arc<AtomicBool>`）；`spawn_subagent` 在 `Agent::new` 后注册子代理的 `runtime.cancel_flag`，结束（同步/异步）时注销；新增 `cancel_subagent` 工具（`request_cancel` 翻转标志 + 标记记录），注册进主 toolset；protocol 新增 `UserCommand::CancelSubagent`，driver 直接操作 manager（不依赖父 agent 空闲）；TUI 新增 `/subagent_cancel <child-id>` slash 命令与运行中子代理卡片上的 `[Cancel]` 按钮（`parse_async_launched` 从 `async_launched { id }` 结果提取 child_id，渲染层返回按钮 rect，mouse 点击发送命令）。异步结束路径检测标志：被取消的运行标为 `Cancelled`（而非 Completed），summary 前缀 `(cancelled by user)`。

**Behavior after:** 运行中的后台子代理可通过三种入口取消：`cancel_subagent { child_id }` 工具（模型可调用）、`/subagent_cancel <child-id>`（用户）、live 卡片 `[Cancel]` 按钮（鼠标）。子代理循环在下一个检查点协作退出，运行记录标为 Cancelled，结果回注时 success=false。**父级退出联动**：driver 循环收尾与 headless 运行结束都会调用 `SubagentManager::cancel_all()`，翻转所有存活子代理的取消标志，避免后台子代理成为孤儿。

---

## 1. 2026-08-29 — Claude marketplace 插件全功能兼容（skills / commands / agents / hooks / MCP）

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/plugin/{install,model,store,hooks}.rs`, `crates/tact/src/skill/mod.rs` (`load_plugin_commands`), `crates/tact/src/mcp/mod.rs` (`installed_plugin_mcp_servers`, `McpProjectConfig`), `crates/tact/src/agent_def.rs` (declarative agents), `crates/tact/src/tool/{subagent,registry}.rs` (`agent` field, `subagent_toolset_for`), `crates/tact/src/hook/mod.rs` (`UserPromptSubmit`), `crates/tact/src/agent/mod.rs` (`apply_user_prompt_hooks`, `with_post_tool_hook`), `crates/tact-ui/src/{interactive,headless}.rs`, `crates/tui/src/widgets/state/app/extensions.rs`; design `docs/superpowers/specs/2026-08-29-claude-plugin-compat-design.md`, plan `docs/superpowers/plans/2026-08-29-claude-plugin-compat.md`; Ch 2, 8, 9, 12, 21, 23 |

**Symptom / motivation:** Tact 的插件只消费 `skills/` 一项，且安装校验硬性要求 `skills/*/SKILL.md`，导致官方 claude-plugins-official 中 15+ 无 skills 的插件（LSP 文档类、`commit-commands`、`code-review` 等）无法安装；`commands/*.md`、`agents/*.md`、plugin.json hooks、`.mcp.json` 全部被忽略（ponytail 的 hooks 完全没跑）。

**Decision:** 以 Claude Code 插件契约为准做五类兼容：
1. **安装/清单** — 校验放宽为"至少一种受支持功能"（skills/commands/agents/hooks/mcp）；完整解析 plugin.json（name/description/version/author/hooks/mcpServers）；`InstalledPlugin` 新增 `command_count`/`agent_count`/`has_hooks`/`has_mcp`（serde default，旧记录兼容）。
2. **Commands** — `commands/*.md` 加载为 `plugin:<name>` 技能（Claude：与 skills 加载方式相同），同一插件内命令覆盖同名技能；frontmatter 解析 `argument-hint`/`allowed-tools`/`model`（v1 不强制）。
3. **MCP** — 扫描已安装插件缓存的 `.claude-plugin/plugin.json` `mcpServers` 与插件根 `.mcp.json`，服务器命名 `plugin__<id>__<server>`；`http`/`url` 类型跳过并告警（客户端仅 stdio）。
4. **Agents** — 新 `agent_def` 注册表加载 `.tact/agents/*.md`（原名）与插件 `agents/*.md`（`plugin:<name>`）；`spawn_subagent` 新增 `agent` 字段：定义正文作 system prompt，`tools` 过滤子代理工具集（Read/Glob/Grep→read_file, Bash→bash, Edit→edit_file, Write→write_file, Sleep→sleep），`model`/`permissionMode` 覆盖（Auto 保持粘性）。
5. **Hooks** — 新 `plugin/hooks.rs`：解析 Claude hooks JSON（matcher/command/commandWindows/timeout/statusMessage/async），`run_command_hook` 以 `sh -c` 执行并注入 `CLAUDE_PLUGIN_ROOT`/`CLAUDE_PROJECT_DIR`，stdin JSON 负载、stdout 双格式解析（`decision` 与 `hookSpecificOutput`）；失败/超时/非法 JSON → warning + Continue（fail-open）。`Hook` 新增 `UserPromptSubmit`（agent_loop 入口对用户消息追加 `additionalContext`）；`SubagentStart` 作为独立 trait 存于 `ToolContext.subagent_start_hooks`（spawn 路径无 LoopState），在 `spawn_subagent` 中注入子代理 system prompt。

**Behavior after:** 官方 marketplace 全部 39 个插件可安装；`/plugin:commit` 等命令即装即用；带 agents 的插件可通过 `spawn_subagent agent=…` 使用；带 hooks 的插件（如 ponytail、官方 hookify/security-guidance 等）在会话启动 / 用户提交 / 工具调用前后 / 子代理启动时执行命令 hook；插件 `.mcp.json` stdio 服务器出现在 `mcp__` 工具集；`tact plugin list` 与 TUI `/plugin list` 显示功能摘要。

**Follow-up fixes (2026-08-30):** hooks 默认发现路径 `hooks/hooks.json`（manifest 缺省时回退，官方 6 个 hooks 插件受益）；声明式 agents 的 `model` 别名处理（`inherit` 不覆盖、`sonnet/opus/haiku` 警告忽略、具体 id 透传）；`tools:` 全部无法映射时报错而非回退默认五件套（防权限扩大）；UserPromptSubmit `Block` 真正阻止提交、SubagentStart `Block` 传播到 `spawn_subagent` 使其失败；SubagentStart 输入补 `agent_type` 字段（ponytail matcher 依赖）；`timeout: 0` 表示不设超时；新格式 `suppressOutput` 解析；SessionStart 纯文本输出显式告警。

**Limitations (v1):** Python SDK `tools/`、http/url MCP、`Notification`/`Stop`/`SubagentStop`/`PreCompact`/`PostCompact`/`SessionEnd` 事件、SessionStart `systemPrompt` 输出、skills `allowed-tools`/`model` 强制执行均不支持（见设计文档 §2）。

---

## 1. 2026-08-29 — Slash & select popups scroll long lists (selection always visible + mouse wheel)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `crates/tui/src/render/popups/slash_command.rs` (scroll window/offset), `crates/tui/src/widgets/state/slash_command.rs` (`step_slash_selection`), `crates/agent_tui_kit/src/widgets/select_popup_widget.rs` (`select_popup_layout`, footer), `crates/agent_tui_kit/src/render/popups/select.rs`, `crates/agent_tui_kit/src/render/popups/system_prompt_popup.rs` (session-stats footer), `crates/agent_tui_kit/src/i18n.rs` (footer strings), `crates/tui/src/render/popups/select.rs` (mouse area), `crates/tui/src/handlers/mouse.rs` (wheel routing), `crates/tui/src/handlers/insert.rs` (Up/Down reuse), `crates/agent_tui_kit/src/state/mouse_state.rs` (`slash_popup_area`, `select_popup_area`); Ch 23 |

**Symptom / motivation:** With a long list the popup appeared not to scroll. The slash popup's scroll window was sized `max_visible + 2` regardless of the popup's real content height (`area.height - 2`); on short terminals the `List` widget clipped the bottom rows and the selected row was anchored at window index `max_visible - 1` — below the visible content — so Up/Down moved the selection off-screen and the visible rows looked frozen. The select popup (`/model`, permission, ask_user) had the same failure: its List chunk was sized `Constraint::Length(option_count)` (the full option count) even when the popup height was capped, so the selected row sat below the popup border and the list never scrolled. The mouse wheel over either popup also had no effect: neither popup is an overlay popup, so wheel events fell through to the log panel behind it.

**Decision:** (1) Both popups now size their scroll window from the area that actually fits (`min(rows, max_visible + 2, area.height - 2)` for slash; a shared `select_popup_layout(state, area, fg)` helper for select) so the window and the popup height always agree and nothing is clipped; the offset keeps the selected row inside the window (pinned near the bottom with ~2 context rows below once the list overflows), preserving the previous anchor at normal sizes while guaranteeing the highlight is always on screen. (2) `render_slash_command_popup` and the select wrapper take `&mut App`, record their rects in `MouseState::slash_popup_area` / `select_popup_area`, and run every frame (clearing the area when inactive); `handle_mouse_event` routes `ScrollUp`/`ScrollDown` over those rects to the selection. (3) Slash Up/Down and the wheel share one `App::step_slash_selection(delta)` helper; select reuses `SelectPopup::move_up`/`move_down`. (4) The select popup gained a bottom-border navigation hint (same style as code/mermaid popups: keys in accent, labels muted, centered in `title_bottom`) — `↑↓/j/k` select, `Enter` confirm, `Esc` cancel, plus `Space` toggle for multi-select; the popup width widens to fit the hint, replacing the old hardcoded inner "Space toggle · Enter confirm" row (which also freed one content row for options). (5) The `/stats` session-stats popup (a `SystemPromptPopup` reused from `/view-system-prompt`) gained the same bottom-border footer (`j/k` scroll · `Esc` close), matching every other scrollable popup.

**Behavior after:** The selected command/option is always visible at any terminal height; a long slash or select list scrolls with Up/Down or the mouse wheel (wheel over the popup moves the selection instead of scrolling the log behind it); a closed popup clears its recorded mouse area; the select popup footer shows the navigation keys at the bottom border; the `/stats` session-stats popup shows the same `j/k scroll · Esc close` footer.

---
## 2. 2026-08-27 — Subagent worktree isolation (`worktree: true`) + same-wave fan-out for isolated spawns

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/tool/subagent.rs` (`SubagentInput.worktree`, `ensure_subagent_worktree`), `crates/tact/src/agent/tool_dispatch.rs` (`tool_resources_for`), `crates/tact/src/worktree/mod.rs` (`get`), `crates/tact/src/tool/mod.rs` (re-export); plan `docs/superpowers/plans/2026-08-27-subagent-worktree-isolation.md`; Ch 12 |

**Symptom / motivation:** `spawn_subagent` always shared the parent's `work_dir` and stayed `ResourcePolicy::Barrier`, so multi-subagent fan-out was either serial (sync) or only parallel via `run_in_background` + `tokio::spawn`. The 2026-08-26 async-subagent design review named the follow-up: same-wave fan-out of blocking subagents becomes safe once each subagent has a scoped filesystem. The tool description ("shares the filesystem") gave the model no way to request isolation.

**Decision:** (1) `SubagentInput` gains `worktree: Option<bool>`; when `true`, the handler creates (or, on `resume`, reuses) a git worktree lane `subagent-<child_id>` (branch `wt/subagent-<child_id>`) synchronously — failures surface immediately — and points the child's `ToolContext.work_dir` at the lane. The summary gains a `(worktree: <name> at <path>)` note in both sync and async returns. (2) `execute_tool_call` resolves resources per invocation (`tool_resources_for`): a worktree-isolated `spawn_subagent` maps to `ToolResources::independent()` instead of the static `Barrier`, so isolated spawns may fan out in the same wave; non-isolated spawns stay `Barrier`. (3) `WorktreeManager`/`SharedWorktreeManager` expose `get(name)` for resume reuse. Tool description rewritten to state the sync/async/worktree strategy.

**Behavior after:** `spawn_subagent` with `worktree: true` runs in an isolated lane based on repo-root `HEAD`; the child's `bash`/`read_file`/`write_file`/`edit_file` resolve against the lane; lanes persist after completion (inspect via `worktree_status`/`worktree_run`, remove via `git worktree remove`). A non-git `work_dir` fails the spawn with a clear error. A worktree is an organizational boundary, not an OS sandbox — `bash` can still reach outside the lane. Per-agent declarative definitions (`.tact/agents/*.md`) and a worktree-removal tool remain deferred.

---

## 3. 2026-08-27 — Subagent permission inheritance + async `run_in_background` + result re-injection

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/permission/mod.rs` (`PermissionSnapshot`), `crates/tact/src/tool/subagent.rs`, `crates/tact/src/subagent.rs`, `crates/tact/src/store/subagent_store/`, `crates/tact/src/agent/mod.rs` + `tool_dispatch.rs`, `crates/protocol/src/agent.rs`, `crates/tact-ui/src/driver.rs`, `crates/agent_tui_kit/src/components/tool.rs`, `crates/tui/src/…`; design `docs/superpowers/specs/2026-08-26-async-subagent-design.md`, review `…-design-review.md`, plan `docs/superpowers/plans/2026-08-26-async-subagent.md`; Ch 12 |

**Symptom / motivation:** `spawn_subagent` was a single synchronous, blocking tool that always built the child's `PermissionManager` in `PermissionMode::Default` — a `Plan` (read-only) parent could spawn a `Default` child that wrote files, escaping the read-only intent. There was no way to run a subagent in the background, cap its turns, resume it, or query its lifecycle after a restart.

**Decision:** (1) `PermissionSnapshot { mode, always_allowed_tools, settings }` + `PermissionManager::snapshot()`/`from_snapshot()`; `execute_tool_call` stamps the snapshot (and the pending-results queue) onto `ToolContext` after phase-1 pre-flight, and `spawn_subagent` builds the child from it (Claude-style inheritance; `Default`→`Default`, `Plan`→`Plan`, `Auto`→`Auto`, denial counter resets; orphan/test contexts fall back to `Default`). (2) `SubagentInput` gains `run_in_background` / `max_turns` / `resume`; async spawns a detached task, returns `async_launched { id }`, and on completion transitions a `subagent_runs` row and enqueues a `SubagentResult` re-injected as a `<subagent-finished>` message drained before the next LLM call. (3) `AgentUpdate::SubagentFinished` finalizes the keep-live card (with transcript carry-over); `UserCommand::SubagentFinishedNotification` + driver wake-up turn let an idle parent resume; `check_subagent` exposes the persisted lifecycle. `spawn_subagent` stays `ResourcePolicy::Barrier` (background parallelism comes from `tokio::spawn`, not wave scheduling).

**Behavior after:** subagents inherit the parent's permission context (fixing the read-only escape); `run_in_background` returns an async handle and re-injects the child summary into the parent transcript; `max_turns` bounds runaway children; `resume` reuses a finished child session; `check_subagent` reads `subagent_runs`; orphan `running` rows are repaired to `failed` on startup. Per-agent `permissionMode` override remains deferred (no declarative `.tact/agents/*.md` yet); worktree isolation shipped separately (entry above).

---

## 2. 2026-08-24 — `AgentUpdate` drops the embedded oneshot; select requests use `request_id` + `UiResponse`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/protocol/src/agent.rs`, `crates/tact/src/ui_responder.rs` (new), `crates/tact/src/tool/ask_user.rs`, `crates/tact/src/tool/subagent_ui.rs`, `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact-ui/src/driver.rs`, `crates/agent_tui_kit/src/state/select_popup.rs`, `crates/tui/src/handlers/select.rs`; Ch 25 |

**Symptom / motivation:** `AgentUpdate::RequestSelect` / `RequestMultiSelect` embedded a `tokio::sync::oneshot::Sender`, coupling the protocol enum to an in-process transport handle. The enum could not derive `Clone`/`Serialize`, each request had exactly one in-process responder, and every new "ask the UI" shape would need its own variant carrying a sender.

**Decision:** requests carry a pure-data `request_id: u64`; the TUI answers over the reverse channel with `UserCommand::UiResponse` (`Select` / `MultiSelect`). A new shared `UiResponder` allocates globally-unique ids and routes responses to the exact waiter. Parent and subagents clone the same inner state, so a subagent request forwarded through the parent's tagged channel still resolves. The driver handles `UiResponse` without awaiting the in-flight task and calls `UiResponder::shutdown` on exit so a vanished UI unblocks any waiter.

**Behavior after:** `AgentUpdate` is transport-agnostic data (no embedded handle; free to derive `Serialize`/`Clone` later); select responses route by `request_id` across the parent and all subagents; a closed UI unblocks the agent loop instead of deadlocking it.

---

## 2. 2026-08-24 — Plugin update command: `tact plugin update` / `/plugin update`

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/plugin/{mod,install}.rs`, `crates/tact/src/config/cli.rs`, `crates/tact-ui/src/plugin_cli.rs`, `crates/tact-ui/tests/plugin_cli_tests.rs`, `crates/tui/src/handlers/plugin.rs`, `crates/tui/src/widgets/state/app/extensions.rs`, `crates/agent_tui_kit/src/i18n.rs`; Ch 2 § plugin skills, Ch 23 §7 |

**Symptom / motivation:** installed plugins are revision-locked, so after an upstream marketplace release the only way to move forward was to uninstall and reinstall — losing the marketplace source association implicitly and requiring the exact name again.

**Decision:** add an `Update` request to the plugin worker. `PluginInstaller::update` validates the plugin id, refreshes the owning marketplace (falling back to the previously fetched catalog on transient refresh failure), resolves the latest revision, and either reports up to date (same revision — cache untouched) or installs the new revision and prunes the old revision's cache directory. The CLI gains `tact plugin update <name>`; the TUI slash command gains `/plugin update <name>`. Updates refresh shared skills like install/uninstall/reload.

**Behavior after:** `tact plugin update <name>` and `/plugin update <name>` bring an installed plugin to its marketplace's latest revision and remove the previous revision's cache; up-to-date plugins report as such without touching the cache; missing plugins produce an error.

---

## 2. 2026-08-24 — Plugin uninstall command: `tact plugin uninstall` / `/plugin uninstall`

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | `crates/tact/src/plugin/{mod,install,store}.rs`, `crates/tact/src/config/cli.rs`, `crates/tact-ui/src/plugin_cli.rs`, `crates/tact-ui/tests/plugin_cli_tests.rs`, `crates/tui/src/handlers/plugin.rs`, `crates/tui/src/widgets/state/app/extensions.rs`, `crates/agent_tui_kit/src/i18n.rs`; Ch 2 § plugin skills, Ch 23 §7 |

**Symptom / motivation:** plugins could be installed, listed, and reloaded but never removed; uninstalling required hand-editing `installed.json` and deleting the cache directory manually.

**Decision:** add an `Uninstall` request to the plugin worker. `PluginInstaller::uninstall` validates the plugin id, removes the installed registry entry, and deletes the cached content only when it resolves inside the plugin cache root (legacy or escaping paths are left untouched), pruning now-empty parent directories up to the cache root. A new `PluginStore::commit_removal` persists the registry without requiring a candidate directory (unlike `commit_install`). The CLI gains `tact plugin uninstall <name>`; the TUI slash command gains `/plugin uninstall <name>`. Successful uninstalls trigger the same shared skill refresh as install/reload, so `plugin:<skill>` entries disappear from the registry immediately.

**Behavior after:** `tact plugin uninstall <name>` and `/plugin uninstall <name>` remove the plugin's cache directory and registry entry; missing plugins produce an error; content outside the plugin cache is never deleted.

---

## 2. 2026-08-23 — Attachments are de-inlined: `@file`/`![alt]` kept as path text

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact-ui/src/user_message.rs` (`build_user_message`), `crates/tact-ui/src/{lib,image_attach}.rs` (removed `image_attach`), `crates/tact-ui/Cargo.toml` (dropped `image` dep); Ch 22 § image attachments |

**Symptom / motivation:** `build_user_message` base64-inlined image files into `ContentBlock::Image` and whole-file-inlined text files into `ContentBlock::Text` at attach time. That pushed image bytes and large file contents into every request even when the model did not need them, and text-only models could not read images at all.

**Decision:** de-inline attachments. `@file` keeps `@<path>` text and `![alt](path)` keeps `![alt](path)` text, so the model reads them on demand with `read_file` (text) / `read_image` (image). The `image_attach` module and its `image` dependency were removed from `tact-ui` since the attach path no longer decodes images (the encoder now lives in the `read_image` tool in `tact`).

**Behavior after:** image/file bytes only enter the request when the model explicitly calls `read_image`/`read_file`. The user message carries path references, not inline blobs. Existing `read_file` (offset/limit) is unchanged for text; images go through `read_image`.

---

## 2. 2026-08-23 — `read_image` tool: model-driven image reads, tool-result images fold into a user message

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | `crates/tact/src/tool/read_image.rs`, `crates/tact/src/tool/{mod,registry}.rs`, `crates/tact/src/agent/tool_dispatch.rs` (`ToolCallResult.image` / `ExecResult`), `crates/tact_llm/src/convert.rs` (`messages_to_openai` tool-result image fold); Ch 22 § image attachments |

**Symptom / motivation:** image handling was user-attach only: `@file.png` became an inline `ContentBlock::Image` and text-only models had no path to read a local image on demand. There was no model-callable image tool, so a vision model could not pull an image by path without the user re-attaching it.

**Decision:** add a `read_image` tool (Harness-style): gates on the current model declaring image input, reads a PNG/JPEG/WebP/GIF, re-encodes to a bounded JPEG, and returns a `ToolCallResult` with a text envelope + an adjacent `ContentBlock::Image`. `ToolCallResult` gains an in-memory `image` field; `build_tool_results` emits the companion image block. Because Chat Completions `role:tool` content is string-only, `messages_to_openai` folds that image into a following `role:user` message (`[{text:"Attached image(s) from tool result:"}, {image_url}]`), mirroring DeepSeek Harness `serializeMessagesWithImages`. `ContentBlock::ToolResult` stays unchanged, so Responses/normalize/compact consumers are untouched.

**Behavior after:** a vision model can call `read_image("path")` and receive both a stable text envelope and the image in a follow-up user message; text-only tool results keep riding `role:tool`. Non-image tools are unchanged.

---

## 2. 2026-08-23 — Tool-card step indices resolve to the plan position (bugfix, task #43 review)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Task #43 (ToolEvent outbox: `crates/agent_tui_kit/src/components/tool.rs`); `crates/agent_tui_kit/src/components/plan.rs`; `crates/tui/src/widgets/state/app/agent.rs`; entry below (ComponentRegistry, task #42); Ch 23 §1 |

**Symptom / motivation:** after the ToolEvent-outbox extraction (task #43),
the step index had three drift points that broke on out-of-order or
restarted tool ids:
1. `PlanComponent` wrote `step.output` at the agent's **raw** `idx`, which
   can differ from the plan position (`resolve_step_idx` exists exactly for
   this) — a finished/failed update could overwrite the *wrong* step's
   output, corrupting the status bar's progress derivation and the plan
   panel.
2. `ToolComponent`'s internal `tool_id → step index` map used
   **last-wins** (`HashMap::insert`), while the shell resolves **first**
   plan position — a same-`tool_id` restart made the card header ("N. tool")
   disagree with the shell's status line.
3. The `StepFailed` system message ("✗ Step N failed: …") used the raw
   `idx` while the card header used the resolved one.

**Decision:**
- `PlanComponent` gained its own `resolve_step_idx` (first plan position by
  `tool_id`, fall back to raw `idx` — identical to the shell's) and both
  `StepFinished` / `StepFailed` writes go through it. The shell tail keeps
  its resolved write; both now write the same value, so the double-write is
  benign and external kit hosts keep working standalone.
- `ToolComponent`'s map is **first-wins** (`entry().or_insert`, never
  overwritten): a restarted `tool_id` keeps the original step number for
  the card's whole lifetime, matching the shell. The map is session-bounded
  (one entry per distinct `tool_id`, same growth as the plan itself) and
  must not be pruned on finalize — pruning would re-record the latest
  arrival index and re-introduce the drift.
- The `StepFailed` missing-card system message formats with the resolved
  step number.
- `ToolEvent::Finalized` carries an explicit `had_active: bool` instead of
  the fragile `phys_idx == 0 && old_rows == 0` sentinel for the
  allocate-vs-resize decision; `on_step_failed_tail` dropped its unused
  `idx`/`tool_id`/`error` parameters.

**Behavior after:** a step's output and every displayed step number (card
header, system message, plan panel, status bar) agree with the first plan
position for the `tool_id`, even when the agent's raw `idx` differs
(out-of-order tool ids, same-id restarts). Covered by new tests:
`step_finished_resolves_divergent_raw_idx`, `step_failed_resolves_divergent_raw_idx`,
`restart_keeps_first_step_mapping`, `step_failed_missing_uses_resolved_step_number`,
`finalize_without_active_marks_had_active_false`.

**Pointers:** `crates/agent_tui_kit/src/components/{plan,tool}.rs`,
`crates/tui/src/widgets/state/app/agent.rs` (`apply_tool_events` /
`on_step_failed_tail`), Ch 23 §1.

---

## 2. 2026-08-23 — TUI render layer extracted into `agent_tui_kit` (reusable, Tact-free)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Design: `docs/superpowers/specs/2026-08-18-tui-component-library-design.md` (+ `-ctx-design.md`); plan: `docs/superpowers/plans/2026-08-18-tui-component-library.md`; Ch 23 |

**Symptom / motivation:** the whole render stack lived in `crates/tui` as one
monolithic crate coupled to `tact` / `tact_llm` (`App` held every panel's
state; render functions took `&App`). Another agent project could not reuse
the thinking/tool cards, streaming markdown log, popup family, input box, or
status bars without pulling in Tact's agent runtime, and `handle_agent_update`
was one giant match with no component boundary to test against.

**Decision:** extract a reusable **agent-TUI kit** (`crates/agent_tui_kit`)
that depends only on `tact_protocol` + ratatui:
- pure render functions take a per-frame `RenderCtx` (disjoint `&` borrows
  built by the host; the only mutation path is an explicit
  `Vec<RenderCommand>` drained after the frame);
- state models (`LogCoordinator`, `ToolState`, `ThinkingState`,
  `StreamState`, `StatusBarState`, `LogScroll`, `PlanPanel`, popup states,
  widgets) moved into the kit verbatim — zero visual change, verified by the
  unchanged scene/render tests at every phase gate;
- the in/out contract is `bridge::Command` (generic 9-variant enum) +
  `AgentBridge` + `BridgeExtension`/`ExtensionEvent`; Tact-only commands
  (`QueryBalance`) ride `ExtensionCommand`.
- mutable "prepare" phases that need app-layer styling (log scroll-cache
  rebuild with skill highlighting, diff/subagent lazy caches, input caret
  clamp) stay in `crates/tui` and feed the kit's pure renderers.

**Behavior after:** `cargo tree -p agent_tui_kit` contains no `tact` /
`tact_llm`; `crates/tui` is the Tact app layer (shell `App`, handlers,
prepare phases, app-layer popups, orchestration `layout.rs`). A headless
mock consumer proves the kit standalone: `cargo run -p agent_tui_kit
--example mock_agent`. Test counts moved with the code (tui 413 + kit 194 =
607 ≥ the 604 baseline; `tact-ui` 105; clippy zero warnings).

**Pointers:** `crates/agent_tui_kit/src/render/*` (bar, input, log, popups,
task_panel, render_md, cells), `state/*`, `bridge.rs`,
`crates/tui/src/widgets/state/app/config.rs` (`App::render_ctx`),
`examples/mock_agent.rs`; per-phase gates recorded in the plan's results
sections. Follow-up (step 9): `Component` registry + handlers migration is
still TODO.

---

## 2. 2026-08-23 — TUI `App` switches to the kit's `ComponentRegistry` (whole-App refactor, task #42)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Plan: `docs/superpowers/plans/2026-08-18-tui-component-library.md` (step 9); design: `docs/superpowers/specs/2026-08-18-tui-component-library-design.md`; Ch 23 |

**Symptom / motivation:** the kit's `Component` trait and six components
(Thinking/Stream/StatusBar/TaskPanel/Plan/Tool) existed, but the Tact app
still held their state as bare `App` fields and `handle_agent_update`'s giant
match ran the old inline handlers — the component boundary was compile-only,
and any host adopting the registry had to replicate the whole shell first.

**Decision:** the whole-App switch keeps behavior identical while making the
registry the single state owner:
- `App` drops `plan` / `thinking` / `stream` / `tools` / `task_panel` /
  `status_bar` fields and holds `registry: ComponentRegistry`; the shared
  `LogCoordinator` stays shell-owned (decision: the coordinator is the
  priority-0 surface the shell already owns and tests).
- typed accessors `app/registry.rs` (`plan()`/`plan_mut()`, …) reach the
  components; `Deref`/`DerefMut` to the underlying state keeps
  `app.<field>.<state>` call sites working with minimal churn.
- `handle_agent_update` = `coordinator_prepass` → `dispatch_components`
  (registry dispatch; `Ctx` borrows the shell-owned log / input mode /
  pending queue via field-split borrows) → `apply_stream_events` (the
  `StreamEvent` outbox; **StreamChunk only**, because the gap checks append
  rows — the pre-dispatch code ran them only on stream chunks) →
  `shell_handle` (status/log effects, tool-card lifecycle, select popups,
  thinking card) → `refresh_tail_scroll`.
- `ThinkingChunk` and `StepFinished`/`StepFailed` are deliberately **not**
  dispatched: the shell owns the log-anchored thinking card and the
  resolved-step tool lifecycle; dispatching would double-process.
- `Component` gained a `Send` supertrait (a registry-holding shell moves
  onto the tokio task in `tact-ui`) and `ComponentRegistry::get_mut` (typed
  mutable downcast for host-side handlers).

**Behavior after:** component state is owned by the registry; the shell
keeps rich behavior in `shell_handle`; hosts write `app.plan()` /
`app.tools_mut()` instead of field access. No user-visible change — all
scene/render tests pass without expectation edits (tui 413 + kit 221 = 634;
`tact-ui` 105; clippy zero warnings).

**Pointers:** `crates/agent_tui_kit/src/components/{registry,thinking,stream,
status_bar,task_panel,plan,tool}.rs`, `crates/tui/src/widgets/state/app/
{agent.rs (dispatch_components / shell_handle / apply_stream_events),
registry.rs, construct.rs, config.rs}`, `crates/tui/src/render/log.rs`
(field-split borrows into prepare), Ch 23 §1.

---

## 2. 2026-08-17 — Responses model switches adapt web-search query fields

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 22 (LLM / Responses); `crates/tact_llm/src/openai/responses/wire.rs`, `crates/tact_llm/src/openai/responses/convert.rs` |

**Symptom / motivation:** Switching from an OpenAI model to a DeepSeek-compatible model on the same Responses endpoint replayed a prior `web_search_call` with `action.query`, while the target endpoint required `action.queries`, causing HTTP 400 `missing field queries`.

**Decision:** Keep the existing Responses baseline and normalize only the hosted web-search action at the outgoing boundary when the request model changes: DeepSeek-like targets receive `queries: [query]`; other targets receive the first query as singular `query`. Same-model replay remains verbatim.

**Behavior after:** Model switching no longer sends the previous model's incompatible `query` / `queries` spelling, while the rest of the opaque Responses baseline and logical conversation remain unchanged.

**Pointers:** `normalize_web_search_call_query_shape_in_items` in `crates/tact_llm/src/openai/responses/wire.rs`; outgoing baseline handling in `create_response` in `crates/tact_llm/src/openai/responses/convert.rs`; regression tests for both conversion directions; Ch 22.

---

## 2. 2026-08-17 — Pending prompt `[Cancel]` sits beside the hint text

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 (TUI); `crates/tui/src/render/input.rs`, `crates/tui/src/handlers/mouse.rs`, `crates/tui/src/widgets/state/app/pending.rs` |

**Symptom / motivation:** The pending queue's `[Cancel]` control was right-aligned at the far edge of the hint row, making it awkward to reach even though it only controls the nearby pending prompt block.

**Decision:** Reserve the button width while truncating the hint, then render `[Cancel]` immediately after the hint text with one space. Keep the existing render-time hit rectangle and queue-only cancellation semantics.

**Behavior after:** On wide terminals the hint reads `Message will be submitted after the current task [Cancel]`; the button is directly beside the explanatory text, clicks clear only queued prompts, and narrow terminals still hide it.

**Pointers:** `render_pending_block` in `crates/tui/src/render/input.rs`; `handle_mouse_down` and `pending_cancel_click_hits_the_rendered_button` in `crates/tui/src/handlers/mouse.rs`; `App::clear_pending_messages` in `widgets/state/app/pending.rs`; Ch 23; `docs/superpowers/specs/2026-08-17-pending-cancel-button-placement-design.md`.

---

## 2. 2026-08-16 — Log rows carry explicit provenance instead of inferring system items from text

| Field | Value |
|-------|-------|
| **Type** | bugfix / optimization |
| **Related** | Ch 23 (TUI); `crates/tui/src/widgets/state/mod.rs`, `crates/tui/src/widgets/state/app/popups.rs`, `crates/tui/src/render/log.rs` |

**Symptom / motivation:** Log rendering inferred user/system ownership from raw prefixes and indentation. A normal item beginning with two spaces could enter the system plain-text path, so Markdown such as `  **bold text**` displayed literal `**` markers. The same heuristics also made user continuation detection and row indentation depend on neighboring raw strings.

**Decision:** Add `LogItemKind` metadata to every physical log row. Insertion paths assign the source explicitly: user, assistant Markdown, system plain/Markdown, system tool, or Thinking. History uses the original `Role` / content path; live updates use their `AgentUpdate` variant. Rendering, indentation, category separators, and user-gap handling consume this metadata. Explicit system marker prefixes only choose a system color after the source is already known.

**Behavior after:** Indented assistant/system Markdown keeps Markdown styling; user rows and continuations use their recorded source; tool and Thinking placeholders retain their dedicated indentation; raw text is no longer used to infer row ownership or category. `RawMessageType`, `is_user_message_line`, `user_line_mask`, and `classify_system_message` are removed from the main log path.

**Pointers:** `LogItemKind` / `SystemMsgStyle` in `crates/tui/src/widgets/state/mod.rs`; synchronized metadata operations in `app/popups.rs`; explicit insertion paths in `app/messages.rs`, `app/agent.rs`, and `handlers/mod.rs`; metadata-driven rendering in `render/log.rs`, `render/log_style.rs`, and `app/visibility.rs`; Ch 23.

---

## 2. 2026-08-16 — Nested Markdown list items no longer join the parent row

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 (TUI); `crates/tui/src/render/pulldown.rs` (`Writer::start_tag`, `Tag::List`) |

**Symptom / motivation:** With pulldown-cmark events, a nested `Tag::List` arrived while the parent item's inline spans were still in `pending`. The first child marker was therefore appended to the parent row (`• parent    • child one`), while later children rendered on separate rows.

**Decision:** When entering a nested list and the writer has an unfinished parent list row, flush that row before pushing the nested list context. The existing marker and per-level indentation logic remains unchanged.

**Behavior after:** The parent and every nested item render on separate rows; nested bullets retain four-column indentation per list level. Existing ordered-list and task-list behavior is unchanged.

**Pointers:** `Writer::start_tag` in `crates/tui/src/render/pulldown.rs`; regression test `nested_list_items_render_on_separate_lines`; Ch 23.

---

## 2. 2026-08-16 — Tables render a horizontal separator between body rows

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23 (TUI); `crates/tui/src/render/render_md.rs` (`render_table_chunk`, `push_separator`) |

**Symptom / motivation:** Pipe tables only drew the header separator; multi-row bodies rendered as consecutive wrapped blocks, so long rows were hard to tell apart and the table did not read as a grid.

**Decision:** `render_table_chunk` now emits a separator row (accent-colored dashes matching the column widths, same look as the header rule) after every body row except the last. Rows that wrap into several visual sub-rows stay above their rule. A body row already followed by an explicit dash-only row (`/skills` style dividers) does not get an extra rule. The separator is generated from the same width data, so it stays pipe-aligned with every row, including chunk-split tables and streamed rows (`format_table_lines` shares the implementation).

**Behavior after:** Multi-row tables display with grid lines between rows (header rule + inter-row rules); single-row tables are unchanged. Wrapped continuation lines stay grouped under their row, separators keep display-width alignment.

**Pointers:** `render_table_chunk` / `push_separator` in `crates/tui/src/render/render_md.rs`; updated row-count assertions in `format_table_aligns_cjk_and_ascii`, `format_table_keeps_pipe_inside_cell`, `format_table_splits_chunks_only_when_compact_cannot_fit`, `format_table_renders_row_separators_aligned`.

---

## 2. 2026-08-16 — Over-wide tables stay intact when a compact layout fits (chunk splits are the last resort)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23 (TUI); `crates/tui/src/render/render_md.rs` (`format_table`, `fit_columns_to_width`, `MIN_COL_WIDTH` / `COMPACT_COL_WIDTH`), `crates/tui/src/render/cells/markdown.rs` (rendered-width regression tests) |

**Symptom / motivation:** When a table's natural width exceeded the main area, `format_table` shrank columns to the readability floor (`MIN_COL_WIDTH = 8`) and, if the table was still too wide, split it into column chunks that each repeat the header. The greedy chunking produced lopsided blocks (e.g. 12 columns at 95 wide → 8+4, the second block using only half the width; a 4-column long table at 35 wide → 3+1 with a lonely single-column block), even when the whole table could have been shown intact at narrower columns.

**Decision:** Two-phase fit. First shrink to the readability floor; if the table still does not fit, shrink further to the compact floor (`COMPACT_COL_WIDTH = 4` — still ~2 CJK glyphs per line) and keep the table intact whenever that fits. Chunk splitting (`split_into_fitting_chunks`) now runs only when even the compact floor cannot fit (e.g. 10 columns at 40 wide), and the readability floor is restored before splitting so chunk cells stay 8 columns wide. `fit_columns_to_width` gained a `floor` parameter.

**Behavior after:** Tables wider than the main area are compressed column-wise (≥ 4 cols each) and displayed as one complete table with a single header whenever possible; only genuinely unrenderable tables split into chunks (header repeated per chunk), which keeps the existing `wide_table_chunks_into_fitting_blocks` guarantees (rows ≤ panel width, per-block pipe alignment).

**Pointers:** `format_table` / `fit_columns_to_width` / `COMPACT_COL_WIDTH` in `crates/tui/src/render/render_md.rs`; tests `format_table_keeps_overwide_table_intact_when_compact_fits`, `format_table_splits_chunks_only_when_compact_cannot_fit`.

---

## 2. 2026-08-16 — Streamed table rows no longer clip their rightmost pipes after the reply indent

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 (TUI); `crates/tui/src/widgets/state/app/visibility.rs` (`App::table_layout_width`), `crates/tui/src/widgets/state/app/agent.rs` + `visibility.rs` (`format_table_lines` call sites), `crates/tui/src/render/render_md.rs` (`format_table`), `crates/tui/src/render/cells/markdown.rs` (regression test) |

**Symptom / motivation:** Streamed and system pipe tables were laid out at build time against `log_scroll.width` — the full content width of the log panel. When rendered, assistant / system rows are indented (`nested_log_indent` returns at least `LOG_THINKING_INDENT + 1 = 3` for LLM rows), so the actual render width is 3 columns narrower. Long tables that were shrunk to the full content width then had their trailing pipe and rightmost column clipped, and the columns read as misaligned. The `MarkdownCell` path never had this bug (`render_if_needed` subtracts the indent from `content_width`), which is why only streamed / system tables misaligned.

**Decision:** New `App::table_layout_width()` returns `log_scroll.width - (LOG_THINKING_INDENT + 1)` (floored at 1) and every `format_table_lines` call site (`agent.rs` streaming flushes and `/plugin` + `/marketplace` lists, `visibility.rs::flush_stream_pending`) now lays tables out against it. Every table row renders with indent ≥ 3 (LLM rows = 3, SysTool rows = 4, `/plugin` tables classify as LLM), so a table laid out at the reduced width always fits the real render width.

**Behavior after:** Streamed and system table rows never exceed the rendered content width; trailing pipes stay visible and columns keep their display-width alignment at any panel size. Regression test `streamed_table_rows_stay_aligned_after_reply_indent` asserts real buffer pipe coordinates (aligned within each block, trailing pipe present) at 40/60/80 columns.

**Pointers:** `App::table_layout_width` in `crates/tui/src/widgets/state/app/visibility.rs`; call sites in `widgets/state/app/agent.rs` (lines ~790/810/872/887/972/1028) and `visibility.rs` (`flush_stream_pending`); test in `crates/tui/src/render/cells/markdown.rs`.

---

## 2. 2026-08-16 — Table cells containing `|` no longer split into phantom columns

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 (TUI); `crates/tui/src/render/render_md.rs` (`format_table`, `format_table_lines`), `crates/tui/src/render/pulldown.rs` (`flush_table`), `crates/tui/src/widgets/state/app/agent.rs` + `visibility.rs` (streaming flush sites) |

**Symptom / motivation:** `format_table` took raw `| a | b |` strings and re-split every row on `'|'`. `pulldown.rs::flush_table` round-tripped collected cells through such strings, so a cell containing a literal pipe (escaped `\|`, inline code) gained phantom columns and the whole row misaligned. The line-oriented streaming renderer (`agent.rs` / `visibility.rs` table buffers) had the same hazard.

**Decision:** `format_table` now takes structured cells (`headers: &[String]`, `rows: &[Vec<String>]`); it trims cells, synthesizes the GFM header separator, and keeps dash-only body rows as `/skills`-style dashed dividers. `flush_table` passes pulldown's collected cells directly — no `|` round-trip. Raw-source-line callers (streaming buffers, `/plugins`, `/marketplace` lists) moved to a new `format_table_lines`, which joins the buffered lines and renders through the standard pulldown pipeline, so escaping rules apply exactly once. Known upstream limit: pulldown-cmark splits table rows at unescaped pipes even inside code spans (`| x | `a|b` |`), so only the escaped-pipe form is supported end-to-end; `format_table` itself is correct for any cell content it receives.

**Behavior after:** Escaped pipes and pipes inside inline code are cell data, never column separators; structural pipes (column boundaries and row edges) stay aligned because all rows share uniform display-width padding. Tests assert intact cell content plus aligned structural pipes (`format_table_keeps_pipe_inside_cell`, `table_cell_with_pipe_stays_aligned_end_to_end`).

**Pointers:** `format_table` / `format_table_lines` in `crates/tui/src/render/render_md.rs`; `flush_table` in `crates/tui/src/render/pulldown.rs`; streaming flush sites in `widgets/state/app/agent.rs` and `visibility.rs`.

---

## 2. 2026-08-16 — Over-wide tables split into fitting column chunks (no more shredded pipe rows)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 (TUI); `crates/tui/src/render/render_md.rs` (`format_table`, `split_into_fitting_chunks`, `render_table_chunk`, `row_width`), `crates/tui/src/render/pulldown.rs` (`flush_table`), `crates/tui/src/render/cells/markdown.rs` (`wide_table_chunks_into_fitting_blocks` test) |

**Symptom / motivation:** A Markdown pipe table wider than the panel (e.g. 10 columns at a 40-column width) rendered rows wider than the available width: `fit_columns_to_width` stops shrinking at the `MIN_COL_WIDTH = 8` readability floor, so 10×8 + 3×10 + 1 = 111 columns cannot fit in 40. The panel-level `wrap_line` pass in `MarkdownCell::render_if_needed` then broke each table row at arbitrary points, dropping leading pipes and misaligning every subsequent row — the table looked like shredded garbage.

**Decision:** `format_table` now guarantees every rendered row fits the available width. After the floor-limited global shrink, if the whole table is still too wide (`row_width > available_width`), the columns are split into contiguous chunks that each fit (`split_into_fitting_chunks`); each chunk renders as its own table block with the header and separator repeated (`render_table_chunk`). A single column that alone cannot fit is shrunk below the floor (down to 1) so its chunk always fits. `wrap_line` then never sees an over-wide table row, so it cannot break pipe alignment. The unlimited-width path (`available_width = None`, log/popup prose) is unchanged — those rows are clipped by the widget, not shredded.

**Behavior after:** Any table fits the panel; genuinely wide tables appear as vertically stacked column chunks with repeated headers, each chunk internally aligned (CJK-aware display-width padding, in-table cell wrapping). Rows never exceed the panel width in the width-aware message path.

**Pointers:** `format_table` chunk dispatch + `split_into_fitting_chunks`/`render_table_chunk` in `crates/tui/src/render/render_md.rs`; regression test `wide_table_chunks_into_fitting_blocks` in `crates/tui/src/render/cells/markdown.rs`.

---

## 2. 2026-08-16 — `/stats` responds immediately via a shared stats snapshot (no longer awaits the running task)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 25; `crates/tact/src/stats.rs`, `crates/tact/src/agent/mod.rs`, `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact/src/hook/rtk_filter.rs`, `crates/tact-ui/src/driver.rs` |

**Symptom / motivation:** `UserCommand::QueryStats` fell into the command driver's `other =>` branch, which awaits the in-flight `SubmitTask` handle before touching the `Agent` (the Agent is exclusively owned by the running task). During a long task, `/stats` appeared to do nothing until the task finished — sometimes minutes.

**Decision:** Make the stats shared instead of Agent-exclusive: `AgentRuntime.stats` is now `Arc<RwLock<SessionStats>>` (std lock; all update sites use `write().unwrap()`, rtk atomic counters unchanged). The command loop clones the Arc before entering the receive loop, `QueryStats` became its own match arm that reads `stats.read().unwrap().summary()` and emits `SessionStats` via the pre-cloned `ui_tx` — no `Agent` access, no await. `headless.rs` / `interactive.rs` end-of-run summaries read through the same lock.

**Behavior after:** `/stats` answers instantly even mid-run, showing a snapshot of all stats recorded so far (in-flight LLM call is not counted yet — same as before, stats are only written after each call). The Agent itself is untouched; other commands (`/compact`, `/model`, `/background`, …) still serialize behind the running task.

**Pointers:** `crates/tact-ui/src/driver.rs` (`run_command_loop_with_account` QueryStats arm + `stats` clone), `crates/tact/src/stats.rs`, stats update sites in `agent/mod.rs` (main loop + compaction), `agent/tool_dispatch.rs` (tool counters), `hook/rtk_filter.rs` (`record_rtk`); test `query_stats_responds_immediately_while_task_runs` (blocked responder + AssertionStats arrives before release).

---

## 2. 2026-08-16 — Codex-style queued messages while the agent is busy (submit after the current task)

| Field | Value |
|-------|-------|
| **Type** | feature |
| **Related** | Ch 23; `crates/tui/src/handlers/insert.rs`, `crates/tui/src/handlers/skills.rs`, `crates/tui/src/widgets/state/app/pending.rs`, `crates/tui/src/render/input.rs`, `crates/tui/src/lib.rs` |

**Symptom / motivation:** While the agent was `Planning`/`Executing`, pressing Enter flashed a "⏳ Still processing previous prompt" message and dropped the typed text — a message typed during a long tool run was lost. Codex CLI instead queues it: "Messages to be submitted after next tool call (press esc to interrupt and send immediately)".

**Decision:** Replace the busy-reject with a Codex-style queue. `App.pending_messages` holds `PendingMessage { display, agent_task }`; Enter during `Planning`/`Executing` clears the input and queues (char-limit validation runs at queue time). The main loop calls `handlers::skills::flush_pending_when_idle` after draining `agent_rx`: once status reaches `Idle`/`Done`, every queued message is dispatched as its own `SubmitTask` in order — the tact-ui command driver already serializes in-flight `SubmitTask`s, so each becomes the next user turn. Submission is **fully automatic** — no "send now" path exists (the `[Send now]` button and Normal-mode `s` were removed at the user's request 2026-08-16: "send now 去掉吧，自动处理即可"). The **only** way to drop queued messages is the `[Cancel]` button on the pending block (`pending_cancel_btn_area` hit test in the mouse handler) — it clears the queue without touching the running task. `/cancel` and Normal-mode `c` are **unrelated to the queue**: they cancel only the in-flight task, exactly as before (user decision: "/cancel 也不用处理 prompt 队列"). Esc is untouched (always exits insert mode — a stray Esc never interrupts a task). The hint + `↳ message` rows render above the input box (`render_pending_block` in `render/input.rs`, button hidden on narrow terminals); the layout adds `pending_display_lines()` (hint + per-message row, capped at 4) to the input height. `submit_user_task` was split into `task_within_limits` / `dispatch_user_task` so queueing and flushing share dispatch. `/compact` keeps the old busy flash (`input_busy_msg`).

**Behavior after:** Messages typed while the agent is busy are queued, shown above the input box with the Codex-style hint, and auto-submitted when the current task finishes (including when the task was ended by `/cancel`); the `[Cancel]` button drops the queue only. Multiple queued messages submit sequentially as separate turns. Oversized messages are rejected at queue time. Busy-submission is never lost.

**Pointers:** `handlers/insert.rs` (`handle_enter_submit`, Esc arm), `handlers/skills.rs` (`submit_user_task`, `flush_pending_when_idle`, `interrupt_and_submit_pending`), `widgets/state/app/pending.rs`, `render/input.rs` (`render_pending_block`, `truncate_to_width`); tests `submit_queued_while_agent_busy`, `esc_with_pending_interrupts_and_submits_immediately`, `flush_pending_when_idle_submits_all_queued_in_order`, `input_box_renders_pending_block_above_input`; Ch 23 §6.6.

---

## 2. 2026-08-16 — Inline code uses accent text instead of a background patch

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/render/pulldown.rs`, `crates/tui/src/render/log_style.rs` |

**Symptom / motivation:** The earlier fix stopped prose lines containing inline code from being repainted as full-width code blocks, but the inline-code span still carried a narrow `code_block_bg` patch. That rectangular background remained visually heavy in ordinary prose and lists, especially after wrapping.

**Decision:** Render inline code with `theme.accent` as its foreground and no background. The log restyle pass applies the same rule to legacy inline-code spans, while genuine fenced-code lines continue to use `code_block_bg` and `code_block_fg`.

**Behavior after:** Inline code is distinguished by accent-colored text without a rectangular background patch. Fenced code blocks retain their themed background and foreground, so code-block boundaries remain visible.

**Pointers:** `crates/tui/src/render/pulldown.rs` (`push_inline_code`), `crates/tui/src/render/log_style.rs` (`restyle_log_line_with_skills`); tests `inline_code_uses_accent_without_background` in both modules; Ch 23.

---

## 2. 2026-08-16 — Prose/list lines with inline code no longer paint a full code-block background

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/render/log_style.rs` |

**Symptom / motivation:** `restyle_log_line_with_skills` treated ANY line whose spans contained `theme.code_block_bg()` as a fenced-code line and repainted the WHOLE line with the code background. A common Markdown list item like `- run `cargo build`` therefore rendered as a full-width highlight block; when the item wrapped, `wrap_line` re-sliced the background onto every continuation row, leaving shadow-like bands that lingered between frames (the recurring "shadow" class of bugs).

**Decision:** Only lines where EVERY span carries the code background (i.e. genuine fenced-code lines emitted by `flush_code_block`) are restyled as code. Mixed prose/list lines keep their styling; the inline-code span retains its narrow background patch (the text/fg special rendering stays).

**Behavior after:** List items and paragraphs containing inline code no longer render as full-width code blocks, so wrapped rows no longer show a shadow band. Fenced code blocks still get the theme code background (and the light-theme fg fix via `restyle_code_line`).

**Pointers:** `crates/tui/src/render/log_style.rs` (`restyle_log_line_with_skills`, `restyle_code_line`); test `inline_code_line_keeps_narrow_patch_not_full_block_bg`; Ch 23 render pipeline.

---

## 2. 2026-08-16 — Task-stats line is localized and drops the wide 📊 icon

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/i18n.rs`, `crates/tui/src/widgets/state/app/messages.rs`, `crates/tui/src/handlers/mouse.rs` |

**Symptom / motivation:** The per-turn stats row was hardcoded as `📊 任务统计：…` (`TASK_STATS_PREFIX` in `messages.rs`), so it stayed Chinese even in English UI mode, and the wide `📊` emoji rendered too large in the log. The `[copy]` affordance was also a hardcoded English string.

**Decision:** The prefix and copy button moved into the i18n `Messages` table: EN `Task stats:` / `[copy]`, ZH `任务统计：` / `[复制]`, with no emoji icons — the wide `📊` prefix and the `🧠` before the model name were both removed. The copy affordance renders **before** the stats body; only clicks inside the button glyphs trigger the copy (the rest of the row text-selects normally). `add_task_stats_block` reads them via `self.msgs()`; `is_task_stats_line` now recognizes every supported language's prefix with an optional leading button **plus** the legacy `📊 任务统计：` rows (so `[copy]` keeps working on sessions persisted before this change); the mouse handler finds the localized button's byte range via `find_task_stats_copy_button`.

**Behavior after:** The stats row renders as `[copy]  Task stats:⏱ mm:ss · model · N tokens …` in English and `[复制]  任务统计：⏱ mm:ss · model · N tokens …` in Chinese, with no wide emoji prefix or model icon. Old sessions' stats rows remain copyable.

**Pointers:** `crates/tui/src/i18n.rs` (`task_stats_prefix`, `task_stats_copy_btn`), `crates/tui/src/widgets/state/app/messages.rs` (`is_task_stats_line`, `find_task_stats_copy_button`, `add_task_stats_block`), `crates/tui/src/handlers/mouse.rs`; tests `task_stats_block_localizes_prefix_and_copy_button`, `task_stats_line_detection_covers_all_languages_and_legacy_rows`.

---

## 2. 2026-08-16 — `install.sh` no longer errors on `tmp: unbound variable` or leaks the clone dir

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `scripts/install.sh` |

**Symptom / motivation:** Under `set -u`, the installer printed `bash: line 351: tmp: unbound variable` after a successful release install. `try_install_release` set `trap 'rm -rf "$tmp"' RETURN` on a `local tmp`; the RETURN trap is inherited and fires again when the caller `main` returns, where `tmp` is out of scope, so the unguarded `$tmp` expansion aborted with `nounset`. Separately, `main` declared `work` as a `local` and cleaned it with an `EXIT` trap; that trap fires at script exit after `main`'s locals are gone, so `${work:-}` always saw an empty value and the `git clone` directory leaked on the from-source / non-repo path.

**Decision:** (1) The release tmpdir trap is now `trap '[[ -n "${tmp:-}" ]] && rm -rf "$tmp"' RETURN` — the guard makes the inherited caller-context firing a no-op while still cleaning up on `try_install_release`'s own return. (2) `work` is no longer `local`; it is a global initialized to `""` so the existing `EXIT` trap actually removes the clone directory at script exit (including the `die` / `exit 1` paths).

**Behavior after:** `curl … | bash` (and `./scripts/install.sh`) finishes with `Done. Run: tact-ui --help`, exit 0, no `unbound variable` error, and both the release tmpdir and the cloned repository directory are removed.

**Pointers:** `scripts/install.sh` (`try_install_release`, `main`).

---

## 2. 2026-08-16 — `plugin install` no longer panics and parses the official `url` plugin sources

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 02; `crates/tact-ui/src/main.rs`, `crates/tact/src/plugin/marketplace.rs` |

**Symptom / motivation:** `tact-ui plugin install <plugin>@claude-plugins-official` failed in two ways. First, the main entrypoint's early self-upgrade check used `args.command.take()`, which consumed *any* non-`Upgrade` command and left `args.command = None`; `Plugin` (and `Headless`) then fell through to `run_interactive`, and because `plugin` commands resolve config without an LLM provider (`install_without_llm`), the interactive path panicked at `get_provider()` with "LLM provider not initialized". Second, the official `anthropics/claude-plugins-official` catalog uses a `"source": "url"` object form (150 of its 286 plugins: `url` + `sha`, some with `path`) that `PluginSource::from_catalog_value` did not understand, so the whole catalog failed to parse with "invalid marketplace source: url".

**Decision:** (1) The upgrade early-return now checks with `matches!(args.command, Some(CliCommand::Upgrade { .. }))` and only `take()`s inside the matched branch, so non-upgrade commands survive to the later dispatch. (2) `from_catalog_value` treats both `git-subdir` and `url` as Git repository sources (clone `url`, optional `path`, pinned revision), preferring the `sha` pin and falling back to `ref`.

**Behavior after:** `plugin install` (and `headless`) dispatch correctly; `plugin install frontend-design@claude-plugins-official` clones the official marketplace, parses all 286 entries, and installs `frontend-design` (1 skill) at its pinned revision. Plugin commands never require an LLM provider.

**Pointers:** `crates/tact-ui/src/main.rs` (upgrade early-return), `crates/tact/src/plugin/marketplace.rs` (`RawPluginSource::Object.sha`, `PluginSource::from_catalog_value`); tests `parses_url_plugin_source`, `parses_url_plugin_source_with_subdirectory`, `git_source_falls_back_to_named_ref_without_a_sha`; spec `docs/superpowers/specs/2026-07-20-plugin-install-design.md`.

---

## 2. 2026-08-16 — Overlay list popups stay inside the main area

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/lib.rs`, `crates/tui/src/render/test_harness.rs` |

**Symptom / motivation:** The four overlay popups rendered from the main frame loop (`command_palette`, `select`, `file_picker`, `slash_command`) were centered on the **whole frame** with a height cap of `frame.height - 4`. With a long command/file/option list on a short terminal the popup grew past the log panel and covered the command-line input box and bottom bar; popup list rows interleaved with the input box's border glyphs (outside the popup's `Clear` rect) and read as a shadow-like mess, and the user could no longer see the command being typed.

**Decision:** The popup call sites pass the **main area** (`chunks[1]`: status bar below, input box above) instead of the full frame. Popups now center and cap their height inside the main area only.

**Behavior after:** Palette / select / file-picker / slash popups always stay above the input box and bottom bar regardless of list length or terminal height; the input chrome stays fully visible while filtering.

**Pointers:** `crates/tui/src/lib.rs` (frame loop popup calls), `crates/tui/src/render/test_harness.rs` (`draw_full_ui`, kept in sync), `crates/tui/src/render/popup_scene_tests.rs` (`full_frame_palette_popup_stays_inside_main_area`); Ch 23.

---

## 2. 2026-08-16 — Main-area headings no longer paint the highlight band

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/render/log_style.rs` |

**Symptom / motivation:** The restyle pass (added in the pulldown-cmark migration, #69) painted H1 headings with `theme.highlight` as their background — a leftover of tui-markdown's direct H1 styling. In the log panel the band only covered the glyph columns of the heading, and on a wrapped heading every wrapped row carried the band, so a long heading plus its list read as a multi-row shadow block behind the text. The whole-Markdown path (`MarkdownCell`, e.g. `/skills` pages) never painted the band, so the two main-area paths disagreed.

**Decision:** The restyle pass no longer assigns any background to heading spans (the pulldown renderer emits headings backgroundless). The legacy `Color::Rgb(70, 90, 140)` → `theme.highlight` remap (dead since the fork was dropped) was removed with it.

**Behavior after:** H1 headings render as bold+underlined heading-color text with no background in both main-area paths; no highlight band can appear behind (wrapped) list headings.

**Pointers:** `crates/tui/src/render/log_style.rs` (`restyle_log_line_with_skills`, `heading_keeps_no_background`), `crates/tui/src/render/log_render_tests.rs` (`heading_rows_carry_no_highlight_band`); Ch 23 render pipeline.

---

## 2. 2026-08-15 — `/stats` popup renders through ratatui-markdown directly

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The system-prompt popup (shared by `/stats` session stats and the assembled-system-prompt view) went through the pulldown-cmark pipeline with Tact's width-aware pipe-table pass, laid out at the popup's content width. For a quick statistics popup that extra layout machinery was not worth it. |
| Decision | `render_system_prompt_popup` now renders via `render_markdown_ratatui` (`crates/tui/src/render/render_md.rs`): a plain `ratatui_markdown::markdown::MarkdownRenderer` at the popup's content width, using the same `TuiRichTextTheme` as the Mermaid renderer. The width-aware table pass and Mermaid routing stay for the main-area Markdown cells. |
| Behavior after | The `/stats` and system-prompt popups are laid out by ratatui-markdown's default renderer (tables included); popup tests (`session_stats_popup_renders_gfm_table`) pass unchanged. |
| Pointers | `crates/tui/src/render/popups/system_prompt_popup.rs`, `crates/tui/src/render/render_md.rs` (`render_markdown_ratatui`); `docs/token_usage_schema.md` Session Stats Display. |

---

## 2. 2026-08-15 — Auto-compaction no longer enables thinking on the summary call

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The local compaction summary call forwarded the agent's Claude-style `thinking_budget` (`with_thinking`, clamped below the wire `max_tokens`) and its explicit `reasoning_effort`, and reserved an effort-tiered reasoning share on top of the text budget. Thinking on a handoff summary adds little value and consumes output tokens from the same `max_tokens` envelope (effort models) — the user asked to stop enabling it during auto-compaction. |
| Decision | The summarizer request no longer carries any thinking: no `thinking` block and no `reasoning_effort` are forwarded (main-loop thinking config is untouched), and the input reservation no longer subtracts a thinking budget. The **server-default** reasoning reserve stays for DeepSeek / Kimi K3 only (fixed 75% of the text budget on top): they default thinking ON + effort high server-side even when the request omits effort, so without the reserve their forced reasoning would starve the summary text and force truncation continuations. The native `/responses/compact` request was already `{model, input}`-only; its dead `.with_reasoning_effort` was removed. |
| Behavior after | The compact summary call is a plain non-streaming `create_message` with `max_tokens` = classic text budget (OpenAI / Anthropic) or text + 75% reserve (DeepSeek / Kimi K3); `AgentUpdate::ModelInfo` emitted during compaction reports no thinking/effort. |
| Pointers | `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`, `compact_summary_reasoning_reserve_percent`, `compact_responses_native`), book [Ch 5](./05_chapter_compact.md) §summarization call. |

---

## 2. 2026-08-15 — Bottom-bar `out` renamed to `max_out_token` with the real output budget

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The bottom-bar output segment was labeled `out`/`输出` and showed the raw `max_tokens` envelope. For effort-semantic models (openai / deepseek / kimi k3) reasoning counts inside the same envelope as output text, so `out 128K` next to `think high` overstated the tokens actually left for text — the user asked for the segment to show the **max output token** value, with the reasoning share subtracted. |
| Decision | Rename the label to `max_out_token` (language-invariant, both locales) and make the value the effective text-output budget: for effort-semantic models subtract the reasoning share using the same tier convention as the compaction reserve (percent of the text budget added on top → text = envelope × `100/(100+pct)`; `high` → 73K on a 128K envelope). Budget-semantic models (Anthropic-style `thinking_budget`) keep thinking in a separate envelope, so they show the full `max_tokens`. Computed in the TUI from `status_bar.model_max_tokens` + `model_thinking_budget` + `model_reasoning_effort`; no protocol change. |
| Behavior after | Bottom-bar row 2 shows `max_out_token {n}` instead of `out {n}`; `n` equals `max_tokens` for budget/no-think states and `max_tokens × 100/(100+pct)` when an effort is displayed (`none`/no effort → unchanged, `low` → 80%, `medium` → ~67%, `high` → ~57%, `xhigh`/`max` → 50%). |
| Pointers | `crates/tui/src/render/bar.rs` (`format_max_out_tokens`), `crates/tui/src/i18n.rs` (`bottom_out`), book [Ch 23](./23_chapter_tui.md) §6.6, `docs/token_usage_schema.md`. |

---

## 2. 2026-08-15 — Model→context-window mapping overrides manual `model_context_window` config

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | `agent.model_context_window` was purely manual (CLI/TOML, default `200_000`) with no model inference. With `deepseek-v4-pro` (real window 1M) the bottom-bar `ctx` meter showed `…/256K` from a stale 256k config and auto-compaction fired at ~80% of that (~205k) instead of ~800k, causing premature compaction. `max_tokens` already had a model-based default (`kimi_k2x → 32_000`) to copy; the window had no equivalent. |
| Decision | Add `model_context_window_for_model(model)` in `resolve.rs` and resolve the window as: **model→window mapping (highest) → CLI/TOML → default `200_000`**. Values follow official model docs (2026-08): OpenAI `gpt-5.6` family + `gpt-5.5` → `1_050_000`, `gpt-5.4` → `1_000_000`, `gpt-5`…`gpt-5.3`/`gpt-5.4-mini` → `400_000`, `gpt-4o` family → `128_000`; Anthropic (API + Claude Code) `claude-sonnet-5`/`claude-fable-5`/`claude-opus-5`/`claude-opus-4-8`/`claude-opus-4-7`/`claude-opus-4-6`/`claude-sonnet-4-6` → `1_000_000`, `claude-sonnet-4-20250514`/`claude-opus-4-20250514`/`claude-haiku-4-5`/`claude-haiku-4-20250514` → `200_000`; DeepSeek V4 → `1_000_000`, `k3-256k` → `256_000`. A mapping match deliberately overrides user file config so a stale manual window can never under-report a well-known model. |
| Behavior after | The `ctx` bottom-bar meter and the derived auto-compact threshold (80% of window) use the mapped window for the mapped models. GPT-5.6/5.5 models show `…/1.05M`, Claude 1M models (incl. Claude Code ids) `…/1M`, DeepSeek V4 `…/1M`, GPT-5.x `…/400K`, GPT-4o `…/128K`, `k3-256k` `…/256K`. Manual `model_context_window` only takes effect for models without a built-in mapping. The nonzero `model_context_window > max_tokens` validation still applies to the resolved value. |
| Pointers | `crates/tact/src/config/resolve.rs` (`model_context_window_for_model`, resolution at ~`:587`); `config.example.toml` `[agent]`; book [Ch 21](./21_chapter_config.md) §5, [Ch 5](./05_chapter_compact.md) §settings tables. |

*(Superseded 2026-09-13 — the precedence was inverted: an explicit CLI flag or `[agent]` value now wins, and the mapping is only a fallback for unconfigured models. See the newest entry, which also keeps the "stale manual window under-reports a long-context model" trade-off as documentation rather than enforcement.)*

---

## 2. 2026-08-15 — Markdown body moves to pulldown-cmark; ratatui-markdown kept for Mermaid only

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Plan | `docs/superpowers/plans/2026-08-15-pulldown-cmark-migration.md` |
| Symptom / motivation | The consolidation below landed on a local `ratatui-markdown` fork carrying ~350 lines of patches across 8 files (H4–H6, ordered-list numbers, hard breaks, nested emphasis, CJK flanking, themed code slots) purely so Tact's prose renderer could match `tui-markdown`'s output. The fork is a rebase burden and a path dependency that only resolves on this machine; `steer` and xAI's `grok-build` both parse CommonMark with `pulldown-cmark` and render in their own code instead of forking a Markdown crate. |
| Decision | Replace the fork's block renderer with a `pulldown-cmark` 0.13 event loop (`crates/tui/src/render/pulldown.rs`) that reuses Tact's width-aware pipe-table `format_table`, the `▎` blockquote gutter, fenced-code styling, and Mermaid routing. `ratatui-markdown` becomes an upstream git dependency (`celestia-island/ratatui-markdown` @ `3a8bcbe`, `mermaid` feature) used only for non-sequence Mermaid diagrams; `sequenceDiagram` stays in Tact's own `mermaid_sequence.rs`. The `feat/tact` fork and the `TuiRenderHooks`/`RenderHooks` adapter are removed. |
| Behavior after | Prose/heading/list/task/table/blockquote rendering is owned by Tact from `pulldown-cmark` events. `ENABLE_SMART_PUNCTUATION` is left off so `...` is never turned into `…` (system messages and user text stay byte-stable). GFM task lists now apply to ordered lists too (`1. [X]` → `1. ☑`). Mermaid output is unchanged. |
| Pointers | `crates/tui/src/render/pulldown.rs`, `render_md.rs`; `Cargo.toml` (`ratatui-markdown` git dep + `pulldown-cmark`); book [Ch 23](./23_chapter_tui.md) §6.7; the entry below records the intermediate fork approach. |

---

## 2. 2026-08-15 — Main-area Markdown consolidated onto ratatui-markdown

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Plan | `docs/superpowers/plans/2026-08-15-ratatui-markdown-migration.md` |
| Symptom / motivation | The TUI main area maintained two Markdown stacks in parallel: `tui-markdown` 0.3.x (crates.io) rendered prose / headings / lists in the log panel, while `ratatui-markdown` (celestia-island git fork, branch-pinned) rendered Mermaid and the `/tasks-dag` popup. Two style adapters, two color palettes, and fork workarounds (task-list marker escaping, hardcoded-color remaps in `log_style.rs`, fence-marker bookkeeping) had to be kept in sync. |
| Decision | Consolidate on `ratatui-markdown` (local fork at `../ratatui-markdown`, branch `feat/tact`, based on `chore/update-ratatui-0.30` @ `3a8bcbe`) with capability patches: H4–H6 headings, ordered-list numbers preserved, 4-column-per-level nested indent, recursive nested emphasis (`**bold _x_ italic**`), link URL suffixes, themed inline-code / fenced-code color slots, soft-break collapse + hard-break preservation, space-run preservation, and CJK-aware emphasis flanking. Tact side: `render_plain_markdown` swaps `tui_markdown::from_str_with_options` for fork parse+render with a `TuiRenderHooks` (fences / box chrome hidden, code background applied directly); the blockquote `▎` gutter and the H1 highlight background moved to post/restyle passes; tables render through a `table` RenderHooks adapter that delegates to Tact's width-aware `format_table` (pipe style), so the fork's own `render_table` is unused and kept upstream; triple-click code-block detection now matches the code background instead of ````` ``` ````` markers; the diff popup highlights via `syntect` directly (same Base16 Ocean Dark theme), so `tui-markdown` and the direct `pulldown-cmark` dep are removed. |
| Behavior after | The log panel renders Markdown through one crate. Bullets render as `•` and task items as `☐` / `☑` (previously the literal `-` / `[ ]` text); fenced code keeps the themed background without fence markers; raw copy rows mirror the rendered text (markers are consumed at parse time); `/stats` renders its GFM table as a pipe table via `format_table`; the diff popup keeps syntax highlighting. |
| Pointers | `crates/tui/src/render/render_md.rs` (`TuiRenderHooks`, `render_plain_markdown`, `apply_blockquote_indicator`), `crates/tui/src/render/log_style.rs` (H1 highlight rule), `crates/tui/src/render/popups/diff_popup.rs` (syntect), `crates/tui/src/widgets/state/app/popups.rs` (`find_code_block_containing_logical`), `Cargo.toml` (`ratatui-markdown` path dep + `syntect`); fork repo `../ratatui-markdown` `feat/tact`; [Ch 23](./23_chapter_tui.md) §6.7. |

---

## 2. 2026-08-15 — Thinking, command-output, and read cards drop the redundant line count from their top titles

| Field | Value |
|-------|-------|
| Type | `removal` |
| Symptom / motivation | The Thinking card showed the total line count twice — once in the top title (`🧠 Thinking (N lines)`) and again in the bottom bar (`↕ visible/N lines …`) — and the `bash` command-output card repeated it too (top `Live output (N lines)` / `Command output (N lines)` vs. the bottom `preview/total lines` hint), as did the `read_file` card (top `Read <path> (N lines)`). Whenever both were visible, the top count duplicated the bottom bar's number. |
| Decision | Card top titles no longer carry counts: `🧠 Thinking` (active and completed), `Live output` (running bash), `Command output` (completed bash), `Read <path>` (read_file). The bottom bars remain the single count source (`↕ visible/total lines` on Thinking; `preview/total lines` when the command output overflows the preview). Removed the now-unused `thinking_card_title_pl` field, and `tool_live_output_title_tmpl` lost its `{}` placeholder and was renamed `tool_live_output_title`. |
| Behavior after | Thinking cards read `🧠 Thinking` / `🧠 思考中`; live bash cards read `Live output` / `实时输出`; completed command cards read `Command output`; read cards read `Read <path>`. All line counts live in the card bottom bars. Popup titles unchanged (they already used the command text or bare `Command output`). |
| Pointers | `crates/tui/src/i18n.rs`, `crates/tui/src/render/cells/thinking.rs`, `crates/tui/src/widgets/tool_widget.rs` (`detail_card_title`), `crates/tui/src/render/cells/tool.rs` (`card_bottom_text`); tests `live_output_total_excludes_command_prefix_but_popup_keeps_it`, `log_tool_card_renders_when_scrolled_into_placeholder_rows`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 2. 2026-08-15 — Log word-wrap at word boundaries; selection UX made symmetric

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | (1) `wrap_line` hard-cut every visual line at the exact display width (`split_at_display_width`), so long URLs, paths and words split mid-word with no continuation hint; (2) wrapped partial selections lost the REVERSED overlay on continuation lines because the wrapped path flattened every span to one base style; (3) double-click word selection only understood ASCII, so double-clicking 中文 selected nothing; (4) selection UX was asymmetric: clicking a whole-Markdown row (or dragging into one) created an invisible selection (the MarkdownCell renderer draws no overlay), and clicking empty space / outside the panel kept a stale selection; (5) mouse hit-testing simulated hard wraps at panel width while rendering wrapped at width − indent, drifting up to the indent width on indented rows. |
| Decision | A shared `wrap_break_offsets` computes visual-line start offsets once and both `wrap_line` and `visual_pos_to_byte_offset` consume it, so render and hit-test cannot disagree. Wrap is greedy word wrap: break at the last whitespace run that fits; hard-cut only when an unbroken word exceeds the line; trailing whitespace rides on the previous line (invisible) so segments stay contiguous. `wrap_line` now re-slices the original styled spans per segment, so per-span styles (REVERSED included) survive onto continuation lines. `find_word_bounds` classifies the char under the cursor into ASCII-word or CJK-run (Han/kana/Hangul) and expands within that class. `handle_log_click`/`handle_mouse_drag`/`handle_log_triple_click` refuse to start or extend a selection on Markdown rows, and clicks on empty space below the log or anywhere outside the panel clear the selection. Hit-testing subtracts the row indent from the wrap width. |
| Behavior after | Words no longer break mid-word (URLs/paths/CJK stay intact until they genuinely exceed the line); selection highlight stays visible across every wrapped line; double-click selects whole 中文 runs; Markdown cards are never silently "selected"; stray clicks clear stale selections instead of keeping them; clicks on indented rows map to the right byte. |
| Pointers | `crates/tui/src/render/util.rs` (`wrap_break_offsets`, `wrap_line`, `visual_pos_to_byte_offset`, `col_to_byte_offset`), `crates/tui/src/widgets/state/app/visibility.rs` (`find_word_bounds`, `is_markdown_row`, `byte_offset_from_log_position`), `crates/tui/src/handlers/mouse.rs` (click/drag/triple-click guards + outside-click clear), `crates/tui/src/render/cells/text.rs`; tests `wrap_break_offsets_prefers_word_boundaries`, `wrap_line_keeps_word_intact_and_preserves_span_styles`, `wrap_break_offsets_agree_with_byte_offset_hit_testing`, `partial_selection_reverses_target_span_across_wrapped_lines`, `double_click_selects_cjk_run`, `click_below_last_message_clears_selection`, `click_on_markdown_row_does_not_create_invisible_selection`, `drag_into_markdown_row_does_not_extend_selection`, `click_outside_log_clears_selection`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 2. 2026-08-15 — Log scroll becomes visual; `/skills` paginated

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The log panel scrolled in logical-message-row units (`log_scroll.offset ± 1` per `j`/`k`/wheel tick). A whole-Markdown message taller than the viewport — e.g. the `/skills` pipe table with ~60 skills ≈ 400+ rendered lines — therefore exposed only its first and last screenful: `resolve_visual_scroll` bottom-pinned at the max logical offset, so the middle rows (where `lark-*` sorts alphabetically) were unreachable in both directions. |
| Decision | The viewport's first visible **visual** line is now authoritative (`LogScroll.visual_top`, `usize::MAX` = pin-to-bottom sentinel); `offset` becomes a derived logical mirror for read-only consumers (mouse hit-testing, code popup). Pure step functions `visual_step_up/down` move half a viewport per `j`/`k` (3 lines per wheel tick) *inside* a cell taller than the viewport and jump across row boundaries otherwise; entering a tall row from below lands on its bottom so upward traversal stays continuous. `resolve_visual_scroll` / `effective_max_logical_scroll` deleted. `/skills` output is additionally paginated into 15-skill chunks, each a separate Markdown message with a `(n/k)` heading. |
| Behavior after | Any cell taller than the viewport (long tables, expanded tool cards) is fully traversable in both directions with `j`/`k`/wheel; `g`/`G` still jump to top/bottom, and auto-follow-the-stream keeps working (`is_log_pinned_to_bottom` compares visual positions). `/skills` renders 15 skills per page with numbered headings. |
| Pointers | `crates/tui/src/widgets/state/app/scroll.rs` (step functions + scroll API), `crates/tui/src/widgets/state/log_scroll.rs` (`visual_top`), `crates/tui/src/render/log.rs` (visual clamp + mirror derivation), `crates/tui/src/handlers/{normal,mouse,mod}.rs` (keys, wheel, `/skills` pagination), `crates/tui/src/widgets/state/app/{agent,messages,visibility}.rs` (pin helpers); regression tests `tall_markdown_cell_is_fully_traversable`, `skills_command_paginates_long_lists`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 2. 2026-08-15 — Main-area render polish: markdown indent, theme links, code backgrounds, hidden markers

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Render-path review found: (1) whole-Markdown messages (`MarkdownCell`, e.g. `/skills`) hugged the left border while streamed replies/tool cards are indented; (2) links used hardcoded palette `Blue` that never adapted to the theme; (3) `is_user_message_line` walked back to the block start per rendered row — quadratic for a long pasted user block; (4) `TextCell` flattened every span's background onto the surface color, so fenced code in streamed replies lost its background (inconsistent with `MarkdownCell`); (5) raw Markdown markers (`# `, `> `, ``` fences) leaked into the rendered text. |
| Decision | (1) `append_markdown` applies the same `LOG_THINKING_INDENT + 1` gutter; (2) links use `theme.heading`, with legacy `Blue` remapped in the restyle pass; (3) a one-pass `user_line_mask` is precomputed per frame and shared by restyle + indent; (4) `TextCell` keeps span-provided backgrounds (code bg, H1 highlight) and the restyle pass only treats code-bg spans as code; (5) styled lines hide fence rows (blank) and strip `#{1,6} ` / `> ` prefixes while `raw_messages` keep the original Markdown for copy, code-block detection and hit-testing. Streamed text also uses the same fg as final rows so completing a reply does not recolor it. |
| Behavior after | Code blocks in streamed replies show their background; H1 keeps its highlight band; quotes render as `▎ text`; headings render without `## `; links adapt per theme; long pastes no longer trigger quadratic row walks; `/skills` and other Markdown notices align with replies. |
| Pointers | `crates/tui/src/render/{log.rs,log_style.rs,render_md.rs}`, `crates/tui/src/render/cells/{text.rs,markdown.rs}`, `crates/tui/src/widgets/state/app/{popups.rs,visibility.rs}`; tests `span_backgrounds_survive_rendering`, `heading_keeps_no_background`, `user_line_mask_matches_the_per_row_walk`, `hardcoded_blue_links_remap_to_theme_heading`, `render_markdown_fenced_code_block`, `render_markdown_heading_markers_are_stripped`, `indented_cell_shifts_content_right`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 2. 2026-08-14 — Cron scheduling feature removed

| Field | Value |
|-------|-------|
| Type | `removal` |
| Symptom / motivation | The cron feature (`cron_create` / `cron_list` / `cron_delete`) only persisted schedule records; nothing ever evaluated the expressions or injected the stored prompts into `agent_loop`. Users who asked for a reminder got a false sense of security — the record existed but would never fire. Building an in-process tick loop was deemed wrong-headed: it would require the interactive TUI process to stay resident and would duplicate the system cron that already runs reliably. |
| Decision | Remove the entire feature: `crates/tact/src/cron/` (scheduler + simulation), `crates/tact/src/store/cron_store/` (trait + SQLite impl), `crates/tact/src/tool/cron.rs` (tools), `ToolContext.cron_scheduler` field, registry routes, startup wiring in `headless.rs` / `interactive.rs`, the `cron_tasks` table row in docs, the TUI tool-name map, and tests. Book chapter 16 (EN/ZH) deleted; chapter 1 / 4 / 7 / 12 / 14 / 15 / 19 / 23, `index.md`, `mindmap.md`, and `ARCHITECTURE.md` cron references cleaned up. |
| Behavior after | No `cron_*` tools; the model can no longer create scheduled prompts. Existing `cron_tasks` rows and the legacy `.tact/cron/` files are left untouched on disk (dead data, removable manually). |
| Pointers | Removed files: `crates/tact/src/cron/*`, `crates/tact/src/store/cron_store/*`, `crates/tact/src/tool/cron.rs`, `book/16_chapter_cron*.md`; edited: `crates/tact/src/lib.rs`, `crates/tact/src/tool/{mod,registry}.rs`, `crates/tact/src/tool/test_support.rs`, `crates/tact/src/store/mod.rs`, `crates/tact-ui/src/{headless,interactive}.rs`, `crates/tact-ui/tests/{subsystem_tools.rs,harness/mod.rs}`, `crates/tui/src/widgets/tool_widget.rs`, `book/01_chapter_store*`; [Ch 1](./01_chapter_store.md), [Ch 7](./07_chapter_tool.md). |

## 2. 2026-08-14 — Background output stored hybrid: full log file + `output_path` on the record

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | `background_run` output lived only in the `background_tasks` DB record, capped at the first 50,000 chars — and the cap kept the *beginning* of long logs (usually the least useful part) while dropping the ending where errors live. The model had no way to deep-inspect a big output: polling `check_background` pulled the whole ≤50k JSON into context, and `grep`/`tail` were useless because there was no file. |
| Decision | Hybrid storage: the DB record keeps metadata + the first 50k chars (unchanged, cheap to poll), while the **full** stdout+stderr stream is appended as it arrives to `<workdir>/.tact/background/<id>.log`. `BackgroundTaskRecord` gains `output_path` (SQLite column `output_path TEXT NOT NULL DEFAULT ''`, added via `PRAGMA table_info` + `ALTER TABLE` migration for existing DBs). Log-file creation is best-effort — failure degrades to the capped record only. The `check_background` listing appends `(log: <path>)` per line. |
| Behavior after | Polled JSON includes `output_path`; the agent can `bash tail <path>` / `grep error <path>` on the full log instead of ingesting a 50k blob. The log file exists from the moment the task starts (record is written with the path before spawn), so a long-running task can be inspected live. |
| Pointers | `crates/tact/src/background.rs` (`BackgroundTaskRecord.output_path`, `open_log_file`, `log_write`, `run_background_process`), `crates/tact/src/store/background_store/sqlite.rs` (schema + migration + upsert/read), `crates/tact/src/tool/background_run.rs` (listing); tests `run_writes_full_output_to_log_file_and_truncates_db_record`, `migrates_legacy_table_without_output_path`; [Ch 13](./13_chapter_background.md) §2, §3, §6, §8; [Ch 1](./01_chapter_store.md). |

## 2. 2026-08-13 — Plan-mode read-only shell classification hardened against newline command separators

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | `split_plain_command` skipped `\n` / `\r` as plain whitespace, but to `sh -c` a bare newline (and `\r` in CRLF input) is a command separator, not a word separator. A command like `ls\nrm file` therefore passed the plain-command split, classified the safelisted first word `ls` as Read, and was auto-allowed in plan mode — letting a second, mutating command ride along silently. |
| Decision | The splitter now returns `None` as soon as it meets a bare `\n` / `\r` while scanning separators, so any newline-separated multi-command string stays unclassified (falls back to `Write` / prompt). A literal newline inside single or double quotes is still a word character and remains accepted. In the same pass, git global-option handling was unified into a single `GIT_GLOBAL_OPTIONS` table (each entry carrying whether it consumes the next token) that drives both `find_git_subcommand`'s skip logic and `git_has_unsafe_global_option`, so the two checks cannot drift apart again. |
| Behavior after | `ls\nrm file`, `echo hi\nrm -f x`, CRLF variants and leading/trailing bare newlines are all classified **Write** (prompted / denied in plan mode); `echo "line1\nline2"` and `cat "file\nname"` (quoted literal newlines) remain Read. |
| Pointers | `crates/tact/src/tool/readonly_shell.rs` (`split_plain_command`, `GIT_GLOBAL_OPTIONS`, `find_git_global_option`, `git_has_unsafe_global_option`); regression tests in the same file; [Ch 10](./10_chapter_permission.md) §7. |

## 2. 2026-08-13 — OpenAI-compatible Chat Completions surfaces transport failures as `LlmError::Request`

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The OpenAI-compatible adapter reported send/connection/response-read failures as `LlmError::Unsupported("HTTP request failed: …")`, conflating "this endpoint can't do something" with "the request never went through"; token counts were cast `u64 as u32` (truncating, misleading on oversized values); a malformed tool-call `arguments` payload was silently replaced by `{}` with no trace. |
| Decision | New `LlmError::Request(String)` variant for request transport / deserialization errors (API HTTP errors keep `HttpError`). Both streaming and non-streaming paths now send the already-serialized JSON bytes (`body` + explicit `Content-Type`) instead of re-serializing via `.json()`. Token counts convert `u64 → u32` saturating (`u32_token_count`). Malformed tool-call arguments log at `debug` (error, tool name, raw args) and fall back to an empty object. |
| Behavior after | A dead endpoint / dropped connection surfaces `request error: …` instead of `unsupported: …`; oversized token counts saturate instead of wrapping; malformed tool args are visible in debug logs. |
| Pointers | `crates/tact_llm/src/error.rs` (`LlmError::Request`), `crates/tact_llm/src/openai/compatible/mod.rs` (`OpenAiAdapter` chat/stream paths, `u32_token_count`, `tool_use_block_from_parts`); [Ch 22](./22_chapter_llm.md). |

## 2. 2026-08-13 — TUI input box soft-wraps long lines and maps the caret through wrapped rows

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Input-box height and line stats counted only explicit `\n` splits, so a single overlong line overflowed past the box's 3-row cap, and the caret/scroll math (logical rows) disagreed with the rendered text (caret drawn on the wrong row/column for long input). |
| Decision | `render/input.rs` gained `wrap_line` (character-boundary soft-wrap, CJK double-width aware; `Paragraph` stays unwrapped and draws exactly those rows) and `caret_in_wrapped` (logical cursor column → display row/column). Box height, line stats, scroll clamping, and cursor placement all now operate on display rows. |
| Behavior after | Long input lines wrap inside the box instead of overflowing; height auto-expands with wrapped rows (1–3 display rows + border); caret and scroll follow the wrapped row. Submitted text is unchanged. |
| Pointers | `crates/tui/src/render/input.rs` (`wrap_line`, `caret_in_wrapped`); `crates/tui/src/lib.rs` (input height); tests in `input.rs` (`wrap_line_splits_at_column_width`, `caret_in_wrapped_maps_logical_column_to_display_row`, `input_box_soft_wraps_overlong_line`, `input_box_scrolls_to_caret_on_wrapped_line`); [Ch 23](./23_chapter_tui.md) §6.2, §6.6. |

## 2. 2026-08-13 — Plan mode runs provably read-only shell commands (`ls`, `grep`, …)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Plan mode denies every `Write`-classified tool, and shell commands were always classified `Write` (except `sudo ` / `su ` → `High`), so even `ls` / `grep` — the inspection commands a planning agent needs most — were hard-denied in plan mode. |
| Decision | `PermissionPolicy::ShellCommand::resolve` now classifies a command string as `Read` when it is provably read-only, via a new conservative classifier `crates/tact/src/tool/readonly_shell.rs`: (1) a plain-command split that rejects any shell metacharacter (pipes, redirections, `$`, backticks, globs, escapes, …) so the classification cannot disagree with what `sh -c` runs; (2) a safelist of programs whose options alone cannot write (`ls`, `grep`, `cat`, `head`, `tail`, `wc`, `git status/log/diff/show/branch`, `find`/`rg`/`base64`/`sed` with dangerous flags excluded, …), mirroring OpenAI Codex's `is_known_safe_command` (`codex-rs/shell-command/src/command_safety/is_safe_command.rs`). Anything ambiguous stays `Write` — the classifier is deliberately false-negative-biased so a mutation can never run silently under plan mode. |
| Behavior after | In plan mode `ls -la`, `grep -rn x .`, `git status` run without prompting; `cargo test`, pipes, redirections, unknown programs and unsafe options (`find -delete`, `git push`, …) are still denied. `bash` and `background_run` share the same classification, so read-only commands are also auto-allowed in Default mode. |
| Pointers | `crates/tact/src/tool/readonly_shell.rs`; `crates/tact/src/tool/metadata.rs` (`ShellCommand::resolve`); tests in `crates/tact/src/tool/readonly_shell.rs` and `crates/tact/src/permission/mod.rs` (`plan_mode_allows_readonly_shell_commands_and_denies_others`); [Ch 10](./10_chapter_permission.md) §2, §4, §7. |

## 2. 2026-08-12 — async-openai switched from `vendor/async-openai` to a locally maintained fork at `../async-openai`

| Field | Value |
|-------|-------|
| Type | `docs` (dependency management) |
| Symptom / motivation | The vendored copy under `vendor/async-openai` (2026-08-10 entry) worked, but it duplicated the whole crate in-tree: every upstream sync meant diffing, re-applying patches and keeping the crate-level doctests green under Tact's minimal feature set. |
| Decision | The fork now lives as its own repository at `../async-openai` (clone of `https://github.com/rust-infra/async-openai`, branch `feat/tact`, commit `ca74607` = upstream main at 0.41.3), maintained directly with four local commits: (1) typed `context_management: Option<Vec<ContextManagementParam>>` field on `CreateResponse`; (2) `ReasoningEffort::Max` variant; (3) doctest feature gates for the two `client.chat()` examples; (4) package renamed `async-openai-local` with `[lib] name = "async_openai"` (examples dropped from the fork workspace since they still reference the upstream package name). Tact's workspace dependency becomes `async-openai-responses = { package = "async-openai-local", path = "../async-openai/async-openai", version = "0.41.3", features = ["responses", "byot"] }`; `vendor/async-openai/` is deleted. Code keeps `use async_openai_responses::…` unchanged. |
| Behavior after | No user-visible change: the wire body still carries `context_management` when a threshold is configured. Maintenance moved out-of-tree: patch the local fork (`/Users/rg/Projects/async-openai`, branch `feat/tact`) instead of re-vendoring. |
| Pointers | `/Users/rg/Projects/async-openai` (fork, commits `7de8bb4` / `5e22785` / `12488eb` on `feat/tact`); `Cargo.toml` `async-openai-responses` dependency; `crates/tact_llm/src/openai/responses/convert.rs` (`create_response` builder injection); [Ch 22](./22_chapter_llm.md) §6.2. |

## 2. 2026-08-12 — Worktree storage migrated from JSON files to SQLite (`WorktreeStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Worktree metadata + audit log lived in a single JSON index (`worktrees/index.json`, `Store<WorktreeIndex>`) with read-modify-write and no transaction; duplicate-name checks raced with index writes. |
| Decision | Worktree state moved into the existing `<workdir>/.tact/tact.db` as `worktrees` + `worktree_events` tables via a new async `WorktreeStore` trait (`crates/tact/src/store/worktree_store/`, `SqliteWorktreeStore` with sqlx). `worktrees.name` is UNIQUE (concurrency backstop); the autoincrement `id` preserves insertion order; `worktree_events` orders by its own `id`. Added `session_id` column + index, filled from the tool context at `worktree_create`. `WorktreeManager` became an async facade over `Box<dyn WorktreeStore>`; `SharedWorktreeManager` dropped its mutex (`Arc<WorktreeManager>`, the pool serializes writes — `worktree_run` no longer blocks other worktree tools). Legacy `worktrees/index.json` is not read anymore and is left on disk. With this change, no domain module uses the JSON store (`StoreRoot`/`Store`/`CollectionStore` remain as a generic primitive with their own unit tests). |
| Behavior after | Lanes and events persist in `tact.db` (old `worktrees/index.json` entries are gone unless exported manually); `worktree_*` surfaces unchanged; `session_id` appears in worktree records. |
| Pointers | `crates/tact/src/store/worktree_store/{mod,sqlite}.rs`, `crates/tact/src/worktree/mod.rs`, `crates/tact/src/tool/worktree.rs`, `crates/tact-ui/src/{headless,interactive}.rs`; [Ch 1](./01_chapter_store.md) §5–6, [Ch 15](./15_chapter_worktree.md) §2–5. |

## 2. 2026-08-12 — Team storage migrated from JSON files to SQLite (`TeamStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The roster lived in a single JSON index (`team/config.json` with a `TeamConfig` wrapper) and inboxes as one JSONL file per owner (`team/inbox/{owner}.json`). Both used read-modify-write with no transaction and no cross-process lock; duplicate-name checks raced with roster writes. |
| Decision | Team state moved into the existing `<workdir>/.tact/tact.db` as `teammates` + `inbox_messages` tables via a new async `TeamStore` trait (`crates/tact/src/store/team_store/`, `SqliteTeamStore` with sqlx). `teammates.name` is the PRIMARY KEY; duplicate spawn is rejected with `INSERT OR IGNORE` + `rows_affected == 0` (keeps the `teammate {name} already exists` error and stays race-free). `inbox_messages` gains an autoincrement `id` so reads preserve insertion order (the legacy JSONL append semantics) plus an `owner` index. `TeammateManager` became an async facade over `Box<dyn TeamStore>`; `SharedTeammateManager` dropped its mutex (`Arc<TeammateManager>`, the pool serializes writes). Legacy JSON files are not read anymore and are left on disk. |
| Behavior after | Roster and inboxes persist in `tact.db` (old `team/` JSON entries are gone unless exported manually); `spawn_teammate` / `broadcast` / `read_inbox` / `plan_approval` / `shutdown_*` surfaces unchanged; cross-process inbox writes no longer race on file appends. |
| Pointers | `crates/tact/src/store/team_store/{mod,sqlite}.rs`, `crates/tact/src/team.rs`, `crates/tact/src/tool/team.rs`, `crates/tact-ui/src/{headless,interactive}.rs`; [Ch 1](./01_chapter_store.md) §5–6, [Ch 14](./14_chapter_team.md) §3–5. |

## 2. 2026-08-12 — Cron & background tasks migrated from JSON files to SQLite (`CronStore` / `BackgroundStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Cron persisted as a single JSON index (`cron/scheduled_tasks.json` with a `next_id` counter) and background as one JSON file per record (`background/tasks/{id}.json` plus an in-memory `Mutex<HashMap>` mirror). Both used read-modify-write with no transaction and no cross-process lock; the background manager held duplicate state (disk + memory) that could drift. |
| Decision | Cron and background moved into the existing `<workdir>/.tact/tact.db` as `cron_tasks` + `background_tasks` tables via new async traits `CronStore` (`crates/tact/src/store/cron_store/`) and `BackgroundStore` (`crates/tact/src/store/background_store/`), mirroring the `TaskStore` pattern. `cron_tasks` ids come from `INTEGER PRIMARY KEY AUTOINCREMENT` surfaced as 8-hex strings (`format!("{rowid:08x}")`) — same wire contract as the legacy index; `background_tasks` keeps the timestamp-millis hex `id` with a `CHECK`-constrained status. Both tables gained a `session_id` column + index, filled from the tool context at `cron_create` / `background_run`. `CronScheduler` / `BackgroundManager` became async facades; `SharedCronScheduler` dropped its mutex (`Arc<CronScheduler>`) and `BackgroundManager` dropped its in-memory mirror (the DB is the single source of truth; the spawned tokio task writes back through a cloned store handle). Legacy JSON files are not read anymore and are left on disk; `TactPath::cron_dir()` / `CRON_SUBDIR` removed as dead code. |
| Behavior after | Cron ids restart at `00000001` (legacy entries are gone unless exported manually from `.tact/cron/`); `cron_*` / `background_*` / `/background` surface unchanged; startup orphan repair (`running` → `error`) now sweeps the table; `session_id` appears in cron JSON and background records. |
| Pointers | `crates/tact/src/store/cron_store/{mod,sqlite}.rs`, `crates/tact/src/store/background_store/{mod,sqlite}.rs`, `crates/tact/src/cron/mod.rs`, `crates/tact/src/background.rs`, `crates/tact/src/tool/{cron,background_run}.rs`, `crates/tact-ui/src/{headless,interactive,driver}.rs`; [Ch 1](./01_chapter_store.md) §5–6, [Ch 13](./13_chapter_background.md) §2–5. (Ch 16 was removed with the cron feature on 2026-08-14.) |

## 2. 2026-08-11 — Tasks migrated from JSON files to SQLite (`TaskStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Tasks persisted as one JSON file per record (`tasks/task_{id}.json`) plus a `tasks/index.json` next-id counter. ID allocation and dependency edges (`blockedBy` / `blocks` mirrored on both records) were read-modify-write with no transaction and no cross-process lock; completing a task required an O(n) scan to clear edges. |
| Decision | Tasks moved into the existing `<workdir>/.tact/tact.db` as `tasks` + `task_dependencies` tables via a new `TaskStore` trait (`crates/tact/src/store/task_store/`, `SqliteTaskStore` with sqlx). Edges are rows (composite PK, `INSERT OR IGNORE`), no mirrored fields, no foreign keys; every mutation runs in a `BEGIN IMMEDIATE` transaction, completion clears edges with one `DELETE`. IDs come from `INTEGER PRIMARY KEY AUTOINCREMENT` (`TaskIndex` removed). `TaskManager` became an async facade over `Box<dyn TaskStore>`; `SharedTaskManager` dropped its mutex (`Arc<TaskManager>`, the pool serializes writes). Added `session_id` column + index, filled from the tool context at `task_create`. Legacy JSON files are not read anymore and are left on disk. `tokio` features `macros` + `rt-multi-thread` added to `crates/tact/Cargo.toml` so `#[tokio::test]` works when building `-p tact` alone. |
| Behavior after | New task IDs start at 1 (old 1–233 records are gone unless exported manually from `.tact/tasks/`); dependency updates are atomic; `task_*` tools unchanged on the surface (`session_id` appears in task JSON/snapshots). |
| Pointers | `crates/tact/src/store/task_store/{mod,sqlite}.rs`, `crates/tact/src/task/mod.rs`, `crates/tact/src/tool/task.rs`; [Ch 1](./01_chapter_store.md) §6, [Ch 19](./19_chapter_persistent_tasks.md) §2–3. |

## 2. 2026-08-11 — Summarizer thinking budget clamped below `max_tokens`; Kimi K3 default reasoning reserve

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The 2026-08-10 summarizer budget change forwarded the configured Claude-style thinking budget to the compact summary request with the same `with_thinking(self.thinking_config())` call as the main loop, but the summarizer's `max_tokens` is independently capped at `min(20% × window, 2,000)`. Anthropic requires `budget_tokens < max_tokens` on the wire, so a default 8k/32k thinking budget produced an invalid request (`thinking.budget_tokens = 8,000` with `max_tokens = 2,000`) and local compaction failed with a 400 for every Anthropic user with thinking enabled. Separately, the reasoning reserve only treated DeepSeek as default-reasoning, but Kimi K3 also defaults thinking ON + effort high server-side, so Kimi summaries could still be starved by reasoning without an explicit effort. |
| Decision | New `compact_summary_thinking(configured_budget, summary_max_tokens)` clamps the forwarded budget to `summary_max_tokens - 1` (and disables thinking entirely for a degenerate ≤ 1-token output budget), applied to both the initial and continuation summarizer requests via a small builder closure. The input-side reservation still subtracts the configured budget (conservative). `compact_summary_reasoning_reserve_percent` now reserves the default high tier (75%) for `ProviderKind::Kimi` as well as DeepSeek. |
| Behavior after | Anthropic compaction with a large thinking budget sends `budget_tokens = max_tokens - 1` instead of failing with a 400; a budget that already fits passes through unchanged. Kimi K3 with no explicit effort gets the same 75% reasoning reserve as DeepSeek. |
| Pointers | `compact_summary_thinking` + `compact_summary_reasoning_reserve_percent` in `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`); tests `compact_summary_thinking_clamps_below_max_tokens`, `local_compact_clamps_thinking_budget_below_summary_max_tokens`, `compact_summary_reasoning_reserve_percent_tiers`; [Ch 5](./05_chapter_compact.md) §5 step 3. |

## 2. 2026-08-10 — Vendor async-openai locally as `async-openai-local` for typed `context_management`

| Field | Value |
|-------|-------|
| Type | `docs` (dependency management) |
| Symptom / motivation | OpenAI's Responses API officially supports `context_management` / `compact_threshold` (server-side compaction), and the official Python/Node SDKs expose it typed, but the Rust `async-openai` crate (latest 0.41.3, as of 2026-08-10) only defines `ContextManagementParam` without wiring it into `CreateResponseArgs` — so Tact had to inject it through the byot JSON path (`body["context_management"] = serde_json::json!(...)`). |
| Decision | Vendor the async-openai 0.41.3 source under `vendor/async-openai` and rename the package to `async-openai-local` so the path dependency only satisfies the Responses-protocol 0.41.x dependency and cannot collide with the legacy `async-openai 0.20` used by the Chat Completions path (`async-openai-responses = { package = "async-openai-local", path = "vendor/async-openai", ... }` in the workspace manifest; code keeps `use async_openai_responses::…`, no reference changes). Two source divergences from upstream: a typed `context_management: Option<Vec<ContextManagementParam>>` field on `CreateResponse`, and a `Max` variant on `ReasoningEffort` (upstream stops at `Xhigh`; DeepSeek / Kimi K3 accept `max`). `convert.rs` now builds both through the typed builder — `context_management(...)` setter and `Reasoning { effort: request.reasoning_effort.map(Into::into), summary }` via `impl From<OpenAiReasoningEffort> for ReasoningEffort` in `crates/tact_llm/src/types.rs` — instead of `serde_json::json!` / `Value::String` injection. `vendor/async-openai/README.fork.md` documents how to sync with upstream. |
| Behavior after | No user-visible change: the wire body still carries `context_management` when a threshold is configured. Maintenance is now local: new Responses fields can be added to the vendor without waiting for the upstream Rust crate. |
| Pointers | `vendor/async-openai/` (`README.fork.md`, `src/types/responses/response.rs`); `Cargo.toml` `async-openai-responses` dependency; `crates/tact_llm/src/openai/responses/convert.rs` (`create_response` builder injection); [Ch 22](./22_chapter_llm.md) §6.2. |

## 2. 2026-08-10 — Clear error when a Responses endpoint does not implement `/responses/compact`

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Compatible `/responses` endpoints (e.g. `opencode.ai/zen/go/v1`) often do not implement `POST /responses/compact` and answer 404 with an HTML page. The SDK's `compact_byot` then surfaced a JSON-deserialization error that dumped the entire HTML body, and the message could even match the transient-error retry list ("unavailable"), causing pointless backoff retries before failing. |
| Decision | `compact()` in the Responses adapter now sends the compact request through the raw shared HTTP client (same transport the SDK uses) so the status code is inspectable. HTTP 404/405 maps to `LlmError::Unsupported("endpoint does not support POST /responses/compact (HTTP {status}): native Responses compaction is not implemented by base URL {base_url}")` — deliberately worded to avoid the transient-error keywords so no retry loop is entered. Other non-2xx statuses use the existing `LlmError::HttpError { status, body }`. |
| Behavior after | On an endpoint without `/responses/compact`, triggering compaction immediately shows the clear message naming the missing endpoint and base URL (no HTML dump, no retries); the session state is left untouched. |
| Pointers | `compact()` in `crates/tact_llm/src/openai/responses/mod.rs`; test `compact_reports_missing_endpoint_clearly`; [Ch 22](./22_chapter_llm.md) §6.2, [Ch 5](./05_chapter_compact.md). |

## 2. 2026-08-10 — Responses adapter recovers when a compatible stream ends without a terminal event

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | A compatible `/responses` endpoint (e.g. `opencode.ai/zen/go/v1`) occasionally closes the SSE stream without any terminal event (`response.completed` / `response.incomplete` / `response.failed`). `ResponsesStreamState::finish()` hard-failed with `unsupported response state: OpenAI Responses stream ended without a terminal event`, aborting the whole agent turn even though the stream had delivered a complete `output_item.done` sequence or visible text. |
| Decision | A clean EOF without a terminal event is now treated as terminal when the stream itself is complete: if every announced item finished (`output_item.done` sequence contiguous and no pending `added`), the output is reconstructed from the done sequence; otherwise streamed visible text is recovered (same branch as the existing compatible-endpoint recovery). A minimal completed `Response` is synthesized and flows through the existing normalization/recovery paths, so stop reason inference (including `ToolUse` for tool calls) and provider-state baseline construction are unchanged. A missing compaction boundary (`pending_compactions` non-empty) and an empty stream remain hard protocol errors — recovery must never silently drop a compacted baseline. |
| Behavior after | Turns that previously died with "stream ended without a terminal event" now complete from the done sequence / streamed text when the response was fully delivered; genuinely empty or compaction-incomplete streams still fail loudly. |
| Pointers | `finish()` in `crates/tact_llm/src/openai/responses/stream.rs`; tests `no_terminal_event_recovers_from_complete_done_sequence`, `no_terminal_event_recovers_visible_text`, `no_terminal_event_empty_stream_is_error`, `no_terminal_event_with_pending_compaction_is_error`; [Ch 22](./22_chapter_llm.md) §6.2. |

## 2. 2026-08-10 — Local compaction reserves output for reasoning / thinking tokens

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | `compact_history_local_with_mode` (the summarizer behind every non-OpenAI-Responses compaction) sized the summarizer's `max_tokens` as `min(20% × window, 2,000)` and treated it as the **text** budget, but reasoning-effort providers (OpenAI o-series / DeepSeek / Kimi K3) count reasoning tokens inside that same `max_tokens` envelope. With an explicit `high`/`max` effort — or on DeepSeek, which defaults thinking ON + effort high server-side — reasoning burned most of the 2,000-token budget, leaving too little for the summary text, so every call hit `StopReason::MaxTokens` and the continuation loop (≤3, each with the same shared cap) ended up accepting a best-effort partial summary. The compact request also never forwarded the configured Claude-style thinking budget (unlike the main loop), and the input reservation ignored it. |
| Decision | Split the summarizer output budget: the summary **text** keeps the classic `min(20% × window, 2,000)`; when a reasoning effort is configured (or the provider is DeepSeek with no explicit effort) a tiered reserve is added **on top** (25/50/75/100% of the text budget for minimal|low / medium / high / xhigh|max, DeepSeek default ≈ high = 75%), so `max_tokens` on the wire = text + reserve and reasoning never starves the text. The summarizer now forwards the configured thinking budget (`with_thinking(self.thinking_config())`, matching the main loop) and subtracts it from the input-side reservation. No effort semantics are changed — this is budget accounting only. |
| Behavior after | Compaction summaries with reasoning effort configured (or on DeepSeek) get a larger wire `max_tokens` (e.g. high effort on a 128k window → 2,000 + 1,500 = 3,500) while the text portion still receives its full classic budget; the summarizer request carries the same thinking config as main-loop turns; the input reservation accounts for both reasoning and thinking headroom, and bails with the existing "too small" error when the window cannot fit the prompt after those reservations. |
| Pointers | `compact_summary_reasoning_reserve_percent` + budget math in `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`); tests `compact_summary_reasoning_reserve_percent_tiers`, `local_compact_reserves_reasoning_budget_and_forwards_thinking`, `local_compact_input_reservation_subtracts_thinking_budget`; [Ch 5](./05_chapter_compact.md) §5 step 3. |

## 2. 2026-08-10 — `background_run` streams live output to the tool card (bash-like)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | `background_run` buffered the whole command output with `Command::output()` and its tool card finalized instantly ("started"), so users saw nothing until they polled `check_background` — unlike `bash`, whose card streams output live. |
| Decision | Add a keep-live card contract: `ToolPresentationInfo.keep_live` (mapped from new `LiveOutputPolicy::Background`) makes the TUI leave the card active after `StepFinished`; the manager now reads stdout/stderr incrementally (`read_pipe` + `Utf8Decoder` + ~50ms throttled `ToolProgress`, live preview keeps last ~4 KB) and closes the card with a new `AgentUpdate::BackgroundTaskFinished { tool_id, success, message, output }` carrying ✓/✗, elapsed time and the capped final output. `background_run` passes a `BackgroundProgressSink` (tool_id + `ui_tx`) into `SharedBackgroundManager::run`; record persistence and the 120s timeout are unchanged. |
| Behavior after | `background_run cargo build` shows a spinner + live build output in the TUI card, then finalizes with ✓/✗ and duration when the process exits — even if the agent turn already ended. The model still has no completion push and must poll `check_background`. |
| Pointers | `crates/tact/src/background.rs` (`BackgroundProgressSink`, `run_background_process`); `crates/tact/src/tool/background_run.rs`; `LiveOutputPolicy::Background` in `crates/tact/src/tool/metadata.rs`; `AgentUpdate::BackgroundTaskFinished` in `crates/protocol/src/agent.rs`; TUI `on_step_finished` / `on_background_task_finished` in `crates/tui/src/widgets/state/app/agent.rs`; [Ch 13](./13_chapter_background.md), [Ch 25](./25_chapter_protocol.md). |

## 2. 2026-08-10 — `/background` slash command for background job status

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Background jobs started with `background_run` could only be inspected by asking the model to call the `check_background` tool; there was no direct TUI affordance, so users had to type a prompt just to poll a task. |
| Decision | Add a TUI slash command `/background` (`/background <id>` for one task) backed by a new `UserCommand::QueryBackground(Option<String>)`. The command driver (`crates/tact-ui/src/driver.rs`) calls the shared `ToolContext.background_manager.check(id)` — the same code path as the `check_background` tool — and emits `AgentUpdate::MdInfo` with a `## ⚙️ Background Tasks` fenced code block (or `AgentUpdate::Error` for an unknown id). The command is listed in `PALETTE_COMMANDS`, localized in `i18n.rs` (EN/ZH), and rendered with the `🖥` palette emoji. |
| Behavior after | `/background` prints one line per task (id, status, command); `/background <id>` prints the task's pretty JSON; an unknown id shows an error. No new state, no completion push — the command only reads the persisted/in-memory records. |
| Pointers | `UserCommand::QueryBackground` in `crates/protocol/src/agent.rs`; driver match arm in `crates/tact-ui/src/driver.rs`; `PALETTE_COMMANDS` in `crates/tui/src/widgets/state/mod.rs`; `execute_palette_command` in `crates/tui/src/handlers/mod.rs`; [Ch 13](./13_chapter_background.md), [Ch 23](./23_chapter_tui.md) §3. |

## 2. 2026-08-09 — Hosted web search for OpenAI Responses (`protocol = "responses"`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| PR | https://github.com/laohanlinux/tact/pull/62 (branch `feat/responses-web-search`) |
| Symptom / motivation | The Responses adapter only sent function tools, so OpenAI `protocol = "responses"` sessions had no hosted (provider-executed) web search; users had to wire an MCP `web_search` function tool or drop to Chat Completions. |
| Decision | Hosted web search is a **Responses-protocol capability**, independent of the endpoint/provider: the adapter injects `Tool::WebSearch` on every ordinary `/responses` request whenever `protocol = "responses"` is chosen (`create_response(..., native_web_search = true)`; `false` only for `/responses/compact`, which accepts no tools) — for OpenAI, DeepSeek, and custom OpenAI-compatible endpoints alike, with no per-provider switch (`OpenAiResponsesAdapter` has no `native_web_search` flag; `ResponsesCapabilities::hosted_tools` includes `WebSearch` for every Responses endpoint). The provider executes the search server-side; Tact only renders a tool card from real Step events (`StepStarted` on `output_item.added`, `StepFinished`/`StepFailed` on the first `output_item.done` per index; `in_progress`/`searching` at `done` is a defensive failure). `web_search_call` never becomes `ContentBlock::ToolUse`, and stop reason stays `completed`. Compatible endpoints that emit the search action as a `queries` array instead of the singular `query` are handled by `wire::normalize_web_search_call_query` (fills `query` for typed parsing only; raw items are replayed verbatim). `AgentUpdate::StepFailed` gained `arg_summary` so failed cards keep the query in the title. DeepSeek keeps the code path but remains rejected at config resolution (#57) until its Responses support is re-enabled. |
| Behavior after | Any `protocol = "responses"` session — OpenAI, DeepSeek, or custom OpenAI-compatible — automatically gets hosted web search; the TUI shows a `🔍 Web Search` card with the query as title and sources as expandable detail; failures carry status/query/action diagnostics. |
| Pointers | `crates/tact_llm/src/openai/responses/{convert,stream,wire,mod}.rs`, `crates/tact_llm/src/provider.rs` (`build_openai_responses`), `crates/tui/src/widgets/tool_widget.rs`, AGENTS.md "Hosted tools (Provider-executed) — design invariants", [Ch 22 §6.2.1.1](./22_chapter_llm.md). |

## 2. 2026-08-09 — Task-stats `[copy]` copies the last turn

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | After a turn finished, users had no one-click way to copy that turn's conversation from the task-stats row. |
| Decision | Append a `[copy]` button on each `📊 任务统计：` row; click copies log text from after the previous stats row (or session start) up to but not including the current stats row, skipping blanks and task-end separators. |
| Behavior after | Click `[copy]` on a stats line → clipboard gets that turn's user/assistant content; earlier turns are excluded. |
| Pointers | `add_task_stats_block` / `copy_turn_ending_at_stats` in `messages.rs`; mouse hit in `handlers/mouse.rs`; regression `copy_turn_ending_at_stats_copies_last_turn_only`. |

## 2. 2026-08-09 — Mermaid diagram copy popup (double-click → source)

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Successfully rendered Mermaid diagrams discarded their fence body when splicing ASCII art into the log, so users could not recover the source for re-editing. |
| Decision | Keep a `MermaidBlock { start_idx, end_idx, source }` per successful render; double-click opens a Mermaid popup; popup `y` copies the source. Log selection yank stays ASCII. |
| Behavior after | Double-click any diagram row → source popup (`y` / `j/k` / `Esc`); failed Mermaid still uses the code-card path. |
| Pointers | Spec `docs/superpowers/specs/2026-08-09-mermaid-diagram-copy-popup-design.md`; `finish_stream_code_block`; `popups/mermaid_popup.rs`; regressions `log_renders_streamed_mermaid_without_code_card`, `mermaid_popup_copy_uses_source_not_ascii`. |

## 2. 2026-08-09 — Mermaid sequence self-messages draw a U-shaped loop

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Self-messages (`A->>A`) rendered as a one-cell stub `<│◀`, which looked like broken chevrons rather than a return-to-self arrow. |
| Decision | Render self-messages as a two-row box-drawing loop (`│──┐` / `│◀─┘`, or left-side `┌──│` / `└─▶│` on the last participant) with the label beside the loop. |
| Behavior after | Self calls read as a clear U-turn on the lifeline; last-column self-messages loop left so the shape stays inside the diagram. |
| Pointers | `crates/tui/src/render/mermaid_sequence.rs` (`self_loop_rows`); regressions `self_message_draws_u_shaped_loop`, `self_message_on_last_participant_loops_left`. |

## 2. 2026-08-09 — Mermaid sequence labels no longer drop characters or shift columns

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The custom `sequenceDiagram` renderer dropped any label glyph whose display cells overlapped a lifeline (e.g. `submitTask` → `ubmitTask` / `submi│Task`), and left a ghost space after each width-2 CJK glyph so label rows were wider than lifeline/arrow rows — lifelines looked jagged and arrows appeared broken on multi-participant diagrams. |
| Decision | In `label_row`, clear continuation cells of wide glyphs to empty spans, and reflow label characters past occupied lifeline cells instead of skipping them. |
| Behavior after | Long ASCII and CJK arrow labels keep every character (split around `│` when needed) and every diagram row shares the same display width, so lifelines stay vertically aligned. |
| Pointers | `crates/tui/src/render/mermaid_sequence.rs` (`label_row`); regressions `cjk_label_keeps_same_display_width_as_lifeline_row`, `long_ascii_label_is_not_eaten_by_lifelines`, `self_message_keeps_lifeline_intact`. |

## 2. 2026-08-08 — TUI renders Mermaid sequence diagrams with its own renderer

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The upstream `ratatui-markdown` sequence renderer mishandled three common inputs: `participant A as 用户` aliases were shown verbatim, the `+`/`-` activation shorthand (`A->>+B`) created phantom participant columns (`+B`, `-B`, …), and 2-column CJK arrow labels could overwrite a lifeline (or be dropped), so labelled arrows looked misaligned. |
| Decision | Route `sequenceDiagram` fences in the TUI through Tact's own renderer (`crates/tui/src/render/mermaid_sequence.rs`); all other Mermaid diagram types keep using `ratatui-markdown`. The new renderer parses `as` aliases, strips `+`/`-` activation prefixes before participant lookup, and places label glyphs by display column only when every cell of the glyph's width is free. |
| Behavior after | Only declared participants render as columns; `A->>+B` targets participant `B`; CJK labels stay centered between lifelines and never overwrite a `│`. Unparseable sources still fall back to ordinary code rendering. |
| Pointers | `crates/tui/src/render/mermaid_sequence.rs`; routing: `crates/tui/src/render/render_md.rs` (`render_mermaid_block`); regression tests in `mermaid_sequence.rs`. |

## 2. 2026-08-08 — Subagent model picker uses its own provider

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | `/model-subagent` merged configured subagent models with API model IDs fetched from the main agent's active provider, so separate main/subagent providers could show the wrong models. |
| Decision | Query the `/models` endpoint using the resolved subagent provider's `base_url` and `api_key`; keep configured provider `models = [...]` as the primary candidates and preserve cache keying by `(base_url, api_key)`. |
| Behavior after | The subagent picker shows configured and API-discovered models belonging to the subagent provider. The main `/model` picker keeps using the main provider. |
| Pointers | `crates/tact_llm/src/models.rs`, `crates/tui/src/handlers/select.rs`; regression test `explicit_provider_model_query_uses_subagent_credentials`; design: `docs/superpowers/specs/2026-08-08-subagent-model-picker-provider-design.md`; plan: `docs/superpowers/plans/2026-08-08-subagent-model-picker-provider.md`. |

## 2. 2026-08-08 — DeepSeek and Kimi Responses remain configuration-gated

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The generic Responses adapter can be constructed for OpenAI-compatible endpoints, but DeepSeek/Kimi native compaction and state-continuation behavior are not verified to the production contract. Allowing them through normal config would make unsupported fallback behavior look supported. |
| Decision | Keep DeepSeek and Kimi `protocol = "responses"` rejected at config resolution. Lower-level adapter construction remains available for isolated endpoint tests; production configuration uses Chat Completions until native Responses capabilities are verified. |
| Behavior after | DeepSeek/Kimi users receive a clear configuration error instead of entering an unverified Responses path. OpenAI and explicitly configured custom OpenAI-compatible providers retain their existing Responses routes. |
| Pointers | `crates/tact/src/config/resolve.rs`; provider construction: `crates/tact_llm/src/provider.rs`; related design: `docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`; compaction behavior: Ch 5. |


## 2. 2026-08-08 — OpenAI Responses preserves unknown wire items

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The typed `async-openai` Responses enum rejects a newly introduced output item before Tact can preserve it, making forward-compatible provider state impossible. Tact also had no explicit request extension for Responses-only fields outside the shared Chat/Anthropic request model. |
| Decision | Parse the raw Responses envelope before typed normalization; normalize known items while retaining unknown input/output items as raw JSON. Add a `ResponsesRequestOptions` boundary consumed only by the Responses adapter, and expose conservative provider capability metadata without forking `async-openai` until a reproducible SDK blocker exists. |
| Behavior after | Unknown harmless stream events no longer abort a response. Unknown output items survive ordinary and streamed turns, session state serialization, and the next Responses request. Responses-only request options do not appear in Chat Completions or Anthropic payloads. |
| Pointers | `crates/tact_llm/src/openai/responses/wire.rs`, `request_options.rs`, `stream.rs`, `provider.rs`; design: `docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`; plan: `docs/superpowers/plans/2026-08-08-responses-compatibility-foundation.md`; compaction: Ch 5 and `docs/compaction.md`. |


## 2. 2026-08-08 — Main-area Markdown renders complete Mermaid fences as terminal diagrams

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tui/src/render/render_md.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/state/app/visibility.rs`, `crates/tui/src/widgets/state/stream_state.rs`, Ch 23 §6.7 |
| Symptom / motivation | Every explicit-language streamed fence — including ```mermaid — was promoted to a `CodeBlock` card overlay when it closed, so Mermaid source appeared as syntax-tinted code instead of a diagram. |
| Decision | Route complete, top-level `mermaid` fences through a shared `render_mermaid_block` helper (`ratatui-markdown::mermaid::render_mermaid` with the app-theme adapter) in `render_md.rs`; mark the buffered streamed fence as Mermaid in `stream_state.rs`; on a valid closed fence, `agent.rs` / `visibility.rs` splice the diagram lines into the log instead of pushing a `CodeBlock`. The code-card fallback is kept for invalid, unsupported, or unclosed Mermaid so no source is ever dropped. |
| Behavior after | A complete ```mermaid fence renders as a themed terminal diagram at the log width (nominal 80 columns in fixed-width Markdown paths); a valid streamed Mermaid block closes without creating a code card; invalid/unsupported/unclosed Mermaid and ordinary explicit-language fences keep the existing code-card path; width re-layout and viewport scrolling use the existing log layout/cache behavior. |
| Pointers | `render_md.rs` (`render_mermaid_block`, `route_mermaid_fences`) and tests `render_mermaid_sequence_returns_terminal_lines`, `render_markdown_mermaid_flowchart_uses_box_art`, `render_markdown_invalid_mermaid_falls_back_to_code`, `render_markdown_unclosed_mermaid_fence_keeps_source`; `stream_state.rs` (`code_block_is_mermaid`); `agent.rs` (`finish_stream_code_block`), `visibility.rs` (`flush_stream_pending`); regressions in `render_gap_tests.rs` (`log_renders_streamed_mermaid_without_code_card`, `log_falls_back_to_code_card_for_invalid_streamed_mermaid`, `flush_renders_streamed_mermaid_without_trailing_newline`, `flush_falls_back_to_code_card_for_unclosed_streamed_mermaid`), `cells/markdown.rs` (`markdown_cell_renders_mermaid_at_the_requested_width`); spec `docs/superpowers/specs/2026-08-08-mermaid-main-rendering-design.md`; plan `docs/superpowers/plans/2026-08-08-mermaid-main-rendering.md`; docs `book/23_chapter_tui*.md` §6.7 |

---


## 2. 2026-08-06 — OpenAI Responses exposes detailed reasoning summaries

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | `crates/tact_llm/src/openai/responses/convert.rs`, Responses reasoning request construction |
| Symptom / motivation | Ordinary OpenAI Responses requests asked for `reasoning.summary = auto`, so the streamed thinking block could contain only a short provider-selected summary even when reasoning was enabled. |
| Decision | Keep the Responses API `summary` field, but request `ReasoningSummary::Detailed` (`"detailed"`) whenever Tact enables reasoning. No changes are needed to stream parsing, which already consumes reasoning summary deltas. |
| Behavior after | OpenAI Responses thinking blocks request and display the detailed reasoning summary rather than the automatic summary level. |
| Pointers | Request conversion and regression assertion: `crates/tact_llm/src/openai/responses/convert.rs`; related Responses adapter: `crates/tact_llm/src/openai/responses/`. |



| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | `crates/tui/src/render/log.rs`, `crates/tui/src/render/cells/text.rs`, `crates/tui/src/render/log_render_tests.rs` |
| Symptom / motivation | The main log area wrapped lines to the full panel content width, then added a left indent while drawing ordinary messages. Full-width messages were consequently clipped by several columns at the right edge; selection redraws also used the wrong wrap width. |
| Decision | Subtract each message's actual indent before caching wrapped lines; use the same reply indent for the streaming row. Make `TextCell` selection wrapping use the already-available width after indentation. |
| Behavior after | Full-width ordinary, nested, and streaming text wraps within its actual drawable width, so right-edge characters are preserved. |
| Pointers | Layout and wrap cache: `render/log.rs`; text drawing: `render/cells/text.rs`; regression test: `log_full_width_nested_line_wraps_before_indentation_clip`. |

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact/src/agent/mod.rs` (`set_thinking_budget` / `set_reasoning_effort`), `crates/tact/src/config/mod.rs` (`update_llm_model_and_*`, `update_subagent_*`), `crates/tact/src/config/persist.rs` (TOML removals), `crates/tui/src/render/bar.rs` (`format_think_segment`), Ch 23 §6.6 |
| Symptom / motivation | On OpenAI Chat Completions (effort semantics), the bottom bar showed `think high(32K)`: `high` was the real `reasoning_effort`, but the `32K` was a stale `thinking_budget` left over from a previous budget-semantic model (claude / kimi-for-coding). The budget is meaningless for effort models and is never sent on the wire, yet it rendered next to the effort because neither the runtime setters, the in-memory config updaters, nor the TOML persist path cleared the other field. |
| Decision | Make effort and budget **mutually exclusive** end to end: `set_thinking_budget` clears `reasoning_effort` and `set_reasoning_effort` zeroes `thinking_budget`; the config update fns (`update_llm_model_and_thinking_budget` / `update_llm_model_and_reasoning_effort` and their subagent twins) do the same; the TOML persist fns remove the opposite key from the provider/subagent entry. In the status bar, `format_think_segment` now gives effort precedence (effort present → `think high`, stale budget ignored; budget only → `think 32K`), and an effort without any budget still renders instead of disappearing. |
| Behavior after | Effort-semantic models show `think high` (never `think high(32K)`); budget-semantic models show `think 32K`. Picking an effort removes the stored `thinking_budget` from config.toml; picking a budget removes `reasoning_effort`. Legacy configs that still contain both fields render effort-only and self-heal on the next `/model` persist. |
| Pointers | `format_think_segment` + tests in `crates/tui/src/render/bar.rs`; setters in `crates/tact/src/agent/mod.rs`; config updaters in `crates/tact/src/config/mod.rs`; TOML removals + tests in `crates/tact/src/config/persist.rs`; driver test `set_reasoning_effort_clears_stale_thinking_budget`; TUI test `applying_effort_pick_clears_stale_thinking_budget`; docs `book/23_chapter_tui*.md` §6.6 |

---

## 2. 2026-08-06 — Recovery retry messages include the underlying error

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact/src/recovery.rs` (`error_summary`), `crates/tact/src/agent/mod.rs` (backoff / compact-retry emit sites), Ch 6 §Recovery messages |
| Symptom / motivation | `[Recovery] backoff (1/10): retrying in 1.9s` said *when* the next retry happened but never *why* — the underlying transport error (timeout, connection reset, rate limit, …) was invisible, so a user watching 8+ backoff ticks had no idea what was failing. The compaction summary retries (`[compact retry 1/3] retrying in 1.9s`) had the same defect. |
| Decision | Add `error_summary` in `recovery.rs`: collapse whitespace/newlines to a single line, truncate at 200 chars with an ellipsis. The main-loop backoff message appends the full anyhow chain (outer context → root cause, joined by `": "`); both compaction retry messages append the client error string. Existing tags, counters, and delay text are unchanged, so tests matching `contains("Recovery") && contains("backoff")` keep passing. |
| Behavior after | Recovery retries report the reason, e.g. `[Recovery] backoff (2/10): retrying in 4.3s — http request failed: error sending request for url`. |
| Pointers | `error_summary` + unit tests in `crates/tact/src/recovery.rs`; emit sites in `crates/tact/src/agent/mod.rs`; docs `book/06_chapter_recovery*.md` §Recovery messages |

---

## 2. 2026-08-06 — Unknown provider names allowed as custom OpenAI-compatible providers

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact_llm/src/types.rs` (`ProviderKind::Custom`, `FromStr`), `crates/tact_llm/src/provider.rs` (`build_client`, `model_uses_effort`), `crates/tact_llm/src/hook_select.rs` (`body_hook_for`), `crates/tact_llm/src/models.rs` (`is_models_query_supported`), `crates/tact/src/config/resolve.rs` (`resolve_provider_kind`, `resolve_llm`, `resolve_subagent`), Ch 21 §3–§4 |
| Symptom / motivation | `ProviderKind::from_str` rejected any name outside `anthropic | openai | deepseek | kimi`, so `llm.provider = "moonshot"` (or any self-hosted / gateway provider) failed with "unknown provider" even though the entry carried a working OpenAI-compatible `base_url`. The config layer could not express third-party OpenAI-compatible endpoints. |
| Decision | Add `ProviderKind::Custom(String)` for every non-built-in name. Custom providers reuse the OpenAI protocol end to end: `build_client` dispatches to the OpenAI-compatible adapter (`chat_completions` default, `responses` opt-in), `body_hook_for` follows the same endpoint heuristics as `openai`, `/v1/models` supplementation is supported, and `reasoning_effort` is accepted. They have **no default `base_url`** — resolve fails with "base_url not configured" unless the entry sets one. Built-in gates are unchanged: `responses` protocol stays limited to `openai | deepseek | custom`, `reasoning_effort` to OpenAI-compatible providers (all but anthropic); the map-key validation loop in `resolve_llm` was removed (custom keys are no longer an error). `ProviderKind` lost `Copy` (it now owns a `String`); method receivers changed to `&self`. |
| Behavior after | `llm.provider` / `--provider` accepts any name. Non-built-in names behave as custom OpenAI-compatible providers and require an explicit `base_url` in `[llm.providers.<name>]`. Missing active entries still error at resolve time. |
| Pointers | `ProviderKind` in `crates/tact_llm/src/types.rs`; tests `provider_kind_from_str_accepts_unknown_as_custom` (tact_llm), `custom_provider_resolves_with_openai_protocol` / `custom_provider_without_base_url_errors` / `custom_provider_in_map_resolves` (tact config resolve); docs `book/21_chapter_config*.md` §3–§4, `config.example.toml` |

---

## 2. 2026-08-06 — Account poller reports each outage once instead of every backoff tick

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact-ui/src/account.rs` (`poll_loop`, `spawn_poller`), `crates/tui/src/widgets/state/app/agent.rs` (`handle_account_update` flash), Ch 22 §9 |
| Symptom / motivation | `spawn_poller` forwarded every failed balance / usage query as `AccountUpdate::Error`. On a persistent outage (e.g. network down) the TUI flashed an error every 10 s → 20 s → … → 5 min, forever — a notification storm ("骚扰") that obscured the real state of the app. |
| Decision | Extract the loop into a testable `poll_loop(query, tx, next_delay)` and add an `error_notified` flag: only the **first** failure of a consecutive outage is forwarded; later retries stay silent while backoff continues. A successful query resets the flag, so a fresh outage after recovery reports once again. `NotSupported` still terminates the loop silently (unchanged), and the explicit startup query + `/balance` command keep their one-shot error reporting (user-triggered, not spam). |
| Behavior after | One outage = one flash message, then silent retries with backoff until recovery; after recovery the normal 5–15 s polling resumes and the next outage flashes once again. |
| Pointers | `poll_loop` / `spawn_poller` in `crates/tact-ui/src/account.rs`; tests `poller_forwards_error_once_per_outage_then_resumes`, `poller_stops_on_not_supported_without_error_flash`; docs `book/22_chapter_llm*.md` §9 |

---

## 2. 2026-08-06 — Kimi Code usage quota query restricted to the official `https://api.kimi.com/coding` endpoint

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact_llm/src/account.rs` (`query_kimi_code_usage`, `kimi_usage_url_from_base_url`), `crates/tact_llm/src/provider.rs` (`is_kimi_usage_supported`, `is_account_query_supported`), `crates/tact-ui/src/account.rs` (`query_once`), Ch 22 §3 / §9 |
| Symptom / motivation | `kimi_usage_url_from_base_url` derived `{origin}/v1/usages` from any configured base URL, so a `kimi-for-coding` model behind a custom OpenAI-compatible proxy would send the proxy's API key to a guessed usage endpoint. `is_kimi_usage_supported` was `is_kimi_coding(&model)` — true for any proxy serving `kimi-for-coding` — so the TUI quota widget polled proxies too. |
| Decision | Mirror the DeepSeek / Kimi balance "credential boundary": `kimi_usage_url_from_base_url` returns the official URL only for HTTPS with the exact host `api.kimi.com` and a `/coding` path (a `/v1` suffix is accepted); anything else returns `None` and `query_kimi_code_usage` bails with "Kimi Code usage API is only available for the official endpoint https://api.kimi.com/coding". `ProviderInfo::is_kimi_usage_supported` requires the same official host/path, so `is_account_query_supported` is false for proxy configurations and the TUI hides the quota widget. `is_kimi_coding` itself is unchanged — it still identifies the Kimi Code platform (including proxies) for wire shape. |
| Behavior after | Kimi Code usage polling works only when `base_url` targets the official `https://api.kimi.com/coding` endpoint. Custom proxies (even with `kimi-for-coding`) report "not supported"; the API key is never sent to `api.kimi.com` from a proxy configuration. |
| Pointers | `kimi_usage_url_from_base_url` + test `kimi_usage_url_derivation` in `crates/tact_llm/src/account.rs`; `is_kimi_usage_supported` + test `is_kimi_usage_supported_only_for_official_endpoint` in `crates/tact_llm/src/provider.rs`; docs `book/22_chapter_llm*.md` §3 / §9 |

---

## 2. 2026-08-06 — DeepSeek balance query restricted to the official `https://api.deepseek.com` endpoint

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact_llm/src/account.rs` (`query_deepseek_balance`, `deepseek_balance_url_from_base_url`), `crates/tact_llm/src/provider.rs` (`is_deepseek_balance_supported`, `is_account_query_supported`), `crates/tact-ui/src/account.rs` (`query_once`), Ch 22 §3 / §9 |
| Symptom / motivation | `query_deepseek_balance` derived `{origin}/user/balance` from any configured base URL, so DeepSeek models behind a custom OpenAI-compatible proxy would send the proxy's API key to a guessed balance endpoint. DeepSeek only serves `GET /user/balance` on the official host; the fallback was wrong and could leak credentials to the wrong host or surface confusing 404/403 errors. |
| Decision | Mirror the Kimi "credential boundary": `deepseek_balance_url_from_base_url` returns the official URL only for an empty base URL (config default) or HTTPS with the exact host `api.deepseek.com` (a `/v1` suffix is accepted); anything else returns `None` and `query_deepseek_balance` bails with "DeepSeek balance API is only available for the official endpoint https://api.deepseek.com". `ProviderInfo::is_deepseek_balance_supported` gates `is_account_query_supported`, so the TUI bottom-bar balance widget is hidden for proxy configurations instead of showing errors. |
| Behavior after | DeepSeek balance polling / `/balance` works only when `base_url` targets the official endpoint. Custom proxies (even with a `deepseek-*` model) report "not supported"; the API key is never sent to `api.deepseek.com` from a proxy configuration. |
| Pointers | `deepseek_balance_url_from_base_url` + test `deepseek_balance_url_derivation` in `crates/tact_llm/src/account.rs`; `is_deepseek_balance_supported` + test `is_deepseek_balance_supported_only_for_official_endpoint` in `crates/tact_llm/src/provider.rs`; docs `book/22_chapter_llm*.md` §3 / §9 |

---

## 2. 2026-08-06 — `/tasks-dag` popup does not show tasks added while it is open

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tui/src/widgets/state/app/agent.rs` (`on_tasks_changed`), `crates/tui/src/render/popups/task_dag_popup.rs`, Ch 23 (TUI) |
| Symptom / motivation | The `/tasks-dag` popup rendered its Mermaid lines once when opened. `TasksChanged` updates refreshed `task_panel.snapshot` but never the popup, and the render loop only re-renders when the popup width changes — so tasks created while the popup was open (or between open and render) never appeared until closing and reopening. |
| Decision | `on_tasks_changed` now refreshes an open DAG popup: it re-runs `render_task_dag_lines` with the latest snapshot at the popup's current `render_width` (falling back to `DEFAULT_DAG_RENDER_WIDTH` before the first width-aware frame) and swaps `lines`/`mermaid_source` in place, keeping the scroll offset. This complements the existing width-change re-render in `render_task_dag_popup`; both paths are idempotent. |
| Behavior after | Newly created tasks appear in the open `/tasks-dag` popup immediately (next render frame) without closing it. |
| Pointers | `on_tasks_changed` in `crates/tui/src/widgets/state/app/agent.rs`; regression test `tasks_dag_popup_refreshes_when_new_tasks_arrive` |

---

## 2. 2026-08-06 — `/tasks-dag` renders missing dependency edges (asymmetric task store)

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact/src/task/mod.rs` (`update`, `clear_dependency`), `crates/tui/src/widgets/state/task_dag.rs`, Ch 23 (TUI) |
| Symptom / motivation | `/tasks-dag` showed task nodes but **no dependency arrows** for relationships created via `task_update`'s `addBlockedBy`: `update` mirrored `addBlocks` into the blocked task's `blocked_by`, but `addBlockedBy` never mirrored the blocker's `blocks` (outgoing DAG edge). `tasks_to_mermaid` draws edges only from `blocks`, so those dependencies were invisible. Additionally, `clear_dependency` (task completion) removed the completed id from others' `blocked_by` but left the completed task's own `blocks` — a ghost edge source. |
| Decision | `update` now mirrors both directions: `add_blocked_by` also pushes the current task into each blocker's `blocks` (deduped, sorted), symmetric with the existing `add_blocks` branch. `clear_dependency` additionally clears the completed task's `blocks`. Because `update` holds a copy fetched before `clear_dependency`, the local copy is cleared too so the final write-back does not resurrect ghost edges. |
| Behavior after | Every dependency — whether set via `addBlocks` or `addBlockedBy` — renders as `T{blocker} --> T{blocked}` in `/tasks-dag`. Completing a task removes its outgoing edges. |
| Pointers | `crates/tact/src/task/mod.rs` (`update` add_blocked_by branch, `clear_dependency`); tests `update_add_blocked_by_creates_reverse_outgoing_edge`, `completing_task_clears_blocked_by` |

---

## 2. 2026-08-06 — Task completion shows a stats block (elapsed · model · tokens)

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tui/src/widgets/state/app/messages.rs` (`add_task_stats_block`), `crates/tui/src/widgets/state/app/agent.rs` (`TaskComplete` branch), Ch 23 (TUI) |
| Symptom / motivation | After a task finished, the log showed only the task-end separator (elapsed label); token consumption and model name were visible only in the bottom status bar, which resets when the next task starts. A `// TODO Add task stats block` marker sat in the `TaskComplete` branch. |
| Decision | The `TaskComplete` branch now calls `add_task_stats_block()` right after `add_task_end_separator()`. The block reads already-frozen state — `last_prompt_elapsed_secs` (set by the separator), `status_bar.model_name` (from `ModelInfo`) and `status_bar.token_*` (from `TokenUsage`) — so no new stats struct or duplicate collection was introduced (YAGNI). It renders one markdown line `📊 任务统计：⏱ mm:ss · 🧠 model · N tokens (prompt X · completion Y · cache Z · reasoning W)` through the existing `add_system_message` path; empty parts (no model, zero tokens) are omitted. |
| Behavior after | Every completed task leaves a persistent stats line under the end separator showing elapsed time, model name, and token breakdown when available. Cancelled or failed tasks do not show the block. |
| Pointers | `add_task_stats_block` in `crates/tui/src/widgets/state/app/messages.rs`; tests `task_complete_appends_task_stats_block`, `task_stats_block_skips_empty_parts` in `crates/tui/src/widgets/state/app/agent.rs` |

---

## 2. 2026-08-06 — `/tasks-dag` renders Mermaid via ratatui-markdown (replaces meraid)

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tui/src/widgets/state/task_dag.rs`, `crates/tui/src/render/popups/task_dag_popup.rs`, `crates/tui/src/theme.rs`, root `Cargo.toml` (`ratatui-markdown` git dep), Ch 23 (TUI) |
| Symptom / motivation | The DAG popup used the `meraid` crate's Mono renderer (plain text, no theme, no markdown structure) and the workspace already carried an unused `ratatui-markdown` git dependency (its branch name `update-ratatui-0.30` was stale — the real branch is `chore/update-ratatui-0.30`, so the dependency could not even resolve). |
| Decision | `/tasks-dag` now renders through `ratatui-markdown`: `tasks_to_mermaid` still emits `flowchart TD`, then `render_task_dag_lines` wraps it as `## Tasks DAG` + ` ```mermaid ` block + a `### Legend` list mapping `#id` back to each task's subject (node labels stay narrow: status glyph `○`/`◐`/`✓` + `#id`, since the fork's mermaid grammar terminates `[...]` text at the first `]`). A `DagTheme` adapter maps the app `Theme` into `RichTextTheme`/`MermaidTheme` (light/dark via `MermaidTheme::for_background`), and the popup re-renders at its actual width on first frame (`render_width` cache). The root dependency was fixed to the correct branch with `default-features = false, features = ["markdown", "mermaid"]` (drops `image`/`scroll`/`tree`/`viewer`). |
| Behavior after | The popup shows a themed box-drawing flowchart plus a legend listing task subjects; the `y` copy shortcut still copies the raw Mermaid source. `meraid` is no longer a dependency of the tui crate. |
| Pointers | `crates/tui/src/widgets/state/task_dag.rs` (`tasks_to_mermaid`, `render_task_dag_lines`, `DagTheme`), `crates/tui/src/render/popups/task_dag_popup.rs` (width-aware re-render), tests `tasks_dag_popup_renders_mermaid_markdown`, `ratatui_markdown_renders_diagram_and_legend` |

---

## 2. 2026-08-06 — Compaction summary continues after MaxTokens truncation

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 5 §3 (Summarization call), Ch 5 §4 (Validation), `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`), `crates/tact/src/recovery.rs` (`MAX_CONTINUATION_ATTEMPTS`, `continuation_message`) |
| Symptom / motivation | When the compaction summary LLM call returned `MaxTokens` (output limit hit), the summary loop treated it as an invalid stop reason and bailed with `compaction summary ended with invalid stop reason: MaxTokens`. Compaction then failed outright even though the partial summary was perfectly usable — and with reasoning models the summary frequently runs up against its output budget. |
| Decision | The summarization call now has two independent recovery axes: transient transport errors still retry with bounded backoff; `MaxTokens` truncation appends the partial summary as an assistant message plus a continuation prompt (same `continuation_message` selector as the main loop: direct-resume on attempt 1, convergence prompt on later attempts) and re-calls, up to `MAX_CONTINUATION_ATTEMPTS` (3). The request is rebuilt per attempt with the growing message history `[User(summary prompt), Assistant(partial), User(continue), …]`. When continuations are exhausted, the partial summary is accepted as best-effort instead of failing (the Codex-style rebuild keeps recent real user messages anyway); `MaxTokens` is therefore no longer an "invalid stop reason". |
| Behavior after | A truncated summary emits `[compact continue n/3] summary truncated, continuing` Info updates and merges all partial blocks into the final summary; compaction succeeds instead of erroring. Refusal / other abnormal stop reasons and empty text still fail without replacing the old context. |
| Pointers | `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode` summary loop, stop-reason validation), tests `local_compact_continues_truncated_summary`, `local_compact_continues_through_multiple_truncations`, `local_compact_accepts_partial_summary_when_continuations_exhausted`, `crates/tact-ui/tests/recovery_compaction.rs` (`compact_summary_continues_truncated_response`), Ch 5 §3/§4 |

---

## 2. 2026-08-06 — First run auto-writes default config to ~/.tact/config.toml

| Field | Value |
|-------|-------|
| Type  | `feature` |
| Related | Ch 21 §3 (Config Sources and Priority), `config.example.toml`, `crates/tact/src/config/load.rs` |
| Symptom / motivation | The install script ships only the binary — no config. First launch with no config file always failed at resolve with "LLM provider not configured", and the user had to know to copy `config.example.toml` manually (the doc said so, but nothing told them at runtime). |
| Decision | `load_toml_config` now writes the default template to `~/.tact/config.toml` when no config exists on any search path, then parses and returns it. The template is embedded at compile time via `include_str!("../../../../config.example.toml")` so the first-run default never drifts from the checked-in example (currently `deepseek` + `protocol = "chat_completions"`). Only the user-global location is auto-created; project-level candidates (`./.tact/config.toml`, `./config.toml`) are never written so repos stay clean. A hint is printed: `[config] no config found; wrote default template to ... — edit it to add your API key`. |
| Behavior after | First run with no config anywhere: tact-ui creates `~/.tact/config.toml` (template with placeholder `api_key`), prints the edit hint, then fails at resolve on the placeholder key exactly as before — the user edits the file and launches again. Existing configs are never touched or overwritten. Explicit `--config /path` that does not exist is still an error (never auto-created). Write failure (unknown/unwritable HOME) falls back to the previous empty-default behavior. |
| Pointers | `crates/tact/src/config/load.rs` (`DEFAULT_CONFIG_TEMPLATE`, `write_default_config`, `load_toml_config`), `config.example.toml` (header comment), Ch 21 §3 |

---

## 2. 2026-08-06 — Session stats track RTK output-filter metrics

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `docs/token_usage_schema.md` (Session Stats Display), `crates/tact/src/hook/rtk_filter.rs`, `crates/tact/src/stats.rs` |
| Symptom / motivation | With `tools.rtk_filter = true`, bash outputs are piped through `rtk pipe`, but nothing measured whether the filter actually ran, how much output it removed, or how long it took — users could not tell if RTK was saving tokens or silently passing everything through. |
| Decision | `SessionStats` gains six relaxed-atomic counters (`rtk_calls`, `rtk_success_calls`, `rtk_failure_calls`, `rtk_saved_chars`, `rtk_input_chars`, `rtk_elapsed_ms`) mutated directly by the post-tool hook. Atomics (not plain `u64`) are required because hooks only receive `&Agent` (immutable). An attempt counts as a success only when `rtk pipe` exits 0 with non-empty stdout; saved chars are `raw_len − filtered_len` (saturating, counted in chars not bytes) on success only. `rtk_input_chars` accumulates every attempt's raw length (success or failure) so the session-wide savings rate reflects failed attempts as zero savings. The end-of-session summary gains an `RTK tokens saved` estimate using the 1 token ≈ 4 chars length heuristic and an `RTK savings rate` row (saved chars / input chars). |
| Behavior after | Whenever at least one RTK attempt was recorded, the session stats popup / exit summary shows `RTK calls (s/f)`, `RTK chars saved`, `RTK tokens saved` (chars/4), `RTK savings rate` (saved/input %), and `RTK time`. Rows are hidden entirely when `rtk_filter` is off or no bash output was filtered. Failed bash executions (`StepStatus::Failed`) are excluded from both filtering and RTK stats — their output reaches the LLM intact. |
| Pointers | `crates/tact/src/stats.rs` (`SessionStats::record_rtk`, RTK rows in `summary()`), `crates/tact/src/hook/rtk_filter.rs` (`pipe_through_rtk` → `(output, succeeded, elapsed_ms)`, `saved_chars`, `should_filter`, `create_rtk_post_tool_hook`), `crates/tact/src/hook/mod.rs` (`PostToolUseFn` now receives `StepStatus`), `docs/token_usage_schema.md` |

---

## 2. 2026-08-05 — Unify tool-family card labels (background + team)

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | Ch 7, Ch 26 (2026-07-28 entry: Distinct tool-card labels) |
| Symptom / motivation | Two families still had mixed labels: `background_run` (`⚙️ Background Run`) vs `check_background` (`🔍 Check`) — the bare `Check` did not say what it checks; and team collaboration tools `send_message` / `broadcast` / `read_inbox` / `plan_approval` (`✉️ Send` / `📢 Broadcast` / `📬 Inbox` / `✅ Approve`) had no `Team` prefix while `spawn_teammate` / `list_teammates` (`👥 Team Spawn` / `👥 Team List`) did. |
| Decision | Background family: rename `check_background` to `⚙️ Background Check`, sharing the `⚙️ Background` prefix with `background_run`. Team family: add the `Team` family name to the four collaboration tools, keeping their distinct icons (`✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`). Update the TUI `tool_display_name` fallback to match metadata in both families. Task stays on `# Task…` human titles via `format_task_tool_title`. |
| Behavior after | Every family reads as one: `⚙️ Background Run` / `⚙️ Background Check`; `👥 Team Spawn` / `👥 Team List` / `✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`; `⏰ Cron …`; `🌿 Worktree …`; `🔌 Shutdown …`. |
| Pointers | `crates/tact/src/tool/background_run.rs` (`CHECK_BACKGROUND_METADATA`); `crates/tact/src/tool/team.rs`; `crates/tui/src/widgets/tool_widget.rs` (`tool_display_name`) |

---

## 2. 2026-08-05 — `/model` 按 provider 分流 budget/effort + model→档位映射 + effort/model per-agent

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | `docs/superpowers/specs/2026-08-05-llm-presets-design.md`, Ch 21 (config: `[llm.model_profiles]`, `reasoning_effort` 校验), Ch 22 (§2 ProviderInfo 静态化, §6.3 wire 表) |
| Symptom / motivation | `/model` 对任何 provider 都弹同一 5 档 thinking budget，但 openai/deepseek/kimi-k3 实际发的是 `reasoning_effort`；effort 无选择入口、运行时不可改；effort 是进程全局共享（subagent 污染主 agent）；OpenAI Responses 的 effort 是 client 构建时 snapshot，运行时修改不生效；"模型↔档位"无静态配置。 |
| Decision | 1) `/model`（及 `/model-subagent`）第二步按 `model_uses_effort` 分流：openai/deepseek/kimi k3/k3-256k → effort 选择器（deepseek 3 档 low/high/max，kimi k3 3 档，openai 6 档 minimal..max，无 none 档）；anthropic/kimi coding 系 → budget 选择器。2) 新增 `[llm.model_profiles."<model>"]`（`thinking_budgets` / `reasoning_efforts` 数组）限定第二步档位，TOML 逐字段覆盖内置 `builtin_model_profiles()`。3) **effort/model per-agent**：`CreateMessageParams.reasoning_effort` + `AgentSettings.model/reasoning_effort`；删全局 `set_model`、`ProviderInfo.reasoning_effort`、`current_reasoning_effort_from_budget` 及 budget→effort 波段映射（无存量兼容）；`/model` 改发 `UserCommand::SetModel` / `SetReasoningEffort`（busy 排队）。4) wire 注入全部从 request 读；DeepSeek 纯 effort 驱动（None=不传，默认 ON+high，按官方文档）；Kimi k3 支持 effort（None=默认 high；不提供关闭 thinking——会路由到 K2.6）；OpenAI Responses `create_response` 不再 snapshot effort。5) 持久化：effort 语义写 provider/subagent 的 `reasoning_effort` 字段（`[llm.model_profiles]` 是静态选项集合，不被持久化触碰）；resolve 校验放宽为 openai/deepseek/kimi。 |
| Behavior after | `/model` 选 openai/deepseek/kimi-k3 模型 → effort 选择器（映射档位或 provider 默认）；选 anthropic/kimi-coding 模型 → budget 选择器（现状）。运行中改 model/effort 只影响当前 agent（主/subagent 独立），wire 立即跟随；Responses 也跟随。持久化后重启生效。Kimi 关闭思考被路由到 K2.6 的行为未提供 UI 入口。 |
| Pointers | `crates/tui/src/handlers/select.rs`（分流/选择器）、`crates/tact/src/config/{types,resolve,persist,mod}.rs`（model_profiles/校验/持久化）、`crates/tact_llm/src/{provider,deepseek,kimi,openai/*}.rs`（per-request effort/wire）、`crates/tact/src/agent/mod.rs`（SetModel/SetReasoningEffort）、Ch 21/22。 |

---

## 3. 2026-08-04 — `tact upgrade` self-upgrade command

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | README (CLI / self-upgrade), Ch 21 (config: `install_without_llm` path) |
| Symptom / motivation | Users had no in-place way to move to a newer release; upgrades meant re-running `scripts/install.sh` (or rebuilding from source) by hand. |
| Decision | Add a `tact upgrade` CLI subcommand. It scans the GitHub releases list (`GET /repos/{repo}/releases?per_page=100`) for the newest non-draft, non-prerelease release whose assets include `tact-ui-v<ver>-<triple>.tar.gz` for the current platform — skipping tags published without build assets (e.g. `v1.1.1` initially shipped with zero assets) — downloads the archive, verifies it against the release's published `SHA256SUMS`, and atomically replaces the running binary on Unix. Flags: `--check` (print only), `--yes` (skip the y/N prompt), `--repo owner/name` or `TACT_UPGRADE_REPO` (track a fork). Windows is not yet supported in-place; the command points users at `scripts/install.ps1`. The command resolves config via `install_without_llm` so no LLM provider is required. |
| Behavior after | `tact-ui upgrade --check` prints current vs latest-installable; `tact-ui upgrade` prompts and (on `y`/`--yes`) downloads, verifies SHA-256, and replaces the executable, printing a restart hint. A mismatched checksum aborts before replacement. |
| Pointers | `crates/tact/src/upgrade.rs` (`run_upgrade`, `find_latest_release_with_asset`, `replace_current_binary`), `crates/tact/src/config/cli.rs` (`CliCommand::Upgrade`), `crates/tact-ui/src/main.rs` (dispatch), README §3 Run |

---

## 2. 2026-08-04 — Google voice transcription honors standard proxy environment variables

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 21, Ch 23 |
| Symptom / motivation | The Google Speech-to-Text client was constructed with `reqwest::ClientBuilder::no_proxy()`. On networks where `speech.googleapis.com` is reachable only through `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`, recording completed but transcription bypassed the configured proxy and timed out or failed to connect. |
| Decision | Remove the Google client's forced proxy bypass and let reqwest apply its standard proxy-environment resolution. Keep API-key-safe error reporting: connection errors are not rendered with the request URL because the Google API key is currently carried in the query string. |
| Behavior after | Google voice transcription follows the process proxy environment, including the corresponding lowercase variable names and `NO_PROXY`; direct connections still work when no proxy is configured. A child-process regression test verifies that a request reaches a configured HTTP proxy rather than the original host. |
| Pointers | `crates/tact/src/voice/transcriber.rs` (`GoogleTranscriber::new`, `google_transcriber_honors_http_proxy`); Ch 21 (voice configuration), Ch 23 (TUI voice flow) |

---

## 2. 2026-08-02 — Pre-push hook no longer leaks `GIT_DIR`/`GIT_WORK_TREE` into `cargo test`

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | `scripts/check-rust.sh`, `.githooks/pre-push`, `crates/tact-ui/tests/subsystem_tools.rs` |
| Symptom / motivation | Git exports `GIT_DIR` / `GIT_WORK_TREE` into hook processes. The pre-push hook ran `cargo test -p tact-ui`, and the `worktree_create_lists_and_shows_status` test spawned `git` commands that inherited those variables. Instead of operating on the test's isolated temp repo, the commands targeted the real repo: the test's `git worktree add` registered a stray worktree, and its setup `git init`/`git add`/`git commit` appended destructive `init` commits to the active branch's HEAD — so `git push` could push a repository that had just been polluted by its own pre-push test run. |
| Decision | Clear the hook-injected variables in both hook entry points before any child process runs: `unset GIT_DIR GIT_WORK_TREE` at the top of `.githooks/pre-push` and of `scripts/check-rust.sh`. Also re-point `core.hooksPath` at `.githooks` so git actually uses the version-controlled hook (the previously installed `.git/hooks/pre-push` was a stale inline copy). |
| Behavior after | `git push` runs fmt/clippy/build/test with a clean git environment; integration tests create worktrees and commits only inside their `tact-tool-test-*` temp dirs, never in the real repo. |
| Pointers | `.githooks/pre-push`; `scripts/check-rust.sh`; `scripts/install-git-hooks.sh`; `crates/tact/src/worktree/mod.rs` (still `current_dir`-based; hooks must not leak env); `crates/tact-ui/tests/subsystem_tools.rs` |

---

## 2. 2026-08-02 — Google Cloud API-key voice transcription provider

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | Ch 21, Ch 23 |
| Symptom / motivation | Voice input supported OpenAI-compatible transcription and local `whisper.cpp`, but users with a Google Cloud Speech-to-Text API key had no direct provider. |
| Decision | Add `VoiceProvider::Google` using synchronous `POST {base_url}/speech:recognize?key=...` with base64 LINEAR16 mono 16 kHz WAV JSON. Reuse `voice.api_key`, `voice.language`, and `voice.model`; default to `https://speech.googleapis.com/v1` and `latest_short`. Limit Google recordings to `1..=60` seconds. Service Accounts, OAuth, long-running recognition, streaming, and automatic segmentation remain out of scope. |
| Behavior after | Configuring `provider = "google"` sends a short recording to Google Cloud and concatenates returned `results[].alternatives[0].transcript` values into the existing TUI input flow. Missing keys, HTTP failures, malformed/empty responses, and cancellation are reported without exposing credentials. |
| Pointers | `crates/tact/src/config/{types.rs,resolve.rs}`; `crates/tact/src/voice/transcriber.rs`; `docs/superpowers/specs/2026-08-02-google-voice-transcription-design.md`; Ch 21, Ch 23 |

---

## 2. 2026-08-02 — Compaction handoff is now a typed message cell

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Related | Ch 5 |
| Symptom / motivation | The Codex-style rebuild appended the handoff as a plain `Role::User` text message; the only "special handling" was string-prefix matching (`is_summary_message`). The model could not distinguish a system-generated handoff from a real user turn, consecutive `[User: summary][User: prompt]` messages risked provider-side merging, and detection was fragile (prefix-only, lost on non-Text cells). |
| Decision | Make the handoff a first-class message cell: `MessageKind::Summary` on `tact_llm::Message` (`#[serde(skip)]`, in-memory only — the Anthropic wire, OpenAI conversion, and JSONL transcripts stay byte-identical) plus `<context-handoff>` … `</context-handoff>` framing in the cell text. Detection is by kind first, with `SUMMARY_PREFIX` / tag string fallback for sessions reloaded from the SQLite store (which persists only role + content). |
| Behavior after | `build_compacted_history` / `compacted_context` emit a framed, kind-marked cell: `<context-handoff>\nThis conversation was compacted…\n\n{summary}\n</context-handoff>`. `collect_user_messages` skips it by type; reloaded sessions are re-detected by content. Wire format is unchanged for Normal messages; Anthropic never sees `kind`. |
| Pointers | `crates/tact_llm/src/content.rs` (`MessageKind`, `Message::with_kind/is_summary`); `crates/tact/src/compact/mod.rs` (`summary_message`, `is_summary_message`, `build_compacted_history`, `compacted_context`); `crates/tact/src/store/session_store/sqlite.rs` (`load_session`); `book/05_chapter_compact.md` |

## 2. 2026-08-02 — DeepSeek can now use the OpenAI Responses protocol

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | Ch 21, Ch 5 |
| Symptom / motivation | `protocol = "responses"` was rejected for every non-OpenAI provider, so DeepSeek was pinned to Chat Completions even though the Responses adapter is endpoint-agnostic and the DeepSeek endpoint can serve `/responses`. |
| Decision | Accept `responses` for the DeepSeek provider in `resolve_llm` and route `ProviderInfo::build_client()` by protocol: DeepSeek + `chat_completions` keeps the dedicated `DeepSeekAdapter`; DeepSeek + `responses` builds the same generic `OpenAiResponsesAdapter` used by OpenAI, pointed at the DeepSeek `base_url`. Automatic `context_management` compaction, `reasoning.effort` from `thinking_budget`, and Responses conversation-state continuation apply unchanged. Kimi and Anthropic still reject `responses`. |
| Behavior after | A DeepSeek entry may set `protocol = "responses"`; requests go to `{base_url}/responses` with automatic compaction and reasoning semantics. Explicit `POST /responses/compact` is not implemented by the DeepSeek endpoint (live-verified 2026-08-02), so DeepSeek + Responses compacts through the local summary pipeline and clears the stale baseline; OpenAI Responses keeps the strict no-fallback contract. The default remains `chat_completions`. |
| Pointers | `crates/tact/src/config/resolve.rs` (`resolve_llm` validation); `crates/tact_llm/src/provider.rs` (`build_client`); `docs/superpowers/specs/2026-08-02-deepseek-responses-design.md`; `docs/superpowers/plans/2026-08-02-deepseek-responses.md`; Ch 21 (config), Ch 5 (compaction) |

## 2. 2026-08-01 — Responses compact threshold now reaches ordinary `/responses` requests (native `context_management`)

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 5, Ch 22, Ch 23 |
| Symptom / motivation | `responses_compact_threshold` (and its derived value) was resolved and validated, but the resolved threshold was never handed to the Responses adapter: ordinary `stream_message` / `create_message` calls built `/responses` bodies with `context_management` hard-disabled (`None`). Automatic provider-side compaction was therefore silently off in production, and only the explicit `/responses/compact` path worked. |
| Decision | Wire the resolved threshold through the whole configuration → adapter chain and send it on **every ordinary** `/responses` request: `LlmSettings.provider_info()` → `ProviderInfo.responses_compact_threshold` → `OpenAiResponsesAdapter` → `create_response` (`context_management: [{ "type": "compaction", "compact_threshold": N }]`). Native state is persisted and replayed: the opaque baseline (`input_items`, `compaction_id`, `logical_context_hash`) is committed atomically with messages and replayed verbatim on later requests. Endpoints lacking native Responses compaction are unsupported — **no** local summary fallback. |
| Behavior after | A configured/derived threshold produces `context_management` on every ordinary `/responses` request (stream and non-stream). The endpoint may compact the baseline automatically mid-conversation; a returned `compaction` item round-trips as opaque state and is never rendered. Explicit compaction (`/compact`, auto trigger, recovery) sends `POST /responses/compact` and replaces the baseline atomically; diagnostics show item count and compaction id only, never `encrypted_content`. Regression tests assert the wire body carries `context_management` when configured and omits it when not. |
| Pointers | `crates/tact_llm/src/openai/responses/convert.rs` (`create_response` → `context_management`); `crates/tact_llm/src/openai/responses/mod.rs` (`OpenAiResponsesAdapter::build_wire_request`, wiremock regression tests); `crates/tact_llm/src/provider.rs` (`ProviderInfo.responses_compact_threshold`); `crates/tact/src/config/types.rs` (`LlmSettings::provider_info`); `crates/tact/src/config/resolve.rs` (threshold derivation); `crates/tact/src/agent/mod.rs` (`compact_responses_native`, atomic `replace_persisted_context_and_state`); `docs/token_usage_schema.md` (automatic vs explicit compaction accounting); Ch 5, Ch 22, Ch 23 |

---

## 2. 2026-08-01 — Empty fenced block after markdown list no longer hijacks the tail line into a code card

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 23, Ch 24 |
| Symptom / motivation | In the TUI log stream, an empty-language fenced block (plain ```) appearing immediately after an in-progress markdown list/paragraph could be promoted into a standalone code card too early. The trailing line that followed the fence then rendered inside the code card instead of staying in normal markdown flow, making the tail line look “swallowed” or mis-rendered. This was a Tact rendering bug, not a Responses-protocol issue. |
| Decision | Keep the existing code-card path for real streamed code blocks (for example ```rust), but stop promoting **empty-language** fences into code cards when they appear directly after an in-progress markdown paragraph/list. In that case, keep the fence line in the markdown paragraph buffer and let the normal markdown renderer handle it. Add a high-level log regression test for the list → empty fence → tail-line case, plus a low-level markdown test proving the parser layer itself did not lose the tail line. |
| Behavior after | A markdown list followed by an empty fence snippet no longer turns the remaining tail line into a `Click for full code` card. Real language-tagged streamed code blocks still render as code cards. |
| Pointers | `crates/tui/src/widgets/state/app/agent.rs` (stream fence promotion guard); `crates/tui/src/render/render_gap_tests.rs` (`log_markdown_list_then_empty_fence_stays_in_markdown_flow`); `crates/tui/src/render/render_md.rs` (`render_markdown_list_then_fenced_code_then_list_tail`); Ch 23, Ch 24 |

## 2. 2026-07-28 — Theme detection fallback wrong theme (Ink vs Retro)

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 23 |
| Symptom / motivation | `detect_terminal_theme()` doc comment said "Fallback: Retro" but the code returned `ThemeName::Ink`. The unit test matching this contract (`test_detect_terminal_theme_env_vars`) expected the fallback to be `Dark`, `Light`, or `Retro`, so it failed on `Ink`. CI broke consistently for any runner without `COLORFGBG` / `COLORTERM` and no macOS dark-mode override. |
| Decision | Change the fallback return from `ThemeName::Ink` to `ThemeName::Retro`, matching the doc comment and test expectation. |
| Behavior after | When no terminal theme env vars are set, `detect_terminal_theme()` returns `Retro` (neutral dark) instead of `Ink`. |
| Pointers | `crates/tui/src/theme_detection.rs` |

---

## 2. 2026-07-28 — Log left-border scrollbar residue

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 23 |
| Symptom / motivation | On Ink and similar themes, wide graphemes in Thinking card titles (e.g. 🧠) could briefly desync some terminals' cursors while the accent scrollbar thumb (formerly `█`) was painted. Ghost thumb cells then stuck on the Log left border as intermittent light-blue “shadows”. Because unchanged border cells are skipped by `Buffer::diff`, the residue persisted across frames. |
| Decision | After content and scrollbar draw, re-stamp the left vertical border every frame and mark those cells `CellDiffOption::AlwaysUpdate`. Switch the thumb glyph to half-block `▐` so a momentary desync is less visually harsh. |
| Behavior after | The left border is force-emitted each frame in the theme `border` color; accent residue from wide-title desync cannot persist on the chrome column. |
| Pointers | `crates/tui/src/render/log.rs` (`restamp_log_left_border`); `crates/tui/src/render/log_render_tests.rs` |

---

## 2. 2026-07-28 — Distinct tool-card labels for CRUD-style tool families

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | Ch 7, Ch 13–16, Ch 23 |
| Symptom / motivation | Cron / worktree / team family tools shared one display label (e.g. all cron ops showed `⏰ Cron`). Header titles looked identical unless the user parsed `arg_summary` JSON. Generic `visual_kind` also ignored metadata `display_name` and always used the TUI fallback map. |
| Decision | Append the verb to each shared family label (`⏰ Cron Create` / `Delete` / `List`, same pattern for Worktree / Team / Shutdown). Align `tool_display_name` fallbacks. Prefer non-empty presentation `display_name` when it differs from the raw tool id so metadata is the source of truth for Generic tools. Leave Task alone — it already uses `# Task…` human titles via `format_task_tool_title`. |
| Behavior after | Tool cards show distinct action labels at a glance. `background_run` / `check_background` fallbacks match metadata (`⚙️ Background Run` / `⚙️ Background Check`). |
| Pointers | `crates/tact/src/tool/{cron,worktree,team}.rs`; `crates/tui/src/widgets/tool_widget.rs` (`display_name_from_presentation`, `tool_display_name`) |

---

## 2. 2026-07-28 — Bash tool card label restored (`$ Bash`)

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 7, Ch 23 |
| Symptom / motivation | After binding builtin `ToolPresentation` beside handlers, `bash` used `display_name: "$ Shell"`. The TUI card showed **Shell** even though the tool id and legacy fallback remain `bash` / `$ Bash`. |
| Decision | Set `BASH_METADATA.presentation.display_name` back to `"$ Bash"`. Runtime still spawns `sh -c` (unchanged). |
| Behavior after | Tool cards and titles render `$ Bash` again for the `bash` tool. |
| Pointers | `crates/tact/src/tool/bash.rs`; fallback still `$ Bash` in `crates/tui/src/widgets/tool_widget.rs` |

---

## 2. 2026-07-28 — Voice keybind ate all keyboard input

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 21, Ch 23 |
| Symptom / motivation | With `voice.voice_keybind` set, the TUI `if let Some(keybind) = … else if …` chain treated *any* key as handled by the voice branch whenever the option was present. Non-matching keys never reached `handle_insert_mode` / Normal dispatch, so the input box appeared to reject typing. |
| Decision | Match the configured shortcut first; only then skip normal dispatch. On non-match, fall through to slash / overlay / mode handlers unchanged. |
| Behavior after | `voice_keybind = "ctrl+g"` toggles recording only on that chord. All other keys type and navigate as before. Unset keybind keeps the previous mouse-only path. |
| Pointers | `crates/tui/src/lib.rs` (key event dispatch); `crates/tui/src/widgets/state/app/voice.rs` (`toggle_voice_recording`); Ch 21 `[voice]`, Ch 23 §6.6 |

---

## 2. 2026-07-28 — Input title-bar border restored; voice button centered

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 23 |
| Symptom / motivation | Centering the voice label with space-padding plus a background style overwrote the Block top-border cells between the Input title and `🎙 Voice`, so the horizontal line looked “eaten”. |
| Decision | Render the left Input title and the voice label as two `Block` titles (left + `Alignment::Center`) instead of one padded span. Click hit-testing uses the same centered geometry. |
| Behavior after | With voice enabled, the top border remains visible between the Input label and the centered voice control on wide enough terminals. Narrow widths may still collide (ratatui left title paints after center). |
| Pointers | `crates/tui/src/render/input.rs` (`voice_title`, `update_voice_button_area`); Ch 23 §6.6 |

---

## 2. 2026-07-28 — Configurable voice recording keybind

| Field | Value |
|-------|-------|
| Type  | `feature` |
| Symptom / motivation | Voice recording could only be started via mouse click on the title-bar button. Keyboard-centric users had no way to trigger it without reaching for the mouse. |
| Decision | Add `voice.voice_keybind` config option accepting `ctrl+<char>` format (e.g. `"ctrl+g"`, `"ctrl+r"`). When set, pressing the configured shortcut toggles voice recording in any input mode (idle → record, recording → stop). When unset (default), voice remains mouse-only. The active keybind is shown in the help panel (`Ctrl+?`) under Global shortcuts. Only an exact keybind match consumes the event. |
| Behavior after | `[voice] voice_keybind = "ctrl+g"` in config.toml enables keyboard-triggered voice. The shortcut works globally (any input mode). Non-matching keys still reach Insert/Normal handlers. The help panel dynamically shows the configured key. Empty, multi-character, or non-ctrl keybinds are rejected at config resolution. |
| Pointers | Config: `crates/tact/src/config/types.rs`, `config/resolve.rs`, `config.example.toml`; TUI dispatch: `crates/tui/src/lib.rs` (global shortcut section), `crates/tui/src/widgets/state/app/voice.rs`; Help: `crates/tui/src/widgets/help_widget.rs`, `render/popups/help.rs`; i18n: `crates/tui/src/i18n.rs` (`help_voice_record_tmpl`); Ch 21, Ch 23 |

---

## 2. 2026-07-28 — Permission: shell Write risk, settings allow for High, headless ask defaults

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 10 |

**Symptom / motivation:** Three logic bugs: (1) `PermissionPolicy::ShellCommand` classified non-elevated commands as Read, so `bash` / `background_run` / `worktree_run` bypassed Default-mode prompts; (2) headless `ask_user` always denied, which made Default mode unusable without a TUI; (3) High-risk tools ignored settings **allow** rules and always asked.

**Decision:** Non-elevated shell → Write; `sudo`/`su` → High. Non-interactive `ask_user(tool, risk)` allows Write/Read once and denies High. Settings Deny/Allow apply at all risks; High without Deny/Allow still asks and skips the in-session bare allowlist.

**Behavior after:** Normal shell calls prompt (or headless-allow) like other writes. Project allow rules can approve High for a matching input pattern. Unattended High still needs Auto mode or an explicit allow rule.

**Pointers:** `crates/tact/src/permission/mod.rs`, `crates/tact/src/tool/metadata.rs`, `crates/tact/src/agent/tool_dispatch.rs`; Ch 10.

---

## 2. 2026-07-28 — `/model` thinking budget not synced to status bar

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 21, Ch 23 |

**Symptom / motivation:** After `/model` saved a new thinking budget (e.g. 32K), the bottom bar could still show the previous value (e.g. `think high(64K)`). Persist succeeded; the running agent and bar did not.

**Decision:** `UserCommand::SetThinkingBudget` is processed only after an in-flight task finishes. That task’s older `ModelInfo` overwrote the TUI’s optimistic update, and `set_thinking_budget` did not emit a fresh `ModelInfo`. Emit `ModelInfo` from `set_thinking_budget`, and expand/sync session `max_tokens` in the TUI apply path so `out` / `think` stay aligned.

**Behavior after:** Confirming a budget updates the status bar immediately; when the queued agent command runs, another `ModelInfo` resyncs `thinking_budget` and any auto-expanded `max_tokens`.

**Pointers:** `crates/tact/src/agent/mod.rs` (`set_thinking_budget` / `emit_model_status`), `crates/tact/src/config/mod.rs` (`update_llm_model_and_thinking_budget`), `crates/tui/src/handlers/select.rs` (`apply_model_and_budget_pick`), `crates/tact-ui/src/driver.rs`.

---

## 2. 2026-07-28 — Clickable voice-to-text input (title bar)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 21, Ch 23; `docs/superpowers/specs/2026-07-28-voice-to-text-design.md`; `docs/superpowers/plans/2026-07-28-voice-to-text-input.md` |

**Symptom / motivation:** Keyboard-only input is awkward for long prompts on macOS; users wanted hands-free capture with a chance to review before submit.

**Decision:** Add `[voice]` config (independent API key), `tact::voice` worker (cpal capture → WAV → OpenAI-compatible transcription), and a right-aligned title-bar button in the TUI. Successful transcripts insert at the UTF-8 cursor; `/help` in a transcript stays plain text until Enter. Recording/transcription run off the event loop; `Esc` or Stop cancels.

**Behavior after:** `enabled = false` (default) hides the control. `enabled = true` shows the button; missing `[voice].api_key` flashes a config hint on click. No interim transcription, auto-submit, or local Whisper in this release.

**Pointers:** `crates/tact/src/voice/`, `crates/tui/src/widgets/state/voice.rs`, `crates/tui/src/render/input.rs`, `crates/tui/src/handlers/mouse.rs`, `crates/tui/src/handlers/insert.rs`, `crates/tui/src/lib.rs`, `crates/tact-ui/src/interactive.rs`.

---

## 2. 2026-07-28 — Subagent metadata rendered in tool-card header

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 12, Ch 23; `docs/token_usage_schema.md` |

**Symptom / motivation:** Subagent `TokenUsage` and `ModelInfo` were forwarded to the shared parent UI channel as `ToolProgress` inline chunks, producing repetitive `⚡ N tokens` and `🤖 Model: …` lines in the output stream. TokenUsage also overwrote the main agent's bottom-bar meters.

**Decision:** Introduce `AgentUpdate::ToolMeta` — a dedicated update path that writes model name and token count directly to the parent tool card's header row, alongside the existing phase/duration info. The forwarder no longer emits `ToolProgress` chunks for these events and no longer forwards them to the shared channel. The tool-card meta row now shows `🤖 {model} · ⚡ {total}` for subagent invocations.

**Behavior after:** Bottom bar consistently shows main-agent token stats. Subagent model and token total appear in the tool card's meta row (e.g. `⠋ Running · 🤖 deepseek-v3 · ⚡ 4.2K · 3.2s`), updated live via `ToolMeta` and preserved on completion. No inline clutter in the output stream.

**Pointers:** `crates/tact/src/tool/subagent_ui.rs`, `crates/tui/src/widgets/tool_widget.rs`, `crates/tui/src/render/cells/tool.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/protocol/src/agent.rs`; `docs/token_usage_schema.md`; Ch 12, Ch 23.

---

## 2. 2026-07-27 — Permission settings persistence (JSON-based dynamic rules)

| Field | Value |
|-------|-------|
| **Type** | docs |
| **Related** | Ch 7, Ch 21; `docs/superpowers/specs/2026-07-27-permission-settings-design.md`; `docs/superpowers/plans/2026-07-27-permission-settings.md` |

**Symptom / motivation:** Permission decisions were only stored in session-scoped memory (`always_allowed_tools`). The "Always allow this tool" choice granted every invocation of a bare tool name with no parameter awareness, persisted nowhere between sessions, and there was no way to pre-configure deny or ask rules without modifying `config.toml` (a TOML file not designed for dynamic rule writes).

**Decision:** Introduce JSON-based permission settings with two scopes: `$HOME/.tact/settings.json` (global) and `<workdir>/.tact/settings.json` (project). Rules use a Claude-like tool-and-argument syntax (`tool(field:pattern)`) with glob matching. Precedence is `deny > ask > allow`, independent of array order. Project writes are atomic (temp file + rename), preserve unknown JSON fields, and suppress duplicates. Malformed files or invalid rules are soft failures (warn + skip). High-risk confirmation remains mandatory regardless of allow rules.

**Behavior after:** Dynamic allow/ask/deny rules live in JSON settings files — not in `config.toml`. "Always allow this tool" writes a parameter-aware rule (e.g. `bash(command:cargo test *)`) to the project file. Missing files are empty policies. The TOML `[permission].mode` continues to control mode only (`default` | `plan` | `auto`). Plan and Auto mode semantics are unchanged.

**Pointers:** `crates/tact/src/permission/settings.rs`, `crates/tact/src/permission/mod.rs`, `crates/tact/src/consts.rs`, `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact/src/tool/subagent.rs`, `crates/tact-ui/src/interactive.rs`, `crates/tact-ui/src/headless.rs`; `docs/superpowers/specs/2026-07-27-permission-settings-design.md`; `docs/superpowers/plans/2026-07-27-permission-settings.md`; `docs/state_machines.md §5`; `config.example.toml`; Ch 7, Ch 21.

## 2. 2026-07-27 — Log scroll restores the theme background

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`; `docs/superpowers/plans/2026-07-27-log-scroll-artifact-fix.md` |

**Symptom / motivation:** After scrolling away from a code-card or other styled Log content, a normal text row could retain a prior frame's background style. The artifact was especially visible on the dark Ink theme as a shadow behind text.

**Decision:** Keep the Log viewport reset and make `TextCell` explicitly apply the active `theme.bg` while writing each normal glyph. The rule is theme-independent; card and overlay layers keep their existing backgrounds and order.

**Behavior after:** Any ordinary Log row newly exposed by scrolling has the active theme's background, while its foreground styling and selection reverse modifier remain intact. No Ink-only branch or global terminal clearing policy is used.

**Pointers:** `crates/tui/src/render/log.rs`; `crates/tui/src/render/cells/text.rs`; `crates/tui/src/render/log_render_tests.rs`; `docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`; Ch 23.

---

## 2. 2026-07-27 — Subagent popup shows its model

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 12, Ch 23; `docs/token_usage_schema.md` |

**Symptom / motivation:** The live/completed `spawn_subagent` popup showed the child call's token total, cache rate, and prompt context, but not the model that produced them. The agent emits `ModelInfo`, but the subagent UI forwarder discarded that event.

**Decision:** Format the child `ModelInfo` as a structural popup-transcript line: `🤖 Model: {model}`. Keep it on the `ToolProgress` path rather than forwarding it to the shared parent UI channel.

**Behavior after:** Every child model call adds its model name to that child popup alongside its existing token line. The parent bottom bar retains the parent agent's model name (see 2026-07-28 for the matching TokenUsage fix).

**Pointers:** `crates/tact/src/tool/subagent_ui.rs`; `docs/token_usage_schema.md`; Ch 12, Ch 23.

---

## 2. 2026-07-27 — Ink themes + unified popup chrome

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 21, Ch 23; `docs/tui_rendering.md` |

**Symptom / motivation:** Default theme was `retro`; popup overlays had inconsistent border types, hardcoded colors, no shared chrome.

**Decision:** Added `ink`/`ink-light` themes with pixel-matched colors, `heading`/`version`/`muted` Theme fields, unified `render_popup_chrome` for all overlays. Default changed to `ink`.

**Behavior after:** Default theme is `ink`; all overlay popups share a consistent border, title bar (bold title, `[x]` hint), and footer layout; popup code is DRY.

**Pointers:** `crates/tui/src/theme.rs`, `crates/tui/src/render/popups/mod.rs`, `crates/tui/src/render/render_md.rs`, `crates/tact/src/config/resolve.rs`

---

## 2. 2026-07-26 — Subagent tool renamed `task` → `spawn_subagent`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 7, Ch 10, Ch 11, Ch 12, Ch 19 |

**Symptom / motivation:** The subagent spawn tool was named `task`, sharing a prefix with the four persistent-task tools (`task_create` / `task_get` / `task_list` / `task_update`) while meaning something entirely different. Models and readers conflated "the `task` tool finished" with "the task record is complete" — an observed failure left a checklist item Pending after its subagent had returned. Ch 1 / 11 / 12 / 19 each carried a disambiguation note as a workaround.

**Decision:** Rename the tool to `spawn_subagent` (verb + object, matching its description); wrapper type `TaskTool` → `SpawnSubagentTool`, handler `task()` → `spawn_subagent()`. The persistent-task tools keep the `task_*` prefix. `spawn_subagent` remains `CapabilityRisk::High` and remains a scheduling barrier.

**Behavior after:** The model-facing tool name is `spawn_subagent`; no tool named `task` exists. Restored sessions containing historical `task` tool_use blocks still load — `load_history` renders only `Text` blocks and the router resolves names only for live dispatch, so an absent name causes no error. The in-memory `always_allowed_tools` list is session-scoped, so nothing needs migrating.

**Pointers:** `crates/tact/src/tool/subagent.rs`, `crates/tact/src/tool/registry.rs`, `crates/tact/src/permission/mod.rs`

---

## 2. 2026-07-26 — `TasksChanged` no longer appends a Log card

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 19, Ch 23 |

**Symptom / motivation:** `on_tasks_changed` used to append a `📋 # Task.N · …` system message, duplicating the `task_*` tool row that already renders the same title. Commit `4116c23` commented the emission out as collateral damage rather than removing it, leaving `format_tasks_log_card` behind `#[allow(dead_code)]` and `tasks_changed_shows_panel_and_appends_log` red.

**Decision:** Keep the tool row as the only Log representation. Delete `format_tasks_log_card`, `focus_changed_task`, and `primary_action_for_change`; rewrite the test to assert the sticky updates while the Log length stays unchanged. `AgentUpdate::TasksChanged` keeps its `reason` field — producers and the protocol are unchanged.

**Behavior after:** A `task_create` / `task_update` call produces one Log row (the tool card) plus a sticky refresh, never two.

**Pointers:** `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/state/task_panel.rs`

---

## 2. 2026-07-26 — Sticky host separates tabs from body

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** `sticky_host_content_height` reserved `1 + body` rows and the renderer drew the body at `inner.y + 1`, so the tab row (`[Tasks] [Subagent] …`) sat flush against `── Pending ──` / the subagent log, with the Log box border immediately above. Everything read as one crowded block.

**Decision:** Reserve one extra row (`2 + body` for Tasks, `3 + header + lines` for Subagent) and draw a muted full-width `─` rule between the tab row and the body.

**Behavior after:** The expanded sticky shows tabs, a hairline, then content. Collapsed height is unchanged at one row.

**Pointers:** `crates/tui/src/render/task_panel.rs`

---

## 2. 2026-07-26 — Bash non-zero exit is Failed

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 7 |

**Symptom / motivation:** `bash` collected `ExitStatus` but ignored it, so `cargo test` failures and other non-zero exits still rendered as `Success · …` while stdout/stderr showed the error.

**Decision:** After the process exits cleanly (no timeout/cancel/pipe failure), `!status.success()` returns `Err` via `error_with_partial` (`exit code N` or `terminated by signal`), mapping to `StepStatus::Failed` with captured output retained for the model.

**Behavior after:** Non-zero shell exits show Failed in the TUI; zero exits unchanged.

**Pointers:** `crates/tact/src/tool/bash.rs`

---

## 2. 2026-07-25 — Subagent sticky tab (clean main Log)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 12, Ch 23; `docs/superpowers/specs/2026-07-25-subagent-sticky-pane-design.md` |

**Symptom / motivation:** Subagent shared the parent `ui_tx`, so Stream/Step/Thinking mixed into the main Log and child `TokenUsage` overwrote the bottom bar.

**Decision:** Tag subagent updates as `AgentUpdate::Subagent`; sticky host tabs Tasks | Subagent; main Log keeps only the parent `task` tool row; `RequestSelect*` passthrough; first-run auto-tab, later badge.

**Behavior after:** Nested work is visible under Subagent; main Log and ctx meter stay parent-scoped during `task`.

**Pointers:** `crates/tact/src/tool/subagent_ui.rs`, `crates/tui/src/widgets/state/subagent_pane.rs`, `crates/tui/src/render/task_panel.rs`

---

## 2. 2026-07-25 — Subagent sessions linked via `ref_id`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 1, Ch 12; `docs/superpowers/specs/2026-07-25-subagent-session-ref-design.md` |

**Symptom / motivation:** `task` subagents had no `session_id` / store — turns, token usage, and DeepSeek `user_id` isolation were missing; crashes mid-`task` lost all subagent history.

**Decision:** Each subagent gets a new session row with `sessions.ref_id` = parent id (`''` if parent has none). `list_sessions` returns only top-level (`ref_id = ''`). `delete_session` cascades children. No `SessionLock` on children.

**Behavior after:** Subagent messages / `token_usages` persist under the child id; `--list-sessions` stays parent-only; deleting a parent removes its children.

**Pointers:** `crates/tact/src/tool/subagent.rs`, `crates/tact/src/store/session_store/sqlite.rs`, `ToolContext.session_id` / `session_store`

---

## 2. 2026-07-25 — Ctx meter visible at low usage

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `docs/token_usage_schema.md` |

**Symptom / motivation:** With a 1M context window, ~1% usage (`13.7K/1M`) painted `▏` (1/8 block). Next to `·` that hairline read as an empty bar, so the numeric `1%` looked wrong.

**Decision:** Any positive fractional cell clamps to at least `▍` (3/8); never fall back to `·` for `frac > 0`.

**Behavior after:** Non-zero ctx usage always shows a clearly filled partial in `[…]` (e.g. 1% → `[▍·······]`).

**Pointers:** `crates/tui/src/render/bar.rs` (`partial_block_char` / `render_usage_bar`)

---

## 2. 2026-07-25 — Task tool titles, short Log cards, sticky tree, `/tasks-dag`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 11, 19, 23, 25; `docs/superpowers/specs/2026-07-25-task-tool-ui-redesign.md` |

**Symptom / motivation:** `task_*` tools dumped raw JSON; Log cards repeated full checklists; dependency graph was hard to see in-terminal.

**Decision:** Human tool titles (`# Task.N · …`); sticky defaults expanded as a `blocks` tree with `#id`; `/tasks-dag` opens a Mermaid DAG popup (nodes: status + id only; rendered via ratatui-markdown since 2026-08-06). `TaskSnapshot` carries `blocks`/`blocked_by`. Log does **not** append task system cards (progress lives in sticky + tool rows).

**Behavior after:** Readable tool rows; sticky tree; slash DAG viewer; no task system spam in Log.

**Pointers:** `crates/tact/src/task/display.rs`, `crates/tui/src/widgets/state/task_panel.rs`, `crates/tui/src/widgets/state/task_dag.rs`

---

## 2. 2026-07-25 — Task checklist renders fully (no `… +N`)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 19, Ch 23 |

**Symptom / motivation:** Log detail cards and sticky expand capped the checklist at 6 rows (`… +N`), so an 8-task board looked incomplete even when all items were updated.

**Decision:** Drop `STICKY_BODY_CAP`; sticky height and Log cards list every task.

**Behavior after:** Full checklist in both sticky expand and each `TasksChanged` Log card.

**Pointers:** `crates/tui/src/widgets/state/task_panel.rs`

---

## 2. 2026-07-25 — Serialize persistent `task_*` tools in one turn

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 11, Ch 19 |

**Symptom / motivation:** Models often emit many `task_update` / `task_create` calls in one turn. When those ran in the same wave, TaskManager updates and `TasksChanged` UI events interleaved, producing a jammed Log and a single incomplete progress card.

**Decision:** Classify `task_create` / `task_update` / `task_get` / `task_list` as writers of a synthetic `__tact_tasks__` resource so they always land in separate waves (order preserved) while still overlapping unrelated `read_file` calls.

**Behavior after:** Within one assistant tool batch, task tools run one-at-a-time; each mutating call can emit its own `TasksChanged` in order.

**Pointers:** `crates/tact/src/agent/tool_schedule.rs`

---

## 2. 2026-07-24 — Persistent task progress sticky + Log card

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 19, Ch 23, Ch 25; `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md` |

**Symptom / motivation:** Persistent tasks (`task_create` / `task_update`) only appeared as ordinary tool JSON/text in the Log. There was no always-visible checklist and no structured timeline card for mutations.

**Decision:** Emit `AgentUpdate::TasksChanged` after successful mutating tools. TUI keeps a sticky strip under the Log via an **outer layout split** (Log internals unchanged), collapsed by default with click-to-expand, and appends a Log detail card on each change. Hide the sticky when no pending/in_progress items remain; do not show on resume until the first `TasksChanged` this session.

**Behavior after:**

- Sticky one-liner: `▸ Tasks done/total · focus` (click expands full checklist)
- Each `TasksChanged` adds a system Log checklist card
- `task_get` / `task_list` do not emit

**Pointers:** `crates/protocol/src/agent.rs`, `crates/tact/src/tool/task.rs`, `crates/tui/src/render/task_panel.rs`, `crates/tui/src/render/layout.rs`

---

## 2. 2026-07-24 — Remove redundant `[Log]` from bottom bar

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 23 |

**Symptom / motivation:** Bottom bar Row 1 always started with `[Log]` even
though the UI is permanently single-column log-only, so the focus label added
noise without information.

**Decision:** Drop the focus segment from `render_bottom_bar` Row 1. Top status
bar may still mention Log where useful; bottom bar starts with cwd / uptime.

**Behavior after:** Row 1 no longer shows `[Log]`; first segment is workspace
path (then uptime, branch, optional account).

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/bar.rs` |

---
## 3. 2026-07-24 — Slash popup Esc hint + priority over overlay

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** Opening `/` while the agent was busy felt "stuck":
no Esc-to-close hint on the title, and Esc could be swallowed by a thinking/diff
overlay instead of dismissing the slash list.

**Decision:** Append shared `popup_close_hint` (`[Esc] Close`) to the slash
popup title (including empty state). Route Insert+slash keys before
`handle_overlay_key` so Esc always closes the slash popup first.

**Behavior after:** Slash popup title shows Esc close; Esc dismisses slash
without clearing typed input; overlay Esc only after slash is closed.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/popups/slash_command.rs`, `crates/tui/src/lib.rs` |

---
## 4. 2026-07-24 — Idle bottom-bar `Up` ticks without CPU spin

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** Fully idle TUI never dirtied on poll timeout, so
`Up MM:SS` froze until the next key/mouse/agent event.

**Decision:** On idle poll (~1000 ms), dirty only when the displayed uptime
whole-second changes. Active statuses still dirty for spinners; poll intervals
unchanged. Done keeps force-repaint via `should_repaint`.

**Behavior after:** Idle `Up` advances about once per second; no faster idle
redraw loop.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/lib.rs` (`on_poll_timeout`) |

---
## 5. 2026-07-24 — Prompt elapsed moves to task-end separator

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23 |

**Symptom / motivation:** Bottom-bar `Elapsed` competed with cwd/branch/balance
and was easy to miss relative to the response it measured.

**Decision:** Freeze prompt duration into the task-end sentinel
(`\x07tact-task-end\x1f{secs}`) and render it centered on the accent rule
(`──── Elapsed 00:03 ────`). Remove elapsed from the bottom bar.

**Behavior after:** Each completed/cancelled task shows its duration on the
trailing separator; bottom row 1 no longer shows `Elapsed`.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/cells/separator.rs`, `widgets/state/app/popups.rs`, `render/bar.rs` |

---

## 6. 2026-07-24 — Bottom bar readability restore

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23, `docs/token_usage_schema.md` |

**Symptom / motivation:** After the icon-only polish, the bottom bar was hard to
decode (`8K/32K`, bare `∑` / `▣`, faint ` · ` separators). Thinking effort level
was not shown even though `model_reasoning_effort` was already available.

**Decision:** Short i18n labels beside icons; thinking shows effort+budget
(`high(32K)`); row 1 uses ` │ `, row 2 uses two spaces; cache as `缓存%` /
`cache%`; last-call total as `∑ₜₒₖ`; ctx meter fill uses mid-height `■` / `·`
inside `[]`.

**Behavior after:** Readable two-row bar without a legend; same underlying
token/cache numbers. Narrow drop order: cache → uptime → path → ∑ → ctx.

| Pointer | Path |
|---------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-readability-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-readability.md` |
| Code | `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs` |

---

## 7. 2026-07-24 — Slash popup: Tab completes, Enter runs skills

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 2, Ch 23 |

**Symptom / motivation:** After restoring Insert-mode `Tab` for the slash
popup, **Tab** and **Enter** still did the same thing for skills (both only
filled `/name `). Users could not tell complete vs run apart.

**Decision:** Slash popup **Tab** always autocompletes to `/name `. **Enter**
invokes skills and runs built-ins immediately. `/plugin` (needs a subcommand)
still only autocompletes. Command palette Enter on a skill still prefills
Insert (undo-friendly).

**Behavior after:** Pick a skill in `/` → Tab to edit args, or Enter to run
now.

**Pointers:** `crates/tui/src/handlers/insert.rs`, Ch 2 §7, Ch 23 slash skills.

---

## 8. 2026-07-24 — TUI left Execution Plan panel removed

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 23, Ch 25 |

**Symptom / motivation:** The left plan panel duplicated information already
visible in the log (tool blocks appear on `StepStarted`), while adding
`Tab` focus switching, an `e` visibility toggle, a draggable divider, and a
`panel_split_ratio` layout knob that most users never touched. The extra
panel-focus state also complicated mouse hit testing and keyboard handling.

**Decision:** Remove the panel UI entirely; keep `PlanStep` tracking as an
internal, panel-less store (`app.plan.steps` / `steps_set`) so step data
stays available for future consumers. The log is now permanently
single-column. `FocusedPanel` keeps only its `Log` variant. Delete `Tab`
focus switching, the `e` toggle, and divider drag/resize; `j`/`k`/`g`/`G`/`y`/`Y`/`V`
now always act on the log. `Insert`-mode `Tab` for slash-command
autocompletion (previously shadowed by the global `Tab` handler) now fires
correctly since nothing above it in `lib.rs` intercepts `Tab` first.

**Behavior after:** `render_main_area` always renders the log panel at full
width; there is no plan panel, divider, or panel-focus indicator in the top
or bottom bar. `StepAdded` still updates `app.plan.steps` for internal
bookkeeping but never draws a dedicated panel.

**Pointers:** `crates/tui/src/widgets/state/plan_panel.rs`,
`crates/tui/src/render/layout.rs`, `crates/tui/src/widgets/state/mod.rs`
(`FocusedPanel`), `crates/tui/src/handlers/normal.rs`,
`crates/tui/src/handlers/mouse.rs`, `book/23_chapter_tui*.md`.

---

## 9. 2026-07-24 — Project config file renamed `tact.toml` → `config.toml`

| Field | Value |
|-------|-------|
| **Type** | docs |
| **Related** | Ch 21 |

**Symptom / motivation:** Auto-discovery listed `./tact.toml` while user-global /
`.tact/` paths already used `config.toml`, which was easy to misplace.

**Decision:** Search `./config.toml` instead of `./tact.toml`. Rename
`tact.example.toml` → `config.example.toml`.

**Behavior after:** Discovery order is `./.tact/config.toml`, `./config.toml`,
`~/.tact/config.toml`. Explicit `--config` unchanged.

**Pointers:** `crates/tact/src/config/load.rs`, `book/21_chapter_config*.md`,
`config.example.toml`.

---

## 10. 2026-07-24 — Session Stats GFM cells padded for plain-text alignment

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**Symptom / motivation:** End-of-session `eprintln` of `SessionStats::summary()`
printed unpadded GFM (`| Elapsed | 1.2s |` next to longer metric names), so
pipe columns did not line up in the terminal after `tact-ui` exited.

**Decision:** Keep GFM pipe tables for tui-markdown. Pad header / body cells to
the per-column max width (right-align numeric columns from `:` separators).

**Behavior after:** CLI / headless / TUI exit summaries show aligned columns in
monospace; `/stats` popup still renders via tui-markdown box tables.

**Pointers:** `crates/tact/src/stats.rs`, `docs/token_usage_schema.md`
(Session Stats Display).

---

## 11. 2026-07-24 — Extra `skill_dirs` + project-local `.tact/skills`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-extra-skill-dirs-design.md` |

**Symptom / motivation:** Only fixed skill roots existed; teams could not point at
shared or vendor skill trees. The old `<workdir>/skills/` root also sat outside
`.tact/`.

**Decision:** Replace `<workdir>/skills/` with `<workdir>/.tact/skills/`. Add
optional `[agent].skill_dirs = [...]` (relative to workdir; `~` expands). Load
order: `.tact/skills` → `~/.tact/skills` → `~/.agents/skills` → `.claude/skills`
→ config extras → plugin cache. Missing dirs soft-skipped.

**Behavior after:** Config can append skill roots that override earlier
same-named standalone skills. Bare `<workdir>/skills/` is no longer scanned.

**Pointers:** `crates/tact/src/consts.rs`, `crates/tact/src/skill/mod.rs`,
`crates/tact/src/config/types.rs`, `config.example.toml`, Ch 2.

---

## 12. 2026-07-24 — `/skills` list via tui-markdown (no pipe table)

| Field | Value |
|-------|-------|
| **Type** | bugfix |

**Symptom / motivation:** `/skills` built a Skill/Description pipe table through
`format_table`. Long frontmatter descriptions made each row wider than the log
panel; visual wrap shattered `|` columns into unreadable fragments.

**Decision:** Keep the titled block + blank separators. Emit wrap-friendly
markdown (`**\`name\`**` then description paragraph) and render with
`render_markdown_tui` / tui-markdown. Do **not** use a GFM table here (unlike
Session Stats): catalog descriptions are too wide for fixed columns in the log.

**Behavior after:** `/skills` shows one skill name + description block per entry;
text wraps cleanly at any panel width. Namespace names (`plugin:skill`) unchanged.

**Pointers:** `crates/tui/src/handlers/mod.rs` (`show_skills_command`,
`skills_list_markdown`).

---

## 13. 2026-07-24 — Session Stats as GFM tables via tui-markdown

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**Symptom / motivation:** `/stats` fed comfy-table UTF8 box output through
`render_markdown_tui`. Soft breaks became spaces, so the whole table collapsed
into one wrapped line and looked unreadable in the popup.

**Decision:** Keep `SessionStats::summary() -> String`. Emit **GFM pipe tables**
(with right-aligned numeric columns). TUI keeps using `render_markdown_tui` /
[tui-markdown](https://github.com/joshka/tui-markdown) table rendering (Unicode
box borders). Drop the `comfy-table` dependency. CLI / headless print the same
markdown source.

**Behavior after:** Session Statistics popup shows aligned box tables; exit
summaries are GFM markdown. Counters and visibility rules unchanged.

**Pointers:** `crates/tact/src/stats.rs`,
`crates/tui/src/widgets/state/app/agent.rs`, `docs/token_usage_schema.md`.

---

## 14. 2026-07-24 — Session Stats rendered with comfy-table

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-session-stats-table.md` |
| **Superseded by** | §7 (GFM + tui-markdown) |

**Symptom / motivation:** End-of-session Tool calls rows used ad-hoc space
padding, so columns drifted as names and timings grew.

**Decision:** Keep `SessionStats::summary() -> String`. Render a head
Metric/Value table, an optional Tool calls table
(`Tool | Count(s/f) | Total | Avg`), then a trailing Metric/Value table for
tool aggregates / cache / reasoning. *(Originally used `comfy-table` UTF8
boxes; that path conflicted with TUI markdown — see §7.)*

**Behavior after:** Same counters and visibility rules; layout is aligned
tables instead of free-form lines.

**Pointers:** `crates/tact/src/stats.rs`, `docs/token_usage_schema.md`
(Session Stats Display).

---

## 15. 2026-07-24 — `/model` supplements config from `/v1/models`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-openai-models-api-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-openai-models-api.md` |

**Symptom / motivation:** `/model` required a hand-maintained `models = [...]`
list; providers already expose `GET /v1/models`.

**Decision:** Config remains primary; API appends missing ids; conflicts keep
config; fetch once per `(base_url, api_key)` on first `/model`; Anthropic skipped;
failures soft-fail to config-only / empty hint.

**Behavior after:** See Ch 21 `/model` section.

**Pointers:** `crates/tact_llm/src/models.rs`, `crates/tui/src/handlers/select.rs`,
Ch 21, Ch 22 (account-style queries).

---

## 16. 2026-07-24 — `read_file` pagination and `batch_read` removal

| Field | Value |
|-------|-------|
| **Type** | optimization + removal |
| **PR** | [#50](https://github.com/rust-infra/tact/pull/50) |
| **Spec** | `docs/superpowers/specs/2026-07-24-read-file-pagination-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-read-file-pagination.md` |

### 6.1 Symptom

`read_file` loaded the whole file with `read_to_string`, then silently discarded the tail with `chars().take(50000)`. That conflicted with line-based `offset` / `limit`, gave the model no recovery signal (hallucination risk — see [Ch 20](./20_chapter_hallucination.md)), and competed with dispatch-level `persist_large_output` (30k characters → `<persisted-output>`).

`batch_read` was a second multi-file API with its own 200k-character hard cap, duplicating schedule / recent-file special cases.

### 6.2 Decision

1. Delete `batch_read`. Parallel multi-file reads use concurrent `read_file` waves.  
2. Stream lines with Tokio `BufReader` (no whole-file buffer for the page).  
3. Bound pages with prefixed constants in `read_file.rs`:

```rust
const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;
```

Token estimate: existing `approx_token_count` (`ceil(UTF-8 bytes / 4)`).  
4. No per-line character limit (a single oversized line errors; never silent mid-line cut).  
5. Incomplete **implicit** / default pages return a leading marker:

```text
[PARTIAL view — lines {start}-{end}; continue with offset={next}]

{joined lines}
```

6. **Explicit** `offset` and/or `limit` that still exceed the token budget → **error** (do not silently return less than requested).  
7. `run_native_tool` **skips** `persist_large_output` when `name == "read_file"`.  
8. Tool `description` stays short — limits are enforced at runtime, not duplicated in the schema blurb.

### 6.3 Behavior after

| Case | Result |
|------|--------|
| Small file, no args | Full content, no PARTIAL |
| File longer than 2000 lines, no args | First 2000 lines + PARTIAL with `offset=2001` |
| Token budget hit on implicit read | Complete lines that fit + PARTIAL with next `offset` |
| Explicit range over token budget | `Err` asking to reduce `limit` / shrink the section |
| Single line alone over budget | `Err` (cannot recover via line offset) |
| Offset past EOF | Empty string |
| Large `read_file` vs bash / MCP | `read_file` never gets `<persisted-output>`; others still may |

### 6.4 Pointers

| Area | Path |
|------|------|
| Implementation | `crates/tact/src/tool/read_file.rs` |
| Persist exemption | `crates/tact/src/agent/tool_dispatch.rs` (`run_native_tool`) |
| Tool registration | `crates/tact/src/tool/registry.rs` (no `BatchReadTool`) |
| Approx tokens | `crates/tact/src/utils/truncate.rs` |
| Tool chapter | [Ch 7](./07_chapter_tool.md) |
| Compaction / spill | [Ch 5](./05_chapter_compact.md), `docs/compaction.md` |

---

## 17. 2026-07-24 — Bottom bar visual polish

| Field | Value |
|-------|-------|
| **Type** | optimization |

**Symptom / motivation:** The bottom bar mixed emoji, long bilingual labels (`Elapsed:`, `Balance:`, `cache hit:`), and mixed separators (`│` / `|`). Both rows used a single `Paragraph` style, giving flat color hierarchy that was hard to scan.

**Decision:** Replace emoji with narrow Unicode icons (`◷`, `⊙`, `⎇`, `¤`, `∑`, `▣`). Unify separators to ` · `. Compact model limits to `8k/32k` format and collapse verbose balance/quota strings. Render with ratatui `Line` / `Span` segments: dim icons & separators, bright primary values, accent branch, success/error balance.

**Behavior after:** Two-row bottom bar with consistent iconography and color hierarchy. Pure formatting helpers (`format_model_compact`, `format_balance_entry`, `format_quota_window`, `format_cache_pct`) are unit-testable without a terminal. Narrow-width drop order removes uptime → path on row 1, cache → tokens → meter on row 2.

| Area | Path |
|------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-polish-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-polish.md` |
| Implementation | `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs` |
| Docs | `docs/tui_rendering.md` (Bottom Bar section) |
| Rendering framework | [Ch 23](./23_chapter_tui.md) |

---

## Related Docs

- [Tool System](./07_chapter_tool.md)
- [Context Compaction](./05_chapter_compact.md)
- [Hallucination in Agent Loops](./20_chapter_hallucination.md)
- [AGENTS.md](../AGENTS.md) — documentation sync triggers including this chapter
