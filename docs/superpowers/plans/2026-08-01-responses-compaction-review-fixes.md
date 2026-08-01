# Responses Compaction Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the remaining correctness, safety, configuration, numeric-conversion, and diagnostics findings from the Rust review of native OpenAI Responses compaction.

**Architecture:** Keep Responses provider state opaque and independent from logical `Message` history. Harden the stream state machine so incomplete compaction output is never replaced by visible-text recovery, derive thresholds consistently for main and subagents, and validate protocol/numeric boundaries before state or usage persistence. Preserve reasoning encrypted envelopes as internal non-renderable history signatures while strictly isolating compaction encrypted content from user-facing surfaces.

**Tech Stack:** Rust 2024, Tokio, serde_json, async-openai-responses 0.41.x, SQLx SQLite, existing Tact Agent/TUI tests and wiremock fixtures.

## Global Constraints

- Responses must use `store: false` and must not use `previous_response_id` or `conversation`.
- Responses must not register the model-facing `compact` tool; the user `/compact` command remains available.
- Responses must not fall back to local summary compaction; native protocol failure preserves old state and returns an error.
- `compaction.encrypted_content` remains opaque provider state/request-body data and never enters `ContentBlock`, logs, error strings, Info updates, or TUI text.
- Reasoning encrypted envelopes may remain in the internal non-renderable history signature required for protocol reconstruction; tests must prove they never become visible text or diagnostics.
- Provider state and Tact messages must be committed atomically in SQLite.
- Unknown typed terminal output items remain hard protocol errors; do not invent a fallback or silently discard them.
- Usage token conversions must reject values that do not fit Tact's `u32` usage fields.
- Other providers must retain their existing local compaction behavior.
- Never run multiple Cargo commands concurrently.

---

### Task 1: Harden stream compaction recovery

**Files:**
- Modify: `crates/tact_llm/src/openai/responses/stream.rs`
- Test: stream unit tests in the same file

**Interfaces:**
- Consumes `ResponsesStreamState::pending_added`, `done_items`, and `finish()` from the current implementation.
- Produces a hard protocol error whenever an incomplete output-item sequence could contain a compaction boundary.

- [ ] **Step 1: Write failing tests.**

Add tests for:

```rust
#[test]
fn incomplete_added_compaction_never_uses_visible_text_recovery() {
    let mut state = ResponsesStreamState::default();
    state.apply(output_item_added(0, serde_json::json!({
        "type": "compaction",
        "id": "cmp_1",
        "encrypted_content": "opaque"
    }))).unwrap();
    state.apply(event(serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": 2,
        "item_id": "msg_1",
        "output_index": 1,
        "content_index": 0,
        "delta": "visible fallback",
        "logprobs": []
    }))).unwrap();
    state.apply(completed_with_output(serde_json::json!([]))).unwrap();

    let error = state.finish().unwrap_err().to_string();
    assert!(error.contains("compaction") || error.contains("incomplete"));
    assert!(!error.contains("visible fallback"));
}
```

Also cover a compaction `added` followed by `done` for a different index and an empty terminal output; it must fail rather than reconstruct a baseline missing the compaction item.

- [ ] **Step 2: Run the focused stream tests and verify failure.**

```bash
cargo test -p tact_llm openai::responses::stream --lib
```

Expected: the new tests fail because `finish()` currently permits visible-text recovery whenever deltas exist.

- [ ] **Step 3: Implement the minimal state-machine guard.**

Before the visible-text recovery branch in `finish()`, reject incomplete `pending_added` sequences when any pending item is a compaction item. Track pending item types in `ResponsesStreamState` (for example `pending_compactions: BTreeSet<u32>`) when `ResponseOutputItemAdded` receives `OutputItem::Compaction`; remove the index on `done`.

Return:

```rust
LlmError::Unsupported(
    "OpenAI Responses stream ended with an incomplete compaction item sequence".into(),
)
```

Do not change ordinary visible-text recovery for incomplete non-compaction items.

- [ ] **Step 4: Run stream and Responses tests.**

```bash
cargo test -p tact_llm openai::responses::stream --lib
cargo test -p tact_llm openai::responses --lib
```

Expected: all tests pass, including existing text-recovery tests and the new compaction guard.

- [ ] **Step 5: Commit.**

```bash
git add crates/tact_llm/src/openai/responses/stream.rs
git commit -m "fix(responses): reject incomplete streamed compaction"
```

---

### Task 2: Derive Responses threshold for subagents

**Files:**
- Modify: `crates/tact/src/config/resolve.rs`
- Test: config resolution tests in the same file

**Interfaces:**
- Consumes `resolve_responses_compact_threshold`, `SubagentSettings`, and resolved `model_context_window`/`max_tokens`.
- Produces a subagent `ProviderInfo.responses_compact_threshold` that is derived when omitted and validated against the subagent's own output budget.

- [ ] **Step 1: Write failing tests.**

Add:

```rust
#[test]
fn subagent_responses_threshold_is_derived_when_omitted() {
    let config = config_with_responses_main_and_subagent(None, 8_000, 200_000);
    let resolved = resolve_config(&empty_cli_args(), &config, None).unwrap();
    let subagent = resolved.agent.subagent.unwrap();
    assert_eq!(subagent.provider.responses_compact_threshold, Some(172_000));
}
```

Use the existing TOML helpers and ensure the subagent max token override is used in the expected calculation. Add a second test where subagent `max_tokens` differs from the main value and assert the derived threshold uses the subagent value.

- [ ] **Step 2: Run focused config tests and verify failure.**

```bash
cargo test -p tact config::resolve --lib
```

Expected: the new omitted-threshold test fails because `resolve_subagent` currently passes `None` through unchanged.

- [ ] **Step 3: Implement shared threshold resolution.**

After resolving subagent `max_tokens`, call the same calculation used by the main provider:

```rust
let subagent_threshold = resolve_responses_compact_threshold(
    entry.responses_compact_threshold,
    protocol,
    model_context_window,
    max_tokens,
)?;
```

Store `subagent_threshold` in `ProviderInfo`. Preserve `None` for non-Responses and zero context windows.

- [ ] **Step 4: Run config and Agent tests.**

```bash
cargo test -p tact config::resolve --lib
cargo test -p tact agent --lib
```

- [ ] **Step 5: Commit.**

```bash
git add crates/tact/src/config/resolve.rs
git commit -m "fix(config): derive Responses threshold for subagents"
```

---

### Task 3: Make usage conversion overflow-safe

**Files:**
- Modify: `crates/tact_llm/src/openai/responses/normalize.rs`
- Test: normalization tests in the same file

**Interfaces:**
- Consumes Responses usage JSON from explicit compact resources.
- Produces `Option<TokenUsageInfo>` only when all required token values fit `u32`; otherwise returns `Result<_, LlmError>` with a protocol/serialization error.

- [ ] **Step 1: Write failing overflow tests.**

Change the helper to return a `Result` and add:

```rust
#[test]
fn compact_usage_overflow_is_rejected() {
    let value = serde_json::json!({
        "output": [{"type":"compaction","id":"cmp","encrypted_content":"opaque"}],
        "usage": {"input_tokens": u64::from(u32::MAX) + 1, "output_tokens": 1, "total_tokens": 1}
    });
    let error = parse_compact_resource(value).unwrap_err().to_string();
    assert!(error.contains("token") || error.contains("usage"));
}
```

Add coverage for `cached_tokens` and `reasoning_tokens` overflow if those fields are present.

- [ ] **Step 2: Run focused normalization tests and verify failure.**

```bash
cargo test -p tact_llm openai::responses::normalize --lib
```

Expected: the overflow test fails because current `as u32` casts truncate.

- [ ] **Step 3: Implement checked conversion.**

Use a helper:

```rust
fn token_u32(value: &serde_json::Value, field: &str) -> Result<u32, LlmError> {
    let raw = value.as_u64().ok_or_else(|| {
        LlmError::Unsupported(format!("Responses usage field '{field}' is not an unsigned integer"))
    })?;
    u32::try_from(raw).map_err(|_| {
        LlmError::Unsupported(format!("Responses usage field '{field}' exceeds u32"))
    })
}
```

Make optional cache/reasoning fields default to zero only when absent; if present but invalid or overflowing, return an error.

- [ ] **Step 4: Run focused and full LLM tests.**

```bash
cargo test -p tact_llm openai::responses --lib
cargo test -p tact_llm --lib
```

- [ ] **Step 5: Commit.**

```bash
git add crates/tact_llm/src/openai/responses/normalize.rs
git commit -m "fix(responses): reject overflowing usage counters"
```

---

### Task 4: Strengthen state/resource validation and safe diagnostics

**Files:**
- Modify: `crates/tact_llm/src/openai/responses/convert.rs`
- Modify: `crates/tact_llm/src/openai/responses/normalize.rs`
- Modify: `crates/tact/src/agent/mod.rs`
- Test: conversion/normalization/Agent tests

**Interfaces:**
- Consumes persisted `ResponsesConversationState`, `parse_compact_resource`, and native compact status output.
- Produces explicit version validation, top-level compact-resource validation, and redacted diagnostics.

- [ ] **Step 1: Write failing tests.**

Add tests for:

```rust
#[test]
fn state_with_unknown_version_is_rejected() {
    let request = request_with_history();
    let mut state = state_covering_first_message(&request);
    state.version = 2;
    assert!(create_response(
        &request,
        Some(&ProviderConversationState::OpenAiResponses(state)),
        None,
        None,
    ).is_err());
}

#[test]
fn compact_resource_requires_expected_object_and_id() {
    let missing_object = serde_json::json!({
        "id":"cmp","output":[{"type":"compaction","id":"item","encrypted_content":"opaque"}]
    });
    assert!(parse_compact_resource(missing_object).is_err());
}
```

Add an Agent assertion that the success Info line contains only a bounded id prefix or hash, not the complete compaction id.

- [ ] **Step 2: Run focused tests and verify failure.**

```bash
cargo test -p tact_llm openai::responses --lib
cargo test -p tact agent --lib
```

Expected: version/object/diagnostic tests fail against current behavior.

- [ ] **Step 3: Implement validation and bounded diagnostics.**

- Reject `state.version != 1` with `LlmError::Unsupported` before hashing or slicing.
- Require compact resource `object == "response.compaction"` and a non-empty top-level `id` before accepting output.
- Add a helper such as:

```rust
fn compact_id_display(id: &str) -> String {
    const MAX_ID_CHARS: usize = 12;
    id.chars().take(MAX_ID_CHARS).collect()
}
```

Use it only for `AgentUpdate::Info`; retain the full id inside provider state and SQLite metadata.
- Keep reasoning encrypted history signatures internal and add a test proving only `ContentBlock::Thinking.signature` carries the envelope while no `ContentBlock::Text`, Info, TUI, log, or error contains the payload.

- [ ] **Step 4: Run focused tests.**

```bash
cargo test -p tact_llm openai::responses --lib
cargo test -p tact agent --lib
cargo test -p tact-ui --test recovery_compaction
```

- [ ] **Step 5: Commit.**

```bash
git add crates/tact_llm/src/openai/responses/convert.rs crates/tact_llm/src/openai/responses/normalize.rs crates/tact/src/agent/mod.rs
git commit -m "fix(responses): validate state and redact compact diagnostics"
```

---

### Task 5: Make explicit usage persistence observable

**Files:**
- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `docs/token_usage_schema.md` if wording needs clarification
- Test: Agent persistence-failure test

**Interfaces:**
- Consumes `persist_llm_call()` and the explicit native compact path.
- Produces no silent usage persistence failure for `responses_compact`; state commit remains the atomic correctness boundary.

- [ ] **Step 1: Write a failing persistence test.**

Force token usage insertion to fail after the native compact endpoint succeeds. Assert the Agent returns an error (or emits a structured warning if the existing contract explicitly remains best-effort) and that provider state/messages are not falsely reported as fully persisted.

- [ ] **Step 2: Run the focused test and verify failure.**

```bash
cargo test -p tact agent --lib
```

- [ ] **Step 3: Implement explicit error handling.**

Replace:

```rust
let _ = self.persist_llm_call("responses_compact", usage, body).await;
```

with:

```rust
self.persist_llm_call("responses_compact", usage, body)
    .await
    .context("failed to persist Responses compact usage")?;
```

Keep state/messages replacement after usage persistence so a usage failure leaves the old committed state intact. Preserve best-effort behavior for ordinary stream/local compact unless separately changed.

- [ ] **Step 4: Run Agent/store/integration tests.**

```bash
cargo test -p tact agent --lib
cargo test -p tact session_store --lib
cargo test -p tact-ui --test recovery_compaction
```

- [ ] **Step 5: Commit.**

```bash
git add crates/tact/src/agent/mod.rs
git commit -m "fix(agent): surface Responses compact usage failures"
```

---

### Task 6: Documentation, final review, and full verification

**Files:**
- Modify: `book/05_chapter_compact.md`, `book/05_chapter_compact_zh.md`
- Modify: `book/22_chapter_llm.md`, `book/22_chapter_llm_zh.md`
- Modify: `book/23_chapter_tui.md`, `book/23_chapter_tui_zh.md`
- Modify: `docs/token_usage_schema.md` if needed
- Test: full workspace

**Interfaces:**
- Consumes all previous fixes.
- Produces synchronized documentation and a clean release checkpoint.

- [ ] **Step 1: Update docs.**

Document:

- incomplete streamed compaction is a hard error, not visible-text recovery;
- subagent thresholds are derived using the subagent budget;
- checked usage overflow is rejected;
- compaction id diagnostics are truncated;
- reasoning encrypted data may exist only in an internal non-renderable signature envelope, while compaction encrypted data remains provider state only.

Keep English/Chinese heading hierarchy aligned.

- [ ] **Step 2: Run documentation checks.**

```bash
git diff --check
for pair in \
  "book/05_chapter_compact.md book/05_chapter_compact_zh.md" \
  "book/22_chapter_llm.md book/22_chapter_llm_zh.md" \
  "book/23_chapter_tui.md book/23_chapter_tui_zh.md"; do
  # Compare heading levels, not translated heading text.
  awk '/^#{1,6} / { match($0,/^#+/); print RLENGTH }' $pair | paste -sd, -
done
```

- [ ] **Step 3: Run Rust verification sequentially.**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p tact_llm openai::responses --lib
cargo test -p tact session_store --lib
cargo test -p tact agent --lib
cargo build --workspace
cargo test --workspace
```

- [ ] **Step 4: Final acceptance review.**

Verify:

```text
incomplete compaction never falls back to visible text
subagent threshold automatic behavior
checked usage conversion
reasoning/compaction encrypted boundary
version/object validation
bounded diagnostics
explicit usage persistence failure is visible
non-Responses behavior unchanged
EN/ZH docs aligned
```

- [ ] **Step 5: Commit docs and update progress.**

```bash
git add book/05_chapter_compact.md book/05_chapter_compact_zh.md \
  book/22_chapter_llm.md book/22_chapter_llm_zh.md \
  book/23_chapter_tui.md book/23_chapter_tui_zh.md docs/token_usage_schema.md
git commit -m "docs(responses): document review hardening"
```
