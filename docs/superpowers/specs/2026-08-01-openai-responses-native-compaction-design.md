# OpenAI Responses Native Compaction Design

- **Date:** 2026-08-01
- **Status:** Approved design; implementation not started
- **Scope:** Native OpenAI Responses compaction for `protocol = "responses"`

## 1. Goals and non-goals

### Goals

1. Support explicit native compaction through `POST /v1/responses/compact`.
2. Enable Responses `context_management` compaction on ordinary `/responses` requests.
3. Preserve and replay OpenAI's opaque `compaction` state across turns and process restarts.
4. Keep Tact's logical `Message` history separate from Responses wire-level state.
5. Keep the user `/compact` command, but do not register a `compact` tool with Responses models.
6. Make message history and provider state persist atomically in SQLite.
7. Handle streaming and non-streaming Responses consistently.
8. Avoid silently dropping unknown Responses items or encrypted compaction state.

### Non-goals

1. No local-summary fallback for the Responses provider.
2. No use of `previous_response_id` or `conversation` state.
3. No server-side stateful conversation ownership; requests remain stateless with `store: false`.
4. No `ContentBlock::ResponsesCompaction` shared content variant.
5. No model-facing `compact` tool for Responses.
6. No attempt to interpret `encrypted_content` on the client.

Other providers retain the existing local compaction implementation.

## 2. Current implementation and gap

Tact currently performs compaction in `Agent::compact_history()` through a local summary LLM call and rebuilds `runtime.context`. The Responses adapter then converts the rebuilt `Vec<Message>` to `/responses` input items.

The Responses SDK already exposes the protocol primitives needed by this design:

- `OutputItem::Compaction`;
- `CompactionBody` and `CompactionSummaryItemParam`;
- `CompactResponseRequest` and `CompactResource`;
- `ContextManagementParam`.

The current adapter does not use them. `normalize_response()` handles reasoning, messages, and function calls, but silently ignores other output items, including `OutputItem::Compaction`. The current LLM return tuple cannot carry provider-specific state. SQLite stores Tact messages but has no provider-state table.

## 3. Architecture

### 3.1 Two distinct contexts

The runtime owns two related but separate states:

```text
Tact logical history
    runtime.context: Vec<Message>

Responses protocol state
    runtime.provider_state: Option<ProviderConversationState>
```

`runtime.context` remains the source for tools, TUI, session logic, and provider-independent operations. `provider_state` is opaque protocol state used to build the next Responses request.

The compaction item is not a user message, assistant message, thinking block, or tool call. It is a wire-level protocol item and must not be placed in shared `ContentBlock`.

### 3.2 Provider-specific state

Add a provider-specific state boundary:

```rust
pub enum ProviderConversationState {
    OpenAiResponses(ResponsesConversationState),
}
```

The Responses state is versioned and serializable:

```rust
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
```

`input_items` is the protocol baseline for the next request. It may contain compaction, reasoning, function-call, function-call-output, message, and unknown future items. The state is stored as JSON rather than an SDK-specific binary format so unknown fields and future item types survive SDK upgrades.

`logical_message_count` and `logical_context_hash` identify the prefix of `runtime.context` already represented by the provider baseline. If no persisted provider state exists, the first request converts the complete current `runtime.context` into an initial baseline and records its provider/base URL/model binding. Before sending later requests, Tact verifies the hash. A mismatch is a hard provider-state error; Tact must not silently duplicate, truncate, or reconstruct the state.

### 3.3 State update semantics

LLM results carry provider state updates explicitly:

```rust
pub enum ProviderStateUpdate {
    Unchanged,
    Replace(ProviderConversationState),
}

pub struct LlmResponse {
    pub blocks: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Option<TokenUsageInfo>,
    pub request_body: Option<LlmRequestBody>,
    pub state_update: ProviderStateUpdate,
}
```

A normal Responses turn produces an appended protocol baseline. A response containing native compaction produces `Replace`, because compaction is a reset boundary rather than an ordinary append.

## 4. Request and response flows

### 4.1 Ordinary Responses request

The adapter builds the request as:

```text
provider_state.input_items
    + convert(runtime.context[state.logical_message_count..])
    → /responses input
```

The request keeps:

```json
{
  "store": false,
  "stream": true,
  "context_management": [
    {
      "type": "compaction",
      "compact_threshold": 160000
    }
  ]
}
```

`previous_response_id` and `conversation` remain absent.

Because the current SDK `CreateResponse` type does not expose `context_management`, the adapter injects it into the serialized JSON request body, following the existing pattern used for `stream` and reasoning effort.

### 4.2 Automatic server-side compaction

When the service reaches `compact_threshold` during an ordinary `/responses` request, the terminal response may contain a `compaction` item. It must not be normalized to text, thinking, or a tool call.

Terminal response output is the authoritative source for final logical blocks and protocol output. If a compaction boundary is present, the adapter constructs a candidate replacement state:

```text
compacted retained items + compaction item
    + current response output items
    → candidate ResponsesConversationState
```

The exact retained-item boundary must follow the Responses API response contract. Phase 0 must provide fixtures from the target endpoint that establish whether automatic compaction returns the complete retained baseline or only a compaction item. If the endpoint returns only a compaction item and not enough information to determine the retained baseline, the adapter returns a protocol error rather than guessing. If the API contract permits deriving the retained prefix from the current request input, that derivation must be explicit, validated, and covered by fixtures.

The candidate state is not committed until the corresponding Tact context and SQLite update succeed.

### 4.3 Explicit user `/compact`

`UserCommand::Compact` continues to call `Agent::compact_history(None)`, but that method becomes provider-aware:

```text
OpenAiResponses → native POST /responses/compact
other providers → existing local summary compaction
```

For Responses, `focus` is ignored because the native endpoint does not implement Tact's summary-focus semantics. The adapter sends the current protocol baseline plus any logical messages not yet represented in it. A valid `CompactResource` must contain exactly one non-empty compaction item and a valid output sequence.

On success:

```text
CompactResource.output
    → new Responses protocol baseline
    → Tact logical context candidate
    → atomic persistence
```

On failure, all old memory and persistence state remains unchanged. No local summary fallback is attempted.

### 4.4 Automatic Tact threshold

The existing two agent-loop trigger points remain:

1. before appending a new user turn;
2. after tool results are appended and before the next model call.

For Responses, the call to `compact_history(None)` dispatches to `/responses/compact`. On the entry path, old history is compacted before the current user turn is pushed, preserving the current turn verbatim. On the in-loop path, tool results are included in the protocol input before native compaction.

### 4.5 Streaming

Streaming uses the terminal `Response` as the authoritative final output. The stream state may collect `response.output_item.added` and `response.output_item.done` for compatibility, but it must:

- prefer terminal `response.output` when present;
- deduplicate added/done items by `output_index`;
- use completed item events only when terminal output is absent and the item sequence is complete;
- return a protocol error when neither source is sufficient;
- never combine text deltas with terminal text in a way that duplicates output.

A compaction item produces no TUI text/thinking/tool update. It is retained only in the provider state update.

## 5. Agent loop and API changes

The provider API changes from tuple returns to `LlmResponse`.

Conceptually:

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

Non-Responses providers return `Unsupported` from native `compact()` and continue using the existing local path through Agent dispatch.

After a model response, the Agent constructs candidate logical context and candidate provider state before committing. The recommended commit boundaries are:

1. assistant response + state baseline;
2. tool result + updated logical anchor/state.

Tool side effects are not replayed after a persistence failure. If state persistence fails, the loop stops before sending another request.

## 6. Tool registration

The native `compact` tool remains available for non-Responses providers. It is filtered out of the tool specification cache for `LlmProvider::OpenAiResponses`.

The user command remains available because it does not depend on model tool specs:

```text
model tools: no compact
user /compact: native /responses/compact
```

Tests must verify both sides of this provider-specific behavior.

## 7. SQLite persistence

### 7.1 New table

Add an additive migration in `SqliteSessionStore::new()`:

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
);
```

`state_json` contains the versioned `ResponsesConversationState`. The opaque encrypted content is never logged or displayed.

### 7.2 Store API

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

The SQLite implementation performs message replacement, provider-state replacement/deletion, and session timestamp update in one transaction. The existing message-only method remains for non-Responses callers.

On startup, `Agent::ensure_session()` loads messages and provider state together. A missing state on an old database is valid: the first successful Responses request initializes it from existing Tact messages. A corrupt state is a hard error and is not silently deleted.

State is bound to provider, base URL, and model. A mismatch is rejected before sending the request; it is not silently reset or reused.

Deleting a session deletes its `responses_states` row.

## 8. Usage, diagnostics, and security

Extend usage call types with:

```text
stream
compact
responses_compact
```

`responses_compact` is used only for explicit `/responses/compact` HTTP calls. Automatic compaction performed inside an ordinary stream remains attached to that stream's usage record and state metadata; it does not create a fictitious second HTTP usage row.

Diagnostics may include:

```text
[responses compacted: items=7, id=cmp_123, input_tokens=...]
```

They must not include `encrypted_content`. Provider state is stored in SQLite and not printed in tracing logs or TUI output. State size and item count may be logged for troubleshooting.

## 9. Failure semantics

### Retryable

Timeouts, connection resets, HTTP 429, and HTTP 5xx may be retried up to the existing bounded retry policy with exponential backoff. No state is changed during retries.

### Non-retryable

Protocol errors, malformed compaction output, missing/empty encrypted content, state-anchor mismatch, and provider/model/base URL mismatch fail immediately.

### Atomicity invariant

Before a successful commit:

```text
runtime.context        = old
runtime.provider_state  = old
SQLite messages         = old
SQLite provider state   = old
```

After a successful commit:

```text
runtime.context        = candidate
runtime.provider_state  = candidate
SQLite messages         = candidate
SQLite provider state   = candidate
```

There is no local fallback, silent state deletion, or retry that re-executes a tool side effect.

## 10. Configuration

Native Responses compaction is enabled whenever `protocol = "responses"`:

- `context_management` is sent on ordinary Responses requests;
- explicit native compaction is used by user `/compact` and automatic Tact threshold triggers;
- local summary compaction is not used;
- `compact` is not registered as a model tool.

An optional provider-specific threshold may be added:

```toml
[llm.providers.openai]
protocol = "responses"
responses_compact_threshold = 160000
```

When omitted, the threshold is derived from `model_context_window`, `max_tokens`, and safety headroom. Endpoints that do not support the required native Responses features are unsupported; they do not fall back to local summary compaction.

## 11. Test matrix

### Protocol conversion

- state JSON round-trip;
- compaction item id/encrypted content round-trip;
- unknown item preservation;
- baseline plus message delta without duplicates;
- reasoning and compaction coexistence;
- anchor hash match/mismatch;
- zero, multiple, or empty compaction item rejection.

### Adapter HTTP

- ordinary request contains `context_management` and `store: false`;
- no `previous_response_id` or `conversation`;
- initial state construction;
- state-aware incremental request;
- explicit `/responses/compact` request and response;
- retry classification;
- automatic compaction state replacement.

### Streaming

- output item added/done deduplication;
- terminal response precedence;
- compaction plus text/tool output;
- terminal-output absence recovery only when complete done items exist;
- no compaction TUI chunks;
- stream errors do not commit state.

### Agent and persistence

- Responses tool specs exclude `compact`;
- other providers still expose it;
- user `/compact` dispatches natively for Responses;
- automatic pre-turn and post-tool native compaction;
- current turn is not absorbed before entry compaction;
- failed compaction leaves all state unchanged;
- restart restores messages and provider state;
- provider/model/base URL mismatch is rejected;
- atomic SQLite replacement and session deletion cleanup.

## 12. Documentation synchronization

Update in the same implementation change:

- `book/05_chapter_compact.md` and `_zh.md`: local vs native Responses compaction, context management, user `/compact`, opaque state, no fallback, and atomicity;
- `book/22_chapter_llm.md` and `_zh.md`: state-aware conversion, native endpoint, terminal output, and round-trip semantics;
- `book/23_chapter_tui.md` and `_zh.md`: user compact command and safe display of native compaction status;
- `book/26_chapter_issue.md` and `_zh.md`: newest-first bugfix/feature entry for dropped compaction state;
- `docs/token_usage_schema.md`: `responses_compact` and automatic compaction accounting;
- `config.example.toml`: Responses threshold and endpoint capability requirements.

## 13. Implementation phases

1. **Protocol fixtures:** capture explicit compact, automatic compaction, and streaming terminal examples.
2. **Provider state and SQLite:** add state types, table, load/save, and atomic replacement.
3. **State-aware adapter API:** introduce `LlmResponse`, state updates, baseline/delta conversion, and anchor validation.
4. **Explicit native compact:** implement `/responses/compact`, Agent dispatch, and user `/compact`.
5. **Automatic context management:** inject request field and commit returned compaction state.
6. **Streaming compatibility:** handle output item events, terminal precedence, and mixed compaction/tool cases.
7. **Integration and docs:** complete regression tests, migration tests, bilingual documentation, and full verification.

## 14. Acceptance criteria

- Responses never registers `compact` as a model tool.
- User `/compact` calls `/responses/compact`.
- Automatic Tact threshold calls `/responses/compact`.
- Ordinary Responses requests include `context_management`.
- Returned compaction state is never silently discarded.
- The next request replays the state without `previous_response_id` or `conversation`.
- No local-summary fallback is used for Responses.
- Messages and provider state commit atomically in SQLite.
- Restart restores a usable Responses session.
- Model/base URL mismatch is rejected.
- Streaming and non-streaming semantics agree.
- Encrypted content never enters `ContentBlock`, logs, or TUI.
- Other providers retain current local compaction behavior.
- English and Chinese documentation remain structurally aligned.
