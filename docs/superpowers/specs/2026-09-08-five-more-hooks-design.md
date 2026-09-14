# Design: five more lifecycle hooks — Codex-aligned (SubagentStop · Stop · SessionEnd · PreCompact · PostCompact)

- **Date:** 2026-09-08
- **Status:** draft — awaiting approval before implementation
- **Reference:** OpenAI Codex hooks source
  (`codex-rs/hooks/src/events/{stop,compact,session_end}.rs`, `schema.rs`, `engine/output_parser.rs`).
  Tact's hook framework is Claude-Code-compatible; this design follows **Codex's** event semantics
  for the five events Codex has that tact does not, mapped onto tact's existing
  `HookControl::{Continue, Block(String)}` contract.

---

## 1. What Codex does (the authoritative model)

### 1.1 Matchers per event (Codex)

| Event | Matcher target |
|-------|----------------|
| `PreToolUse` / `PostToolUse` / `PermissionRequest` | tool name |
| `PreCompact` / `PostCompact` | `trigger` string |
| `SessionStart` | `source` (`startup`/`resume`/`clear`/`compact`) |
| `SessionEnd` | `reason` (currently `"other"`) |
| `SubagentStart` / `SubagentStop` | `agent_type` |
| `UserPromptSubmit` / `Stop` / `Interrupt` | none (matcher ignored) |

### 1.2 Output "universal" shape (Codex)

Every command hook parses a `HookUniversalOutputWire`:
`{ "continue": bool, "stopReason": str|null, "suppressOutput": bool, "systemMessage": str|null }`
plus event-specific fields (`decision`/`reason`/`additionalContext`/`updatedInput`).

- `"continue": false` + `stopReason` → **`should_stop`** (force stop of the operation).
- `"decision": "block"` + non-empty `reason` → **`should_block`** (event-specific meaning).
- `systemMessage` → surfaced as a warning; `suppressOutput` → drop output.

### 1.3 The five events in Codex, exactly

**Stop / SubagentStop** (shared `stop.rs` machinery):
- Input: `session_id, turn_id, transcript_path, cwd, hook_event_name, model, permission_mode,
  stop_hook_active(bool), last_assistant_message`; `SubagentStop` adds
  `agent_id, agent_type, agent_transcript_path`.
- Matcher: `Stop` → none; `SubagentStop` → `agent_type`.
- Semantics: `decision:"block"`+`reason` → **continue the turn, reason becomes the next prompt**
  (continuation fragment); `continue:false`+`stopReason` → actually stop. `continue:false`
  **overrides** `block`. Exit code `2` + stderr → legacy "continue with stderr reason".

**PreCompact / PostCompact** (`compact.rs`):
- Input: `session_id, turn_id, agent_id, agent_type, transcript_path, cwd, hook_event_name, model,
  trigger`.
- Matcher: `trigger`.
- Semantics: **stateless** — output is only the universal shape; `continue:false`+`stopReason`
  → `should_stop`. No `additionalContext`, no `decision`/`reason`, no context mutation.
  PreCompact `should_stop` = veto compaction; PostCompact `should_stop` = stop after compact.

**SessionEnd** (`session_end.rs`):
- Input: `session_id, transcript_path, cwd, hook_event_name, reason`.
- Matcher: `reason`. Semantics: **observational only** — exit code `0` = completed, anything else
  failed; no JSON output parsed, no control effects. Runs during teardown with a tight timeout
  (`DEFAULT=1s`, `MAX=3s`).

---

## 2. Tact mapping (what we will build)

Tact keeps its simpler contract: a hook returns `HookControl::{Continue, Block(String)}` and gets a
dedicated `&mut` arg. The Codex `should_stop`/`should_block`/`continue` distinctions map to
`Block` as follows, using the **"block the operation"** reading consistently:

| Codex | Tact `HookControl` |
|-------|--------------------|
| `continue:false` (stop operation) | `Block(reason)` |
| `decision:block` (Pre/PostToolUse, UserPromptSubmit) | `Block(reason)` |
| `decision:block` on Stop/SubagentStop (continue turn) | `Block(reason)` — *blocked "stop" ⇒ continue* |
| default / `continue:true` | `Continue` |

The one semantic inversion is Stop/SubagentStop: there the "operation" being blocked is *the stop
itself*, so `Block(reason)` means "don't stop, keep going with `reason` as the next prompt" —
exactly Codex's continuation fragment. This is the only place tact's `Block` means "continue".

### 2.1 Stop

- **Fire point:** the **outer** turn boundary — when `agent_loop` finishes a normal turn and is
  about to return `Ok` (the `EndTurn|StopSequence|MaxTokens|PauseTurn|None` branch +
  the cancel/cap returns). Not inside the inner loop (it `continue`s many times).
- **Storage:** new `Hook::Stop` variant (`Agent.hooks`) + `dispatch_stop_hooks`.
- **Hook arg:** read-only `&LoopState` + `last_assistant_message: Option<String>`.
- **Control:** `Continue` = stop normally. `Block(reason)` = **continue the turn** with `reason`
  as the next synthetic user message. Implemented by a small driver-level loop: after
  `agent_loop` returns, run Stop hooks; if any `Block`s, call `agent_loop` again with `reason`
  (bounded by the existing turn cap). `continue:false`-equivalent is `Continue` (the turn already
  ends there), so no extra "force stop early" path is needed.
- **Plugin matcher/payload:** no matcher (Codex ignores it). Payload
  `{ "stop_hook_active", "last_assistant_message" }`.

### 2.2 SubagentStop

- **Fire point:** after the child's `agent_loop` returns, where `summary`/`succeeded` are computed
  — sync tail (`tool/subagent.rs:~699`) and async `tokio::spawn` tail (`~585`).
- **Storage:** new `ToolContext.subagent_stop_hooks: Vec<Arc<dyn SubagentStopFn>>` (mirrors
  `subagent_start_hooks`), built by `plugin_subagent_stop_hooks`, stamped in both drivers.
- **Hook arg (mutable):** new `SubagentStopContext { agent_id, agent_type, prompt, success,
  cancelled, summary }` — the hook may rewrite `summary`.
- **Control:** post-completion, so `Block` is **log-only** in v1 (the child already returned;
  re-entering a finished child's loop is out of scope). `Continue` always. *(Codex would treat
  `block` as "continue the subagent"; tact's subagents are one-shot, so we defer that.)*
- **Plugin matcher/payload:** matcher against `agent_type`; payload
  `{ "agent_id", "agent_type", "last_assistant_message" }`.

### 2.3 PreCompact

- **Fire point:** top of `Agent::compact_history` (`agent/mod.rs:1113`), before the native/local
  branch and before any summarizer call.
- **Storage:** new `Hook::PreCompact` variant.
- **Hook arg:** read-only `&LoopState` + `trigger: CompactTrigger`.
- **Control:** `Block(reason)` = **veto compaction** (Codex `should_stop`). `compact_history`
  emits an info update and returns `Ok(())` without compacting. `Continue` proceeds.
- **Signature change:** `compact_history` gains an explicit
  `trigger: CompactTrigger { Auto | Manual | Recovery | Command }` param (resolves earlier D1 to
  Codex's explicit `trigger`). Five call sites updated: agent_loop auto `:677`/`:731` (Auto),
  recovery `:829` (Recovery), manual `:1018`/`:1045` (Manual), `/compact` driver (Command).
- **Plugin matcher/payload:** matcher against the trigger string (`"auto"|"manual"|"recovery"|"command"`).

### 2.4 PostCompact

- **Fire point:** after the compacted context + persistence commit, once on **each** success arm —
  native end of `compact_responses_native` (`agent/mod.rs:~1210`) and local end after
  `std::mem::replace`+persist (`~1560`). A failed compact never fires PostCompact.
- **Storage:** new `Hook::PostCompact` variant.
- **Hook arg:** read-only `&LoopState` (new context + `last_summary` in place) + `trigger`.
- **Control:** `Block(reason)` = log-only (compaction already committed). `Continue` always.
- **Plugin matcher/payload:** matcher against `trigger` string; payload `{ "trigger" }`.

### 2.5 SessionEnd

- **Fire point:** a **new** `Agent::dispatch_session_end_hooks`, invoked at real teardown —
  symmetrical with `dispatch_session_start_hooks` (`agent/mod.rs:1083`):
  - headless (`headless.rs`): after the single `agent_loop` + `cancel_all_and_persist`, before
    `shutdown_mcp`.
  - interactive (`driver.rs` run loop / `interactive.rs`): after the `UserCommand` channel closes,
    before MCP shutdown — the same agent that fired `SessionStart` is dropped.
  - **not** inside `agent_loop` (per-turn) and **not** in `spawn_subagent` (subagents fire
    SubagentStart/SubagentStop, matching today's lack of SessionStart for children).
- **Storage:** new `Hook::SessionEnd` variant.
- **Hook arg:** read-only `&LoopState`.
- **Control:** observational (Codex parses nothing; only exit code). `Block` is log-only.
- **Plugin matcher/payload:** matcher against `reason` (we use `"other"` for parity); payload
  `{ "reason" }`. Plugin path should use a short timeout (1–3s) like Codex to avoid stalling
  teardown.

---

## 3. Files touched (estimated)

- `crates/tact/src/hook/mod.rs` — new traits/context (`SubagentStopFn`, `SubagentStopContext`,
  `StopFn`, `SessionEndFn`, `PreCompactFn`, `PostCompactFn`), new `Hook` variants + `HookTypes`.
- `crates/tact/src/agent/mod.rs` — builder methods, `dispatch_session_end_hooks`,
  `dispatch_stop_hooks`, PreCompact/PostCompact dispatch, `compact_history(trigger, focus)` signature.
- `crates/tact/src/compact/mod.rs` — `CompactTrigger` enum (or reuse existing trigger if present);
  expose summary for PostCompact (already in `CompactState.last_summary`).
- `crates/tact/src/tool/subagent.rs` — SubagentStop dispatch in sync + async tails.
- `crates/tact/src/tool/mod.rs` — `ToolContext.subagent_stop_hooks`.
- `crates/tact/src/plugin/hooks.rs` — 5 `HookEventKind` variants + `as_str`/`parse` arms +
  `apply_plugin_hooks` arms + `plugin_subagent_stop_hooks` + trigger strings.
- `crates/tact-ui/src/interactive.rs`, `headless.rs`, `driver.rs` — stamp `subagent_stop_hooks`,
  SessionEnd/Stop dispatch sites, driver Stop re-run loop.
- `book/09_chapter_hook.md` + `_zh.md` — §2 table, §6 mapped-events bullets (both languages).
- `book/26_chapter_issue*.md` — user-visible change entry (per AGENTS.md).

## 4. Deviations from Codex (explicit)

1. **SubagentStop continuation** — Codex `block` continues the subagent; tact subagents are
   one-shot, so we defer "continue a finished subagent" and make SubagentStop `Block` log-only in v1.
2. **Stop `continue:false`** — in Codex Stop can *force* an early stop mid-turn; tact's Stop fires
   only at the natural turn end, so `Continue` already means "stop", and only the `Block`→continue
   direction is implemented.
3. **SessionEnd JSON** — Codex parses only the exit code; tact keeps the same observational-only
   behaviour (no stdout JSON contract).
4. **PermissionRequest / Interrupt / PostCompact mutation** — not in scope of these five; Codex's
   `PermissionRequest` and `Interrupt` remain unimplemented (separate follow-up).

## 5. Open decision (one remaining)

- **D2 (Stop continuation driver loop):** confirm the `Block(reason)` → "run one more `agent_loop`
  with `reason`" continuation (Codex parity, touches `driver.rs` + the subagent sync/async call
  sites to preserve the child's own loop boundary). If you'd rather land Stop as **read-only /
  telemetry first** and defer the continuation loop, say so — everything else (SubagentStop,
  PreCompact, PostCompact, SessionEnd) is independent of that choice.
