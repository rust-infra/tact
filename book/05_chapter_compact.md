# Context Compaction

> Language: [English](./05_chapter_compact.md) · [中文](./05_chapter_compact_zh.md)

This chapter explains how Tact keeps a long-running conversation **inside the model's context window**: cheap in-place truncation every turn (`micro_compact`), full LLM-generated summarization when the limit is reached (`compact_history`), and disk spill for both transcripts and oversized tool outputs. The primitives live in `crates/tact/src/compact.rs`; the orchestration lives in `Agent::compact_history` in `crates/tact/src/agent/mod.rs`.

Compaction is also a **recovery strategy**: when the provider rejects a request as too long, the agent compacts and retries. See [Error Recovery](./06_chapter_recovery.md).

---

## 0. Why Compaction Exists

A coding agent accumulates messages every turn: user text, assistant reasoning, tool calls, and especially **tool results** (file contents, command logs, search hits). Context growth has three costs:

| Cost | Effect |
|------|--------|
| Hard limit | Provider returns prompt-too-long → turn fails without recovery |
| Soft cost | Longer prompts → slower TTFT, higher $ / tokens |
| Attention | Distant tool dumps dilute the signal the model needs *now* |

```mermaid
flowchart LR
    subgraph Growth["Context growth over a long task"]
        U1[user] --> A1[assistant]
        A1 --> T1[tool results × N]
        T1 --> U2[user]
        U2 --> A2[assistant]
        A2 --> T2[more tools…]
        T2 --> Huge["usage → model_context_window"]
    end
    Huge -->|without compaction| Fail[API reject / degraded quality]
    Huge -->|with compaction| Fit[fit + continue]
```

Tact’s answer is **progressive defense**: free local stubs first, then one paid summarization call only when needed, plus opportunistic spill of single huge outputs so they never enter the window at full size.

---

## 1. Three Levels of Defense

| Level | Mechanism | Cost | When | What is lost from *context* |
|-------|-----------|------|------|-----------------------------|
| 1 | `persist_large_output` | Free (disk I/O) | Every successful native or MCP result > 30,000 chars | Full output (kept on disk + preview) |
| 2 | `micro_compact` | Free | Start of every LLM turn | Old tool-result bodies (stub left behind) |
| 3 | `compact_history` | One extra LLM call | 80% threshold, prompt-too-long, or `compact` tool | Assistant/tool history (recent real users + summary remain; full JSONL on disk) |

```mermaid
flowchart TB
    subgraph L1["Level 1 — spill one result"]
        Bash[successful tool returns] --> Big{> 30k chars?}
        Big -->|yes| Disk1["write .claude/tool-results/id.txt"]
        Disk1 --> Env["replace with &lt;persisted-output&gt;"]
        Big -->|no| Keep[keep full output]
    end

    subgraph L2["Level 2 — stub old results"]
        Turn[each agent_loop turn] --> MC[micro_compact]
        MC --> Stub["old ToolResult > 120 chars → stub<br/>keep last 12 intact"]
    end

    subgraph L3["Level 3 — summarize everything"]
        Est{estimate > limit?} -->|yes| CH[compact_history]
        Err[prompt-too-long] --> CH
        Manual[compact tool] --> CH
        CH --> Disk2[JSONL transcript]
        CH --> Sum[LLM summary ≤ 2k tokens]
        Sum --> One[context ← recent users + summary]
    end

    L1 -.->|prevents floods| L2
    L2 --> Est
    Est -->|no| Prompt[build prompt / call LLM]
    One --> Prompt
```

**Mental model:** Level 1 protects *this turn’s* stdout; Level 2 protects *history shape* without an LLM; Level 3 resets the conversation when even stubs are not enough.

---

## 2. Where Compaction Sits in the Agent Loop

Compaction is not a separate daemon — it is woven into `Agent::agent_loop`. Reading the loop top-to-bottom:

```mermaid
flowchart TD
    Start([agent_loop iteration]) --> Cancel{cancelled?}
    Cancel -->|yes| Exit([return])
    Cancel -->|no| MC[micro_compact context]
    MC --> Build[build CreateMessageParams]
    Build --> Stream[stream_message]
    Stream -->|Ok| Assist[push assistant message]
    Stream -->|prompt too long| Rec["[Recovery] compact<br/>compact_history(None)"]
    Rec --> Start
    Stream -->|transient| Backoff[sleep + retry]
    Backoff --> Start
    Assist --> Tools{tool_use blocks?}
    Tools -->|yes| Exec[execute_tool_call]
    Exec --> Persist[push tool_result user message]
    Persist --> Man{manual_compact?}
    Man -->|yes| MC2["[manual compact]<br/>compact_history(focus)"]
    MC2 --> Start
    Man -->|no| MC3[micro_compact context]
    MC3 --> Size{should_auto_compact?<br/>tokens or estimate ≥ 80% window}
    Size -->|yes| Auto["emit [auto compact]<br/>compact_history(None)"]
    Auto --> Start
    Size -->|no| Start
    Tools -->|no| Done{stop / continue?}
```

Key ordering facts:

1. **`micro_compact` runs before every actual model request**, including the first request and continuation requests. Automatic full compaction is checked after tool results are appended, so an explicit manual `compact` request can take priority.
2. **After tool execution, manual compaction wins** — when `manual_compact` is present, `compact_history(focus)` runs; otherwise the agent runs `micro_compact` and then checks `should_auto_compact`.
3. **Prompt-too-long recovery** runs `compact_history` then `continue`s the loop (same turn, new context). Cap: `MAX_RECOVERY_ATTEMPTS` (3). Details in [Error Recovery](./06_chapter_recovery.md).
4. **Manual `compact` tool** cannot rewrite context *inside* the tool handler (API validity). Dispatch records a flag; `compact_history` runs **after** tool results are appended.

---

## 3. Micro-Compaction

`micro_compact(messages, enabled)` runs before each model request (disable via config, see §9). It only touches **user-role** messages that contain `ContentBlock::ToolResult`. The full auto-compact check is deliberately deferred until after tool results are appended, where manual `compact` can take priority.

```rust
const KEEP_RECENT_TOOL_RESULTS: usize = 12;
const COMPACTED_TOOL_RESULT: &str =
    "[Earlier tool result compacted. If you need the full content to continue editing, re-read the relevant file.]";
```

### Algorithm

```mermaid
flowchart TD
    A[scan messages] --> B[collect ToolResult positions<br/>in chronological order]
    B --> C{count ≤ 12?}
    C -->|yes| Z[no-op]
    C -->|no| D["compact_until = count − 12"]
    D --> E[for each of the oldest compact_until results]
    E --> F{chars > 120?}
    F -->|yes| G[replace body with COMPACTED_TOOL_RESULT]
    F -->|no| H[leave short result alone]
```

### Before / after (conceptual)

```mermaid
flowchart LR
    subgraph Before["Before micro_compact"]
        R1["TR#1 long log"]
        R2["TR#2 file dump"]
        R3["…"]
        R12["TR#12"]
        R13["TR#13 recent"]
        R14["TR#14 newest"]
    end

    subgraph After["After — keep last 12"]
        S1["stub"]
        S2["stub"]
        S3["…"]
        K12["TR#12 intact"]
        K13["TR#13 intact"]
        K14["TR#14 intact"]
    end

    Before --> After
```

Rules of thumb encoded in the constants:

| Rule | Why |
|------|-----|
| Keep last **12** results | Current workflow usually needs recent tool I/O |
| Stub only if **> 120** chars | Short oks / errors are dense; stubbing saves nothing |
| Never touch assistant / thinking / user text | Only tool dumps are the bulk offenders |

The stub text is deliberate: it tells the model **how to recover** (`read_file` / re-run tools). The system prompt reinforces the same idea:

> If a tool result was compacted and you need the details, re-run the relevant tool (e.g., `read_file`)

---

## 4. Auto Trigger and Size Estimation

The shared threshold is **`agent.model_context_window`** — the model context window in **tokens** (default **200,000**). The same value drives auto-compaction and the TUI bottom-bar usage meter.

### Decision (OR)

`should_auto_compact` fires when **either** condition holds (optionally reserving an *incoming* user turn not yet in context):

```text
last_token_total > 0
  && last_token_total + estimate_message_tokens(incoming_turn) >= 80% of model_context_window
  || estimate_context_tokens(context) + estimate_message_tokens(incoming_turn) >= 80% of model_context_window
```

Both sides of the OR compare against the same **token** window. Serialized content is estimated by counting ASCII at roughly four characters per token and non-ASCII conservatively at one character per token.

- **Entry (`agent_loop`)**: compact **old** history first with `incoming_turn_tokens = estimate(user_turn)`, then `push` the turn verbatim.
- **Loop / recovery / manual**: turn already in context → `incoming_turn_tokens = 0`.

Rebuild after summarize (Codex-style): **`[recent real User messages…] + [SUMMARY_PREFIX + handoff]`**, not a single summary-only message. Both plain-text turns and block-based UI turns are real users; tool-result-only block messages and prior summaries are excluded. The retained-user budget is `min(20k estimated tokens, window - max output - estimate(system + tools + summary) - 20% headroom)`. A block turn is kept verbatim when it fits; an oversized block turn falls back to its text tail, or an omission marker when it contains only images. Base64 is never sliced. Legacy single-summary path remains as `compact_history_legacy`.

The summary request reserves two separate costs before selecting history: the summary output budget (up to 2,000 tokens) and **10% of the model window as safety headroom**. For a 200,000-token window, the headroom is 20,000 tokens. This absorbs estimation error, JSON serialization overhead, and differences between the conservative estimate and the provider tokenizer. The percentage is rounded up, so the reserve is never rounded down.

```rust
pub fn estimate_context_tokens(messages: &[Message]) -> usize {
    match serde_json::to_string(messages) {
        Ok(serialized) => approx_text_tokens(&serialized),
        Err(_) => usize::MAX / 2, // prefer compact over underestimating
    }
}
```

```mermaid
flowchart TD
    MC[micro_compact] --> Tok{tokens (+ incoming) ≥ 80% window?}
    Tok -->|yes| Auto[auto compact_history]
    Tok -->|no| Est["estimated context + incoming tokens<br/>≥ 80% window?"]
    Est -->|yes| Auto
    Est -->|no| Call[LLM call]
```

| Setting | Default | Notes |
|---------|---------|-------|
| `agent.model_context_window` | **200,000** | Tokens; CLI `--model-context-window` / TOML. Breaking rename from `context_limit_chars` — **no silent alias**. |

After compaction, `last_token_total` is **reset to 0** (the summarizer call's usage reflects a large history prompt, not the replacement context size); the next main-loop LLM call writes a fresh value. See §11.

---

## 5. Full Compaction: `compact_history`

`Agent::compact_history(focus: Option<&str>)` is the expensive path. It never “deletes” work permanently: the pre-compact context is always written to a transcript first.

### End-to-end sequence

```mermaid
sequenceDiagram
    autonumber
    participant AgentLoop as agent_loop
    participant CH as compact_history
    participant Disk as filesystem
    participant LLM as create_message
    participant Store as SessionStore

    AgentLoop->>CH: compact_history(focus?)
    CH->>Disk: write transcript to .claude/transcripts/transcript_ts.jsonl
    CH-->>AgentLoop: Info "[transcript saved: …]"
    CH->>CH: select recent messages within a 20k-token cap
    CH->>CH: build summarize prompt + optional focus + recent_files
    CH->>LLM: create_message with window-aware input/output budgets
    LLM-->>CH: validated complete non-empty text summary
    CH->>CH: reset message-id window (first/last/llm_call ids = 0)
    CH->>CH: append "Recently accessed files…" to summary
    CH->>CH: context = build_compacted_history(users + summary)
    CH->>Store: replace_session_messages (SQLite matches new context)
    CH->>CH: stats.compactions += 1
```

### Step details

**1. Transcript spill** — `write_transcript` atomically creates a unique `.claude/transcripts/transcript_<unix_nanos>_<collision>.jsonl`, one JSON message per line. TUI shows `[transcript saved: …]`. Full history is recoverable offline; the model is **not** automatically pointed at this path in the summary message (gap in §11).

**2. Recent-window selection** — walk `context` **from the end** within both the model-window budget and a **20,000 estimated-token cap**. An oversized message is converted to a valid text-only view; images become omission markers, so base64 is never cut. No message is forced in when it cannot fit. Earlier turns survive only via transcript + whatever the summary can infer.

```mermaid
flowchart LR
    subgraph Context["Full context (oldest → newest)"]
        Old[… early turns …]
        Mid[middle]
        New[recent ≤ 20k estimated tokens]
    end
    Old -.->|not sent to summarizer| X[omitted]
    Mid -.->|not sent| X
    New -->|serialized into prompt| SumLLM[summarization LLM]
```

**3. Summarization call** — a fresh non-streaming `create_message` (at most 2,000 output tokens, no tools, no thinking) reserves output and 10% safety headroom before selecting input. If the fixed summary instructions alone exceed this input limit, compaction fails early because no valid summary request can be constructed, even after removing all history. Transient transport failures get up to three retries with backoff. `MaxTokens`, refusal/other abnormal stop reasons, and empty text are rejected without replacing the old context. The prompt asks the model to preserve:

1. Current goal and accomplishments  
2. Findings, decisions, architectural insights  
3. Files read/changed (types, signatures, APIs when relevant)  
4. Remaining work / next steps  
5. User constraints and preferences  
6. Errors and causes  

Optional appendages:

- `Focus to preserve next: {focus}` — from the manual `compact` tool  
- `Recent files to reopen if needed:` — from `CompactState.recent_files`

**4. Context replacement** — Codex-style rebuild via `build_compacted_history`:

```text
[0] User  "<earlier real user text…>"
[1] User  "<more recent real user text…>"
[2] User  "This conversation was compacted so the agent can continue working.

           <summary…>

           Recently accessed files (re-read if you need their contents):
           - crates/tact/src/agent/mod.rs
           - …"
```

(`compact_history_legacy` still replaces with a **single** summary user message.)

### Compaction failure behavior

The context is replaced only after the summary has been validated and the rebuilt request fits the model window. If summary generation fails, returns empty text, uses an invalid stop reason, or the rebuilt request cannot fit, the original in-memory context remains in place. If persisting the rebuilt context to SQLite fails, the replacement is rolled back as well. The transcript written at the start of compaction remains available for diagnosis or offline recovery. The current agent loop then propagates the error and normally ends the task; it does not blindly retry the same oversized context. Transient summary transport errors are the exception: they are retried up to three times before failing.

**5. Bookkeeping**

| Action | Why |
|--------|-----|
| `has_compacted = true`, store `last_summary` | Session knows compaction occurred |
| Reset `first_message_db_id` / `last_message_db_id` / `llm_call_last_message_id` | New message-id window after rewrite |
| `last_token_total = 0` | Summarizer usage is a large prompt, not the new context; avoids re-triggering compact every turn |
| `replace_session_messages` | Reopening the session must **not** resurrect pre-compaction SQLite rows |
| `stats.compactions += 1` | Observability |

### CompactState and recent files

```rust
pub struct CompactState {
    pub has_compacted: bool,
    pub last_summary: Option<String>,
    pub recent_files: Vec<String>,   // last 5 read_file paths, deduped, LRU
}
```

```mermaid
flowchart TD
    RF[read_file succeeds] --> Remember[remember_recent_file]
    Remember --> Dedup[drop existing same path]
    Dedup --> Push[push to end]
    Push --> Cap{len > 5?}
    Cap -->|yes| Drain[drain oldest]
    Cap -->|no| Ok[keep]
    Ok --> Use1[listed in summarization prompt]
    Drain --> Use1
    Use1 --> Use2[appended to final summary message]
```

`remember_recent_file` is fed only by successful final tool results for `read_file`, `batch_read`, `write_file`, `edit_file`, and non-dry-run `apply_patch`. It keeps the last five deduplicated paths as “amnesia insurance.”

### Before / After Comparison

The most visible effect of `compact_history` is removing assistant/tool history while preserving recent real user turns and appending one handoff summary. Let’s walk through a concrete example.

#### Before: `self.runtime.context` (`Vec<Message>`)

The full conversation grows with the task — a mix of roles and content:

```text
[0] User      "Add an early 80% trigger to the compact module"
[1] Assistant  reasoning + tool_use(read_file compact.rs)
[2] User       ToolResult(full compact.rs, ~5k chars)
[3] Assistant  tool_use(read_file agent/mod.rs)
[4] User       ToolResult(mod.rs excerpt, ~8k chars)
[5] Assistant  tool_use(bash cargo test)
[6] User       ToolResult(test log, ~40k chars)
[7] Assistant  tool_use(edit_file compact.rs)
[8] User       ToolResult("edit applied")
 …             (dozens of entries, potentially hundreds of thousands
                of chars / approaching the window)
[N] Assistant  "Threshold updated, moving on to tests"
```

Characteristics: complete `tool_use` / `ToolResult` pairs, per-step reasoning, and intermediate artifacts are all retained — which is exactly where the bulk comes from.

#### After: `self.runtime.context`

Recent real user turns remain within budget, followed by the handoff summary:

```text
[0] User  "Add an early 80% trigger to the compact module"
[1] User  "This conversation was compacted so the agent can continue working.

           <LLM summary, organized around the 6 points:>
           1. Current goal: add an early 80% trigger to the compact module
           2. Key finding: should_auto_compact uses reported and estimated tokens
           3. Files involved: crates/tact/src/compact.rs (should_auto_compact),
              crates/tact/src/agent/mod.rs (compact_history)
           4. Remaining work: add unit tests, run cargo test
           5. User preference: add TODOs first, optimize later
           6. Errors: none so far

           Recently accessed files (re-read if you need their contents):
           - crates/tact/src/compact.rs
           - crates/tact/src/agent/mod.rs"
```

Every `tool_use` / `ToolResult` / reasoning block from `[1]`–`[N]` is **no longer in the window** — it survives in only two places: the `transcript_<ts>.jsonl` written before compaction, and whatever the model chose to keep in this summary.

#### Item-by-Item Changes

| Dimension | Before | After |
|-----------|--------|-------|
| Message count | N messages | Recent real users + **1 summary** |
| Role structure | User / Assistant / ToolResult interleaved | **User** turns only |
| `tool_use` / `ToolResult` | Fully retained | **All dropped** (disk transcript only) |
| Reasoning / thinking | Retained | Dropped (summarizer produces no thinking) |
| Size | Up to hundreds of thousands of chars | Budgeted users + summary ≤ 2k output tokens + file list |
| Raw details | Directly readable | Recoverable via `recent_files` hints + `read_file` |
| Disk transcript | — | `.claude/transcripts/transcript_<ts>.jsonl` |

#### Runtime Fields Reset Alongside

Besides `context` itself, `compact_history` also resets the message-id window and flips the compaction state:

| Field | Before | After |
|-------|--------|-------|
| `first_message_db_id` | some value > 0 | `0` |
| `last_message_db_id` | some value > 0 | `0` |
| `llm_call_last_message_id` | some value > 0 | `0` |
| `last_token_total` | pre-compact / summarizer usage | `0` (rewritten on next main-loop call) |
| `compact_state.has_compacted` | possibly `false` | `true` |
| `compact_state.last_summary` | old value / `None` | this summary text |
| `stats.compactions` | `k` | `k + 1` |

SQLite stays in sync: `replace_persisted_context` rewrites the `messages` table with the rebuilt context, guaranteeing that **reopening the session cannot resurrect** pre-compaction rows.

```mermaid
flowchart LR
    subgraph Before["context before"]
        B0["User goal"]
        B1["Assistant tool_use"]
        B2["ToolResult 5k"]
        B3["Assistant tool_use"]
        B4["ToolResult 40k"]
        B5["… N entries …"]
    end

    subgraph After["context after"]
        A0["recent real Users<br/>+ summary + recent_files"]
    end

    subgraph Disk["disk (not in context)"]
        D0["transcript_&lt;ts&gt;.jsonl<br/>full pre-compaction history"]
    end

    Before -->|write_transcript| D0
    Before -->|LLM summary| A0
```

**In one sentence:** after compaction the model sees recent user intent plus a **handover memo it wrote itself** and a file list; assistant/tool detail retreats to disk.

---

## 6. Manual Compaction: the `compact` Tool

The model can request compaction via `compact` (`crates/tact/src/tool/compact.rs`).

```mermaid
sequenceDiagram
    autonumber
    participant Model
    participant AgentLoop as agent_loop
    participant Dispatch as execute_tool_call
    participant Tool as compact tool fn
    participant CH as compact_history

    Model->>AgentLoop: assistant message with tool_use name=compact
    AgentLoop->>Dispatch: execute_tool_call
    Dispatch->>Tool: call compact(focus?)
    Tool-->>Dispatch: "Compacting conversation…"
    Note over Dispatch: set manual_compact = Some(focus)
    Dispatch-->>AgentLoop: tool_result blocks + flag
    AgentLoop->>AgentLoop: push tool_result user message + persist
    AgentLoop->>CH: compact_history(Some(focus))
    Note over CH: real rewrite happens here after results are appended
```

Why the tool body is nearly a no-op: rewriting `runtime.context` **inside** a tool call would leave the conversation mid-flight (assistant `tool_use` without matching results, or a half-applied summary). The dispatcher pattern keeps the wire protocol valid, then runs Level 3 afterward. Optional `focus` steers what the summarizer must keep.

The dispatcher uses `manual_compact = Some(focus)` as a request flag. A string `focus` is copied into the flag; a missing or non-string `focus` becomes `Some("")`. That empty value still means “perform manual compaction,” but it supplies no extra instruction and is ignored by `compact_history`. `None` means that this tool was not requested. In the normal sequence, the tool result is first appended and persisted, then `compact_history(Some(focus))` performs the actual rewrite.

---

## 7. Large Output Spill (`persist_large_output`)

Independent of history compaction, a **single** oversized tool result must not enter the context at full size. Dispatch applies this to every successful native and MCP call:

```rust
persist_large_output(&tact_path, tool_use_id, &output)
```

| Constant | Value |
|----------|-------|
| `PERSIST_THRESHOLD` | 30,000 chars |
| `PREVIEW_CHARS` | 2,000 chars |

```mermaid
flowchart TD
    Out[successful tool output] --> Th{chars > 30_000?}
    Th -->|no| Full[return unchanged]
    Th -->|yes| Write["fs::write .claude/tool-results/&lt;tool_use_id&gt;.txt"]
    Write --> Prev["take first 2_000 chars"]
    Prev --> Wrap["wrap in &lt;persisted-output&gt; envelope"]
    Wrap --> TR[ToolResult content in context]
```

Replacement shape:

```xml
<persisted-output>
Full output saved to: .claude/tool-results/<tool_use_id>.txt
Preview:
[first 2000 characters…]
</persisted-output>
```

Persistence failure changes the tool step to failed instead of reporting a successful result whose full output was lost.

### Why `<persisted-output>` tags

The tags are **for the model, not for runtime parsing** — nothing in the codebase matches them back out. They mark the whole block as a **system-generated envelope**, so the LLM can tell:

- “Full output saved to …” / “Preview:” are framework metadata, not tool output
- this turn’s result was intentionally spilled (not silent truncation)
- full text is recoverable via `read_file` on the path

Without the wrapper, those lines blend into ordinary tool-result text. Same lightweight XML-ish convention as other prompt markers (e.g. `<skill>`).

### Stub vs envelope

```mermaid
flowchart TB
    subgraph Micro["micro_compact stub"]
        M1[Older ToolResult in history]
        M2["[Earlier tool result compacted. …]"]
        M1 --> M2
    end

    subgraph Spill["persist_large_output envelope"]
        S1[This turn's huge tool output]
        S2["&lt;persisted-output&gt; path + preview"]
        S1 --> S2
    end
```

| Marker | When | Meaning |
|--------|------|---------|
| `[Earlier tool result compacted. …]` | Level 2, old history | Body gone from context; re-read / re-run |
| `<persisted-output>…</persisted-output>` | Level 1, this turn | Full body on disk; preview + path in context |

---

## 8. On-Disk Layout

Compaction spills two kinds of artifacts under the workdir (via `TactPath`):

```mermaid
flowchart TB
    WD["&lt;workdir&gt;"]
    WD --> Claude[".claude/"]
    Claude --> TR["transcripts/<br/>transcript_&lt;unix_nanos&gt;_&lt;n&gt;.jsonl"]
    Claude --> OR["tool-results/<br/>&lt;tool_use_id&gt;.txt"]
    WD --> Tact[".tact/tact.db"]
    Tact --> Msg["messages table<br/>(rewritten on full compact)"]
```

| Path | Writer | Contents |
|------|--------|----------|
| `.claude/transcripts/transcript_<ts>.jsonl` | `write_transcript` | Full pre-compact conversation |
| `.claude/tool-results/<id>.txt` | `persist_large_output` | Full oversized native/MCP output |
| `.tact/tact.db` messages | `replace_session_messages` | Post-compact retained-users + summary context |

After each write, each spill directory keeps at most the 100 newest files; older regular files are removed by modification time.

---

## 9. Configuration

| Setting | Default | Effect |
|---------|---------|--------|
| `agent.model_context_window` (`--model-context-window`) | 200,000 | Token window: auto-compact at 80% + TUI usage meter; when nonzero it must exceed `max_tokens` |
| `agent.micro_compact_enabled` (`--no-micro-compact`) | `true` | Enables the per-turn stub pass |

Resolved through layered config in `crates/tact/src/config/` (CLI > TOML > default). Compile-time constants (`KEEP_RECENT_TOOL_RESULTS`, `PERSIST_THRESHOLD`, …) are **not** configurable yet.

---

## 10. Code Map

| File | Role |
|------|------|
| `crates/tact/src/compact.rs` | `micro_compact`, `should_auto_compact`, `estimate_context_tokens`, `collect_user_messages`, `build_compacted_history`, `write_transcript`, `persist_large_output`, `compacted_context`, `CompactState` |
| `crates/tact/src/agent/mod.rs` | Loop triggers; `compact_history` / `compact_history_legacy`; `remember_recent_file`; `replace_persisted_context` |
| `crates/tact/src/agent/tool_dispatch.rs` | `persist_large_output` for native/MCP results; `manual_compact` flag; recent-file tracking |
| `crates/tact/src/tool/compact.rs` | `compact` tool stub + `focus` |
| `crates/tact/src/recovery.rs` | Prompt-too-long classification → compaction |
| `crates/tact/src/consts.rs` | `transcript_dir()`, `tool_results_dir()` |
| `docs/compaction.md` | Behavior / tuning companion |

```mermaid
flowchart LR
    Loop[agent/mod.rs loop] --> Compact[compact.rs]
    Loop --> Dispatch[tool_dispatch.rs]
    Dispatch --> Compact
    Dispatch --> CompactTool[tool/compact.rs]
    Loop --> Recovery[recovery.rs]
    Recovery --> Loop
    Compact --> Paths[consts::TactPath]
    Loop --> Store[session store replace]
```

---

## 11. Current Gaps

| Gap | Detail |
|-----|--------|
| Cold-start / post-tool token estimate | ASCII uses ~4 chars/token and non-ASCII uses a conservative 1 char/token. Still OR'd with reported token total to cover growth after tool results are appended |
| Simple usage % | Meter is `used / model_context_window` (no Codex 12K baseline / effective-window math yet) |
| Only recent 20k estimated tokens summarized | Early turns live in transcript; model is not told that path in the replacement message |
| Fixed stub thresholds | 12 / 120 / 30k are compile-time constants |

---

## Related Docs

- [Error Recovery](./06_chapter_recovery.md) — compaction as the prompt-too-long strategy
- [Agent Main Loop](./18_chapter_agent_loop.md) — full loop structure around these hooks
- [System Prompt](./04_chapter_prompt.md) — rebuilt every turn; includes compacted-tool guidance
- [Store and Persistence](./01_chapter_store.md) — session message rewrite after compact
- [Tasks and Tool Scheduling](./11_chapter_task.md) — where `manual_compact` is detected in dispatch
- [docs/compaction.md](../docs/compaction.md) — tuning notes
- [ARCHITECTURE.md](../ARCHITECTURE.md) — §6 context compaction
