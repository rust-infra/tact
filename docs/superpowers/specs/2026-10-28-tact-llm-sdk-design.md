# Tact LLM SDK Architecture

> Date: 2026-10-28
> Status: delivered; full repository gate green (2026-10-29)
> Related: `crates/tact_llm/`, `crates/agent_api/`, `crates/agent_sdk/`,
> `ARCHITECTURE.md`
> Plan: `docs/superpowers/plans/2026-10-28-tact-llm-sdk.md`

## Goal

Turn `tact_llm` into a third-party-ready provider SDK without creating a second
agent runtime.

The stable SDK is one object graph:

```text
provider constructor -> ProviderBuilder -> Provider
Provider: tact_agent_api::ModelPort
```

`ProviderBuilder::build` is synchronous. It validates local configuration and
constructs transport; it does not resolve refreshable credentials or perform a
network request. Credentials are resolved once per dispatched model call.

Each built-in constructor has a default endpoint and an explicit
`ProviderBuilder::base_url` override. An override changes only the endpoint;
the constructor still fixes provider policy, so a custom hostname cannot
silently change protocol semantics.

The crate remains one package in this phase. A curated crate root hides private
policy, transport, wire, SSE, normalization, and compatibility modules. Feature
gates keep unrelated provider dependencies out of a consumer build.

The SDK must:

- build Anthropic, OpenAI Chat Completions, OpenAI Responses, DeepSeek, Kimi,
  and custom OpenAI-compatible providers;
- return a `Provider` that implements `tact_agent_api::ModelPort` directly;
- accept static API keys, static bearer tokens, refreshable credential
  sources, or explicit anonymous access;
- expose session identity and model limits as construction-time facts;
- stream events, cancel calls cooperatively, compact where supported, and
  return body-free typed errors;
- report provider facts and capabilities without exposing wire or transport
  types;
- integrate with `tact_agent_sdk::AgentBuilder` without the SDK depending on
  `tact_llm`;
- allow tracing through the normal host-owned `tracing` subscriber model;
- preserve existing Tact wire behavior during a staged product migration.

## Non-Goals

- Do not add a runtime provider registry, provider identity, install lifecycle,
  service locator, or dynamic loader.
- Do not add a second public model-call trait beside
  `tact_agent_api::ModelPort`.
- Do not make `tact_llm` depend on or re-export `tact_agent_sdk`.
- Do not expose `reqwest`, `async-openai`, the Responses fork, SSE frames, wire
  DTOs, body hooks, transport clients, or normalization helpers.
- Do not make account queries or model discovery part of the stable SDK.
- Do not split protocol adapters into separate crates in this phase.
- Do not expose a custom HTTP transport trait in v1.
- Do not replace `ModelResponse.request_body` with a new
  `tact_agent_api::RequestCapture` type in this phase. Stable provider builds
  must leave that field absent; the product capture migration is separate.
- Do not redesign the `/model` host protocol in this phase. Product code must
  build a replacement provider before publishing a model change, but the full
  request/ack protocol remains a separate design.
- Do not install a tracing subscriber from library code.
- Do not claim crates.io publication before every path and Git dependency is
  registry-safe.

## Constraints

- The current crate exports almost its entire implementation tree and has
  process-global provider and credential state.
- `ProviderInfo::build_client`, `Client::new`, and `get_llm_client` duplicate
  provider construction and policy.
- Credentials are currently resolved during some construction paths.
- `ProviderModel::stream_params` eagerly spawns a producer, so dropping an
  unpolled stream can still dispatch work.
- Chat Completions and Anthropic already use direct HTTP paths. Responses uses
  a Git-pinned provider SDK whose error and tracing paths can retain or log
  response bodies.
- `tact_agent_api::ModelCapabilities.context_window` is currently `usize`.
  Tact uses `0` as an unknown sentinel in several paths.
- `ModelResponse.request_body` and `UsageRecord.request_body` are currently
  `Option<Vec<u8>>`; product usage storage and `/view-system-prompt` depend on
  exact bounded request bytes.
- Existing Tact code imports `ProviderInfo`, `LlmProvider`, `ProviderProfile`,
  `MockClient`, `neutral::*`, `ProviderConversationState`, and the concrete
  Responses adapter.
- The workspace uses two reqwest generations and two secrecy generations
  because its provider dependencies differ.
- Plugin style means constructor injection, small typed contracts, and static
  composition. A custom wire protocol is implemented by supplying an
  `Arc<dyn ModelPort>` to the host, not by registering it with `tact_llm`.
- Run one Cargo invocation at a time and preserve unrelated untracked files.

## Caller Usage

### Minimal direct provider

```rust
use futures_util::StreamExt;
use tact_llm::{
    CancelToken, ModelEvent, ModelLimits, ModelPort, ModelRequest, SessionId,
    openai_responses,
};

# async fn run(api_key: String) -> Result<(), Box<dyn std::error::Error>> {
let limits = ModelLimits::new(Some(400_000), 16_384)?;
let provider = openai_responses("gpt-5.6")
    .api_key(api_key)?
    .session_id(SessionId::new("chat-1")?)
    .limits(limits)
    .build()?;

let request = ModelRequest {
    model: "gpt-5.6".into(),
    system: "Be concise.".into(),
    messages: Vec::new(),
    tools: Vec::new(),
    max_output_tokens: 1_000,
    options: Default::default(),
};
let cancel = CancelToken::new();
let mut stream = provider.stream(request, None, cancel.clone()).await?;

while let Some(event) = stream.next().await {
    match event? {
        ModelEvent::Finished(_) => break,
        _ => {}
    }
    if should_stop() {
        cancel.cancel();
    }
}
# Ok(())
# }
# fn should_stop() -> bool { false }
```

`Provider` is already a `ModelPort`. `Provider::into_model()` is a convenience
for hosts that need `Arc<dyn ModelPort>`.

### Use with `tact_agent_sdk`

```rust
use tact_agent_sdk::AgentBuilder;
use tact_llm::{ModelLimits, SessionId, anthropic};

# async fn run(api_key: String) -> Result<(), Box<dyn std::error::Error>> {
let session_id = SessionId::new("chat-1")?;
let provider = anthropic("claude-sonnet-4-5")
    .api_key(api_key)?
    .session_id(session_id.clone())
    .limits(ModelLimits::new(Some(200_000), 8_192)?)
    .build()?;

let mut session = AgentBuilder::new(provider.into_model())
    .system_prompt("You are concise.")
    .build_session(session_id.as_str())?;
let outcome = session.run_text("Summarize the issue.").await?;
# let _ = outcome;
# Ok(())
# }
```

An external host declares `tact_llm` and `tact_agent_sdk` independently. The
agent SDK does not infer provider-specific session headers, and `tact_llm`
does not re-export the runtime.

### Refreshable authentication

```rust
use std::sync::Arc;

use futures_util::future::BoxFuture;
use tact_llm::{Credential, CredentialError, CredentialSource};

#[derive(Debug)]
struct OAuthSource {
    // Host-owned token store or refresh client.
}

impl CredentialSource for OAuthSource {
    fn resolve(&self) -> BoxFuture<'_, Result<Credential, CredentialError>> {
        Box::pin(async move {
            let token = refresh_token().await?;
            Credential::bearer(token)
        })
    }
}

# async fn refresh_token() -> Result<String, CredentialError> {
#     Ok("token".into())
# }
# fn configure(
#     source: Arc<OAuthSource>,
#     limits: tact_llm::ModelLimits,
# ) -> Result<(), Box<dyn std::error::Error>> {
let provider = tact_llm::openai_responses("gpt-5.6")
    .refreshable(source)
    .limits(limits)
    .build()?;
# drop(provider);
# Ok(())
# }
```

Build stores the source but never invokes it. The first dispatched call resolves
the credential after request validation and cancellation checks. A failed
refresh is a permanent authentication `ModelError` before dispatch.

### Custom OpenAI-compatible endpoint

```rust
use tact_llm::{ChatDialect, ModelLimits, custom_chat};

# fn configure(limits: ModelLimits) -> Result<(), tact_llm::BuildError> {
let provider = custom_chat(
    "local-vllm",
    "Qwen/Qwen3-Coder-30B-A3B-Instruct",
    "http://127.0.0.1:8000/v1",
)
.anonymous()
.chat_dialect(ChatDialect::Standard)
.limits(limits)
.build()?;
# drop(provider);
# Ok(())
# }
```

Custom constructors default to the standard policy for their selected
protocol. DeepSeek and Kimi dialects are selected by typed constructors or an
explicit `chat_dialect` override. A custom endpoint never selects policy from
its display name or URL text.

## Public API

### Constructors

Each constructor is feature-gated by the protocol it builds:

```rust
pub fn anthropic(model: impl Into<String>) -> ProviderBuilder;

pub fn openai_chat(model: impl Into<String>) -> ProviderBuilder;
pub fn openai_responses(model: impl Into<String>) -> ProviderBuilder;

pub fn deepseek_chat(model: impl Into<String>) -> ProviderBuilder;
pub fn deepseek_responses(model: impl Into<String>) -> ProviderBuilder;

pub fn kimi_chat(model: impl Into<String>) -> ProviderBuilder;
pub fn kimi_responses(model: impl Into<String>) -> ProviderBuilder;

pub fn custom_chat(
    name: impl Into<String>,
    model: impl Into<String>,
    base_url: impl Into<String>,
) -> ProviderBuilder;

pub fn custom_responses(
    name: impl Into<String>,
    model: impl Into<String>,
    base_url: impl Into<String>,
) -> ProviderBuilder;
```

Constructors capture the provider family, protocol, built-in endpoint defaults,
auth requirement class, and policy defaults. They do not perform IO.

Built-in endpoint defaults are:

| Constructor | Default endpoint |
| --- | --- |
| `anthropic` | `https://api.anthropic.com` |
| `openai_chat`, `openai_responses` | `https://api.openai.com/v1` |
| `deepseek_chat`, `deepseek_responses` | `https://api.deepseek.com` |
| `kimi_chat`, `kimi_responses` | `https://api.moonshot.cn/v1` |

`custom_chat` and `custom_responses` require their caller-supplied endpoint.

### Builder

```rust
pub struct ProviderBuilder { /* private */ }

impl ProviderBuilder {
    pub fn authentication(self, auth: Authentication) -> Self;
    pub fn api_key(self, value: impl Into<String>) -> Result<Self, CredentialError>;
    pub fn bearer(self, value: impl Into<String>) -> Result<Self, CredentialError>;
    pub fn refreshable(self, source: Arc<dyn CredentialSource>) -> Self;
    pub fn anonymous(self) -> Self;

    pub fn base_url(self, value: impl Into<String>) -> Self;
    pub fn session_id(self, session_id: SessionId) -> Self;
    pub fn limits(self, limits: ModelLimits) -> Self;

    pub fn chat_dialect(self, dialect: ChatDialect) -> Self;
    pub fn native_compaction(self, enabled: bool) -> Self;
    pub fn responses_compact_threshold(self, tokens: u32) -> Self;
    pub fn supports_vision(self, enabled: bool) -> Self;
    pub fn replay_prior_reasoning(self, enabled: bool) -> Self;

    pub fn build(self) -> Result<Provider, BuildError>;

    #[cfg(feature = "product-runtime")]
    pub fn build_product(self) -> Result<Provider, BuildError>;
}
```

Builder setters do not perform IO. `build` validates the complete immutable
configuration and is synchronous. `build_product` shares the same construction
path but enables bounded exact request capture for the existing Tact product
store; it is not part of the stable third-party surface.

Defaults:

- a built-in provider without explicit auth fails with
  `BuildError::AuthenticationRequired`;
- a custom provider without explicit auth is anonymous;
- `limits` is required;
- `session_id` is optional;
- native compaction, vision, and reasoning replay resolve from provider policy
  unless explicitly overridden.

### Provider

```rust
#[derive(Clone)]
pub struct Provider { /* Arc-backed immutable state */ }

impl Provider {
    pub fn metadata(&self) -> &ProviderMetadata;
    pub fn into_model(self) -> Arc<dyn tact_agent_api::ModelPort>;
    pub fn with_session_id(self, session_id: SessionId) -> Self;
}

impl tact_agent_api::ModelPort for Provider {
    fn capabilities(&self) -> tact_agent_api::ModelCapabilities;

    async fn stream(
        &self,
        request: tact_agent_api::ModelRequest,
        cursor: Option<&tact_agent_api::ModelCursor>,
        cancel: tact_agent_api::CancelToken,
    ) -> Result<tact_agent_api::ModelStream, tact_agent_api::ModelError>;

    async fn complete(
        &self,
        request: tact_agent_api::ModelRequest,
        cursor: Option<&tact_agent_api::ModelCursor>,
        cancel: tact_agent_api::CancelToken,
    ) -> Result<tact_agent_api::ModelResponse, tact_agent_api::ModelError>;

    async fn compact(
        &self,
        request: tact_agent_api::ModelRequest,
        cursor: Option<&tact_agent_api::ModelCursor>,
        cancel: tact_agent_api::CancelToken,
    ) -> Result<tact_agent_api::ModelResponse, tact_agent_api::ModelError>;
}
```

`Provider` has a manually redacted `Debug`. Its state is immutable after build.
The internal model implementation and adapter selection are not nameable.
`with_session_id` returns a new provider with the same immutable policy and
transport configuration but a different session header value; it is used when
a subagent must not inherit its parent's session identity.

### Provider metadata

```rust
pub struct ProviderMetadata { /* private */ }

impl ProviderMetadata {
    pub fn provider(&self) -> &ProviderKind;
    pub fn protocol(&self) -> WireProtocol;
    pub fn model(&self) -> &str;
    pub fn endpoint(&self) -> &Endpoint;
    pub fn capabilities(&self) -> &tact_agent_api::ModelCapabilities;
    pub fn supports_vision(&self) -> bool;
}

#[non_exhaustive]
pub enum WireProtocol {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}
```

Metadata is produced only by a successful `build`. It contains the configured
model, resolved capabilities, vision support, and the canonical endpoint used
for dispatch and provider-state binding. It does not expose endpoint
classification, policy, credentials, transport, or raw configuration strings.

### Endpoint

```rust
pub struct Endpoint(/* canonical URL string */);

impl Endpoint {
    pub fn as_str(&self) -> &str;
}

impl std::fmt::Display for Endpoint;
impl std::fmt::Debug for Endpoint;
```

There is no public endpoint constructor. The builder creates it through the
private parser. The canonical endpoint:

- accepts only absolute `http` and `https` URLs with a host;
- rejects username/password userinfo with `EndpointError::EmbeddedCredentials`;
- rejects query and fragment components, including an empty `?` or `#`;
- lowercases scheme and host, removes the default port, and normalizes an empty
  path to `/`;
- removes one trailing slash only when the remaining path is non-empty;
- preserves path case and percent encoding;
- preserves a non-default port, IPv6 brackets, and an IPv6 port.

Protocol dispatch joins the fixed suffix with exactly one slash. In
particular, a root endpoint canonicalized as `https://api.example.com/`
dispatches to `https://api.example.com/messages`, not
`https://api.example.com//messages`.

`Endpoint` is the sole dispatch base and provider-state binding value. The
adapter appends only its fixed protocol suffix:

- Anthropic: `/messages`;
- Chat Completions: `/chat/completions`;
- Responses: `/responses`.

### Limits and session

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelLimits { /* private */ }

impl ModelLimits {
    pub fn new(
        context_window: impl Into<Option<usize>>,
        max_output_tokens: u32,
    ) -> Result<Self, LimitError>;

    pub fn context_window(self) -> Option<usize>;
    pub fn max_output_tokens(self) -> u32;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(/* private validated header-safe string */);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Result<Self, SessionIdError>;
    pub fn as_str(&self) -> &str;
}
```

`ModelLimits` rejects a known zero context window, zero output tokens, and an
output limit greater than or equal to the known context window. `None` is the
only unknown-window representation. `SessionId` rejects empty, newline,
control, and otherwise HTTP-header-illegal values; an omitted session remains
distinct from an explicit value.

### Authentication

```rust
pub struct Credential(/* private redacted secret */);

impl Credential {
    pub fn api_key(value: impl Into<String>) -> Result<Self, CredentialError>;
    pub fn bearer(value: impl Into<String>) -> Result<Self, CredentialError>;
    pub const fn none() -> Self;
}

pub trait CredentialSource: Send + Sync + std::fmt::Debug {
    fn resolve(
        &self,
    ) -> futures_util::future::BoxFuture<'_, Result<Credential, CredentialError>>;
}

pub enum Authentication {
    Static(Credential),
    Refreshable(Arc<dyn CredentialSource>),
    None,
}
```

`Credential` validates empty values, ASCII control characters, newline, and
header-illegal characters before storing a secret. Its `Debug` is redacted and
it has no `Display` or serde implementation.

The existing compatibility enum named `Credential` is renamed
`LegacyCredential` and remains reachable only through `legacy-tact`; the
stable `Credential` name belongs to the new opaque struct. Product code that
still needs the old enum imports `LegacyCredential` explicitly during the
migration.

Static credentials are validated at build. Refreshable credentials are not
resolved at build. If a required source later returns `Credential::none`, the
request fails with a permanent authentication `ModelError` before dispatch.

Header ownership is explicit:

- Anthropic maps an API key to `x-api-key` and a bearer credential to
  `Authorization: Bearer`;
- built-in OpenAI, DeepSeek, and Kimi endpoints map both API key and bearer to
  `Authorization: Bearer`;
- custom OpenAI-compatible endpoints omit authorization for
  `Authentication::None`;
- account and model-discovery requests remain legacy-only and accept only a
  static API key.

### Errors

```rust
#[non_exhaustive]
pub enum BuildError {
    InvalidModel { field: &'static str },
    InvalidEndpoint(EndpointError),
    InvalidLimits(LimitError),
    InvalidCredential(CredentialError),
    InvalidSession(SessionIdError),
    MissingLimits,
    AuthenticationRequired { provider: ProviderKind },
    ProviderFeatureDisabled {
        provider: ProviderKind,
        protocol: WireProtocol,
    },
    UnsupportedOption {
        option: &'static str,
        protocol: WireProtocol,
    },
    TransportSetup { stage: TransportSetupStage },
}

#[non_exhaustive]
pub enum EndpointError {
    InvalidUrl,
    UnsupportedScheme,
    MissingHost,
    EmbeddedCredentials,
    QueryOrFragmentNotAllowed,
}

#[non_exhaustive]
pub enum LimitError {
    ZeroContextWindow,
    ZeroOutputTokens,
    OutputExceedsContext,
}

#[non_exhaustive]
pub enum SessionIdError {
    Empty,
    ControlCharacter,
    InvalidHeaderValue,
}

#[non_exhaustive]
pub enum CredentialError {
    Empty,
    ControlCharacter,
    InvalidHeaderValue,
    Unavailable,
    RefreshFailed,
}

#[non_exhaustive]
pub enum TransportSetupStage {
    ReqwestClient,
}
```

`BuildError` is a closed, body-free classification. It does not expose reqwest
error text, URLs with credentials, request bodies, response bodies, or provider
configuration payloads. `CredentialError::Unavailable` and `RefreshFailed` are
runtime credential-source outcomes; static constructors use the validation
variants.

Runtime calls return `tact_agent_api::ModelError`:

- validation, authentication, malformed protocol, and unsupported features are
  permanent or unsupported before dispatch as appropriate;
- network failures, 408, 429, and 5xx are transient;
- `Retry-After` populates `ModelError.retry_after` when the direct HTTP path can
  read it;
- cancellation is always `ModelErrorKind::Cancelled`;
- context overflow maps to `ModelErrorKind::ContextOverflow`;
- public messages contain only fixed category text, status class, and safe
  retry facts. No HTTP or SSE body is retained in a public error.

## Package and Module Boundary

```text
tact_llm
  -> tact_agent_api
  -> optional protocol dependencies
  -> optional tracing

tact_agent_sdk
  -> tact_agent_api
  -> tact_agent_core

tact_llm and tact_agent_sdk remain siblings.
```

Planned internal layout:

```text
crates/tact_llm/src/
  lib.rs                    curated crate root
  auth.rs                   Authentication, Credential, CredentialSource
  builder.rs                ProviderBuilder and constructors
  endpoint.rs               opaque canonical endpoint
  error.rs                  build and compatibility errors
  limits.rs                 ModelLimits and SessionId
  metadata.rs               ProviderMetadata, ProviderKind, WireProtocol
  provider.rs               Provider and ModelPort implementation
  model.rs                  shared request conversion and validation
  stream.rs                 lazy deferred stream and terminal arbiter
  policy.rs                 sole provider/model/request policy owner
  transport.rs              private HTTP setup and bounded error reader
  telemetry.rs              optional body-free tracing
  providers/
    anthropic/
    openai_chat/
    openai_responses/
  compat/                   cfg(legacy-tact) only
  test_support.rs           cfg(test-support) only
```

Only `lib.rs` exports stable symbols. Protocol and compatibility modules are
private. `#[doc(hidden)]` is not a privacy boundary.

## Data Flow and Ownership

```text
constructor
  -> ProviderBuilder { profile, auth, session, limits, overrides }
  -> build
  -> validate profile, endpoint, model, limits, session, auth mode, options
  -> resolve private ProviderPolicy and capabilities once
  -> construct transport and concrete ModelPort once
  -> Provider { model, metadata }
```

One owner per invariant:

| Fact | Owner | Timing |
| --- | --- | --- |
| model/endpoint/provider syntax | private profile and endpoint parser | build |
| session-header safety | `SessionId::new` | construction |
| token-limit validity | `ModelLimits::new` | construction |
| auth requirement and option applicability | private `ProviderPolicy` | build |
| resolved capabilities and vision | build result | once |
| request model equality, output limit, cursor binding | `model.rs` | every call before auth/dispatch |
| credential refresh | adapter auth helper | once per dispatch |
| wire JSON, headers, SSE | private adapter | call time |
| HTTP classification and redaction | `transport.rs` | response boundary |
| cancellation terminal result | `stream.rs` | stream lifetime |

### Request validation

All `stream`, `complete`, and `compact` calls use one validation function.
Before credential resolution or dispatch they:

- reject an already-cancelled token;
- reject `max_output_tokens == 0`;
- reject a request limit above the built target limit;
- reject a request model different from the built model;
- validate cursor shape and endpoint binding;
- reject native compaction when the built capabilities do not support it.

The built target model is strict. A host that wants to switch models builds a
new `Provider` and carries transcript/provider state explicitly. The legacy
compatibility path may preserve same-family request-level switching until its
callers migrate.

### Policy

`policy.rs` is the only module that interprets provider, model, endpoint, or
wire-policy strings. It owns:

- wire protocol and adapter family;
- built-in endpoint defaults;
- Chat Completions dialect;
- reasoning encoding;
- hosted tools;
- native compaction defaults;
- vision defaults;
- prior-reasoning replay;
- session-header behavior;
- legacy account/discovery eligibility.

Stable constructors select policy explicitly. A custom provider defaults to
standard Chat Completions or standard Responses behavior and changes dialect
only through a typed builder option. The legacy `ProviderInfo` and
`ProviderProfile` path may preserve existing substring heuristics behind
`legacy-tact`.

## Runtime Semantics

### Streaming and cancellation

`Provider::stream` validates the call and returns a lazy stream wrapper. The
wrapper owns the request, cursor, cancellation token, private transport state,
an optional producer handle, and one terminal slot.

The first `poll_next`:

1. checks cancellation before auth or dispatch;
2. on cancellation, yields one `Err(ModelErrorKind::Cancelled)` and spawns
   nothing;
3. otherwise spawns one producer task and stores its abort-on-drop handle.

The producer:

1. resolves authentication;
2. rechecks cancellation;
3. emits `MessageStarted` only after that check;
4. polls the adapter with cancellation-first biased selection;
5. rechecks cancellation immediately before claiming the terminal slot.

The terminal state is `Open -> Finished | Failed | Cancelled`. Exactly one
terminal item is visible. After it, the wrapper clears producer and receiver
state and the next poll returns `None`. Progress events are bounded and
best-effort; the terminal response is sent with backpressure.

Dropping an unpolled stream dispatches nothing. Dropping a live stream aborts
its producer and closes the receiver. Drop does not call `cancel()` on the
caller-owned token because that token may be shared by other work.

`complete` and `compact` check cancellation before polling the adapter future
and use the same cancellation-first selection.

### Credentials

Construction stores an auth mode but never calls a refreshable
`CredentialSource`. Each dispatched call:

1. validates request and cursor;
2. checks cancellation;
3. checks auth requirement from policy;
4. polls credential resolution inside a cancellation-first select, so a
   cancellation that wins while a refresh is in flight drops the resolution
   future and returns one `Cancelled` terminal outcome without dispatching;
5. transforms it into adapter-private headers or body fields;
6. dispatches.

Credential resolution counts are tested:

- `Provider::build` resolves zero credentials;
- one `stream`, `complete`, or `compact` dispatch resolves exactly one;
- a cancelled or invalid call starts no resolution, or drops an in-flight
  resolution before it completes, and never dispatches;
- a missing required credential fails before network dispatch.

### HTTP errors and redaction

Direct HTTP adapter paths use one bounded helper:

- read at most `MAX_HTTP_ERROR_BODY_BYTES + 1` response bytes;
- parse status, typed class, and `Retry-After` before discarding body bytes;
- classify known context-overflow and authentication shapes inside
  `tact_llm`;
- never copy request or response body text into public errors;
- map 404/405 on native compaction to `ModelErrorKind::Unsupported`.

The Responses adapter must not use a provider SDK HTTP path that logs or retains
the response body in `JSONDeserialize`. It may reuse SDK wire event types, but
stable transport and error parsing are private. A malformed-payload marker must
not appear in any adapter or dependency subscriber output.

Stable `Provider` builds always set `ModelResponse.request_body = None`.
`build_product()` may populate the existing field with bounded bytes for Tact
usage persistence. `ModelResponse` gets a manual redacted `Debug` so even a
product capture cannot be printed through derived diagnostics. A later design
may replace this compatibility carrier with a dedicated opaque type.

`UsageRecord` and the compatibility `LlmResponse` also carry request bytes;
both receive manual redacted `Debug` implementations in this phase. A body
marker must not appear in `Debug`, `Display`, error chains, or tracing.

### Tracing

The `telemetry` feature adds host-owned `tracing` spans and events. Required
fields:

- provider kind, wire protocol, model id, and endpoint host;
- request kind: stream, complete, compact, or legacy account/discovery;
- elapsed time, time to first stream event, total stream time;
- status class, typed error kind, retry delay, cancellation, and outcome;
- token counts when available.

Library telemetry never records prompts, responses, request bodies, response
bodies, tool arguments, authorization headers, credentials, raw SSE frames, or
provider-state payloads. The library never installs a subscriber.

## Core Contract Changes

### `ModelCapabilities.context_window`

Change:

```rust
pub struct ModelCapabilities {
    pub model: String,
    pub context_window: Option<usize>,
    pub max_output_tokens: u32,
    pub supports_streaming: bool,
    pub supports_native_compaction: bool,
}
```

Rules:

- `None` is the only unknown value;
- checked constructors reject `Some(0)`;
- `known_context_window()` returns `None` for `None` or a directly constructed
  `Some(0)`;
- custom serde deserialization maps legacy `0` and `null` to `None`;
- serialization emits numeric `0` for `None` so current stored JSON remains
  readable;
- all context, prompt, compaction, status, and TUI arithmetic uses
  `known_context_window()` or an explicit product fallback;
- product `model_context_window = 0` maps to `None` at the capability boundary.

This migration updates every constructor and consumer in `agent_api`,
`agent_core`, `agent_sdk`, `tact`, `tact-ui`, and `tui` in one independently
tested task.

### Request body compatibility

This phase does not add a new request-capture carrier. It enforces:

- stable provider construction and every stable call leave
  `ModelResponse.request_body` absent;
- `build_product()` may capture exact bounded bytes only for Tact;
- no error, debug rendering, or telemetry exposes captured bytes;
- existing usage storage and `/view-system-prompt` continue to read the same
  bounded bytes;
- a dedicated request-capture type remains a follow-up product persistence
  design.

## Feature Matrix

The stable package default is `core`. Protocol dependencies are opt-in:

| Feature | Adds | Must not add |
| --- | --- | --- |
| `core` (default) | facade, builder, auth, limits, metadata, `ModelPort` wiring | HTTP client or provider SDK |
| `anthropic` | Anthropic adapter | OpenAI crates |
| `openai-chat` | Chat Completions adapter | Responses crate |
| `openai-responses` | Responses adapter and provider state | Chat adapter |
| `openai` | both OpenAI protocols | unrelated provider SDKs |
| `telemetry` | optional tracing instrumentation | subscriber installation |
| `test-support` | deterministic mocks and fixtures | production names |
| `legacy-tact` | legacy globals, `ProviderInfo`, `LlmProvider` | new stable construction |
| `account` | account balance/quota compatibility APIs | stable SDK methods |
| `model-discovery` | model discovery compatibility APIs | stable SDK methods |
| `product-runtime` | product-only bounded capture | stable body access |

```toml
[features]
default = ["core"]
core = []
anthropic = [
  "core",
  "dep:reqwest",
  "dep:reqwest-eventsource",
  "dep:secrecy",
]
openai-chat = [
  "core",
  "dep:reqwest13",
  "reqwest13/stream",
  "dep:eventsource-stream",
  "dep:secrecy10",
  "dep:async-openai",
]
openai-responses = [
  "core",
  "dep:reqwest13",
  "reqwest13/stream",
  "dep:eventsource-stream",
  "dep:secrecy10",
  "dep:async-openai-responses",
]
openai = ["openai-chat", "openai-responses"]
telemetry = ["core", "dep:tracing"]
test-support = ["core"]
legacy-tact = ["anthropic", "openai", "telemetry"]
product-runtime = ["legacy-tact"]
account = ["legacy-tact"]
model-discovery = ["legacy-tact"]
```

The exact dependency edges are finalized against the real import graph during
the feature-gate task. The invariant is fixed: a feature a build does not need
must be absent from that build's dependency tree.

Third-party examples use:

```toml
tact_llm = {
  path = "../tact/crates/tact_llm",
  default-features = false,
  features = ["openai-responses"]
}
```

## Compatibility Surface

The following remain available only under `legacy-tact`:

- process-global `PROVIDER` and `CREDENTIALS`;
- `init_provider`, `init_provider_with_credentials`, `get_provider`,
  `read_provider`, `current_provider_kind`;
- `ProviderInfo`, `ProviderProfile`, `Client`, `get_llm_client`;
- `LlmProvider`, `ProviderModel`, `LlmClient`, `LlmResponse`;
- `LegacyCredential`, `CredentialProvider`, `ApiKeyProvider`;
- concrete `anthropic`, `openai::compatible`, and `openai::responses` modules;
- body hooks, request options, wire types, normalization helpers, locking
  helpers, and provider progress conversion;
- account balance/quota and model-discovery APIs;
- `ProviderConversationState` and legacy provider-state conversion;
- `MockClient` and any test-only legacy names exposed to product tests.

Compatibility constructors delegate to the stable builder or private shared
adapter construction; they do not introduce a second policy implementation.
They are deprecated and may be removed after Tact stops using them.

Existing Tact behavior is frozen before migration by checked-in request JSON,
headers, SSE fixtures, provider-state values, account error shapes, and usage
store tests.

## Product Migration

The product migrates incrementally:

1. Build a `Provider` from `ProviderInfo` and the existing credentials.
2. Store one immutable activation snapshot containing the model, metadata,
   capabilities, and vision facts.
3. Use `ProviderMetadata` for Responses cursor binding, native-compaction
   routing, and vision gating instead of `LlmProvider` downcasts.
4. Read the activation snapshot for the whole turn, including subagent
   inheritance.
5. For a subagent, rebind the parent snapshot with `Provider::with_session_id`
   and the child session before the child runs; the parent session header must
   never leak into the child.
6. Validate the product session id with `SessionId::new` before building the
   provider. An existing `--session` value that is not header-safe fails at
   startup with a clear error instead of silently dropping the session header.
7. Build a replacement provider before publishing `/model` settings or status.
8. Keep account query and model discovery on their existing legacy functions
   until a later migration.

Provider-state load validation uses the canonical endpoint and wire protocol
from `ProviderMetadata`. It never compares raw profile URL spellings. Existing
session rows remain readable; this design adds no new session metadata schema.

The complete `/model` request/ack correlation and session-metadata versioning
work remains a separate design. This phase only prohibits publishing model
settings that do not correspond to a successfully built provider.

## Distribution and Semver

The crate is path/Git consumable after the facade lands. Registry publication
requires:

- registry-safe `tact_agent_api`;
- no Git-only provider dependency in the published feature graph;
- a tested MSRV or explicit policy;
- package metadata, README, examples, and rustdoc feature notes;
- no unresolved compatibility feature required by a stable-only build.

The stable semver surface is:

- provider constructor functions;
- `ProviderBuilder`, `Provider`, and `ProviderMetadata`;
- `ModelLimits`, `SessionId`, `Endpoint`, `ProviderKind`, `WireProtocol`,
  `ChatDialect`;
- `Authentication`, `Credential`, `CredentialSource`;
- `BuildError`, `EndpointError`, `LimitError`, `SessionIdError`,
  `CredentialError`, `TransportSetupStage`;
- required `tact_agent_api` model-call vocabulary re-exported by `tact_llm`.

Everything in the compatibility section is explicitly outside the stable
promise.

## Synthesis Decision

Three structures were compared:

- **Candidate A:** `ProviderClient -> ModelSpec -> BuiltModel -> ModelPort`;
- **Candidate B:** `provider constructor -> ProviderBuilder -> Provider(ModelPort)`;
- **Candidate C:** `ProviderFactory -> ModelTarget -> BuiltModel + FacadeMetadata`.

Candidate B is the base because it has the smallest public call chain, makes the
built provider itself the runtime seam, and stays closest to the existing
adapters during migration. Cross-judge scores were B 26, A 25, C 18.

Grafts retained:

- from A: synchronous build, strict request-model equality, canonical opaque
  endpoints, layered validation errors, and a no-HTTP `core` default;
- from C: `ModelCapabilities.context_window: Option<usize>`, canonical
  endpoint/provider-state binding, bounded HTTP error reads, and
  dependency-subscriber redaction tests;
- from the incumbent failure analysis: lazy streaming, one terminal arbiter,
  per-dispatch credentials, and feature-specific external fixtures.

Rejected:

- `BuiltModel::into_model` as the only route to the runtime seam;
- public policy/configuration stages;
- build-time refreshable credential probes;
- unknown context represented as zero at the stable `ModelPort` boundary;
- all-provider default features;
- runtime registries and dynamic loading;
- immediate multi-crate splits;
- request-capture and `/model` protocol migrations as prerequisites.

## Alternatives Considered

### Keep the current public module tree with a shorter README

Rejected. It leaves globals, duplicate construction, wire types, body-bearing
errors, and concrete adapter mutation as the third-party contract.

### Split core, Anthropic, OpenAI Chat, and OpenAI Responses into separate crates

Deferred. Feature gates isolate dependencies now without multiplying semver and
release coordination before policy is stable. Private modules preserve a later
split.

### Add a new `tact_llm_sdk` facade over the existing product crate

Rejected. It creates two live provider layers and two ways to construct a
model. The existing package can be made deep without a permanent wrapper.

### Add a runtime provider registry

Rejected. Static constructor injection already supports built-in and custom
providers. A registry adds identity, lifecycle, and discovery without a user
need.

### Make `tact_llm` depend on `tact_agent_sdk`

Rejected. The provider implements a port; the SDK composes the runtime.
Reversing that dependency creates a cycle and pulls HTTP stacks into every SDK
host.

### Use `LlmProvider` as the stable constructor

Rejected. It exposes enum variants, concrete adapters, mutation, and downcasts.
`Provider: ModelPort` is the runtime interface.

## Tradeoffs

- We accept one larger crate in exchange for shared policy and a staged product
  migration.
- We accept explicit model limits in exchange for capabilities that are facts,
  not guesses.
- We accept a synchronous build in exchange for no hidden OAuth or network work
  during construction.
- We accept per-dispatch credential resolution overhead in exchange for
  refreshable authentication.
- We accept an opaque endpoint type and provider metadata in the stable API in
  exchange for correct provider-state binding.
- We accept no public transport configuration in v1; hosts with custom mTLS or
  in-process transports implement `ModelPort`.
- We accept compatibility globals behind a feature during migration instead of
  a flag-day rewrite.
- We accept `request_body: None` as the stable contract until the product
  persistence carrier is redesigned separately.

## Risks and Open Questions

- Can Responses move to the private direct HTTP/SSE path without changing any
  checked-in fixture or cursor update?
- Can the cross-crate `context_window: Option` migration update all arithmetic
  without one missed zero assumption?
- Does feature unification in the workspace hide an accidental dependency
  edge? External fixture builds must check the actual consumer graph.
- Can existing Tact provider-state rows be validated by canonical endpoint
  without a metadata schema change?
- Does the current `/model` flow have a safe internal prepare/commit point
  without the deferred protocol redesign?
- Can the crate become registry-safe before a public release, or must the first
  SDK stay path/Git-only?
- What MSRV policy will the package promise?

## Verification

- Checked-in request JSON/header goldens cover Anthropic, OpenAI Chat,
  DeepSeek, Kimi, custom Chat, OpenAI Responses, custom Responses, compaction,
  and request-level legacy model switches.
- All checked-in Responses SSE fixtures replay to identical normalized content,
  stop reason, usage, and cursor update.
- `Provider` compiles as a `ModelPort` and works directly with
  `tact_agent_sdk::AgentBuilder` in an independent fixture.
- Direct and agent-SDK fixtures use `default-features = false` plus exactly one
  provider feature.
- `cargo tree` proves Anthropic-only excludes OpenAI SDKs, Chat-only excludes
  Responses, Responses-only excludes Chat, and `core` excludes HTTP/provider
  crates.
- Feature-disabled provider constructors are not nameable, and public API
  compile-fail tests reject wire, transport, hook, adapter, and compatibility
  imports in stable builds.
- Builder tests cover missing limits, required authentication, custom anonymous
  access, static key/bearer equivalence, malformed session ids, invalid limits,
  endpoint normalization, unsupported typed options, and disabled providers.
- `Provider::build` resolves zero refreshable credentials. Successful,
  cancelled, invalid, stream, complete, and compact calls have exact
  per-dispatch resolve counts.
- Request validation proves strict model equality, zero and oversized output
  limits, cursor endpoint mismatch, and native-compaction rejection before
  credential resolution or network dispatch.
- Stream tests prove one terminal outcome with cancellation-first arbitration,
  no dispatch before first poll, no dispatch after drop, abort on drop
  mid-flight, cancellation during credential refresh, and no item after the
  terminal item.
- HTTP error tests prove bounded reads, typed classification, `Retry-After`
  propagation, and absence of request/response bodies and request URLs from
  public errors.
- Tracing tests prove spans exist, durations and time-to-first-event are
  recorded, and prompt, credential, body, tool-argument, and SSE canaries are
  absent from both `tact_llm` and dependency targets.
- Stable provider calls always leave `ModelResponse.request_body` absent.
  Product capture remains bounded and reachable only through product code.
- `ModelCapabilities.context_window` serde round-trips legacy `0` and `null` to
  `None`; all context, prompt, compaction, status, and TUI tests compile and
  pass.
- Root endpoints join protocol suffixes with one slash, and the Anthropic
  constructor can build from its documented default endpoint without a
  base-URL setter call.
- Product tests prove activation-scoped provider metadata drives Responses
  binding, native compaction, vision gating, and subagent inheritance without
  `LlmProvider` downcasts.
- Product model-switch failure preserves the previous provider and host
  settings; a successful rebuild publishes one coherent activation snapshot.
- `cargo doc -p tact_llm --no-deps` has no broken links and feature docs match
  the actual manifest.

## Definition of Done

- A third-party host can select one provider feature, build a model, and pass
  it to `tact_agent_sdk::AgentBuilder` without globals.
- Static and refreshable authentication work, and construction performs no
  credential resolution or network IO.
- Every stable provider build returns a `Provider` implementing `ModelPort`.
- One internal policy module owns all provider/model/endpoint interpretation.
- Unknown context windows are `None` end to end; no stable capability is zero
  as a sentinel.
- Stable calls never expose or retain request/response bodies.
- Cancellation is lazy, single-terminal, and drop-safe.
- Credential refresh is cancellation-aware and cannot keep a cancelled stream
  waiting for an OAuth refresh.
- Wire behavior is unchanged, proven by checked-in goldens and fixtures.
- Provider dependencies are feature-isolated and recorded in external consumer
  checks.
- Tact uses provider metadata rather than adapter downcasts for provider-state,
  compaction, and vision decisions.
- A subagent receives the parent policy with a child session id, and a product
  `responses_compact_threshold` remains injectable through the builder.
- Compatibility globals and concrete adapters are unreachable from a stable
  build.
- The spec and plan pass a balanced Markdown-fence and relative-link check.

## Next Implementation Step

Create the independent provider and agent-SDK consumer fixtures plus checked-in
wire/SSE/redaction baselines, then migrate
`tact_agent_api::ModelCapabilities.context_window` to `Option<usize>` while
keeping the workspace green.
