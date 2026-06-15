# Context Compaction

This document details tact's context compaction mechanism, including three tiers of compaction strategy and their configuration parameters.

---

## Overview

LLMs have limited context windows, and longer contexts result in slower responses and higher costs. When an agent runs long tasks, conversation history grows continuously and must be compacted to preserve usable context space.

tact implements a **three-tier progressive compaction**:

| Tier | Trigger | Target | Strategy |
|------|---------|--------|----------|
| Tier 1: Large Output Persist | Single tool output > 30K chars | Single tool result | Write to disk, keep preview |
| Tier 2: Micro Compaction | Before each LLM call | Old tool results | Replace with placeholder (keep last 12) |
| Tier 3: Full Compaction | Context > 500K chars | Entire conversation | Archive + LLM summary → reset context |

---

## Tier 1: Large Output Persist

**Trigger**: A single tool call result exceeds `PERSIST_THRESHOLD` (30,000 characters).

**Process**:
1. Full output written to `.claude/tool-results/{tool_use_id}.txt`
2. Tool result in context replaced with a preview of `PREVIEW_CHARS` (2,000 characters) + file path

**Replacement format**:
```xml
<persisted-output>
Full output saved to: .claude/tool-results/abc123.txt
Preview:
[first 2000 characters...]
</persisted-output>
```

### Related Constants

| Constant | Default | Location | Description |
|----------|---------|----------|-------------|
| `PERSIST_THRESHOLD` | 30,000 | `compact.rs:26` | Char threshold to trigger persistence |
| `PREVIEW_CHARS` | 2,000 | `compact.rs:28` | Preview chars kept in replacement text |
| `OUTPUT_DIR` | `.claude/tool-results` | `compact.rs:29` | Directory for large output files |

---

## Tier 2: Micro Compaction

**Trigger**: Before each LLM request in the agent loop iteration, via `micro_compact()`.

**Process**:
1. Scan all `tool_result` blocks in user messages
2. Keep the last `KEEP_RECENT_TOOL_RESULTS` (12) results intact
3. For older tool results exceeding 120 characters, replace with a placeholder

**Placeholder text**:
```
[Earlier tool result compacted. If you need the full content to continue editing, re-read the relevant file.]
```

**Design intent**:
- Short results (≤120 chars, e.g., error messages, confirmations) are kept — they have high information density and low space cost
- Long results are compacted, but the agent can re-run tools to recover original data
- The most recent 12 results are preserved to avoid interrupting the current workflow

### Related Constants

| Constant | Default | Location | Description |
|----------|---------|----------|-------------|
| `KEEP_RECENT_TOOL_RESULTS` | 12 | `compact.rs:23` | Number of recent tool results kept |
| `COMPACTED_TOOL_RESULT` | see above | `compact.rs:31` | Placeholder text for compacted results |

---

## Tier 3: Full Compaction

**Trigger**: After micro compaction, if serialized context still exceeds the `context_limit()` threshold.

### Context Size Limit

```
Default: 500,000 characters (~125K tokens)
Environment override: TACT_CONTEXT_LIMIT_CHARS
```

Adjust via environment variable:
```bash
export TACT_CONTEXT_LIMIT_CHARS=1000000  # ~250K tokens
```

**Process**:

### Step 1: Save Full Transcript

Serialize the complete conversation history as a JSONL file to `.claude/transcripts/transcript_{timestamp}.jsonl`.

### Step 2: Select Recent Messages

Traverse conversation history from the end backward, collecting recent messages (up to 80,000 characters), **keeping at least one**. This ensures the summary LLM receives the most relevant context, not the earliest history.

### Step 3: Generate Summary

Send selected messages as context to the LLM (max_tokens=2000), asking it to preserve:

1. **Current goal and work completed**
2. **Key findings, decisions, and architectural insights**
3. **Files read or modified** (with key code structures: types, signatures, APIs)
4. **Remaining work and next steps**
5. **User constraints and preferences**
6. **Errors encountered and their causes**

If the user specifies a `focus`, it's appended to the prompt.

If `recent_files` is not empty (last 5 files accessed via `read_file`), they're injected into the prompt.

### Step 4: Inject Recent Files

At the end of the LLM-generated summary, append the recently accessed file list to help the agent recover context after "amnesia":

```
Recently accessed files (re-read if you need their contents):
- src/main.rs
- src/lib.rs
```

### Step 5: Replace Context

The entire conversation history is replaced with a single user message:

```
This conversation was compacted so the agent can continue working.

[LLM-generated summary]

Recently accessed files (re-read if you need their contents):
- [file list]
```

`compact_state.has_compacted` is set to `true`, and `last_summary` stores the current summary.

### Related Constants

| Constant | Default | Location | Description |
|----------|---------|----------|-------------|
| `context_limit()` | 500,000 | `lib.rs:74` | Char threshold triggering full compaction |
| `TRANSCRIPT_DIR` | `.claude/transcripts` | `compact.rs:29` | Transcript output directory |
| Summary prompt max_tokens | 2,000 | `lib.rs:775` | Max tokens for summary LLM call |
| Recent message selection cap | 80,000 chars | `lib.rs:732` | Context size for summary LLM |

---

## Recent File Tracking

`CompactState.recent_files` tracks file paths recently accessed by the agent via the `read_file` tool (max 5).

**Update logic** (`remember_recent_file`):
- If file already exists in list, remove old entry first (dedup)
- Append file to end of list
- If > 5 entries, remove oldest (FIFO)

**Usage**:
- Listed in the full compaction summary prompt, hinting at which files are the current focus
- Injected into the final summary to help agent quickly locate key files after context reset

---

## Data Flow Overview

```
Agent Loop — each iteration
│
├─ micro_compact()                         [Tier 2]
│   └─ Replace old tool results (keep last 12)
│
├─ estimate_context_size() > limit?        [Tier 3 trigger check]
│   ├─ No → continue
│   └─ Yes → compact_history():
│       ├─ write_transcript()              → .claude/transcripts/*.jsonl
│       ├─ Select recent messages (≤80K chars)
│       ├─ LLM generates summary
│       ├─ Inject recent_files
│       └─ context = compacted_context(summary)
│
└─ LLM call
    │
    └─ Tool execution → intercept
        ├─ read_file → remember_recent_file(path)
        └─ persist_large_output()          [Tier 1]
            ├─ Output ≤30K chars → no change
            └─ Output >30K chars → write disk + return preview
```

---

## Integration with Agent System Prompt

Compacted placeholders instruct the agent: "compacted tool results can be recovered by re-running the relevant tool." The system prompt includes corresponding guidance:

```
- If a tool result was compacted and you need the details, re-run the relevant tool (e.g., read_file)
```

This ensures the agent can proactively recover needed data via `read_file` or other tools when encountering compacted tool results.
