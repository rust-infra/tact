# OpenAI Responses Compatibility Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the typed-core/raw-JSON Responses foundation that preserves unknown protocol data, exposes explicit Responses request options, and keeps the existing OpenAI text/reasoning/function/compaction paths working without forking `async-openai` prematurely.

**Architecture:** Keep `async-openai 0.41.1` for transport and known Responses types. Add a Tact-owned raw wire boundary before typed deserialization, a Tact-owned `ResponsesRequestOptions` extension for fields not present in the shared Chat/Anthropic request model, and a versioned state representation that round-trips unknown input/output items. Do not add an SDK fork unless a fixture demonstrates that the current `byot`/raw boundary cannot implement a required behavior; if that happens, isolate the fork to the Responses dependency and keep Tact-specific state/TUI/permission logic outside it.

**Tech Stack:** Rust 2024, `async-openai-responses 0.41.1` with `responses` and `byot`, `serde_json::Value`, Tokio, WireMock, SQLite/sqlx, existing Tact `LlmClient` and provider-state APIs.

## Global Constraints

- Preserve the existing `async-openai 0.20` Chat Completions dependency; do not replace both protocol versions with one unverified fork.
- Unknown Responses events that do not affect protocol state must not abort a normal response.
- Unknown output/input items must be retained as raw JSON and round-trip through provider state.
- Compaction items remain opaque protocol state and never become `ContentBlock` values.
- Hosted provider-executed tools never enter the local Tact tool dispatcher.
- No API key, encrypted reasoning content, or complete sensitive input may appear in diagnostics.
- Async tests waiting on channels must use bounded timeouts; no unbounded `recv().await`.
- Never run multiple Cargo commands concurrently; clear `http_proxy`, `https_proxy`, and `all_proxy` for local WireMock tests.
- Do not add a dependency when `serde_json`, the existing SDK, or existing Tact infrastructure is sufficient.
- Update `book/05_chapter_compact.md` and `book/05_chapter_compact_zh.md` when compaction behavior changes; update both issue-log languages for shipped user-visible behavior.

---

## File Map

- Create: `crates/tact_llm/src/openai/responses/wire.rs` — raw event/response envelope parsing and known/unknown item partitioning.
- Create: `crates/tact_llm/src/openai/responses/request_options.rs` — Responses-only request options and JSON patch serialization; re-export `ResponsesRequestOptions` from `openai::responses`.
- Modify: `crates/tact_llm/src/openai/responses/mod.rs` — use raw wire parsing for streaming and non-streaming responses.
- Modify: `crates/tact_llm/src/openai/responses/convert.rs` — serialize `ResponsesRequestOptions` and preserve raw baseline items.
- Modify: `crates/tact_llm/src/openai/responses/normalize.rs` — normalize known items while retaining raw output order.
- Modify: `crates/tact_llm/src/openai/responses/stream.rs` — envelope-first event state machine with raw unknown-item retention.
- Modify: `crates/tact_llm/src/types.rs` — carry optional Responses-only request options without changing non-Responses wire behavior.
- Modify: `crates/tact_llm/src/provider_state.rs` — versioned raw item/state schema and migration checks.
- Modify: `crates/tact_llm/src/error.rs` — contextual wire errors that do not include sensitive payloads.
- Modify: `crates/tact_llm/src/openai/responses/*.rs` tests — fixtures and round-trip tests.
- Modify: `crates/tact/src/agent/mod.rs` — preserve existing state/compaction behavior while consuming the extended state update.
- Modify: `crates/tact/src/store/session_store/sqlite.rs` — verify existing JSON persistence accepts the extended state without leaking payloads.
- Modify: `config.example.toml` — document only options that are actually exposed by Tact after implementation.
- Modify: `docs/compaction.md`, `book/05_chapter_compact.md`, `book/05_chapter_compact_zh.md` — update raw-state/native-compaction behavior if the wire baseline contract changes.
- Modify: `book/26_chapter_issue.md`, `book/26_chapter_issue_zh.md` — add a newest-first user-visible behavior entry if the shipped behavior changes.

## Interfaces Produced by This Plan

The implementation must expose these crate-internal interfaces; names may only change with an equivalent type/signature and corresponding plan update:

```rust
// crates/tact_llm/src/openai/responses/wire.rs
pub(crate) struct RawResponseEnvelope {
    pub(crate) value: serde_json::Value,
    pub(crate) typed: async_openai_responses::types::responses::Response,
    pub(crate) output_items: Vec<serde_json::Value>,
    pub(crate) unknown_output_items: Vec<serde_json::Value>,
}

pub(crate) fn parse_response_envelope(
    value: serde_json::Value,
) -> Result<RawResponseEnvelope, crate::LlmError>;

pub(crate) fn raw_output_items(value: &serde_json::Value)
    -> Result<Vec<serde_json::Value>, crate::LlmError>;
```

```rust
// crates/tact_llm/src/openai/responses/request_options.rs
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ResponsesRequestOptions {
    pub parallel_tool_calls: Option<bool>,
    pub truncation: Option<String>,
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    pub user: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub text: Option<serde_json::Value>,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ResponsesRequestOptions {
    pub(crate) fn apply_to(
        &self,
        body: &mut serde_json::Value,
    ) -> Result<(), crate::LlmError>;
}
```

```rust
// crates/tact_llm/src/types.rs
impl CreateMessageParams {
    pub fn with_responses_options(
        self,
        options: crate::openai::responses::ResponsesRequestOptions,
    ) -> Self;
}
```

The field is `None` by default, skipped by non-Responses adapters, and never serialized into Chat Completions or Anthropic requests.

---

### Task 1: Add the Responses-only request options boundary

**Files:**
- Create: `crates/tact_llm/src/openai/responses/request_options.rs`
- Modify: `crates/tact_llm/src/openai/responses/mod.rs`
- Modify: `crates/tact_llm/src/types.rs`
- Modify: `crates/tact_llm/src/convert.rs`
- Test: `crates/tact_llm/src/openai/responses/request_options.rs` (`#[cfg(test)]`)
- Test: `crates/tact_llm/src/openai/responses/convert.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: existing `CreateMessageParams` and Responses JSON body builder.
- Produces: `ResponsesRequestOptions` and `CreateMessageParams::with_responses_options` used by the Responses adapter only.

- [ ] **Step 1: Write the failing serialization tests.**

```rust
#[test]
fn responses_options_patch_only_populates_responses_fields() {
    let options = ResponsesRequestOptions {
        parallel_tool_calls: Some(false),
        truncation: Some("auto".into()),
        metadata: Some(serde_json::Map::from_iter([(
            "ticket".into(),
            serde_json::json!("r-1"),
        )])),
        user: Some("user-1".into()),
        prompt_cache_key: Some("cache-1".into()),
        text: Some(serde_json::json!({"format": {"type": "text"}})),
        extra: Default::default(),
    };
    let mut body = serde_json::json!({"model": "gpt-5", "input": []});
    options.apply_to(&mut body).unwrap();
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["truncation"], "auto");
    assert_eq!(body["metadata"]["ticket"], "r-1");
    assert_eq!(body["user"], "user-1");
    assert_eq!(body["prompt_cache_key"], "cache-1");
    assert_eq!(body["text"]["format"]["type"], "text");
}

#[test]
fn responses_options_rejects_non_object_metadata() {
    let options = ResponsesRequestOptions {
        metadata: None,
        extra: serde_json::Map::from_iter([("text".into(), serde_json::json!("bad"))]),
        ..Default::default()
    };
    let mut body = serde_json::json!({});
    let error = options.apply_to(&mut body).unwrap_err().to_string();
    assert!(error.contains("Responses request option 'text'"));
}
```

- [ ] **Step 2: Run the focused test and verify it fails because the type and method do not exist.**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm responses_options_patch_only_populates_responses_fields --lib
```

Expected: compile failure naming the missing `ResponsesRequestOptions` or `apply_to` implementation.

- [ ] **Step 3: Implement the options type and JSON patch.**

Use `serde_json::Value` only for fields whose shape is intentionally extensible (`text` and `extra`). Reject non-object request bodies and conflicting `extra` keys with an `LlmError::Unsupported` message naming the field. Apply explicit typed fields first, then reject `extra` collisions instead of silently overwriting known fields.

- [ ] **Step 4: Add the optional field and builder to `CreateMessageParams`.**

Declare `mod request_options; pub use request_options::ResponsesRequestOptions;` in `crates/tact_llm/src/openai/responses/mod.rs`. The field must be skipped by serde and default to `None`:

```rust
#[serde(skip)]
pub responses_options: Option<crate::openai::responses::ResponsesRequestOptions>,
```

Add `with_responses_options`, and update `Default` construction only as needed by the compiler. Chat/Anthropic conversion tests must prove the field does not appear on their wire bodies.

- [ ] **Step 5: Apply options from `convert::create_response`.**

After the existing typed request is serialized and before state input replacement, call `options.apply_to(&mut body)` when `request.responses_options` is `Some(_)`. Existing context-management patching remains separate and wins only for its own `context_management` key.

- [ ] **Step 6: Run the focused tests and the existing Responses conversion tests.**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::convert --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm responses_options --lib
```

Expected: all focused tests pass, including existing request/compaction tests.

- [ ] **Step 7: Commit the task.**

```bash
git add crates/tact_llm/src/types.rs crates/tact_llm/src/convert.rs crates/tact_llm/src/openai/responses/mod.rs crates/tact_llm/src/openai/responses/request_options.rs
git commit -m "feat: add Responses request options boundary"
```

---

### Task 2: Introduce envelope-first raw response parsing

**Files:**
- Create: `crates/tact_llm/src/openai/responses/wire.rs`
- Modify: `crates/tact_llm/src/openai/responses/mod.rs`
- Modify: `crates/tact_llm/src/openai/responses/normalize.rs`
- Modify: `crates/tact_llm/src/openai/responses/stream.rs`
- Test: `crates/tact_llm/src/openai/responses/wire.rs` (`#[cfg(test)]`)
- Test: `crates/tact_llm/src/openai/responses/normalize.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: raw JSON returned by `create_byot`, `create_stream_byot`, and terminal SSE events.
- Produces: `RawResponseEnvelope`, raw output item order, known typed `Response`, and non-fatal unknown-event handling.

- [ ] **Step 1: Add a fixture with an unknown output item and a valid message.**

Create a test-local JSON value:

```rust
fn response_with_unknown_output_item() -> serde_json::Value {
    serde_json::json!({
        "id": "resp_unknown_item",
        "object": "response",
        "status": "completed",
        "output": [
            {"type": "future_item", "id": "future-1", "payload": {"x": 1}},
            {"type": "message", "id": "msg-1", "status": "completed", "role": "assistant",
             "content": [{"type": "output_text", "text": "hello", "annotations": []}]}
        ],
        "usage": {"input_tokens": 1, "input_tokens_details": {"cached_tokens": 0},
                  "output_tokens": 1, "output_tokens_details": {"reasoning_tokens": 0},
                  "total_tokens": 2}
    })
}
```

- [ ] **Step 2: Run the test against the current typed boundary and verify the expected failure.**

Add a test calling `parse_response_envelope(response_with_unknown_output_item())` and assert that it returns two raw output items and one unknown item. Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::wire --lib
```

Expected: compile failure until the new module/function exists; after temporarily wiring only typed deserialization, the fixture must fail with the SDK unknown-variant error. The final implementation must remove that failure.

- [ ] **Step 3: Implement raw output extraction before typed parsing.**

`raw_output_items` must require an object response and array `output`, clone each item as `Value`, and return contextual errors for missing/wrong-shaped fields. `parse_response_envelope` must:

1. extract raw output items;
2. classify each item by its string `type`;
3. deserialize the complete response through the SDK only when all items are known;
4. otherwise deserialize a typed response with unknown items removed/replaced only for normalization, while retaining the original raw sequence in `RawResponseEnvelope`;
5. never expose unknown payloads in an error string.

The typed surrogate must preserve response status, usage, ids, and all known output items in original order.

- [ ] **Step 4: Route non-streaming Responses through the envelope parser.**

In `OpenAiResponsesAdapter::create_message`, request `Value`, call `parse_response_envelope`, normalize the typed surrogate, and pass the raw output sequence to `NormalizedResponse` for state update. Do not deserialize `Value` directly into `Response` before the raw parser.

- [ ] **Step 5: Add unknown event envelope handling for streams.**

`parse_stream_event` must inspect `type` before typed deserialization. Existing known events continue through `ResponseStreamEvent`; an unknown event becomes `Ok(None)` unless it is an output-item lifecycle event with a missing/invalid item object, which returns a contextual protocol error. Keep the raw event only for diagnostics bounded to event type, not payload.

- [ ] **Step 6: Run the wire, normalize, and stream test groups.**

Run:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::wire --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::normalize --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses::stream --lib
```

Expected: unknown output item is retained, ordinary text remains visible, unknown harmless events do not abort, and existing compaction sequence tests remain green.

- [ ] **Step 7: Commit the task.**

```bash
git add crates/tact_llm/src/openai/responses/wire.rs crates/tact_llm/src/openai/responses/mod.rs crates/tact_llm/src/openai/responses/normalize.rs crates/tact_llm/src/openai/responses/stream.rs
git commit -m "feat: preserve unknown Responses wire items"
```

---

### Task 3: Make provider state round-trip raw Responses items

**Files:**
- Modify: `crates/tact_llm/src/provider_state.rs`
- Modify: `crates/tact_llm/src/openai/responses/normalize.rs`
- Modify: `crates/tact_llm/src/openai/responses/convert.rs`
- Modify: `crates/tact/src/store/session_store/sqlite.rs`
- Test: `crates/tact_llm/src/provider_state.rs` (`#[cfg(test)]`)
- Test: `crates/tact/src/store/session_store/sqlite.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `RawResponseEnvelope.output_items` from Task 2.
- Produces: versioned `ResponsesConversationState` that preserves unknown baseline/output JSON and validates context hashes.

- [ ] **Step 1: Write the failing state round-trip test.**

Extend the existing state fixture with an unknown item and assert its exact JSON survives serialize/deserialize and `create_response`:

```rust
#[test]
fn state_with_unknown_items_round_trips_exact_json() {
    let unknown = serde_json::json!({
        "type": "future_item", "id": "future-1", "payload": {"nested": [1, 2, 3]}
    });
    let state = state_with_input_items(vec![unknown.clone()]);
    let encoded = serde_json::to_string(&ProviderConversationState::OpenAiResponses(state)).unwrap();
    let decoded: ProviderConversationState = serde_json::from_str(&encoded).unwrap();
    let ProviderConversationState::OpenAiResponses(decoded) = decoded else { panic!("wrong state variant") };
    assert_eq!(decoded.input_items[0], unknown);
}
```

- [ ] **Step 2: Run the focused state tests and confirm the new fixture fails or exposes the missing state field.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm provider_state::tests::state_with_unknown_items_round_trips_exact_json --lib
```

Expected: failure until the state fixture/helper and baseline handling are updated.

- [ ] **Step 3: Keep state schema version 1 backward-compatible and add raw item validation.**

Do not change existing persisted field names unless required. Ensure `input_items: Vec<Value>` remains the canonical raw baseline. Add a state validation helper that rejects non-object input items and preserves unknown fields. If a schema change is unavoidable, add an explicit version 2 enum and a migration test from the current version 1 JSON; do not silently reinterpret old state.

- [ ] **Step 4: Pass raw terminal output into `ProviderStateUpdate`.**

Update `NormalizedResponse` so state creation receives the exact ordered raw output items. Known items remain normalized for Tact blocks; unknown items remain only in protocol state. The logical message count/hash rules remain unchanged.

- [ ] **Step 5: Verify SQLite persistence and sensitive-data redaction.**

Add an SQLite test that persists/loads a state containing a marker in an unknown item, confirms the state JSON contains it when loaded by the owning session, and confirms the corrupt-state error path reports only structural metadata and not the marker. Use existing session-store helpers rather than adding a new persistence layer.

- [ ] **Step 6: Run state and SQLite tests.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm provider_state --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact store::session_store::sqlite --lib
```

Expected: all existing state/SQLite tests plus the new exact round-trip and redaction tests pass.

- [ ] **Step 7: Commit the task.**

```bash
git add crates/tact_llm/src/provider_state.rs crates/tact_llm/src/openai/responses/normalize.rs crates/tact_llm/src/openai/responses/convert.rs crates/tact/src/store/session_store/sqlite.rs
git commit -m "feat: round-trip raw Responses state items"
```

---

### Task 4: Add Responses capability metadata without enabling unverified providers

**Files:**
- Create: `crates/tact_llm/src/openai/responses/capabilities.rs`
- Modify: `crates/tact_llm/src/provider.rs`
- Modify: `crates/tact_llm/src/types.rs`
- Modify: `crates/tact/src/config/types.rs`
- Modify: `crates/tact/src/config/resolve.rs`
- Test: `crates/tact_llm/src/openai/responses/capabilities.rs` (`#[cfg(test)]`)
- Test: `crates/tact/src/config/resolve.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: provider kind, protocol, base URL, and current Responses adapter construction.
- Produces: stable `ResponsesCapabilities` metadata for routing and configuration validation.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResponsesToolKind {
    WebSearch,
    FileSearch,
    CodeInterpreter,
    ImageGeneration,
    Computer,
    LocalShell,
    Shell,
    Custom,
    Namespace,
    ApplyPatch,
    ToolSearch,
    RemoteMcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsesCapabilities {
    pub responses: bool,
    pub streaming: bool,
    pub compact: bool,
    pub hosted_tools: std::collections::BTreeSet<ResponsesToolKind>,
}
```

- [ ] **Step 1: Write capability and config rejection tests.**

Assert that official OpenAI Responses defaults to `responses = true`, `streaming = true`, `compact = true`, while custom providers default to only the capabilities explicitly configured. Assert that DeepSeek and Kimi remain rejected by current config resolution until their endpoint validation is separately approved.

- [ ] **Step 2: Run the tests and verify missing capability APIs fail.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm responses_capabilities --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact config::resolve::tests::reject_deepseek_responses_protocol --lib
```

Expected: the new capability test fails to compile before implementation; the existing DeepSeek rejection remains green.

- [ ] **Step 3: Implement capability defaults and provider exposure.**

OpenAI official provider gets the capabilities already implemented by this foundation. Custom providers must not silently claim Hosted Tool support. Keep `build_openai_responses` usable for custom endpoints, but use capabilities to gate requests that require unsupported features. Do not remove DeepSeek/Kimi config rejection in this task.

- [ ] **Step 4: Keep capability configuration out of TOML in this foundation.**

Expose static capability metadata from `ProviderInfo`: official OpenAI enables only capabilities implemented by this repository, while custom providers expose core Responses streaming but no Hosted Tool capability. Do not infer capabilities from model names or base URL substrings. A later Hosted Tools plan may add an explicit TOML allow-list after its request and execution semantics are implemented.

- [ ] **Step 5: Run capability and config tests.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm provider::tests --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact config::resolve --lib
```

Expected: all provider construction and existing rejection tests pass, with new capability assertions green.

- [ ] **Step 6: Commit the task.**

```bash
git add crates/tact_llm/src/openai/responses/capabilities.rs crates/tact_llm/src/provider.rs crates/tact_llm/src/types.rs crates/tact/src/config/types.rs crates/tact/src/config/resolve.rs
git commit -m "feat: describe Responses provider capabilities"
```

---

### Task 5: Add wire fixtures and end-to-end regression coverage

**Files:**
- Create: `crates/tact_llm/src/openai/responses/fixtures/unknown_output_item.json`
- Create: `crates/tact_llm/src/openai/responses/fixtures/unknown_event_stream.jsonl`
- Modify: `crates/tact_llm/src/openai/responses/mod.rs`
- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact/src/store/session_store/sqlite.rs`
- Test: existing Responses unit and WireMock test modules in the files above

**Interfaces:**
- Consumes: Tasks 1–4 request options, raw envelope parser, raw state, and capability metadata.
- Produces: regression coverage proving existing behavior and new compatibility behavior coexist.

- [ ] **Step 1: Add fixtures for ordinary response, unknown item, unknown event, function call, and compaction.**

Use stable JSON fixtures with no real encrypted payloads or secrets. Keep unknown payloads short and deterministic. The stream fixture must contain terminal event, output item added/done, one harmless unknown event, and a valid visible text delta.

- [ ] **Step 2: Add failing end-to-end tests.**

Cover these exact behaviors:

```rust
#[tokio::test]
async fn responses_unknown_item_survives_next_request() {
    let server = wiremock::MockServer::start().await;
    // Register two `/responses` mocks, run two adapter turns, then assert
    // the second request contains the first turn's unknown raw item.
    let requests = record_requests(&server).await;
    assert_eq!(requests[1]["input"][0]["type"], "future_item");
}

#[tokio::test]
async fn responses_unknown_stream_event_does_not_abort_text() {
    let server = wiremock::MockServer::start().await;
    // Serve `unknown_event_stream.jsonl` as SSE and assert the final block is
    // `ContentBlock::Text { text: "hello" }`.
    let response = stream_one_turn(&server).await;
    assert_eq!(response.blocks, vec![ContentBlock::Text { text: "hello".into() }]);
}

#[tokio::test]
async fn responses_request_options_are_sent_without_affecting_chat_requests() {
    let server = wiremock::MockServer::start().await;
    let request = request_with_responses_options(&server).await;
    assert_eq!(request["parallel_tool_calls"], false);
    assert!(request.get("reasoning_effort").is_none());
}
```

Define test helpers in the same test module before using them: `record_requests(&MockServer) -> Vec<serde_json::Value>` parses `server.received_requests()` bodies; `stream_one_turn(&MockServer) -> LlmResponse` constructs the existing `OpenAiResponsesAdapter`, calls `stream_message`, and returns the response; `request_with_responses_options(&MockServer) -> serde_json::Value` constructs a request with `ResponsesRequestOptions`, runs `create_message`, and parses the captured body. Each async channel receive inside these helpers must use `tokio::time::timeout(Duration::from_secs(2), ...)`.

- [ ] **Step 3: Run the new tests before implementation wiring is complete.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses --lib
```

Expected: the new assertions fail until all previous tasks are integrated; existing tests identify any regression.

- [ ] **Step 4: Wire the full adapter and agent loop.**

Ensure `Agent::ensure_session`, ordinary stream calls, automatic compaction, explicit compaction, and SQLite replacement all use the new state update without changing the existing atomic persistence order.

- [ ] **Step 5: Run the complete focused matrix.**

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact agent::tests::responses --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact store::session_store::sqlite --lib
```

Expected: all focused tests pass, including existing compaction/state tests and the new raw compatibility cases.

- [ ] **Step 6: Commit the task.**

```bash
git add crates/tact_llm/src/openai/responses/fixtures crates/tact_llm/src/openai/responses crates/tact/src/agent/mod.rs crates/tact/src/store/session_store/sqlite.rs
git commit -m "test: cover Responses raw compatibility round trips"
```

---

### Task 6: Document the extension boundary and fork decision gate

**Files:**
- Modify: `config.example.toml`
- Modify: `docs/compaction.md`
- Modify: `book/05_chapter_compact.md`
- Modify: `book/05_chapter_compact_zh.md`
- Modify: `book/26_chapter_issue.md`
- Modify: `book/26_chapter_issue_zh.md`
- Modify: `docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`

**Interfaces:**
- Consumes: behavior delivered by Tasks 1–5.
- Produces: user-facing configuration and architecture documentation matching the implementation.

- [ ] **Step 1: Write documentation assertions/checklist before editing.**

The docs must explicitly state:

- Responses-specific options are not sent by Chat Completions or Anthropic adapters.
- Unknown protocol items are retained as raw JSON when state integrity is known.
- Native Responses compaction remains distinct from local summary compaction.
- Custom providers must not claim Hosted Tool support without capability declaration.
- SDK fork is not used unless a reproducible fixture demonstrates that the current `byot`/raw boundary cannot implement the required behavior.

- [ ] **Step 2: Update the config example and compaction docs.**

Only document fields that are implemented and tested. Keep English and Chinese compaction chapters structurally aligned.

- [ ] **Step 3: Add the issue-log entry if behavior is user-visible.**

Use the newest-first format with date, type, motivation, decision, observable behavior, and code/spec/chapter pointers. Do not replace existing subsystem chapters.

- [ ] **Step 4: Run documentation and formatting checks.**

```bash
cargo fmt --check
git diff --check
```

Expected: format and diff checks pass.

- [ ] **Step 5: Commit the task.**

```bash
git add config.example.toml docs/compaction.md book/05_chapter_compact.md book/05_chapter_compact_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md
git commit -m "docs: document Responses compatibility foundation"
```

---

## SDK Fork Decision Gate

After Tasks 1–5, fork `async-openai` only if at least one of these tests is impossible with the current dependency and raw boundary:

1. A required official request type cannot be emitted without changing SDK transport or builder behavior.
2. A required known response/event cannot be parsed into a typed surrogate while preserving the terminal response contract.
3. SDK transport cannot expose the response bytes needed for state-safe raw round-trip.
4. A required official endpoint cannot be reached through the current client configuration without a general-purpose transport fix.

If none applies, keep the crates.io dependency and continue with Hosted Tool-specific plans. If one applies, create a separate fork integration task that:

- pins a commit in `Cargo.toml` under the `async-openai-responses` package alias;
- keeps `async-openai 0.20` unchanged;
- adds only provider-agnostic SDK types/transport behavior;
- adds an upstream-compatible fixture for the blocker;
- leaves Tact state, permissions, tool execution, and TUI logic in this repository.

## Verification Gate

Before declaring this foundation complete, run these commands serially from the worktree:

```bash
env -u http_proxy -u https_proxy -u all_proxy cargo test --workspace --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact_llm openai::responses --lib
env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact agent::tests::responses --lib
cargo fmt --check
env -u http_proxy -u https_proxy -u all_proxy cargo clippy --workspace --all-targets --all-features -- -D warnings
```

The baseline recorded before implementation was 1246 passed, 0 failed, and 1 ignored; the final report must provide fresh command output rather than relying on that baseline.
