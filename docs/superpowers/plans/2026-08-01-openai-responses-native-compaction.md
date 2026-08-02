# OpenAI Responses Native Compaction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add native OpenAI Responses compaction to Tact, including explicit `/responses/compact`, automatic `context_management`, durable opaque provider state, and no local fallback for the Responses provider.

**Architecture:** Keep Tact's provider-independent `Vec<Message>` separate from a versioned `ProviderConversationState::OpenAiResponses` containing the exact Responses input-item baseline as JSON. Extend the LLM result/API with provider-state updates, persist messages and provider state in one SQLite transaction, and dispatch compaction by provider: Responses uses native endpoints while every other provider keeps the existing local summary compaction.

**Tech Stack:** Rust 2024, Tokio, `async-openai-responses 0.41.1`, `serde_json`, SQLx SQLite, existing Tact agent loop and TUI command driver.

## Global Constraints

- Responses must use `store: false` and must not use `previous_response_id` or `conversation`.
- Responses must not register the `compact` tool; the user `/compact` command remains available.
- Responses must not fall back to local summary compaction; native protocol failure preserves all old state and returns an error.
- `compaction` items and `encrypted_content` must never become a shared `ContentBlock`, log payload, or TUI text.
- Provider state and Tact messages must be committed atomically in SQLite.
- Other providers must retain their current local compaction behavior and tool registration.
- Do not run multiple Cargo commands concurrently; use one Cargo invocation at a time.
- Keep English/Chinese book chapters structurally aligned whenever behavior is documented.
- Do not commit unrelated working-tree changes.

---

## File map and ownership

### New files

- `crates/tact_llm/src/provider_state.rs` — versioned provider-specific state, Responses state update types, JSON/hash helpers.
- `crates/tact_llm/src/openai/responses/fixtures/explicit_compact.json` — sanitized explicit compact fixture.
- `crates/tact_llm/src/openai/responses/fixtures/automatic_compact.json` — sanitized automatic `context_management` fixture from the target endpoint.
- `crates/tact_llm/src/openai/responses/fixtures/stream_compact_events.jsonl` — sanitized stream event fixture.
- `docs/superpowers/plans/2026-08-01-openai-responses-native-compaction.md` — this plan.

### Modified LLM files

- `crates/tact_llm/src/lib.rs` — export provider-state and response types.
- `crates/tact_llm/src/client.rs` — replace tuple result with `LlmResponse`, add state-aware calls and native `compact` capability.
- `crates/tact_llm/src/openai/responses/convert.rs` — baseline/delta conversion, `context_management`, state validation, assistant normalization.
- `crates/tact_llm/src/openai/responses/history.rs` — preserve existing reasoning envelope and add state item helpers only where needed.
- `crates/tact_llm/src/openai/responses/normalize.rs` — retain protocol output, validate compaction, produce state candidates.
- `crates/tact_llm/src/openai/responses/stream.rs` — terminal response precedence and output-item event collection.
- `crates/tact_llm/src/openai/responses/mod.rs` — state-aware stream/create/compact endpoints and request-body recording.
- `crates/tact_llm/src/anthropic/mod.rs`, `crates/tact_llm/src/openai/mod.rs`, `crates/tact_llm/src/deepseek/mod.rs`, `crates/tact_llm/src/kimi/mod.rs`, `crates/tact_llm/src/mock.rs` — adapt existing clients to return `LlmResponse` with `ProviderStateUpdate::Unchanged`.
- `crates/tact_llm/src/provider.rs` — expose Responses compaction capability and preserve adapter construction.
- `crates/tact_llm/src/types.rs` — no planned changes; keep Responses state out of shared `ContentBlock` and place threshold/capability types in the provider-state/client boundary.

### Modified Agent/store/config files

- `crates/tact/src/agent/mod.rs` — runtime provider state, provider-aware compaction, state-aware LLM calls, atomic commits, and Responses tool filtering.
- `crates/tact/src/store/session_store/mod.rs` — provider-state load and atomic replacement APIs.
- `crates/tact/src/store/session_store/sqlite.rs` — `responses_states` table, migration, CRUD, transaction, cleanup, tests.
- `crates/tact/src/config/types.rs` — optional `responses_compact_threshold` provider setting and resolved setting.
- `crates/tact/src/config/resolve.rs` — resolve/validate threshold.
- `crates/tact/src/agent/mod.rs` — ensure the Responses provider excludes `compact` at `Agent::new()`/`all_tool_specs()` assembly without changing other providers.
- `crates/tact-ui/src/driver.rs` — retain the existing `UserCommand::Compact` dispatch and update its success/error info text to identify native Responses compaction without exposing encrypted state.

### Modified documentation files

- `book/05_chapter_compact.md`, `book/05_chapter_compact_zh.md`
- `book/22_chapter_llm.md`, `book/22_chapter_llm_zh.md`
- `book/23_chapter_tui.md`, `book/23_chapter_tui_zh.md`
- `book/26_chapter_issue.md`, `book/26_chapter_issue_zh.md`
- `docs/token_usage_schema.md`
- `config.example.toml`

---

## Task 1: Capture protocol fixtures and define provider-state types

**Files:**
- Create: `crates/tact_llm/src/provider_state.rs`
- Modify: `crates/tact_llm/src/lib.rs`
- Create: `crates/tact_llm/src/openai/responses/fixtures/explicit_compact.json`
- Create: `crates/tact_llm/src/openai/responses/fixtures/automatic_compact.json`
- Create: `crates/tact_llm/src/openai/responses/fixtures/stream_compact_events.jsonl`
- Test: unit tests in `provider_state.rs`

**Interfaces:**
- Produces `ProviderConversationState`, `ResponsesConversationState`, `ProviderStateUpdate`, and `context_hash()` for Tasks 2–6.
- The exact automatic fixture determines whether the adapter can derive a replacement baseline from the response or must reject an insufficient response. Do not invent a fixture shape.

- [ ] **Step 1: Add fixture-loading tests before implementation.**

```rust
#[test]
fn explicit_compact_fixture_contains_one_non_empty_compaction_item() {
    let value: serde_json::Value = serde_json::from_str(
        include_str!("openai/responses/fixtures/explicit_compact.json"),
    ).unwrap();
    let output = value["output"].as_array().unwrap();
    let compactions = output.iter().filter(|item| item["type"] == "compaction").collect::<Vec<_>>();
    assert_eq!(compactions.len(), 1);
    assert!(!compactions[0]["encrypted_content"].as_str().unwrap().is_empty());
}
```

Add equivalent checks for the automatic fixture and ensure the stream fixture contains a terminal event plus either complete terminal output or a complete `output_item.done` sequence.

- [ ] **Step 2: Run the fixture tests and verify the missing-fixture failure.**

Run:

```bash
cargo test -p tact_llm provider_state --lib
```

Expected: FAIL until the sanitized endpoint fixtures are added.

- [ ] **Step 3: Add the versioned state types.**

Implement:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderConversationState {
    OpenAiResponses(ResponsesConversationState),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponsesConversationState {
    pub version: u32,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub input_items: Vec<serde_json::Value>,
    pub compaction_id: Option<String>,
    pub is_compacted: bool,
    pub logical_message_count: usize,
    pub logical_context_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStateUpdate {
    Unchanged,
    Replace(ProviderConversationState),
}
```

Use a stable JSON serialization of the Tact message prefix and SHA-256 for `logical_context_hash`. The helper must hash exactly the serialized `Message` slice supplied by the caller and return a typed serialization error rather than a sentinel.

- [ ] **Step 4: Add round-trip, unknown-item, and hash tests.**

Test that a state containing a `compaction` item and an unknown item serializes/deserializes byte-equivalently as JSON values, and that changing one logical message changes the hash.

- [ ] **Step 5: Export the types and run the focused tests.**

Run:

```bash
cargo test -p tact_llm provider_state --lib
```

Expected: all provider-state tests pass.

- [ ] **Step 6: Commit the self-contained state/fixture work.**

```bash
git add crates/tact_llm/src/provider_state.rs crates/tact_llm/src/lib.rs crates/tact_llm/src/openai/responses/fixtures
git commit -m "feat(responses): add native compaction state types"
```

---

## Task 2: Extend the LLM result and provider APIs without changing behavior

**Files:**
- Modify: `crates/tact_llm/src/client.rs`
- Modify: `crates/tact_llm/src/lib.rs`
- Modify: `crates/tact_llm/src/anthropic/mod.rs`
- Modify: `crates/tact_llm/src/openai/mod.rs`
- Modify: `crates/tact_llm/src/deepseek/mod.rs`
- Modify: `crates/tact_llm/src/kimi/mod.rs`
- Modify: `crates/tact_llm/src/mock.rs`
- Modify: `crates/tact_llm/src/provider.rs`
- Test: existing provider tests and new API compile tests

**Interfaces:**
- Consumes Task 1 `ProviderConversationState` and `ProviderStateUpdate`.
- Produces `LlmResponse` and state-aware `LlmClient` methods for Agent and Responses adapter tasks.

- [ ] **Step 1: Add a compile-time test that asserts the new result shape.**

```rust
#[test]
fn unchanged_provider_response_has_explicit_state_update() {
    let response = LlmResponse {
        blocks: Vec::new(),
        stop_reason: None,
        usage: None,
        request_body: None,
        state_update: ProviderStateUpdate::Unchanged,
    };
    assert!(matches!(response.state_update, ProviderStateUpdate::Unchanged));
}
```

- [ ] **Step 2: Define `LlmResponse` and update trait signatures.**

Use these exact conceptual signatures:

```rust
async fn stream_message(
    &self,
    request: &CreateMessageParams,
    provider_state: Option<&ProviderConversationState>,
    ui_tx: Option<UnboundedSender<AgentUpdate>>,
) -> Result<LlmResponse, LlmError>;

async fn create_message(
    &self,
    request: &CreateMessageParams,
    provider_state: Option<&ProviderConversationState>,
) -> Result<LlmResponse, LlmError>;

async fn compact(
    &self,
    request: &CreateMessageParams,
    provider_state: Option<&ProviderConversationState>,
) -> Result<LlmResponse, LlmError>;
```

Give the trait a default `compact()` returning `LlmError::Unsupported`, then delegate the method through `LlmProvider` so Responses can override it.

- [ ] **Step 3: Adapt non-Responses providers mechanically.**

Wrap each existing tuple return as:

```rust
Ok(LlmResponse {
    blocks,
    stop_reason,
    usage,
    request_body,
    state_update: ProviderStateUpdate::Unchanged,
})
```

Ignore `provider_state` in these adapters. Do not change their request wire format or local compaction behavior.

- [ ] **Step 4: Update mock constructors and tests.**

Keep mock turn semantics unchanged. Ensure mock `stream_message`, `create_message`, and native `compact` behavior compile and preserve existing expected blocks/stop reasons.

- [ ] **Step 5: Run all LLM library tests.**

Run:

```bash
cargo test -p tact_llm --lib
```

Expected: all existing LLM tests pass with the new result type.

- [ ] **Step 6: Commit the API migration.**

```bash
git add crates/tact_llm/src/client.rs crates/tact_llm/src/lib.rs crates/tact_llm/src/anthropic/mod.rs crates/tact_llm/src/openai/mod.rs crates/tact_llm/src/deepseek/mod.rs crates/tact_llm/src/kimi/mod.rs crates/tact_llm/src/mock.rs crates/tact_llm/src/provider.rs
git commit -m "refactor(llm): carry provider state in responses"
```

---

## Task 3: Add durable Responses state to SQLite

**Files:**
- Modify: `crates/tact/src/store/session_store/mod.rs`
- Modify: `crates/tact/src/store/session_store/sqlite.rs`
- Test: SQLite tests in `crates/tact/src/store/session_store/sqlite.rs`

**Interfaces:**
- Consumes Task 1 `ProviderConversationState`.
- Produces `load_provider_state()` and `replace_session_messages_and_provider_state()` for Agent.

- [ ] **Step 1: Write failing SQLite tests.**

Add tests for:

```rust
store.replace_session_messages_and_provider_state(
    "session-1",
    &replacement,
    Some(&state),
).await.unwrap();

assert_eq!(store.load_session("session-1").await.unwrap(), replacement);
assert_eq!(store.load_provider_state("session-1").await.unwrap(), Some(state));
```

Also add a transaction-failure test using invalid state serialization or a deliberately closed connection, and a session-delete test asserting the provider state row is gone.

- [ ] **Step 2: Run the focused tests and verify they fail.**

Run:

```bash
cargo test -p tact sqlite::tests::test_provider_state --lib
```

Expected: FAIL because the table and trait methods do not exist.

- [ ] **Step 3: Add the `responses_states` table creation.**

In `SqliteSessionStore::new()`, add:

```sql
CREATE TABLE IF NOT EXISTS responses_states (
    session_id       TEXT PRIMARY KEY NOT NULL,
    schema_version   INTEGER NOT NULL,
    provider         TEXT NOT NULL,
    base_url         TEXT NOT NULL,
    model            TEXT NOT NULL,
    state_json       TEXT NOT NULL,
    compaction_id    TEXT,
    updated_at       TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
)
```

Use the existing startup migration style; no additional index is needed because `session_id` is the primary key used for all provider-state lookups.

- [ ] **Step 4: Extend the `SessionStore` trait.**

Add:

```rust
async fn load_provider_state(
    &self,
    session_id: &str,
) -> Result<Option<ProviderConversationState>>;

async fn replace_session_messages_and_provider_state(
    &self,
    session_id: &str,
    messages: &[Message],
    provider_state: Option<&ProviderConversationState>,
) -> Result<(i64, i64)>;
```

Keep `replace_session_messages()` unchanged for non-Responses callers.

- [ ] **Step 5: Implement load and atomic replace.**

Serialize the enum to `state_json`, extract the Responses metadata for columns, and perform the following in one SQLx transaction:

```text
DELETE messages
INSERT replacement messages
DELETE responses_states
INSERT candidate state when Some
UPDATE sessions.updated_at
COMMIT
```

If `provider_state` is `None`, delete the row. Deserialize malformed state with contextual errors; never silently discard it.

- [ ] **Step 6: Delete provider state with sessions.**

Update the existing session delete transaction to execute:

```sql
DELETE FROM responses_states WHERE session_id = ?
```

before deleting the session row.

- [ ] **Step 7: Run all session-store tests.**

Run:

```bash
cargo test -p tact session_store --lib
```

Expected: all old and new SQLite tests pass.

- [ ] **Step 8: Commit the persistence layer.**

```bash
git add crates/tact/src/store/session_store/mod.rs crates/tact/src/store/session_store/sqlite.rs
git commit -m "feat(store): persist Responses provider state atomically"
```

---

## Task 4: Make Responses conversion state-aware and add explicit native compact

**Files:**
- Modify: `crates/tact_llm/src/openai/responses/convert.rs`
- Modify: `crates/tact_llm/src/openai/responses/normalize.rs`
- Modify: `crates/tact_llm/src/openai/responses/mod.rs`
- Test: `crates/tact_llm/src/openai/responses/convert.rs`, `normalize.rs`, `mod.rs`

**Interfaces:**
- Consumes Task 1 state types and Task 2 `LlmResponse` API.
- Produces state-aware `/responses` requests, validated compact responses, and `ProviderStateUpdate::Replace` for explicit native compact.

- [ ] **Step 1: Add failing conversion tests.**

Add these test-local helpers beside the tests:

```rust
fn state_covering_first_message(request: &CreateMessageParams) -> ResponsesConversationState {
    let first = serde_json::to_value(&message_to_input(&request.messages[0]).unwrap()[0]).unwrap();
    ResponsesConversationState {
        version: 1,
        provider: "openai_responses".into(),
        base_url: "https://api.openai.com/v1".into(),
        model: request.model.clone(),
        input_items: vec![first],
        compaction_id: None,
        is_compacted: false,
        logical_message_count: 1,
        logical_context_hash: context_hash(&request.messages[..1]).unwrap(),
    }
}

fn compact_resource_without_compaction_item() -> serde_json::Value {
    serde_json::json!({"id":"cmp-test","object":"response.compaction","output":[]})
}

fn compact_resource_with_empty_encrypted_content() -> serde_json::Value {
    serde_json::json!({"id":"cmp-test","object":"response.compaction","output":[
        {"type":"compaction","id":"cmp-item","encrypted_content":""}
    ]})
}
```

Then add concrete tests using the existing `request_with_history()` helper and a state whose `input_items` contains the first converted user message:

```rust
#[test]
fn state_baseline_only_converts_uncovered_messages() {
    let request = request_with_history();
    let state = state_covering_first_message(&request);
    let (body, sent_items) = create_response(&request, Some(&state), Some(160_000), None).unwrap();
    assert_eq!(sent_items.len(), 1);
    assert_eq!(body["input"].as_array().unwrap().len(), 1);
    assert_eq!(body["input"][0]["role"], "assistant");
}

#[test]
fn responses_request_injects_context_management_and_keeps_stateless_fields() {
    let request = request_with_history();
    let (body, _) = create_response(&request, None, Some(160_000), None).unwrap();
    assert_eq!(body["store"], false);
    assert!(body.get("previous_response_id").is_none());
    assert!(body.get("conversation").is_none());
    assert_eq!(body["context_management"][0]["type"], "compaction");
}

#[test]
fn explicit_compact_requires_one_non_empty_compaction_item() {
    let missing = compact_resource_without_compaction_item();
    assert!(parse_compact_resource(missing).is_err());
    let empty = compact_resource_with_empty_encrypted_content();
    assert!(parse_compact_resource(empty).is_err());
}
```

The test helpers must construct typed JSON values locally; they must not contact a live endpoint. The request assertions must verify:

```rust
assert_eq!(body["store"], false);
assert!(body.get("previous_response_id").is_none());
assert!(body.get("conversation").is_none());
assert_eq!(body["context_management"][0]["type"], "compaction");
```

- [ ] **Step 2: Refactor `create_response()` into state-aware conversion.**

Add an internal function with a precise contract:

```rust
pub(crate) fn create_response(
    request: &CreateMessageParams,
    provider_state: Option<&ProviderConversationState>,
    compact_threshold: Option<u32>,
    configured_effort: Option<OpenAiReasoningEffort>,
) -> Result<(serde_json::Value, Vec<serde_json::Value>), LlmError>
```

The second return value is the exact input-item JSON sent in this request. For no state, convert all messages. For an OpenAI Responses state, validate provider/base URL/model and logical prefix hash, then convert only the uncovered suffix.

Use JSON conversion at the boundary so every exact request item can be retained for the next state. Preserve assistant-history normalization and existing reasoning envelope behavior.

- [ ] **Step 3: Inject `context_management`.**

When `compact_threshold` is `Some`, set:

```rust
body["context_management"] = serde_json::json!([
    {
        "type": "compaction",
        "compact_threshold": compact_threshold,
    }
]);
```

When the model context window is disabled and no threshold is supplied, omit the field. Never inject `previous_response_id` or `conversation`.

- [ ] **Step 4: Validate and preserve protocol output.**

Extend `NormalizedResponse` with protocol output/state information. For every terminal response:

- retain every output item as JSON in output order;
- continue mapping only supported message/reasoning/function-call items into `ContentBlock`;
- detect `OutputItem::Compaction` rather than ignoring it;
- reject zero/multiple compaction items where a compact response requires exactly one;
- reject empty `encrypted_content`;
- preserve unknown output item JSON.

For ordinary responses without compaction, build the next state by appending terminal output to the exact request input. For responses with compaction, use the Phase 0 fixture contract to build a replacement baseline; otherwise return a protocol error.

- [ ] **Step 5: Implement `OpenAiResponsesAdapter::compact()`.**

Build the compact request from the state baseline plus uncovered logical messages and call:

```rust
self.client.responses().compact(compact_request).await
```

Convert `CompactResource.output` to a validated replacement `ResponsesConversationState`, return `LlmResponse` with `ProviderStateUpdate::Replace`, and record serialized request body. Do not use the local summary prompt or `create_message()`.

- [ ] **Step 6: Run focused Responses tests.**

Run:

```bash
cargo test -p tact_llm openai::responses --lib
```

Expected: conversion, normalization, explicit compact, state validation, and existing Responses tests pass.

- [ ] **Step 7: Commit the state-aware adapter and explicit endpoint.**

```bash
git add crates/tact_llm/src/openai/responses/convert.rs crates/tact_llm/src/openai/responses/normalize.rs crates/tact_llm/src/openai/responses/mod.rs
git commit -m "feat(responses): support native compact endpoint"
```

---

## Task 5: Add streaming terminal compaction handling

**Files:**
- Modify: `crates/tact_llm/src/openai/responses/mod.rs`
- Modify: `crates/tact_llm/src/openai/responses/stream.rs`
- Test: stream unit tests and fixture-driven tests

**Interfaces:**
- Consumes Task 4 normalized state/output logic.
- Produces identical state updates for streamed and non-streamed terminal Responses.

- [ ] **Step 1: Add failing stream tests.**

Add tests that feed `response.output_item.added`, `response.output_item.done`, and terminal events from the fixture and assert:

```rust
assert!(matches!(response.state_update, ProviderStateUpdate::Replace(_)));
assert!(!response.blocks.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.contains("encrypted"))));
```

Add a duplicate added/done test and an incomplete-terminal-output test.

- [ ] **Step 2: Expand event parsing.**

Consume:

```text
response.output_item.added
response.output_item.done
```

Keep all unrelated/new event types ignored unless they are required to reconstruct a complete terminal response. Normalize terminal response IDs/status as existing code does.

- [ ] **Step 3: Store completed output items by `output_index`.**

In `ResponsesStreamState`, track `done` items in an ordered map. Ignore an `added` item when a corresponding done item exists. At `finish()`:

1. use terminal `response.output` when non-empty;
2. otherwise use the complete done-item sequence;
3. otherwise return `LlmError::Unsupported`;
4. pass exactly one output sequence to `normalize_response()`.

Do not append text deltas to terminal output if terminal message text exists. Preserve the existing compatible-endpoint visible-text recovery only when terminal response output has no message text and the visible delta is the only available text source; this is text recovery, not a compaction fallback. A missing compaction baseline remains a hard protocol error.

- [ ] **Step 4: Verify stream and non-stream parity.**

Run:

```bash
cargo test -p tact_llm openai::responses::stream --lib
cargo test -p tact_llm openai::responses --lib
```

Expected: both paths produce equivalent blocks, stop reason, usage, and provider-state update for equivalent fixtures.

- [ ] **Step 5: Commit streaming support.**

```bash
git add crates/tact_llm/src/openai/responses/mod.rs crates/tact_llm/src/openai/responses/stream.rs
 git commit -m "feat(responses): preserve compaction in streams"
```

---

## Task 6: Add Agent runtime state and provider-aware compaction

**Files:**
- Modify: `crates/tact/src/agent/mod.rs`
- Modify: `crates/tact/src/config/types.rs`
- Modify: `crates/tact/src/config/resolve.rs`
- Test: Agent unit/integration tests in `crates/tact/src/agent/mod.rs`

**Interfaces:**
- Consumes Tasks 1–5 state-aware LLM and store APIs.
- Produces `AgentRuntime.provider_state`, Responses-aware `compact_history()`, threshold calculation, and Responses tool filtering.

- [ ] **Step 1: Add failing Agent tests.**

Use the existing `Agent::new()` test fixture and inspect `agent.all_tool_specs()`:

```rust
#[test]
fn responses_tool_specs_exclude_compact() {
    let agent = responses_test_agent();
    assert!(!agent.all_tool_specs().iter().any(|spec| spec.name == "compact"));
}

#[test]
fn non_responses_tool_specs_keep_compact() {
    let agent = chat_completions_test_agent();
    assert!(agent.all_tool_specs().iter().any(|spec| spec.name == "compact"));
}
```

Add an async test with a mock Responses HTTP server that returns a valid compact fixture. Call `agent.compact_history(None)`, assert exactly one `/responses/compact` request, assert no local summary `create_message()` request, and assert the runtime provider state contains the fixture compaction id.

- [ ] **Step 2: Add provider state to `AgentRuntime`.**

Initialize:

```rust
provider_state: None,
```

In `ensure_session()`, load messages and provider state. Validate the loaded Responses state's provider/base URL/model against the active provider before allowing an LLM call.

- [ ] **Step 3: Add the resolved threshold setting.**

Add optional `responses_compact_threshold: Option<u32>` to `ProviderEntryToml` and resolved `LlmSettings`. Resolve it without changing non-Responses config. Validate a positive configured value and reject values that cannot leave room for `max_tokens` and safety headroom.

If omitted, calculate:

```rust
let headroom = model_context_window.saturating_mul(10).div_ceil(100);
let threshold = model_context_window
    .saturating_sub(max_tokens as usize)
    .saturating_sub(headroom);
```

Return `None` when the model context window is zero.

- [ ] **Step 4: Filter the model-facing compact tool.**

At the cached tool-spec assembly boundary, skip the native tool whose metadata name is `compact` only when the client is `LlmProvider::OpenAiResponses`. Keep MCP tools and all other native tools unchanged. Keep `UserCommand::Compact` untouched.

- [ ] **Step 5: Make `compact_history()` provider-aware.**

Refactor the current method into:

```rust
pub async fn compact_history(&mut self, focus: Option<&str>) -> Result<()> {
    if self.is_openai_responses() {
        self.compact_responses_native().await
    } else {
        self.compact_history_local(focus).await
    }
}
```

`compact_responses_native()` must call the new LLM `compact()` method, construct candidate logical context/state, atomically persist both, then update runtime fields/counters. Ignore `focus` for Responses and emit an informational status without exposing encrypted data.

- [ ] **Step 6: Update the Agent loop to pass state and commit updates.**

Pass `self.runtime.provider_state.as_ref()` into `stream_message()`. After receiving `LlmResponse`, create candidate state and candidate logical context. Persist assistant/context changes and provider state before sending another request. After tool execution, persist the tool result and updated anchor/state before looping.

On any persistence failure, stop the loop without executing the same tool again. On any provider-state mismatch or malformed native compaction, return the error and leave the old committed state intact.

- [ ] **Step 7: Update recovery paths.**

Prompt-too-long recovery for Responses must call native `compact_responses_native()` and must not call local summary compaction. Keep existing local recovery for other providers. Bounded transient retry applies to the native endpoint; protocol errors do not retry.

- [ ] **Step 8: Run Agent and config tests.**

Run:

```bash
cargo test -p tact agent --lib
cargo test -p tact config::resolve --lib
```

Expected: existing agent/config behavior passes, Responses-specific tool filtering and native dispatch tests pass.

- [ ] **Step 9: Commit Agent integration.**

```bash
git add crates/tact/src/agent/mod.rs crates/tact/src/config/types.rs crates/tact/src/config/resolve.rs crates/tact/src/tool
git commit -m "feat(agent): route Responses compaction natively"
```

---

## Task 7: Integrate the user command and add end-to-end persistence tests

**Files:**
- Modify: `crates/tact-ui/src/driver.rs`
- Test: `crates/tact-ui/tests/recovery_compaction.rs` and existing command-loop test support

**Interfaces:**
- Consumes Task 6 Agent dispatch.
- Produces user-visible `/compact` behavior and restart/atomicity coverage.

- [ ] **Step 1: Write failing command tests.**

Drive `UserCommand::Compact` through the existing command loop with a mock Responses agent and assert:

```text
one native compact request
no compact function declaration in the model request
success info after commit
error info and unchanged context after failure
```

- [ ] **Step 2: Keep the command path provider-agnostic.**

Do not add a second compact command. Keep:

```rust
agent.compact_history(None).await
```

The Agent decides whether the provider is native Responses or local-summary capable.

- [ ] **Step 3: Add restart round-trip coverage.**

Use a temporary SQLite database:

1. create a session;
2. persist messages and a Responses state containing a compaction item and unknown item;
3. drop/reopen the store;
4. load both values;
5. assert exact state JSON values and message equality;
6. build the next request and assert the compaction item is present exactly once.

- [ ] **Step 4: Add atomic rollback coverage.**

Force provider-state serialization or database insertion failure and assert:

```text
runtime.context == old_context
runtime.provider_state == old_state
messages table == old_messages
responses_states table == old_state
```

- [ ] **Step 5: Run integration tests.**

Run:

```bash
cargo test -p tact-ui --test recovery_compaction
cargo test -p tact session_store --lib
```

Expected: command, restart, cleanup, and rollback tests pass.

- [ ] **Step 6: Commit command/persistence integration.**

```bash
git add crates/tact-ui/src/driver.rs crates/tact-ui/tests crates/tact/src/agent crates/tact/src/store
 git commit -m "test(responses): cover native compact session recovery"
```

---

## Task 8: Update usage accounting and diagnostics

**Files:**
- Modify: `crates/tact/src/store/session_store/sqlite.rs` if call-type validation or queries require it.
- Modify: `crates/tact/src/stats.rs` — add a native-compaction counter only when the existing stats schema exposes per-kind compaction totals; otherwise keep the existing aggregate counter unchanged.
- Modify: `docs/token_usage_schema.md`
- Test: token usage/store tests

**Interfaces:**
- Consumes Task 6/7 native compact call metadata.
- Produces distinguishable `responses_compact` usage rows without exposing encrypted state.

- [ ] **Step 1: Add failing usage assertions.**

Assert that explicit native compact records:

```text
call_type = "responses_compact"
request_body is present
compaction encrypted content is not copied into a diagnostic message
```

Assert automatic compaction remains associated with the ordinary `stream` call rather than creating a fake second HTTP call.

- [ ] **Step 2: Record the new call type.**

Use `responses_compact` only around the explicit `/responses/compact` call. Preserve existing `compact` rows for local providers. Do not alter usage totals or TUI token calculations unless a test demonstrates a required distinction.

- [ ] **Step 3: Add safe status diagnostics.**

Emit item count, compaction id prefix/hash, and token usage only. Never format `encrypted_content` into `tracing`, `AgentUpdate::Info`, error strings, or TUI cards.

- [ ] **Step 4: Update and test documentation examples.**

Document the new call type and automatic-vs-explicit distinction in `docs/token_usage_schema.md`.

- [ ] **Step 5: Run tests and commit.**

Run:

```bash
cargo test -p tact session_store --lib
cargo test -p tact stats --lib
```

Then commit:

```bash
git add crates/tact/src/store/session_store/sqlite.rs crates/tact/src/stats.rs docs/token_usage_schema.md
git commit -m "docs(usage): document Responses native compaction accounting"
```

---

## Task 9: Synchronize configuration, book chapters, issue log, and examples

**Files:**
- Modify: `config.example.toml`
- Modify: `book/05_chapter_compact.md`
- Modify: `book/05_chapter_compact_zh.md`
- Modify: `book/22_chapter_llm.md`
- Modify: `book/22_chapter_llm_zh.md`
- Modify: `book/23_chapter_tui.md`
- Modify: `book/23_chapter_tui_zh.md`
- Modify: `book/26_chapter_issue.md`
- Modify: `book/26_chapter_issue_zh.md`
- Test: markdown structure/check scripts available in the repository

**Interfaces:**
- Consumes the final behavior from Tasks 4–8.
- Produces bilingual, user-visible documentation aligned with implementation.

- [ ] **Step 1: Add the config example.**

Show:

```toml
[llm.providers.openai]
protocol = "responses"
# Optional; otherwise derived from agent.model_context_window/max_tokens/headroom.
responses_compact_threshold = 160000
```

State that endpoints lacking native Responses compaction are unsupported and do not fall back to local summary compaction.

- [ ] **Step 2: Update Ch 5 in both languages.**

Add a dedicated native Responses section covering:

```text
ordinary request + context_management
Tact threshold + /responses/compact
user /compact without model compact tool
opaque provider state
atomic messages/state persistence
no fallback
```

Preserve existing local compaction sections for non-Responses providers.

- [ ] **Step 3: Update Ch 22 in both languages.**

Document exact conversion/state boundaries, `LlmResponse`, terminal response authority, `compaction` item round-trip, and why the item is not a `ContentBlock`.

- [ ] **Step 4: Update Ch 23 in both languages.**

Document `/compact` status messages and the rule that encrypted state is never rendered. Do not describe compaction as an assistant message or streamed markdown output.

- [ ] **Step 5: Add newest-first Ch 26 entry in both languages.**

Include date, type `bugfix`/`feature`, symptom (compaction output was ignored), decision (native state persisted and replayed), observable behavior, code/spec pointers, and related chapter links. Keep heading hierarchy and section id aligned.

- [ ] **Step 6: Run documentation checks.**

Run the repository's book/documentation checks if available, then inspect heading parity manually:

```bash
git diff --check
rg -n '^#{1,6} ' book/05_chapter_compact.md book/05_chapter_compact_zh.md book/22_chapter_llm.md book/22_chapter_llm_zh.md book/23_chapter_tui.md book/23_chapter_tui_zh.md
```

- [ ] **Step 7: Commit documentation.**

```bash
git add config.example.toml book/05_chapter_compact.md book/05_chapter_compact_zh.md book/22_chapter_llm.md book/22_chapter_llm_zh.md book/23_chapter_tui.md book/23_chapter_tui_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
git commit -m "docs(responses): document native compaction behavior"
```

---

## Task 10: Full verification and release checkpoint

**Files:**
- No intended source changes; only fix failures found by verification, in the owning task's files.

- [ ] **Step 1: Run formatting and static checks sequentially.**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: both commands exit successfully.

- [ ] **Step 2: Run focused protocol/store/agent tests sequentially.**

```bash
cargo test -p tact_llm openai::responses --lib
cargo test -p tact session_store --lib
cargo test -p tact agent --lib
```

Expected: all focused native compaction, state, persistence, and agent tests pass.

- [ ] **Step 3: Run the full workspace build and tests.**

```bash
cargo build --workspace
cargo test --workspace
```

Expected: all workspace crates compile and all tests pass.

- [ ] **Step 4: Verify repository state.**

```bash
git diff --check
git status -sb
git log --oneline -10
```

Expected: no whitespace errors, no unintended generated files, and all implementation commits visible.

- [ ] **Step 5: Final acceptance review.**

Check every acceptance item from the approved spec:

```text
Responses compact tool absent
user /compact native
automatic threshold native
context_management present
compaction state round-trip
no previous_response_id/conversation
no local fallback
atomic SQLite state/messages
restart recovery
model/base_url mismatch rejection
stream/non-stream parity
no ContentBlock/log/TUI encrypted state
other providers unchanged
EN/ZH docs aligned
```

- [ ] **Step 6: Commit only verification fixes.**

If verification found a defect, fix it in the owning task's files, rerun the smallest failing test first, then the full relevant command. Do not bundle unrelated cleanup into the final commit.

---

## Execution order and checkpoints

Tasks must be executed in this order because each later API depends on the previous one:

```text
1 fixtures/state types
2 LLM result/API migration
3 SQLite state
4 conversion + explicit compact
5 streaming
6 Agent integration
7 user command/integration tests
8 usage/diagnostics
9 docs/config
10 full verification
```

Checkpoint after Task 3:

```text
state serializes, persists, reloads, and atomically replaces with messages
```

Checkpoint after Task 5:

```text
non-streaming and streaming adapter paths produce validated provider updates
```

Checkpoint after Task 7:

```text
user /compact and restart recovery work through Agent and SQLite
```

Final checkpoint:

```text
all tests pass and the acceptance checklist is satisfied
```
