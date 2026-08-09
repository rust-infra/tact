# Responses Stream Event Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow supported but currently ignored Responses stream lifecycle and completion events to pass through Tact safely without changing Agent tool execution behavior.

**Architecture:** Expand the raw-event allowlist in `crates/tact_llm/src/openai/responses/mod.rs`, then add explicit no-op branches in `ResponsesStreamState::apply` for accepted events. Terminal Responses remain authoritative; no incremental event is converted into a new tool execution or TUI event.

**Tech Stack:** Rust, serde_json, async-openai 0.41.1, Cargo unit tests.

## Global Constraints

- Do not add new dependencies.
- Do not add hosted web-search-specific SSE events; `response.output_item.added/done` remains its lifecycle.
- Do not execute function calls, MCP calls, file search, code interpreter, image generation, or custom tools in this change.
- Keep `response.completed`, `response.incomplete`, and `response.failed` as the authoritative terminal data source.
- Run Cargo tests without `http_proxy`, `https_proxy`, and `all_proxy` because local wiremock tests use loopback.
- Do not stage or modify the unrelated untracked `agent_loop_flow.png` artifact.

---

### Task 1: Expand the Responses parser allowlist

**Files:**
- Modify: `crates/tact_llm/src/openai/responses/mod.rs:157-181`
- Test: `crates/tact_llm/src/openai/responses/mod.rs` existing `#[cfg(test)]` module

**Interfaces:**
- Consumes: raw JSON SSE events passed to `parse_stream_event_with_raw`.
- Produces: `ResponseStreamEvent` values for the newly accepted event types, or `None` only for genuinely unsupported events.

- [ ] **Step 1: Add a failing parser test**

Add a unit test beside the existing parser tests that builds one minimal valid JSON event for every newly accepted type and asserts `parse_stream_event(...)` returns `Some(...)` rather than `None`:

```rust
#[test]
fn accepts_supported_lifecycle_and_completion_events() {
    let events = [
        serde_json::json!({"type":"response.created","sequence_number":1,"response":{}}),
        serde_json::json!({"type":"response.queued","sequence_number":2,"response":{}}),
        serde_json::json!({"type":"response.in_progress","sequence_number":3,"response":{}}),
        serde_json::json!({"type":"response.content_part.added","sequence_number":4,"item_id":"msg","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}}),
        serde_json::json!({"type":"response.content_part.done","sequence_number":5,"item_id":"msg","output_index":0,"content_index":0,"part":{"type":"output_text","text":"done","annotations":[]}}),
        serde_json::json!({"type":"response.output_text.done","sequence_number":6,"item_id":"msg","output_index":0,"content_index":0,"text":"done","logprobs":[]}),
        serde_json::json!({"type":"response.refusal.done","sequence_number":7,"item_id":"msg","output_index":0,"content_index":0,"refusal":"no"}),
        serde_json::json!({"type":"response.reasoning_summary_part.added","sequence_number":8,"item_id":"rs","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}),
        serde_json::json!({"type":"response.reasoning_summary_part.done","sequence_number":9,"item_id":"rs","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"done"}}),
        serde_json::json!({"type":"response.reasoning_summary_text.done","sequence_number":10,"item_id":"rs","output_index":0,"summary_index":0,"text":"done"}),
        serde_json::json!({"type":"response.reasoning_text.done","sequence_number":11,"item_id":"rs","output_index":0,"content_index":0,"text":"done"}),
        serde_json::json!({"type":"response.function_call_arguments.delta","sequence_number":12,"item_id":"fc","output_index":1,"delta":"{\"x\":"}),
        serde_json::json!({"type":"response.function_call_arguments.done","sequence_number":13,"item_id":"fc","output_index":1,"arguments":"{\"x\":1}"}),
    ];

    for value in events {
        assert!(parse_stream_event(value).unwrap().is_some());
    }
}
```

Use the exact SDK-required fields if the test compilation identifies a fixture mismatch; the assertion must remain about parser acceptance, not a particular variant implementation.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::tests::accepts_supported_lifecycle_and_completion_events
```

Expected: FAIL because `parse_stream_event_with_raw` currently returns `None` for the new event types.

- [ ] **Step 3: Extend the `consumed` match**

Add exactly these strings to the existing `matches!` expression in `parse_stream_event_with_raw`:

```rust
| "response.created"
| "response.queued"
| "response.in_progress"
| "response.content_part.added"
| "response.content_part.done"
| "response.output_text.done"
| "response.refusal.done"
| "response.reasoning_summary_part.added"
| "response.reasoning_summary_part.done"
| "response.reasoning_summary_text.done"
| "response.reasoning_text.done"
| "response.function_call_arguments.delta"
| "response.function_call_arguments.done"
```

Do not add hosted web-search lifecycle event strings.

- [ ] **Step 4: Run the focused test and verify GREEN**

Run the same focused test. Expected: PASS.

- [ ] **Step 5: Commit the parser change**

```bash
git add crates/tact_llm/src/openai/responses/mod.rs
git commit -m "fix: accept more Responses stream events"
```

---

### Task 2: Add explicit no-op state handling

**Files:**
- Modify: `crates/tact_llm/src/openai/responses/stream.rs:212-295`
- Test: `crates/tact_llm/src/openai/responses/stream.rs` existing `#[cfg(test)]` module

**Interfaces:**
- Consumes: newly parsed `ResponseStreamEvent` variants.
- Produces: no `AgentUpdate` for lifecycle, completion, and function-call argument events; existing delta and terminal behavior remains unchanged.

- [ ] **Step 1: Add a failing state-machine test**

Add a test that sends representative lifecycle, done, and function-call events through `ResponsesStreamState::apply` and asserts every result is empty:

```rust
#[test]
fn ignores_non_rendering_response_events_without_changing_stream_state() {
    let mut state = ResponsesStreamState::default();
    let events = [
        serde_json::json!({"type":"response.created","sequence_number":1,"response":{}}),
        serde_json::json!({"type":"response.queued","sequence_number":2,"response":{}}),
        serde_json::json!({"type":"response.in_progress","sequence_number":3,"response":{}}),
        serde_json::json!({"type":"response.output_text.done","sequence_number":4,"item_id":"msg","output_index":0,"content_index":0,"text":"answer","logprobs":[]}),
        serde_json::json!({"type":"response.refusal.done","sequence_number":5,"item_id":"msg","output_index":0,"content_index":0,"refusal":"no"}),
        serde_json::json!({"type":"response.function_call_arguments.delta","sequence_number":6,"item_id":"fc","output_index":1,"delta":"{}"}),
        serde_json::json!({"type":"response.function_call_arguments.done","sequence_number":7,"item_id":"fc","output_index":1,"arguments":"{}"}),
    ];

    for value in events {
        assert!(state.apply(event(value)).unwrap().is_empty());
    }
}
```

Because the current `event` helper is private to the test module, place the test where it can call that helper directly or use the existing module helper pattern without introducing a production-only API.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::stream::tests::ignores_non_rendering_response_events_without_changing_stream_state
```

Expected: FAIL to compile or fail to match until the new event variants are handled by `ResponsesStreamState::apply`.

- [ ] **Step 3: Add explicit no-op match arms**

Before the existing `_ => Vec::new()` arm in `ResponsesStreamState::apply`, add explicit branches:

```rust
            ResponseStreamEvent::ResponseCreated(_)
            | ResponseStreamEvent::ResponseQueued(_)
            | ResponseStreamEvent::ResponseInProgress(_)
            | ResponseStreamEvent::ResponseContentPartAdded(_)
            | ResponseStreamEvent::ResponseContentPartDone(_)
            | ResponseStreamEvent::ResponseOutputTextDone(_)
            | ResponseStreamEvent::ResponseRefusalDone(_)
            | ResponseStreamEvent::ResponseReasoningSummaryPartAdded(_)
            | ResponseStreamEvent::ResponseReasoningSummaryPartDone(_)
            | ResponseStreamEvent::ResponseReasoningSummaryTextDone(_)
            | ResponseStreamEvent::ResponseReasoningTextDone(_)
            | ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(_)
            | ResponseStreamEvent::ResponseFunctionCallArgumentsDone(_) => Vec::new(),
```

Keep the fallback `_ => Vec::new()` temporarily because the dependency enum may gain variants in a future SDK release; the explicit arms document current intentional no-op behavior.

- [ ] **Step 4: Add regression coverage for delta + done de-duplication**

Add a test that applies an output text delta, then its done event, and asserts only the delta update contains the text:

```rust
#[test]
fn output_text_done_does_not_duplicate_streamed_text() {
    let mut state = ResponsesStreamState::default();
    let delta = state
        .apply(event(serde_json::json!({
            "type":"response.output_text.delta",
            "sequence_number":1,
            "item_id":"msg",
            "output_index":0,
            "content_index":0,
            "delta":"answer",
            "logprobs":[]
        })))
        .unwrap();
    assert!(delta.iter().any(|update| matches!(
        update,
        AgentUpdate::StreamChunk(text) if text == "answer"
    )));

    let done = state
        .apply(event(serde_json::json!({
            "type":"response.output_text.done",
            "sequence_number":2,
            "item_id":"msg",
            "output_index":0,
            "content_index":0,
            "text":"answer",
            "logprobs":[]
        })))
        .unwrap();
    assert!(done.is_empty());
}
```

- [ ] **Step 5: Run focused state tests and verify GREEN**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::stream::tests::
```

Expected: PASS, including existing web-search, terminal, compaction, and streamed-text tests.

- [ ] **Step 6: Commit the state-machine change**

```bash
git add crates/tact_llm/src/openai/responses/stream.rs
git commit -m "fix: handle non-rendering Responses stream events"
```

---

### Task 3: Full verification and cleanup

**Files:**
- Modify: none unless formatting changes are required
- Test: existing `crates/tact_llm` Responses tests

- [ ] **Step 1: Run the complete focused crate test suite**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses
```

Expected: PASS with no failures.

- [ ] **Step 2: Run formatting verification**

```bash
cargo fmt --all -- --check
```

Expected: PASS with no formatting changes required.

- [ ] **Step 3: Review the final diff**

```bash
git diff HEAD~2..HEAD --check
git status --short
```

Expected: no whitespace errors; only the two implementation commits plus the previously untracked `agent_loop_flow.png` remain visible.

- [ ] **Step 4: Update the task state**

Mark the implementation task complete only after both test commands pass. Do not stage `agent_loop_flow.png`.
