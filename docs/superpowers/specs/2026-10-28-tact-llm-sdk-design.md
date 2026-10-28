# Tact LLM SDK Architecture

> Date: 2026-10-28
> Status: proposed
> Related: `crates/tact_llm/`, `crates/agent_api/`, `crates/agent_sdk/`,
> `ARCHITECTURE.md`
> Plan: `docs/superpowers/plans/2026-10-28-tact-llm-sdk.md`

## Goal

Turn `tact_llm` into a third-party Rust SDK without creating a second agent
runtime.

The crate should let a host:

- build a concrete model from explicit configuration and credentials;
- use a static API key, a static bearer token, a refreshable credential source,
  or no credential;
- use OpenAI, Anthropic, DeepSeek, Kimi, or a custom OpenAI-compatible
  endpoint;
- receive the existing `tact_agent_api::ModelPort` without an adapter wrapper;
- stream events, cancel a call, compact when supported, and consume typed
  errors;
- configure tracing through the normal `tracing` subscriber model;
- avoid process globals, panics during initialization, and imports from wire or
  transport modules.

The product keeps `tact_llm` as one crate for this phase. The public package is
a facade over deep internal modules, not a collection of public implementation
modules.

The SDK is Tokio-based in this phase, matching the existing adapter and agent
runtime. Runtime-agnostic streaming is not a v1 promise.

This phase makes the crate a path or Git consumable SDK with a stable facade.
Registry publication is a separate readiness gate because `tact_agent_api` is
currently path-only and the Responses SDK is a Git fork. The design must not
claim crates.io readiness until that dependency graph is made registry-safe.

## Non-Goals

- Do not create a runtime provider plugin registry, plugin identity, install
  lifecycle, dynamic loader, or service locator.
- Do not introduce a second model-call trait beside `tact_agent_api::ModelPort`
  for the new SDK path.
- Do not change agent-kernel contracts or make `tact_agent_sdk` depend on
  `tact_llm`.
- Do not expose `reqwest`, `async-openai`, `async-openai-local`, SSE event
  structs, wire DTOs, body hooks, lock helpers, or normalization helpers from
  the stable facade.
- Do not make account quota queries part of the minimum provider build path.
- Do not split protocol adapters into separate crates in this design. The
  split-crate alternative is recorded below with its costs.
- Do not promise a custom HTTP transport trait until the provider SDKs can use
  it without keeping parallel transport stacks.
- Do not make account quota or model discovery part of the stable third-party
  facade in this phase. They remain optional product compatibility APIs until
  their global credential and model-cache dependencies are removed.
- Do not claim crates.io publication before `tact_agent_api` and every optional
  provider dependency are available from a registry.

## Constraints

- Existing Tact code imports `ProviderInfo`, `LlmProvider`, `ProviderModel`,
  `MockClient`, `neutral::*`, `ProviderConversationState`, and
  `openai::responses::OpenAiResponsesAdapter` in production and tests.
- `ProviderInfo::build_client()` and `Client::new()` currently implement
  provider construction twice.
- DeepSeek, Kimi, K2, K3, Responses, vision, and account eligibility rules are
  spread across several modules.
- `PROVIDER`, `CREDENTIALS`, and the model cache are process globals.
- The OpenAI Responses adapter is a provider package inside the crate and
  needs decomposition, but its behavior has extensive fixture coverage and
  must not change during file movement.
- The crate currently pulls two reqwest generations and two secrecy
  generations because its upstream SDKs differ.
- `tact_agent_api::ModelPort` is the runtime seam. `ProviderModel` already
  implements it.
- Plugin style means constructor injection and static composition. A custom
  provider is an injected `Arc<dyn ModelPort>` or a custom endpoint profile,
  not a registered plugin.

## Third-Party Usage

### Minimal model for the agent SDK

```rust
use std::sync::Arc;

use tact_agent_sdk::AgentBuilder;
use tact_llm::{
    CredentialProvider, ModelLimits, ModelTarget, ProviderFactory, ProviderProfile,
};

# async fn run(
#     credentials: Arc<dyn CredentialProvider>,
#     target: ModelTarget,
# ) -> Result<(), Box<dyn std::error::Error>> {
let built = ProviderFactory::new()?.build(target, credentials).await?;
let metadata = built.metadata().clone();
let mut session = AgentBuilder::new(built.into_model()).build_session("chat-1")?;
let outcome = session.run_text("Summarize the issue.").await?;
println!("{:?}", outcome.stop);
println!(
    "{} {:?}",
    metadata.provider(),
    metadata.endpoint_identity()
);
# Ok(())
# }

let profile = ProviderProfile::openai("gpt-5.6")?;
let limits = ModelLimits::try_new(1_050_000, 32_000)?;
let target = ModelTarget::new(profile, limits)?;
```

`ProviderFactory::build_with_api_key` and
`ProviderFactory::build_anonymous` are conveniences over the same factory path.
They do not create separate construction code.

### Custom OpenAI-compatible endpoint

```rust
use tact_llm::{ModelLimits, ModelTarget, OpenAiProtocol, ProviderFactory, ProviderProfile};

let profile = ProviderProfile::custom(
    "local-vllm",
    "Qwen/Qwen3-Coder-30B-A3B-Instruct",
    "http://127.0.0.1:8000/v1",
    OpenAiProtocol::ChatCompletions,
)?;
let limits = ModelLimits::try_new(64_000, 8_000)?;
let target = ModelTarget::new(profile, limits)?;
let built = ProviderFactory::new()?.build_anonymous(target).await?;
let metadata = built.metadata().clone();
let model = built.into_model();
# drop(metadata);
# drop(model);
```

The custom endpoint defaults to standard OpenAI-compatible behavior. A typed
target override is required to opt into DeepSeek, Kimi, or another non-standard
body policy:

```rust
# use tact_llm::{
#     ChatCompletionsDialect, ModelLimits, ModelTarget, OpenAiProtocol,
#     ProviderProfile,
# };
# fn configure() -> Result<ModelTarget, Box<dyn std::error::Error>> {
let profile = ProviderProfile::custom(
    "deepseek-gateway",
    "deepseek-chat",
    "https://gateway.example.com/v1",
    OpenAiProtocol::ChatCompletions,
)?;
let limits = ModelLimits::try_new(128_000, 16_000)?;
let target = ModelTarget::new(profile, limits)?
    .with_chat_dialect(ChatCompletionsDialect::DeepSeek)?;
# Ok(target)
# }
```

No string-keyed hook map is public.

### Direct stream and cancellation

```rust
use futures_util::StreamExt;
use tact_llm::{CancelToken, ModelError, ModelEvent, ModelPort, ModelRequest};

# async fn consume(
#     model: std::sync::Arc<dyn ModelPort>,
#     request: ModelRequest,
# ) -> Result<(), ModelError> {
let cancel = CancelToken::new();
let mut stream = model.stream(request, None, cancel.clone()).await?;
while let Some(event) = stream.next().await {
    match event? {
        ModelEvent::MessageDelta { delta, .. } => println!("{delta:?}"),
        ModelEvent::Finished(_) => {}
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

The same `Arc<dyn ModelPort>` is accepted by `tact_agent_sdk::AgentBuilder`.
Direct callers own their cancellation token; session callers use
`AgentSession::control()`.

Cancellation is cooperative and terminal:

- an already-cancelled token is rejected before dispatch for `stream`,
  `complete`, and `compact`;
- a live stream has exactly one terminal outcome:
  `ModelEvent::Finished(response)`, `Err(Cancelled)`, or another provider
  `Err(ModelError)` such as transport, HTTP, authentication, or parse failure;
- the producer-side terminal state moves once from `Open` to one of
  `Finished`, `Failed`, or `Cancelled`. Cancellation observed before a
  success/failure claim wins and emits `Cancelled`; otherwise the provider
  outcome claims the terminal slot;
- once any terminal outcome claims the slot, no later event or cancellation
  outcome is emitted; alternatively, a cancellation that wins prevents a
  provider failure from being emitted;
- the terminal priority is normative and not scheduler-dependent: each
  producer wait is a biased selection with cancellation readiness checked
  before the provider future. If both are ready, cancellation wins. The
  provider success/failure branch must still recheck cancellation immediately
  before its compare-and-set claim, closing the race between selection and
  claim;
- after the wrapper yields any terminal item, it marks the stream closed,
  clears the inner stream and producer state, and returns `Poll::Ready(None)`
  on the next poll. No terminal or non-terminal item can follow;
- the stream wrapper is lazy: construction only stores the request, cursor,
  cancellation token, and factory-built client state; the producer task is
  created on the first `poll_next`, not in `stream_params`;
- the first `poll_next` rechecks cancellation before spawning the producer or
  polling the adapter; cancellation between `stream()` returning and that
  first poll yields exactly one `Cancelled` outcome and performs no dispatch;
- `ProviderModel` returns a stream wrapper that owns the deferred producer
  `JoinHandle` and an abort-on-drop guard. Dropping the wrapper before the first
  poll performs no dispatch; dropping it mid-flight aborts the producer, drops
  the receiver, and prevents a later send;
- a stream dropped before its first poll performs no provider dispatch;
- a stream dropped mid-flight aborts the producer without waiting on an HTTP
  read. The implementation test uses a task tracker and a bounded yield to
  prove the producer is terminal, rather than merely assuming the receiver
  drop is sufficient.
- terminal emission is owned by one producer-side arbiter. Progress sends are
  ignored once the gate is closed; the first successful claim following the
  normative priority above is `Finished`, `Failed`, or `Cancelled`, and losing
  claims are discarded. A success or failure that claims first remains the
  single terminal event; later cancellation is ignored. No path can emit
  another item after the terminal item.

The public `ModelStream` alias remains the `tact_agent_api` boxed-stream
contract. The wrapper is an internal `tact_llm` stream object boxed into that
alias; it does not add a second public stream type.

## Package Boundary

```text
tact_llm
  -> tact_agent_api
  -> secrecy
  -> serde and serde_json
  -> optional provider SDKs
  -> optional tracing

tact_agent_sdk
  -> tact_agent_api
  -> tact_agent_core

tact_llm and tact_agent_sdk remain siblings.
```

`tact_llm` re-exports the minimum `tact_agent_api` model types a direct caller
needs. It does not re-export the rest of the agent API.

The package name remains `tact_llm`. A future crate split may move internals,
but third-party code should be insulated from that move by the facade.

## Module Map

```text
crates/tact_llm/src/
  lib.rs                    curated public facade
  build_error.rs            typed provider construction errors
  auth.rs                   credential source and resolution
  transport.rs              internal shared HTTP configuration
  model_target.rs           profile + limits + per-model options
  facade_metadata.rs        provider/endpoint binding facts
  profile.rs                credential-free ProviderProfile
  account.rs                optional account queries
  models.rs                 optional model discovery and cache
  provider/
    mod.rs                  provider facade types
    policy.rs               sole provider/model heuristic owner
    factory.rs              one internal construction path
    legacy.rs               deprecated globals and compatibility API
  provider_model.rs         ModelPort implementation
  anthropic/
    mod.rs                  narrow provider constructor
    adapter.rs
    request.rs
    stream.rs
  openai/
    mod.rs
    compatible/
      mod.rs
      adapter.rs
      request.rs
      stream.rs
    responses/
      mod.rs
      adapter.rs
      request.rs
      events.rs
      history.rs
      capabilities.rs
      request_options.rs
      normalize.rs
      state.rs
      headers.rs
      wire.rs
  neutral.rs                internal compatibility conversion
  mock.rs                   test-support adapter
```

Only `lib.rs` and explicitly public facade modules are importable from outside.
Internal modules use `pub(crate)` even when their old paths are temporarily
re-exported for compatibility. `#[doc(hidden)]` is not a privacy boundary:
wire DTOs, SSE frames, body hooks, adapters, and normalization helpers must not
remain publicly nameable through the stable feature set. Compatibility module
paths are compiled only under `legacy-tact` and contain no stable wire types.

## Public API

### Model target

```rust
pub struct ModelLimits {
    context_window: usize,
    max_output_tokens: u32,
}

impl ModelLimits {
    pub fn try_new(
        context_window: usize,
        max_output_tokens: u32,
    ) -> Result<Self, ModelTargetError>;

    pub const fn context_window(&self) -> usize;

    pub const fn max_output_tokens(&self) -> u32;
}

#[non_exhaustive]
pub enum ModelTargetError {
    InvalidProfile(ProfileError),
    InvalidModel { field: &'static str },
    InvalidLimit { field: &'static str, reason: &'static str },
    UnsupportedProtocol {
        provider: ProviderKind,
        protocol: OpenAiProtocol,
    },
    InvalidSessionId,
    UnsupportedOverride { field: &'static str, protocol: WireProtocol },
}

#[non_exhaustive]
pub enum CapabilityPreviewError {
    Policy(PolicyError),
    ProviderFeatureDisabled {
        provider: ProviderKind,
        feature: ProviderFeature,
    },
}

pub struct ModelTarget {
    profile: ProviderProfile,
    limits: ModelLimits,
    chat_dialect: Option<ChatCompletionsDialect>,
    session_id: Option<String>,
    replay_prior_reasoning: Option<bool>,
    native_compaction: Option<bool>,
}

impl ModelTarget {
    pub fn new(
        profile: ProviderProfile,
        limits: ModelLimits,
    ) -> Result<Self, ModelTargetError>;

    pub fn with_session_id(
        self,
        session_id: impl Into<String>,
    ) -> Result<Self, ModelTargetError>;

    pub fn with_replay_prior_reasoning(self, replay: bool) -> Self;

    pub fn with_chat_dialect(
        self,
        dialect: ChatCompletionsDialect,
    ) -> Result<Self, ModelTargetError>;

    pub fn with_native_compaction(
        self,
        supported: bool,
    ) -> Result<Self, ModelTargetError>;

    pub fn profile(&self) -> &ProviderProfile;
    pub fn model(&self) -> &str;
    pub fn limits(&self) -> ModelLimits;
    pub fn session_id(&self) -> Option<&str>;
    pub fn chat_dialect(&self) -> Option<ChatCompletionsDialect>;
    pub fn replay_prior_reasoning(&self) -> Option<bool>;
    pub fn native_compaction(&self) -> Option<bool>;

    pub fn preview_capabilities(
        &self,
    ) -> Result<ModelCapabilities, CapabilityPreviewError>;

    pub(crate) fn policy_overrides(&self) -> ProviderPolicyOverrides;
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ProviderPolicyOverrides {
    pub(crate) chat_dialect: Option<ChatCompletionsDialect>,
    pub(crate) native_compaction: Option<bool>,
    pub(crate) replay_prior_reasoning: Option<bool>,
}
```

`ModelLimits::try_new` rejects `max_output_tokens == 0` and, when
`context_window != 0`, rejects `max_output_tokens >= context_window`.
`context_window == 0` is the existing, explicit unknown-window representation.
Fields are private so callers cannot bypass these checks.

The configured `max_output_tokens` is the default capability limit, not a
silent clamp. `ModelRequest.max_output_tokens` is the caller's authoritative
per-call value. The built-in adapters pass it through unchanged and reject
`0`; they reject values above the target's configured maximum before dispatch.
They do not rewrite an oversized request to the configured maximum, because
that would blur caller intent and make context-budget failures invisible.
`context_window == 0` is never forwarded as a consumer-visible model window:
`ModelPort::capabilities()` reports `0`, and consumers must treat it as unknown
rather than as a zero-token capacity. Tact keeps its configured fallback for
budgeting when the capability is unknown and must not use `0` in arithmetic.

`ModelTarget::new` validates only intrinsic facts: non-empty provider/model,
a valid absolute endpoint, supported provider/protocol shape, and token limits.
It does not resolve vendor policy. `ProviderKind::Anthropic` combined with
explicit `OpenAiProtocol::Responses` returns
`ModelTargetError::UnsupportedProtocol`; its legacy-compatible
`ChatCompletions` value is inert because Anthropic always maps to
`WireProtocol::AnthropicMessages`. Capability overrides that depend on provider
policy are applied by the fallible `with_chat_dialect` and
`with_native_compaction` builders. Streaming is always `true` for the built-in
adapters; native compaction defaults from resolved policy and may be made
explicit for a capable custom endpoint.

The accessors are the supported factory seam. They expose the validated target
without making its fields public or allowing mutation after validation.
`policy_overrides` remains crate-private so callers cannot assemble a policy
mode that bypasses the fallible builders.

`with_session_id` validates the value as an HTTP header value before storing
it. Omission and an empty string are distinct states: omission means "do not
send a session header", while an explicitly supplied empty, control-character,
newline, or otherwise header-illegal value returns
`ModelTargetError::InvalidSessionId`. Adapters never silently drop an invalid
session id.

`ModelTarget` is the validated configuration boundary. It owns the configured
model, endpoint profile, per-client session identity, typed body-policy and
capability overrides, and prior-reasoning replay choice. `policy_overrides`
is the only internal channel by which those explicit choices reach
`ProviderPolicy`; policy never infers them from the profile's display name.

`ModelTarget::preview_capabilities()` is a side-effect-free preview that
performs the same feature-availability and policy checks as the factory before
returning capability facts. It is fallible with
`CapabilityPreviewError::ProviderFeatureDisabled` when the selected adapter
feature is not compiled, or `CapabilityPreviewError::Policy` when a target
override is invalid for its wire protocol. It must not claim a capability the
factory would reject. It performs no credential or transport setup, is not a
provider binding, and does not expose `FacadeMetadata`; only a successful build
does. The factory resolves the same policy once for the built model, and
`ProviderModel::capabilities()` returns the factory-stored value without
re-resolving or guessing.

Compaction capability and reasoning policy are separate facts. Native
compaction routing uses the built model's typed capability; summary effort
uses the resolved typed reasoning policy. Replacing a provider-enum check with
`supports_native_compaction` must not erase the provider-specific effort
choice for local compaction.

In `PolicyMode::StableFacade`, `ModelRequest.model` must equal the validated
target model. Any other value is rejected with a typed permanent `ModelError`
before dispatch. This is deliberately stricter than family validation:
`ModelRequest` carries no capability metadata, so allowing a different id
would let a host budget against one model's context window while dispatching a
smaller-window model under the same `ModelPort`. A host that selects a
different model builds a new `ModelPort` and carries state forward explicitly.

`PolicyMode::LegacyCompatibility` preserves the current product's
same-family request-level switching until its callers migrate. That path still
re-resolves wire dialect and reasoning policy and rejects provider-family or
protocol mismatches, but it remains explicitly compatibility-only and is not
part of the stable facade promise. Neither mode changes
`ModelPort::capabilities()` during the lifetime of one built model.

### Profile

```rust
pub struct ProviderProfile {
    // Existing public fields remain during the compatibility window.
}

impl ProviderProfile {
    pub fn anthropic(model: impl Into<String>) -> Result<Self, ProfileError>;
    pub fn openai(model: impl Into<String>) -> Result<Self, ProfileError>;
    pub fn openai_responses(model: impl Into<String>) -> Result<Self, ProfileError>;
    pub fn openai_with_protocol(
        model: impl Into<String>,
        protocol: OpenAiProtocol,
    ) -> Result<Self, ProfileError>;
    pub fn deepseek(model: impl Into<String>) -> Result<Self, ProfileError>;
    pub fn kimi(model: impl Into<String>) -> Result<Self, ProfileError>;
    pub fn custom(
        name: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
        protocol: OpenAiProtocol,
    ) -> Result<Self, ProfileError>;

    pub fn with_base_url(self, base_url: impl Into<String>) -> Result<Self, ProfileError>;
    pub fn with_responses_compact_threshold(self, threshold: Option<u32>) -> Self;
}
```

The profile carries no secret. Constructors provide the common path; the
factory still validates the complete combination before construction.
`ProfileError` is a public, non-exhaustive enum for invalid names, endpoints,
and protocols. `ProviderProfile::anthropic(model)` uses the built-in base URL
`https://api.anthropic.com/v1`, so the adapter's `/messages` join produces
`https://api.anthropic.com/v1/messages`. The compatibility path canonicalizes
the documented root value `https://api.anthropic.com` to that effective base
before dispatch; a request-capture test pins this behavior.

The stable facade treats `ProviderProfile` as an immutable configuration DTO
created through constructors. Its fields remain public only for Tact's
compatibility window and are not part of the third-party stability promise.
Existing heuristic methods such as `is_kimi_with`, `supports_vision`, and
`dialect_for` remain deprecated compatibility methods that delegate to
`ProviderPolicy`.

### Facade metadata

```rust
pub struct FacadeMetadata {
    provider: ProviderKind,
    protocol: WireProtocol,
    endpoint_identity: EndpointIdentity,
    endpoint_class: EndpointClass,
}

impl FacadeMetadata {
    pub fn provider(&self) -> &ProviderKind;
    pub fn protocol(&self) -> WireProtocol;
    pub fn endpoint_identity(&self) -> &EndpointIdentity;
    pub fn endpoint_class(&self) -> EndpointClass;
}
```

`WireProtocol` is the complete closed wire set:

```rust
#[non_exhaustive]
pub enum WireProtocol {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}
```

`OpenAiProtocol` remains the configuration selector for the two OpenAI
protocols; it is not a metadata type because it cannot represent Anthropic.

`EndpointClass` distinguishes vendor-operated built-in endpoints from
third-party or local endpoints:

```rust
#[non_exhaustive]
pub enum EndpointClass {
    BuiltIn,
    Custom,
}
```

The class is resolved by `ProviderPolicy` from the canonical
`EndpointIdentity`, not from the profile display name or a substring match.
`ProviderProfile::openai_with_protocol` may point at a custom gateway and then
resolves as `Custom`; a `custom` profile pointed at an exact built-in endpoint
resolves as `BuiltIn`. The internal built-in table is derived only after each
adapter's effective path-joining behavior is characterized; an advertised
constructor URL is not automatically the adapter's request URL. In particular,
Anthropic currently appends `/messages` directly to the configured base URL,
so the exact built-in path must be proven by a request-capture test before it
is admitted. Authentication requirements use `EndpointClass`: built-in
endpoints require credentials, custom endpoints may use anonymous credentials.
Account requests remain built-in-only. Model discovery is not coupled to
`EndpointClass`: the current explicit-profile `/models` path supports custom
OpenAI-compatible providers, and `PolicyMode::LegacyCompatibility` preserves
that behavior. `EndpointClass` is metadata for routing and policy; it does not
participate in provider-state binding.

`EndpointIdentity` is an opaque, normalized endpoint fact used by Tact to bind
persisted provider conversation state. It is not a credential or a raw query
payload. Normalization rules are part of the contract:

- use the effective endpoint after applying the provider default;
- accept only absolute `http` or `https` URLs with a host;
- reject URL username/password userinfo with
  `EndpointError::EmbeddedCredentials`;
- reject a non-empty query or fragment with
  `EndpointError::QueryOrFragmentNotAllowed`, because adapters append request
  paths to the base URL and stripping routing dimensions could bind state to a
  different endpoint;
- lowercase scheme and host, remove the default port, and normalize an empty
  path to `/`;
- remove one trailing `/` only when the resulting path would remain non-empty;
  the canonical root path stays `/`, while `/v1/` becomes `/v1`;
- preserve a non-default port and the remaining path;
- preserve path case and percent-encoding exactly as parsed; do not
  case-fold or decode path segments;
- preserve IPv6 brackets and a non-default IPv6 port;
- serialize as the canonical URL string.

`EndpointIdentity` implements `Clone`, `Debug`, `Display`, `PartialEq`, `Eq`,
`Hash`, and serde serialization/deserialization. Debug and Display expose only
the normalized public endpoint, never credentials or query data.

Its public construction and parsing surface is:

```rust
pub struct EndpointIdentity(/* canonical URL string */);

#[non_exhaustive]
pub enum EndpointError {
    InvalidUrl,
    UnsupportedScheme,
    MissingHost,
    EmbeddedCredentials,
    QueryOrFragmentNotAllowed,
}

impl EndpointIdentity {
    pub fn parse(value: &str) -> Result<Self, EndpointError>;
    pub fn as_str(&self) -> &str;
}

impl FromStr for EndpointIdentity {
    type Err = EndpointError;
}
```

`parse` and `FromStr` are equivalent. Deserialization runs the same validation
and canonicalization; a non-canonical serialized value is accepted only by
normalizing it to the canonical string, never by preserving it verbatim.

Provider-state binding has one owner: `FacadeMetadata`. A persisted
`ResponsesConversationState` maps as follows during the compatibility window:

- its legacy `provider` field must equal `"openai_responses"`;
- its legacy `base_url` maps to `EndpointIdentity::parse` with the same
  normalization rules;
- `FacadeMetadata` must report `WireProtocol::OpenAiResponses` and the same
  `EndpointIdentity` before the kernel may use the state;
- the legacy model field remains informational and does not invalidate state;
- on rewrite, the legacy `base_url` receives the canonical endpoint string so
  old and new state compare identically.

Provider-state binding depends only on the Responses wire protocol and the
canonical endpoint identity. It does not depend on credential presence,
anonymous mode, or the advertised native-compaction capability. Those facts
may affect dispatch or capabilities, but changing them must not silently make
otherwise matching opaque state invalid.

The build result carries both `Arc<dyn ModelPort>` and `FacadeMetadata`, so
product code can validate provider state and compaction without downcasting
`LlmProvider`.

Metadata is produced by the validated factory after policy resolution, not by
an unbuilt `ModelTarget`, so there is no public `ModelTarget::binding()`.

Provider-state binding is optional and wire-specific. Anthropic and Chat
Completions builds return metadata for routing/telemetry, but they do not
accept `ResponsesConversationState`; only the Responses path can bind it.
An unrecognized cursor is passed through the existing compatibility conversion
and fails with the existing typed model error rather than being reinterpreted
as a different provider.

Every factory-built `ProviderModel` retains the resolved `FacadeMetadata`,
configured target model, and `PolicyMode::StableFacade`. Before every `stream`,
`complete`, or `compact` dispatch it validates that the request model equals
the configured target model and that any supplied cursor endpoint matches the
retained metadata. This is required for direct third-party callers that use
`BuiltModel::into_model()` and never enter Tact's session-loading path. The
product load-time validator is an additional early rejection over the same
shared Responses binding predicate, not a replacement for the adapter's
structural check. A compatibility `ProviderModel` constructed from a legacy
`LlmProvider` may retain `None` binding metadata and
`PolicyMode::LegacyCompatibility`; it is not a stable facade constructor.

### Factory

```rust
pub struct ProviderFactory {
    // One shared HTTP configuration and no provider state.
}

impl ProviderFactory {
    pub fn new() -> Result<Self, LlmBuildError>;

    pub fn builder() -> ProviderFactoryBuilder;

    pub async fn build(
        &self,
        target: ModelTarget,
        credentials: Arc<dyn CredentialProvider>,
    ) -> Result<BuiltModel, LlmBuildError>;

    pub async fn build_with_api_key(
        &self,
        target: ModelTarget,
        api_key: impl Into<String>,
    ) -> Result<BuiltModel, LlmBuildError>;

    pub async fn build_anonymous(
        &self,
        target: ModelTarget,
    ) -> Result<BuiltModel, LlmBuildError>;
}

pub struct ProviderFactoryBuilder {
    // Internal timeouts and shared transport configuration.
}

impl ProviderFactoryBuilder {
    pub fn read_timeout(self, timeout: Duration) -> Self;
    pub fn connect_timeout(self, timeout: Duration) -> Self;
    pub fn proxy(self, proxy: ProxyUrl) -> Result<Self, LlmBuildError>;
    pub fn user_agent(self, user_agent: impl Into<String>) -> Result<Self, LlmBuildError>;
    pub fn build(self) -> Result<ProviderFactory, LlmBuildError>;
}
```

`ProviderFactory` is immutable and cloneable. It performs one exhaustive
provider/protocol match, constructs the adapter, attaches the resolved
capabilities, and returns a `BuiltModel`.

```rust
pub struct BuiltModel {
    model: Arc<dyn ModelPort>,
    metadata: FacadeMetadata,
}

impl BuiltModel {
    pub fn model(&self) -> &Arc<dyn ModelPort>;
    pub fn metadata(&self) -> &FacadeMetadata;
    pub fn into_model(self) -> Arc<dyn ModelPort>;
    pub fn into_parts(self) -> (Arc<dyn ModelPort>, FacadeMetadata);
}
```

`ProxyUrl` is a facade-owned validated endpoint wrapper. It accepts only
absolute `http`/`https` URLs and can be constructed through `FromStr`; reqwest's
URL type is not part of the public API. `ProviderFactoryBuilder` stores the
normalized value and maps it to each private HTTP client internally. It rejects
embedded username/password userinfo and non-empty query or fragment components
with `TransportConfigError::QueryOrFragmentNotAllowed`, because proxy URLs have
no query semantics here and a query is a plausible secret carrier. Its manual
`Debug` rendering shows only scheme, host, and non-default port; path, query,
fragment, and userinfo are never rendered. Rejected input is not echoed in a
diagnostic.

`BuiltModel` implements redacted `Debug` and has no `Clone`: the `ModelPort`
and its single-use message-id counter remain one built instance. `FacadeMetadata`
and `ProxyUrl` are `Clone`, `Debug`, and `Send + Sync`. `ProviderFactory` is
`Clone + Send + Sync`; `CredentialProvider` is `Send + Sync + Debug`.
`ProviderFactory::new()` can fail only while constructing its default HTTP
configuration; missing credentials, providers, and endpoints are reported
during `build`. Static configuration errors are never deferred to the first
request.

The build methods return `Result<BuiltModel, LlmBuildError>` so callers do not
lose provider binding facts. `BuiltModel::into_model()` returns
`Arc<dyn ModelPort>` for hosts that do not need metadata. Its fields are
private so future metadata or model construction invariants cannot be bypassed;
`into_parts` is the explicit ownership transfer when both values are needed.
The stable factory-built `ProviderModel` must strip `LlmResponse.request_body`
before constructing `ModelResponse`, so a direct stable `ModelPort` caller can
never observe prompt bytes through `ModelResponse.request_body`.

Tact still needs the dispatched request body for its bounded usage store and
`/view-system-prompt`. That requirement therefore has a separate,
feature-gated product assembly path:

```rust
#[cfg(feature = "legacy-tact")]
pub struct ProductBuiltModel {
    model: Arc<dyn ModelPort>,
    metadata: FacadeMetadata,
    legacy_provider: LlmProvider,
}

#[cfg(feature = "legacy-tact")]
impl ProductBuiltModel {
    pub fn model(&self) -> &Arc<dyn ModelPort>;
    pub fn metadata(&self) -> &FacadeMetadata;
    pub fn legacy_provider(&self) -> &LlmProvider;
    pub fn into_parts(self) -> (Arc<dyn ModelPort>, FacadeMetadata, LlmProvider);
}

#[cfg(feature = "legacy-tact")]
impl ProviderFactory {
    pub async fn build_product(
        &self,
        target: ModelTarget,
        credentials: Arc<dyn CredentialProvider>,
        request_body_policy: ProductRequestBodyPolicy,
    ) -> Result<ProductBuiltModel, LlmBuildError>;
}

#[cfg(feature = "legacy-tact")]
pub enum ProductRequestBodyPolicy {
    Retain,
    Strip,
}
```

`build_product` shares the same target validation, policy resolution, adapter
construction, and metadata production as `build`, but returns the legacy
provider mirror Tact still needs during the compatibility window and can retain
request bytes for the product's bounded usage sink. The stable `build` path
always strips them and never returns the mirror. No stable caller can select
`Retain`, and the product path is excluded from the stable consumer feature
matrix.

`ProviderFactory::build`, `build_with_api_key`, and `build_anonymous` use one
internal `assemble(target, initial_credential, credential_source)` helper.
The three public methods differ only in how they obtain the initial credential:
`build` resolves the supplied provider once, `build_with_api_key` constructs an
`ApiKeyProvider`, and `build_anonymous` supplies `Credential::none()`.
Construction fails before a model is returned when a required initial
credential is unavailable. The adapter retains the credential provider and
resolves again before every dispatched request, so refresh still works.

The initial construction resolve is a validation probe only and is never
reused by the first request. For an async factory build, a counting provider
therefore records exactly one build resolve and one resolve per dispatched
request. `ProviderInfo::build_client()` takes the separate synchronous
static-key assembly path and records zero credential-provider resolves.
`Client::new()` and `get_llm_client()` resolve once for construction and then
once per dispatched request. Tests assert these counts per entry point instead
of applying one global count to every constructor.

There is no public `build_llm_client` that exposes the internal adapter enum.
Product-specific startup choices such as session id and reasoning replay are
expressed in `ModelTarget` before construction. The compatibility adapter may
still implement `set_user_id` by rebuilding its internal request defaults; the
stable `ModelPort` does not expose post-construction session mutation.

### Credentials

```rust
pub struct Credential(/* private */);

pub trait CredentialProvider: Send + Sync + Debug {
    fn resolve(&self) -> BoxFuture<'_, Result<Credential, CredentialSourceError>>;
    fn describe(&self) -> &'static str;
}

impl Credential {
    pub fn api_key(secret: impl Into<String>) -> Result<Self, CredentialError>;
    pub fn bearer(secret: impl Into<String>) -> Result<Self, CredentialError>;
    pub fn none() -> Self;
}

pub struct ApiKeyProvider { /* SecretString */ }
pub struct BearerProvider { /* SecretString */ }
pub struct NoCredentialProvider;
```

`CredentialSourceError` is part of the stable credential extension point:

```rust
#[non_exhaustive]
pub enum CredentialSourceError {
    Unavailable,
    RefreshFailed,
    InvalidCredential(CredentialError),
}
```

The trait does not expose the compatibility `LlmError`. A build-time
resolution failure maps to `LlmBuildError::CredentialSource`; a request-time
failure maps to a typed permanent authentication `ModelError` before dispatch.
The error carries no provider SDK type and no free-form secret-bearing text.

Credential resolution remains per request so expiring tokens can refresh.
Provider adapters decide whether a credential is required and how it maps to
headers. The public credential type is an opaque newtype; `secrecy` is not part
of its public shape. Debug output, build errors, runtime errors, and tracing
spans never include the secret.

Credential constructors validate the value before storing it. Empty values,
ASCII control or newline bytes, and characters illegal in an HTTP header value
return `CredentialError`. Request-time header conversion is fallible and maps
to a typed authentication error; no credential path calls `unwrap()` or
`HeaderValue::parse().expect(...)`.

`AuthRequirement` is the typed policy decision for each provider family:

```rust
pub enum AuthRequirement {
    Required(AuthScheme),
    Optional,
    Forbidden,
}

pub enum AuthScheme {
    AnthropicApiKey,
    Bearer,
    AnthropicApiKeyOrBearer,
}
```

The construction probe requires a non-`None` credential for required
families. Request-time resolution applies the same rule again, so a provider
that later yields `None` fails before dispatch. Header ownership is fixed:
`anthropic/headers.rs` owns Anthropic mapping and
`openai/{chat,responses}/headers.rs` own OpenAI mapping; no shared auth helper
chooses headers by inspecting a provider string.

The adapter mapping is explicit:

- Anthropic requires `AnthropicApiKeyOrBearer`, mapping `ApiKey` to
  `x-api-key` and `Bearer` to `Authorization: Bearer`;
- built-in OpenAI Chat and built-in OpenAI Responses require `Bearer`, mapping
  both `ApiKey` and `Bearer` to `Authorization: Bearer`;
- built-in DeepSeek and Kimi Chat Completions or Responses endpoints require
  `Bearer`, mapping both `ApiKey` and `Bearer` to `Authorization: Bearer`;
- custom Chat Completions and custom Responses endpoints are `Optional`;
- `None` omits authorization only for `Optional` endpoints, and otherwise
  returns a runtime authentication error;
- account discovery queries accept only `ApiKey`; other variants fail before
  network dispatch.

An anonymous custom Responses endpoint resolves to standard Responses policy
and retains normal Responses provider-state binding, regardless of whether the
host advertises native compaction. Anonymous status never enables vendor
account or discovery calls and never changes body-dialect detection. Every
Responses request path, including native compaction, uses the same
`AuthRequirement`/header builder: `Credential::none()` omits authorization for
an optional custom endpoint and fails before dispatch for a required built-in
endpoint.

Custom Responses uses the conservative `ResponsesCapabilities::custom_provider`
defaults: Responses and streaming are enabled, native compaction is disabled,
and hosted web search remains a protocol capability. A host may opt into native
compaction only through `ModelTarget::with_native_compaction(true)`; that flag
changes advertised capability, not endpoint detection or provider-state
binding.

`with_native_compaction(true)` is valid only for Responses targets. Anthropic
and Chat Completions targets reject it with `ModelTargetError`; otherwise a
capability the wire cannot express would appear to be enabled.
`with_native_compaction(false)` explicitly disables the capability on
Responses and is an accepted no-op for other protocols. A built model whose
capabilities report `supports_native_compaction == false` rejects
`ModelPort::compact` with a typed permanent error before polling the adapter;
it must not fall back to a normal completion implicitly.

### Errors

```rust
#[non_exhaustive]
pub enum HttpErrorClass {
    Client,
    RateLimited,
    Server,
}

#[non_exhaustive]
pub enum HttpErrorKind {
    ContextOverflow,
    Authentication,
    InvalidRequest,
    RateLimited,
    Server,
    Other,
}

#[non_exhaustive]
pub enum ProviderErrorKind {
    Transport,
    Protocol,
    Deserialization,
    UnsupportedState,
    Authentication,
    Sdk,
}

pub struct ProviderError {
    kind: ProviderErrorKind,
    status: Option<u16>,
}

pub enum MessageError {
    Provider(ProviderError),
}

pub enum LlmError {
    Message(MessageError),
    Provider(ProviderError),
    HttpError {
        status: u16,
        class: HttpErrorClass,
        kind: HttpErrorKind,
        body: HttpErrorBody,
        retry_after: Option<Duration>,
    },
    // Other body-free compatibility categories.
}

pub struct HttpErrorBody(/* private bounded bytes */);

impl HttpErrorBody {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn is_truncated(&self) -> bool;
}

#[non_exhaustive]
pub enum LlmBuildError {
    InvalidProfile(ProfileError),
    UnsupportedProtocol {
        provider: ProviderKind,
        protocol: WireProtocol,
    },
    ProviderFeatureDisabled {
        provider: ProviderKind,
        feature: ProviderFeature,
    },
    InvalidTarget(ModelTargetError),
    InvalidCredential(CredentialError),
    InvalidTransportConfig(TransportConfigError),
    CredentialUnavailable {
        provider: ProviderKind,
        requirement: AuthRequirement,
    },
    CredentialSource(CredentialSourceError),
    TransportSetup {
        stage: TransportSetupStage,
        source: String,
    },
    LegacyAssembly {
        stage: LegacyAssemblyStage,
    },
}

#[non_exhaustive]
pub enum ProfileError {
    EmptyModel,
    EmptyName,
    InvalidBaseUrl,
    UnsupportedScheme,
    MissingHost,
    EmbeddedCredentials,
}

#[non_exhaustive]
pub enum CredentialError {
    Empty,
    ControlCharacter,
    InvalidHeaderValue,
}

#[non_exhaustive]
pub enum PolicyError {
    InvalidOverride {
        field: &'static str,
        protocol: WireProtocol,
    },
    ModelFamilyMismatch {
        target: WireProtocol,
        requested: WireProtocol,
    },
}

#[non_exhaustive]
pub enum TransportSetupStage {
    Reqwest12Client,
    Reqwest13Client,
}

#[non_exhaustive]
pub enum TransportConfigError {
    InvalidProxy,
    EmbeddedProxyCredentials,
    QueryOrFragmentNotAllowed,
    InvalidUserAgent,
}

#[non_exhaustive]
pub enum ProviderFeature {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
}

#[non_exhaustive]
pub enum LegacyAssemblyStage {
    InvalidProfile,
    InvalidCredential,
    InvalidTarget,
    AdapterConstruction,
}
```

`LlmError` and `MessageError` are compatibility error carriers, but they must
not own third-party SDK error values or unbounded response text. The current
`OpenAi(OpenAIError)` and `OpenAiResponses(OpenAIError)` variants are replaced by
a body-free `Provider(ProviderError)` classification before crossing the
adapter boundary. `ProviderError` carries only typed kind/status facts and has no
`source()` pointing at an SDK error. This is required because the SDK's
`JSONDeserialize` variant and tracing path can retain or log the raw response
body even when `Display` is redacted.

`LlmBuildError` is for construction. Runtime model calls continue to return
`tact_agent_api::ModelError`; the provider adapters map transport, HTTP,
authentication, context overflow, cancellation, and protocol errors into its
closed error kinds. Public signatures do not return `anyhow::Error`.

`LlmBuildError`, `CredentialError`, and `ModelError` implement redacted `Debug`
and `Display`. A closed error category is always an enum, never a stringly
typed status. Free-form `source` text exists only for an underlying transport
construction error; it is library-generated diagnostic text and must not
contain credentials, user prompts, request bodies, or response bodies.

`LlmError::HttpError` carries a bounded body in the private
`HttpErrorBody` carrier only while computing typed classification and proving
redaction/truncation behavior inside `tact_llm`. It is not a product
diagnostic channel. The public variant contains only the safe metadata shown
above.

`HttpErrorBody` exposes length and truncation facts, not content. Its `Debug`
and `Display` render only a redacted placeholder. `LlmError`'s derived or
manual rendering shows status, class, body presence, and retry-after, never
payload text. There is no cross-crate body accessor: `tact` and `tact-ui`
cannot read the retained bytes through any public or `pub(crate)` path. Any
crate-private accessor exists only inside `tact_llm` for adapter tests and
compatibility error construction. Provider body text is intentionally not a
supported product diagnostic; product diagnostics use status, class, typed
kind, retry, length, and truncation facts only.

`HttpErrorKind` is extracted from the bounded body or typed SDK response before
the body is discarded. In particular, context-overflow phrases are classified
inside `tact_llm`, and `neutral::model_error` maps `ContextOverflow` to
`ModelErrorKind::ContextOverflow` without copying provider text into
`ModelError.message`. Product recovery uses the same typed kind instead of
re-parsing a redacted message. Client status alone is never treated as context
overflow.

Provider response bodies and request bodies never appear in public rendering
or telemetry. `MAX_HTTP_ERROR_BODY_BYTES` is 8 KiB. Every direct HTTP error path
reads at most `MAX_HTTP_ERROR_BODY_BYTES + 1` bytes from the response before
deciding whether the body is truncated, so an oversized or attacker-controlled
body is never fully allocated first. The retained compatibility body is
truncated to 8 KiB and accompanied by a truncation flag. The same redaction
applies to `MessageError` and any provider-specific HTTP error text that
reaches `ModelError.message`. Anthropic's body-bearing
`MessageError::ApiError(String)` is replaced with a structured bounded HTTP
error; SDK-native error strings are reduced to a safe classification before
they cross the compatibility boundary.

`LlmError`, `MessageError`, `HttpErrorClass`, and `HttpErrorBody` are
`legacy-tact` compatibility types, not stable SDK return types. Stable calls
return `tact_agent_api::ModelError`; only legacy-facing adapter paths may
construct the compatibility error enum. The migration must update every
in-repo `LlmError::HttpError` struct literal, including `crates/tact` recovery
tests and the Anthropic, Chat Completions, and Responses adapters.
The implementation must remove or vendor every dependency error/log path that
can retain or print response bodies. For OpenAI Responses, this includes the
pinned `async-openai-local` JSON-deserialization path; for any executed
`async-openai` path, the same rule applies. Account query failures are
structured and body-free. A captured subscriber test must include dependency
targets, not only `tact_llm` targets.

Adding `retry_after` to the public `LlmError::HttpError` struct variant is an
accepted source-breaking change for this feature branch. The plan enumerates
all in-repo constructors and tests, migrates them together, and keeps the
breaking transition out of the facade release promise.

## Provider Policy

`provider/policy.rs` is the only owner of provider wire heuristics. Tact config
may validate product policy, but it must derive that validation from
`ProviderProfile` and a shared facade query rather than maintaining its own
vendor matrix.

The exhaustive ownership list is:

- `provider/policy.rs` owns provider/model/endpoint string interpretation,
  Chat Completions dialect selection, OpenCode endpoint and request headers,
  official-endpoint recognition, vision heuristics, reasoning-mode
  classification, and native-compaction defaults;
- `types.rs` owns only typed provider/protocol labels, parsing of those labels,
  and static built-in default URLs; it may not infer vendor behavior from a
  model or endpoint string;
- `profile.rs` keeps compatibility delegators that call `ProviderPolicy`;
- product config parses user intent into typed values, applies user overrides,
  and may validate host-specific requirements, but it does not duplicate
  DeepSeek/Kimi/OpenCode/vision/endpoint heuristics;
- adapter modules serialize an already-resolved policy and never branch on
  vendor strings;
- tests and fixtures may contain literal provider/model/endpoint strings.

The policy gate is therefore a reviewed allowlist check over those owners, not
a global search for vendor words.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicyMode {
    StableFacade,
    LegacyCompatibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscoveryPolicy {
    Disabled,
    OpenAiCompatibleModels,
}

pub(crate) struct ProviderPolicy {
    pub(crate) wire: WireProtocol,
    pub(crate) vision: bool,
    pub(crate) reasoning: ReasoningPolicy,
    pub(crate) account: AccountPolicy,
    pub(crate) discovery: DiscoveryPolicy,
    pub(crate) responses: Option<ResponsesPolicy>,
    pub(crate) replay_prior_reasoning: bool,
    pub(crate) endpoint: EndpointPolicy,
}

impl ProviderPolicy {
    pub(crate) fn resolve(
        profile: &ProviderProfile,
        request_model: &str,
        overrides: ProviderPolicyOverrides,
        mode: PolicyMode,
    ) -> Result<Self, PolicyError>;
}
```

`ProviderPolicy::resolve` does not return the compatibility `LlmError`.
Factory-time target validation maps `PolicyError::InvalidOverride` to
`LlmBuildError::InvalidTarget`. Request-time resolution maps
`PolicyError::ModelFamilyMismatch` to a permanent `ModelError` before dispatch.
Feature-disabled adapters are rejected by the factory before policy resolution.

`PolicyMode::StableFacade` is used by `ModelTarget`,
`preview_capabilities()`, and `ProviderFactory` construction. In this mode,
`ProviderKind::Custom` defaults to standard Chat Completions or standard
Responses policy regardless of the custom display name, base URL, or model id;
only typed target overrides such as `with_chat_dialect` may select a vendor
dialect. `PolicyMode::LegacyCompatibility` preserves the current
base-URL/model-id substring heuristics for deprecated product callers until
they migrate to the facade. The mode is explicit at every resolution site, so
stable and compatibility behavior cannot silently diverge by feature flag.

The policy owns:

- Anthropic, Chat Completions, or Responses selection;
- standard, DeepSeek, or Kimi Chat Completions dialect;
- K2/K2.7/K3 and coding-endpoint decisions;
- reasoning effort versus thinking budget;
- vision support for internal compatibility checks; `ModelCapabilities` has no
  vision field in this phase, so product vision gating continues to use the
  typed product model metadata and does not claim that `ModelPort` expresses it;
- official endpoint eligibility for account queries;
- model-discovery eligibility, kept separate from account eligibility so the
  legacy explicit-profile custom `/models` path remains supported;
- Responses capability defaults and prior-reasoning replay;
- body-hook selection and provider-specific request headers.
- OpenCode endpoint classification, session headers, and user-agent policy.

Adapter modules request a resolved policy and serialize it. They do not inspect
vendor strings independently. `ProviderProfile` methods remain as deprecated
compatibility delegates to this policy.

Under `PolicyMode::StableFacade`, custom profiles default to the standard
OpenAI-compatible policy. An explicit `ModelTarget::with_chat_dialect` override
is the only way to opt a custom gateway into non-standard Chat Completions
policy. The policy resolves this typed override before consulting any remaining
built-in policy facts. `PolicyMode::LegacyCompatibility` retains the existing
substring inference so the staged migration does not change current Tact
dispatch before product callers are migrated.

`ProviderProfile::custom` stores the user's name as a diagnostic label only. It
must not participate in DeepSeek, Kimi, OpenCode, or coding-endpoint heuristics.
For example, a custom profile named `deepseek-gateway` remains standard Chat
Completions policy unless the target explicitly sets
`with_chat_dialect(ChatCompletionsDialect::DeepSeek)`.

`with_chat_dialect` is valid only for Chat Completions profiles. Anthropic and
Responses profiles reject it with `ModelTargetError`; a silent ignore would
make a configured DeepSeek/Kimi override appear effective when it is not.

The factory resolves the configured model policy when it validates the target.
A stable requestable adapter rejects a model id different from the target before
resolving request policy. A legacy-compatible adapter may resolve the effective
policy again from the immutable profile and request model to preserve Tact's
same-family switching during migration. The policy resolver is pure and cheap;
it owns no mutable cache.

Stable request-level model mismatch is a permanent error before dispatch, so it
cannot silently change wire policy while leaving context-window or
native-compaction facts stale. A host that needs another target rebuilds
through `ProviderFactory` and migrates session state explicitly.

## Construction Flow

```text
ProviderProfile + ModelLimits
            |
            v
       ModelTarget
            |
            v
  validate profile and limits
            |
            v
 resolve ProviderPolicy for configured model
            |
            v
 choose one adapter in ProviderFactory
            |
            v
 ProviderModel(client, configured capabilities)
            |
            v
 BuiltModel { model, metadata }
```

The factory validates at the boundary. The adapter trusts the resolved profile
and policy. The runtime path does not read process-global configuration.

The factory owns explicitly built HTTP clients and never calls
`SharedHttpClient::default()`. Provider SDK clients that accept a custom HTTP
client receive the factory's configured client. For the Anthropic path, which
owns a reqwest 0.12 client, the builder applies the same timeout/proxy/user-agent
settings and maps construction failures to `LlmBuildError::TransportSetup`.

`ProviderInfo::build_client()` remains a synchronous compatibility constructor.
It uses the static API-key snapshot already stored in `ProviderInfo` and one
shared synchronous adapter-assembly helper. It does not resolve an async
credential provider, create a nested runtime, call the async factory, or block
on it. The helper shares target validation, policy resolution, static
credential validation, and adapter construction with `ProviderFactory`, so
provider branching still has one owner. Its existing
`anyhow::Result<LlmProvider>` signature remains source-compatible: typed
`LlmBuildError` values are mapped into `anyhow::Error` only at this compatibility
boundary and never become the stable facade signature.

`Client::new()` and `get_llm_client()` remain asynchronous because they take a
`CredentialProvider` and must resolve its initial credential before returning.
They call the async factory path. All three compatibility entry points delegate
to the same provider decision and adapter-assembly code but use different
initial-credential adapters; they do not all call the async factory directly.
Existing synchronous callers of `ProviderInfo::build_client()`, including tests
running inside a Tokio runtime, continue to succeed.

The compatibility and product migration contract is concrete:

- `AgentRuntime` keeps its legacy `client: LlmProvider` only through the
  compatibility wrapper; the kernel-facing `TactModelPort` becomes a thin owner
  of `Arc<dyn ModelPort>`.
- `TactKernelConfig` stores `ProductBuiltModel` or separately stores its
  `Arc<dyn ModelPort>`, `FacadeMetadata`, and legacy compatibility mirror; it
  never reconstructs metadata from `LlmProvider` variants.
- Session persistence stores a versioned typed `SessionModelMetadata` derived
  from `FacadeMetadata`. The initial implementation reuses the existing
  `responses_states` row, adding a nullable `facade_metadata_json` column and
  keeping `provider`, `base_url`, `model`, and `state_json`. The existing
  `schema_version` column remains the provider-state version; the JSON carries
  a separate `SessionModelMetadata::schema_version`.
- The migration does not claim that old rows can be fully reconstructed. A
  pre-migration row has no provider kind, session-metadata schema version, or
  endpoint class, and `ResponsesConversationState.provider` is only the legacy
  marker `"openai_responses"`. Migration adds the nullable column and leaves
  every existing row `NULL`; it never fabricates provider or protocol
  metadata.
- Loading a `NULL` row requires the legacy provider marker and a canonical
  `base_url` that matches the current `FacadeMetadata` under the shared
  provider-state binding predicate. That is a one-way compatibility read, not
  evidence that the row originally belonged to the same provider kind;
  indistinguishable rows from different providers are accepted or rejected
  solely by the endpoint/wire rule.
- The first successful state save writes the current versioned
  `SessionModelMetadata`, including provider, protocol, endpoint identity, and
  endpoint class. A later load validates that JSON and rejects unsupported
  schema versions or mismatched endpoint identity before dispatch. Metadata is
  written in the same transaction as provider state and transcript
  replacement. Tests cover a pre-migration row from an indistinguishable
  provider, first-save upgrade, version rejection, canonical endpoint rewrite,
  and one-transaction rollback.
- The product session-store contract exposes the nullable stored metadata
  without changing `agent_api::TranscriptStore`:

```rust
pub struct ProviderStateSnapshot {
    pub state: ProviderConversationState,
    pub metadata: Option<SessionModelMetadata>,
}

trait LoadProviderStateSnapshot {
    async fn load_provider_state_snapshot(
        &self,
        session_id: &str,
    ) -> Result<Option<ProviderStateSnapshot>>;
}
```

  `SessionStore` adds this product-specific method. `SqliteTranscriptStore::load`
  calls it, validates the snapshot against its injected active metadata using
  the shared binding predicate, and only then returns a cursor. A `NULL`
  metadata row performs the legacy-marker/endpoint compatibility check; a
  present row checks the metadata schema version and endpoint identity.
  Unsupported versions and mismatches become `StoreError::Load` before the
  cursor reaches the kernel.
- The `agent_api::TranscriptStore` contract remains provider-neutral and does
  not gain `tact_llm` types. The product `SqliteTranscriptStore` owns the
  active `Option<SessionModelMetadata>` snapshot supplied when it is
  constructed and passes it only when translating
  `CursorUpdate::Replace { cursor: Some(_) }` into the product-specific
  session-store transaction. Product runtime composition must construct the
  adapter after the active model metadata is known and inject it through the
  adapter constructor; there is no hidden global lookup.
  `CursorUpdate::Replace { cursor: None }` clears the provider-state row and
  its metadata; `CursorUpdate::Unchanged` writes neither and therefore does not
  perform the nullable-row upgrade. The first nullable-row upgrade occurs on
  the first actual provider-state replacement after load, including a
  first-save path where the transcript is otherwise unchanged. Tests cover all
  three cursor variants and rollback. On load, a legacy row returns
  `None` metadata; a new row returns the injected versioned metadata so the
  product can validate it against the current model snapshot.
- `Agent::set_model` becomes an async, fallible product operation with an
  internal prepare/commit split. It first resolves the requested product model
  profile and limits into a `ModelTarget` and builds a replacement
  `ProductBuiltModel`; only after a successful build does it atomically publish
  one `Arc<dyn ModelPort>`, `FacadeMetadata`, product vision metadata, and the
  legacy compatibility mirror. Any build, credential, or binding failure
  leaves the previous snapshot and model settings untouched and is reported to
  the caller/TUI.
- The TUI may not optimistically write session config or the status bar before
  that prepare succeeds. `/model` sends a request-correlated
  `ModelSwitchRequest`; the agent/driver replies with a correlated
  `AgentUpdate::ModelSwitchResult`. Only `Applied` responses update config,
  persistence prompts, and the status bar. `Rejected` responses carry a typed
  safe reason and leave all three unchanged. This command/ack path is part of
  `tact_protocol` and `agent_tui_kit`; changing only `tact-ui::driver` is
  insufficient.
- For a same-window model switch, the product must still make the wire model
  change. Rebuilding the model is the default mechanism; changing only
  `AgentSettings.model` is rejected by a behavior test.
- The publish step updates the single activation-scoped model snapshot. A
  running activation keeps the snapshot it started with; the next activation
  observes the replacement. The legacy `AgentRuntime.client` compatibility
  mirror is synchronized for old callers, and new subagents inherit the new
  snapshot. The compactor's model facts come from the same snapshot rather than
  a separately mutable field.
- Product legacy-compatible Responses targets must explicitly preserve the
  current native-compaction routing during migration. The product resolver
  calls `with_native_compaction(true)` for Responses targets other than
  DeepSeek, including Kimi and custom Responses endpoints. Stable custom
  targets still default to native compaction disabled. Tests pin OpenAI, Kimi,
  and custom Responses to native routing and DeepSeek Responses to local
  summary routing before and after the migration.
- Vision remains product metadata rather than a `ModelCapabilities` field. The
  active product model snapshot carries the resolved `supports_vision` fact
  (including the per-model config override) into each activation's
  `ToolContext`; `read_image` and the UI image-attachment gate read that
  snapshot. They must not call `tact_llm::supports_vision()` or re-read mutable
  global config during a turn.
- Account and model-discovery callers receive the product-owned
  `TactProviderContext` explicitly. The context contains the credential-free
  `ProviderProfile`, a refreshable `CredentialProvider`, and the shared
  transport. Account and discovery adapters use their explicit `_for` functions;
  no new code calls the legacy process globals. `tact_llm` keeps the old
  global functions only behind `legacy-tact` for an explicitly reviewed
  migration window.
- The kernel keeps using `ModelPort::capabilities().model` for the current
  built target. Product code that wants `/model` to actually change the wire
  model must rebuild/swap the model or extend the kernel request contract in a
  separate approved design. Mutating only `AgentSettings.model` is not a
  complete implementation.
- `TactModelPort::from_provider` remains a compatibility constructor for
  tests; new product composition uses the explicit factory and passes
  `Arc<dyn ModelPort>` plus its metadata.
- A model switch that changes the context window must preserve the logical
  transcript and cursor rules while rejecting incompatible provider state
  before the next dispatch.
- `validate_provider_state_binding` becomes a product method over
  `FacadeMetadata`; it must not downcast `LlmProvider`. The adapter keeps a
  structural endpoint check at every direct `ModelPort` dispatch, while the
  product validator reuses the same shared binding predicate for load-time
  rejection. These are two call sites over one predicate, not two competing
  implementations.

`ProviderInfo::build_client()` is synchronous today and remains synchronous on
its success path. It validates and assembles from the static `api_key` snapshot
without an async credential resolve. It behaves identically whether called
inside or outside a Tokio runtime; it never creates or blocks a nested runtime.
The plan verifies both runtime contexts and asserts that the static key reaches
the provider header path without a credential-provider call.

`ProviderModel` and `LlmProvider` remain compatibility types during migration,
not recommended third-party constructors. New callers receive
`Arc<dyn ModelPort>`. `ProviderModel` is not a stable facade export: its
constructor accepts the legacy `LlmProvider` enum and would expose adapters
that the stable API intentionally hides. Internal factory construction may
still return the concrete type before erasing it into `Arc<dyn ModelPort>`.

## Tracing

The `telemetry` feature adds `tracing` spans in the provider factory and
adapter call paths. The host installs the subscriber and exporter.

Required span fields:

- provider kind, protocol, and model id;
- request kind: stream, complete, compact, or account query;
- endpoint host without credentials or query parameters;
- status class, error kind, retry-after value when present, and elapsed time;
- time to first stream event and total stream time.

Prompts, response content, API keys, authorization headers, and provider
conversation payloads are never recorded by library telemetry. Existing
content-bearing debug/warning calls must be removed or reduced to metadata,
lengths, status codes, and error classes before the facade is declared stable.
The baseline cleanup belongs to Task 4, not only the final telemetry task.
There is no global tracing callback or subscriber installation inside the
library. Library logs never include request or response bodies, tool arguments,
or raw SSE frames at any level. `request_body` remains internal compatibility
diagnostic data owned by the product usage store: only the feature-gated
`build_product(..., Retain)` path may populate it, the stable facade path
always strips it, and neither path exposes it through stable logging.

## Feature Matrix

| Feature | Enables | Notes |
| --- | --- | --- |
| `anthropic` | Anthropic adapter and reqwest 0.12 stack | Optional |
| `openai-chat` | Chat Completions adapter | Includes OpenAI, DeepSeek, Kimi, custom |
| `openai-responses` | Responses adapter | Includes provider state |
| `openai` | Both OpenAI protocol modules | Convenience |
| `account` | Balance and usage quota APIs | Implies `legacy-tact`; not required for model calls |
| `model-discovery` | `/models` discovery and cache | Implies `legacy-tact`; not required for model calls |
| `telemetry` | tracing instrumentation | Subscriber remains host-owned |
| `test-support` | `MockClient` and deterministic fixtures | Not enabled by default |
| `legacy-tact` | Deprecated Tact compatibility API | Transitional |

The Cargo feature graph makes the compatibility dependency explicit:

```toml
account = ["legacy-tact"]
model-discovery = ["legacy-tact"]
```

Enabling either compatibility feature without `legacy-tact` is not a supported
configuration. Tests cover both features independently and together, and no
stable-only build exports their symbols.

Transitional defaults remain broad so the current workspace keeps building:

```toml
default = [
  "anthropic",
  "openai",
  "account",
  "model-discovery",
  "telemetry",
  "legacy-tact",
]
```

Third-party documentation uses `default-features = false` and explicitly
selects one provider family. A later major release can remove `legacy-tact`
and the broad compatibility modules from defaults. Current integration tests
enable `test-support` through dev-dependencies; production users never receive
mock adapters by default.

The facade symbol contract is feature-aware:

| Symbol | Required feature | Behavior when unavailable |
| --- | --- | --- |
| `ProviderProfile::anthropic` | always | Adapter build requires `anthropic` |
| `ProviderProfile::openai`, `deepseek`, `kimi`, `custom` | always | Selected OpenAI adapter build requires its protocol feature |
| `ProviderProfile::openai_responses`, `openai_with_protocol` | always | Responses build requires `openai-responses` |
| `OpenAiProtocol` | always | Compatibility profile field type; adapters are gated |
| `ChatCompletionsDialect` | always | Target construction remains available; Chat adapter is gated |
| `ProviderKind` | always | Remains a metadata/identity enum |
| `WireProtocol` | always | Remains a closed metadata enum |

The factory always exists. If a target selects a disabled provider protocol,
`build` returns `LlmBuildError::ProviderFeatureDisabled` instead of exposing a
feature-dependent provider constructor. Every facade example compiles in its
declared feature configuration. Constructors stay type-visible because the
compatibility `ProviderProfile` fields still use `OpenAiProtocol` and
`ChatCompletionsDialect`; feature gating governs adapter availability and
typed build rejection, not the DTO's type names.

`MockClient` is a `test-support` API, but `tact-ui` and `tui` currently compile
library code that references it. They must gain matching `test-support`
features and use `#[cfg(any(test, feature = "test-support"))]` at every
production-facing use. `tact-ui`'s ordinary library build must not depend on
the mock; its dev/test builds enable the feature explicitly. `tact` already has
such a feature and follows the same rule. The gate builds both the ordinary
library and the test-support library, so a dev-dependency-only change cannot
mask a broken production build.

`account` and `model-discovery` are transitional product compatibility
features. They remain available only through the `legacy-tact` surface while
their global credential and model-cache dependencies exist. The stable
third-party facade covers model construction, calls, streams, cancellation,
compaction, and tracing. A future explicit account/catalog service can promote
those APIs once they no longer depend on legacy globals.

## Runtime and Transport

`ProviderFactoryBuilder` configures connection timeout, read timeout, and
optional proxy and user-agent values. The factory builds fallible clients for
both reqwest 0.12 and reqwest 0.13. HTTP clients remain private because the
Anthropic adapter and the OpenAI SDKs use different generations.

HTTP errors preserve `retry_after` at the adapter boundary whenever the
underlying response exposes headers. The adapter parses the `Retry-After`
header (delta seconds or an HTTP date), maps it into `ModelError.retry_after`,
and never derives it from response text. A malformed header is treated as
absent, while the status still determines the error kind. The kernel's existing
transient-retry loop consumes the value; adapters must not replace it with a
locally computed delay.

The implementation adds `httpdate` as a direct `tact_llm` dependency for the
HTTP-date branch; relying on an undeclared transitive crate is not acceptable.
Delta-seconds and HTTP-date parsing share one helper so every direct adapter
applies identical rules.

The direct reqwest paths (Anthropic, Chat Completions stream/complete, and
Responses native compaction) are required to propagate `Retry-After`. The
current `async-openai-local` error type drops response headers before
`tact_llm` can observe them, so SDK-backed Responses stream/complete calls
report `retry_after = None` in this phase. The SDK performs its own retry
middleware, and the adapter does not invent a delay. This limitation is
documented in the stable error contract and telemetry as a safe absence, not
as evidence that the provider omitted the header.

The Git dependency is pinned by immutable revision in the workspace manifest:

```toml
async-openai-responses = {
  package = "async-openai-local",
  git = "https://github.com/rust-infra/async-openai.git",
  rev = "12488eb3f6f2dbf35082a1121f2192dd1c2a7c27",
  version = "0.41.3",
  features = ["responses", "byot"],
}
```

Changing that revision is a reviewed dependency change with its own source
proof. The implementation does not modify or publish an external repository as
a side effect of the `tact_llm` refactor. A clean consumer fixture runs with
`--locked`, so a path or Git consumer sees a reproducible resolution.

This phase does not expose a transport trait. A host that needs custom mTLS,
an in-process test transport, or a non-reqwest stack can implement
`tact_agent_api::ModelPort` itself. Adding a transport abstraction is deferred
until both SDK-backed OpenAI adapters can use it without retaining a second
parallel HTTP path.

## Compatibility Surface

The following stay available during migration but are deprecated and emit
rustdoc warnings:

- `ProviderInfo`;
- `Client`;
- `LlmProvider` and its enum dispatch;
- `get_provider`, `read_provider`, and `get_llm_client`;
- global `init_provider` functions;
- `CreateMessageParams` and `LlmClient`;
- public `neutral`, `convert`, `hook_select`, `inject`, `lock`, `opencode`,
  `openai::compatible`, and `openai::responses` implementation modules.

Compatibility code may use process globals. The new factory path must not.
Compatibility functions delegate to the factory and add no provider policy.

The old `openai/compatible` module path remains the compatibility implementation
path in this phase. The internal Chat Completions ownership target is
`openai/chat/`, but the design does not rename the path until compatibility
tests and callers are migrated. Any reference in this document to
`openai/chat` means the internal destination after that migration, not a
second public module.

`ModelPort`, `ModelRequest`, `ModelEvent`, `ModelError`, and `CancelToken` are
re-exported from `tact_agent_api` so direct callers use the same types as the
agent SDK.

## Internal Decomposition

The Responses implementation is split by owned knowledge, not by execution
stage:

- `adapter.rs`: lifecycle and `LlmClient`/`ModelPort` boundary;
- `request.rs`: request policy and wire request assembly;
- `events.rs`: SSE decoding and unknown-event preservation;
- `stream.rs`: incremental response assembly;
- `normalize.rs`: terminal response and output normalization;
- `state.rs`: cursor, conversation state, and compaction state;
- `headers.rs`: authorization and provider-specific headers;
- `history.rs`: historical item conversion shared by request and normalization;
- `capabilities.rs`: Responses capability facts;
- `request_options.rs`: typed Responses request options;
- `wire.rs`: wire-only DTOs and unknown-item preservation.

The root Responses module becomes a private re-export surface. Its tests move
beside the module they protect or into `tests/`. A source file that still
exceeds 1,000 lines must be split by owned responsibility. The mechanical
`provider.rs -> provider/mod.rs` move is step one, not the final layout:
construction moves to `provider/factory.rs`, globals and compatibility
functions move to `provider/legacy.rs`, and `provider/mod.rs` becomes a narrow
re-export/type surface. No final tree retains both `provider.rs` and
`provider/mod.rs`. Fixtures remain unchanged.

## Migration

1. Freeze wire and stream behavior with characterization tests.
2. Add `ProviderPolicy`, `ModelTarget`, credentials, build errors, and the new
   async factory without removing current APIs.
3. Migrate all credential consumers in `anthropic`, `openai/compatible`,
   `openai/responses`, `account`, and `models` to the new credential variants.
4. Route `ProviderInfo::build_client`, `Client::new`, and `get_llm_client`
   through the same factory, with an explicit non-blocking compatibility rule.
5. Add the curated facade and compile-only third-party fixtures.
6. Move Tact production composition to the explicit async factory path after
   the session id is known.
7. Replace provider-state validation and native-compaction enum checks with
   facade metadata and typed capabilities.
8. Move tests and remaining callers off legacy globals and direct adapter
   construction.
9. Split the Responses implementation without changing serialized behavior.
10. Add feature gates, then hide internal modules and keep deprecated
    forwarding paths for one release window.
11. Remove the compatibility surface only in a planned major release, and only
    after the registry dependency graph is ready.

Each step leaves the workspace buildable. No extraction step changes provider
wire behavior.

The `ProviderPolicy` migration has one mechanical prerequisite: move
`crates/tact_llm/src/provider.rs` to `crates/tact_llm/src/provider/mod.rs`
first, with the existing content unchanged and all imports updated. Only then
may `provider/policy.rs` and `provider/factory.rs` be added. Creating a
`provider/` directory while `provider.rs` still exists is outside the plan.
The verbatim move is a temporary compile checkpoint; the same task removes the
old file and splits construction and legacy globals out before its gate passes.

## Synthesis Decision

Two structures were explored:

- a single `tact_llm` crate with a deep facade and compatibility layer;
- a facade plus `tact_llm_core`, `tact_llm_anthropic`, `tact_llm_openai`, and a
  transport crate.

The single-crate design is the base. It solves the primary third-party
problems now: one explicit constructor path, no global initialization,
credential and tracing seams, narrow imports, typed errors, and isolated
provider policy. Feature gates can keep unrelated protocol dependencies out of
a third-party build.

The split design is a valid future option, especially if provider families need
independent release cadence or a transport abstraction becomes viable. It is
not the first step because it adds package and semver boundaries before the
provider policy and facade are stable, and because the current SDK-backed
adapters still own reqwest clients internally.

Grafts retained from the split design:

- the `ProviderFactory` and validated profile/target boundary;
- the rule that only one policy module may interpret vendor and endpoint
  strings;
- compile-only third-party fixtures;
- a feature matrix with protocol selection;
- a compatibility facade that delegates instead of reimplementing.

## Distribution and Semver

Once the dependency graph is registry-safe, package metadata will include
description, repository, license, README, keywords, categories, and rustdoc
feature notes. Until then, "third-party SDK" means consumable by path or Git
with the stable facade below.

The stable API is:

- `ProviderProfile`, `ModelTarget`, `ModelLimits`, `ModelTargetError`, and
  `ProfileError`, `CapabilityPreviewError`;
- `ProviderFactory`, `ProviderFactoryBuilder`, and `LlmBuildError`;
- `Credential`, `CredentialProvider`, `CredentialSourceError`,
  `ApiKeyProvider`, `BearerProvider`, `NoCredentialProvider`, and
  `CredentialError`;
- `BuiltModel`, `FacadeMetadata`, `EndpointIdentity`, and `EndpointError`;
- `WireProtocol`, `EndpointClass`, `ProxyUrl`, `AuthRequirement`,
  `AuthScheme`, `PolicyError`, and `ProviderFeature`;
- `ProviderKind`, `OpenAiProtocol`, and `ChatCompletionsDialect` because they
  appear in directly callable constructor, accessor, or error signatures;
- the required `tact_agent_api` model contracts: `CancelToken`,
  `ModelCapabilities`, `ModelCursor`, `ModelError`, `ModelEvent`, `ModelPort`,
  `ModelRequest`, and `ModelResponse`.

`TransportSetupStage`, `TransportConfigError`, and `LegacyAssemblyStage` are
nameable because they appear inside the non-exhaustive `LlmBuildError`, but
they are construction internals rather than extension points. `LlmError`,
`MessageError`, `HttpErrorClass`, and `HttpErrorBody` stay on the
`legacy-tact` compatibility surface and are not stable SDK return types.

Compatibility modules and types are documented as deprecated. Before the first
public release, they may still be hidden without a semver promise. After the
first release, removing them requires a major version.

`tact_agent_api` must be published or otherwise made available before
`tact_llm`. `tact_llm` and `tact_agent_sdk` remain independent packages; neither
depends on the other.

## Alternatives

### Publish the current crate with a shorter README

Rejected. It leaves globals, duplicate factories, public internals, and
provider policy drift intact.

### Split every protocol into its own crate immediately

Rejected for this phase. It increases dependency coordination and semver work
without first stabilizing the shared facade. It also risks copying shared
DeepSeek/Kimi policy into separate packages.

### Make `tact_llm` depend on `tact_agent_sdk`

Rejected. The provider layer implements a port and the SDK composes the
runtime. Reversing that dependency would create a cycle and pull provider HTTP
stacks into every SDK host.

### Expose `LlmProvider` as the public constructor

Rejected for new users. It exposes enum dispatch, provider-specific mutation,
and internal builder types. `Arc<dyn ModelPort>` is the stable seam.

### Add a runtime provider registry

Rejected. Static constructor injection already supports built-in and custom
providers. A registry adds lifecycle and shared state without a user need.

## Tradeoffs

- We accept a transitional compatibility facade in exchange for staged
  migration of the Tact product and tests.
- We accept an explicit `ModelLimits` argument in exchange for capabilities
  that are facts rather than guesses.
- We accept both reqwest generations during the compatibility window in
  exchange for not rewriting every streaming adapter at once.
- We accept broad default features temporarily in exchange for existing
  workspace compatibility; third-party examples opt into a smaller set.
- We accept one larger provider crate in exchange for one owner of shared
  provider policy and a single SDK package for third parties.
- We accept a Tokio-only v1 and builder-level transport settings in exchange
  for avoiding an unstable transport abstraction and a third HTTP stack.
- We accept that a truly custom wire protocol implements `ModelPort` and
  composes it directly; this is extension through a trait, not provider
  registration.

## Resolved Decisions and Remaining Risks

Resolved for this phase:

- The first public release keeps `legacy-tact`; removing it happens only after
  all workspace callers migrate and is not a prerequisite for this design.
- `ProviderInfo::build_client()` stays synchronous and succeeds from its static
  API-key snapshot both inside and outside a Tokio runtime, without a nested
  runtime.
- `ProviderFactory::new()` remains fallible only for default HTTP transport
  construction; all profile, target, credential, and provider-feature errors
  are reported by `build`.
- Custom Chat Completions and custom Responses endpoints accept anonymous
  credentials; built-in Anthropic, OpenAI, DeepSeek, and Kimi endpoints require
  authentication.
- `EndpointIdentity` uses the normalization rules above and is the sole
  provider-state binding owner.
- `with_chat_dialect` is valid only for Chat Completions and rejects other wire
  protocols.
- `/model` switches that change capabilities rebuild the `ModelPort`; the
  kernel's current request contract is not silently reinterpreted.
- Stable facade requests must use the configured target model; unrestricted
  same-family request-level switching is confined to `legacy-tact`.

Remaining risks:

- Can `async-openai-local` accept the same configured reqwest 0.13 client for
  Chat Completions and Responses? If not, the factory must configure each
  adapter separately while keeping the public API unchanged.
- The minimum supported Rust version is currently unstated. Publishing requires
  a tested MSRV or an explicit policy that matches the workspace.
- Account queries and model discovery remain product compatibility features.
  Their process-global entry points are confined to `legacy-tact`; migrated
  Tact callers use the explicit product context and the existing `_for`
  functions instead.

## Verification

- Existing fixture and wire-shape tests remain green before and after the
  refactor.
- Public API tests compile the three usage paths using only facade imports.
- A provider-policy matrix covers every built-in provider, custom endpoints,
  stable model-mismatch rejection, legacy request-level switches, reasoning
  modes, vision, Responses, and account eligibility.
- Wire and header goldens are checked into the repository; a clean checkout can
  reproduce them without files from `target/`.
- Credential tests cover static keys, refreshable providers, anonymous local
  endpoints, failures during resolution, constructor validation, exact resolve
  counts, and secret redaction.
- Endpoint tests cover query/fragment rejection in addition to userinfo
  rejection and canonical equivalence.
- Auth requirement tests cover Anthropic, OpenAI, DeepSeek, Kimi, and custom
  Chat/Responses endpoints, including anonymous custom endpoints.
- Stream tests cover cancellation before dispatch, during first-event wait,
  during deltas, and after terminal completion. They also force both outcomes
  of the terminal race for success and provider failure: cancellation observed
  before the outcome claim must yield only `Cancelled`, while a `Finished` or
  `Failed` outcome that claims first remains the sole terminal event. Controlled
  channels/barriers establish provider-first and cancellation-first ordering,
  and a simultaneously-ready case deterministically chooses `Cancelled`. The
  contract is: every stream has exactly one terminal `Finished`, `Failed`, or
  `Cancelled` outcome; the next poll after any terminal item returns `None`;
  no item follows a terminal item; dropping a not-yet-polled stream performs
  no dispatch, and dropping a live stream cancels its producer task, which must
  not outlive the next poll cycle.
- Complete and compact check cancellation before polling the adapter future,
  so an already-cancelled token never dispatches work.
- Tracing tests prove spans exist, durations are measured, content is absent
  by default, and no subscriber is installed by the library.
- Error tests prove `Retry-After` becomes `ModelError.retry_after` on every
  direct HTTP path, malformed values are absent, the kernel consumes the
  delay, SDK-backed header absence is explicit, and provider bodies are
  redacted from `Display`/`Debug`.
- Recursive error-chain tests inspect `Debug`, `Display`, `source()`, and direct
  variant matching. No SDK error or response-body marker is recoverable.
- A context-overflow HTTP 400 becomes `ModelErrorKind::ContextOverflow` through
  the typed `HttpErrorKind`, without copying provider text into
  `ModelError.message`.
- Dependency-target subscriber tests cover malformed OpenAI Responses payloads
  and account-query failures; no dependency log or error contains the body.
- Provider-policy and redaction checks are checked-in scripts or Rust tests,
  not manually reviewed prose scans.
- Feature tests compile Anthropic-only, Chat-only, Responses-only, telemetry,
  account, and no-provider configurations.
- Feature-to-symbol fixtures prove `OpenAiProtocol` and
  `ChatCompletionsDialect` stay type-visible while unsupported adapters return
  typed `ProviderFeatureDisabled`.
- Preview parity tests prove `preview_capabilities()` and factory build agree
  on disabled provider features and policy-invalid targets.
- An independent external consumer fixture uses `tact_llm` with
  `default-features = false`, builds with `--locked`, and resolves the
  Responses Git dependency at the pinned immutable revision; in-crate examples
  are not treated as proof of an external consumer build.
- `cargo doc -p tact_llm --no-deps` contains no broken intra-doc links. A
  `trybuild`-style external compile-fail fixture proves that `wire` types,
  adapter modules, body hooks, and normalization helpers are not nameable
  through the stable facade; `#[doc(hidden)]` is not accepted as proof.
- Session-store tests prove transcript, provider cursor, and versioned facade
  metadata commit or roll back together; `CursorUpdate::Unchanged` performs no
  provider-state or metadata write.
- The real `SqliteTranscriptStore::load` path returns a
  `ProviderStateSnapshot`, validates matching session metadata before exposing
  the cursor, accepts the bounded legacy `NULL`-metadata path only for a
  matching legacy marker/endpoint, and rejects metadata-version or endpoint
  mismatches.
- Product construction uses `build_product(..., Retain)`; a unique
  system-prompt marker persists through the bounded usage store,
  `load_latest_request_body`, and `/view-system-prompt`. Stable `build` strips
  the same marker and no public error or telemetry exposes it.
- `/model` request/ack tests prove a rejected switch leaves the active agent
  snapshot, global config, status bar, and persistence prompt unchanged; only
  a correlated `Applied` result commits them.
- Product Responses tests prove OpenAI, Kimi, and custom targets keep native
  compaction routing while DeepSeek uses local summary routing; stable custom
  Responses remains disabled by default.
- Activation tests prove `read_image` and the image-attachment gate consume the
  activation vision snapshot and do not observe a later global config change.
- Product caller-inventory tests prove account and model discovery use
  `TactProviderContext` plus explicit `_for` functions, and no unmigrated
  production path calls a legacy provider initializer or accessor.
- The spec and plan pass a Markdown structure check for balanced code fences
  and valid relative links.

## Definition of Done

- A new third-party host can build a model and pass it to
  `tact_agent_sdk::AgentBuilder` without calling a global initializer.
- Static API key, custom endpoint, anonymous local endpoint, refreshable
  credential, and direct `ModelPort` usage compile and run.
- One policy module resolves every provider/model/endpoint heuristic.
- Current Tact behavior is protected by characterization tests.
- Legacy APIs delegate to the factory and contain no duplicate provider
  branching.
- Requests, streams, compaction, account queries, cancellation, and tracing
  pass focused tests and the repository gate.
- The external consumer fixture and feature-matrix checks pass with `--locked`;
  the workspace and fixture resolve `async-openai-responses` at the reviewed
  immutable Git revision.
- `ProviderInfo::build_client()` succeeds in and out of Tokio and rejects no
  current synchronous caller.
- Retry delays from `Retry-After` flow through `ModelError.retry_after` into the
  kernel's existing transient-retry path on every direct HTTP adapter; the
  SDK-backed Responses absence is explicit and cannot invent a delay.
- Context overflow, authentication, and transient HTTP failures retain typed
  classifications after body redaction.
- No third-party SDK error, dependency log, account error, or source chain
  exposes a response body.
- Product migration has no remaining `LlmProvider` downcast for provider-state
  validation or native-compaction selection.
- Transcript, provider cursor, and session model metadata commit atomically
  with explicit handling for unchanged and cleared cursor updates.
- Session provider metadata has a versioned SQLite representation, a nullable
  pre-migration read path that does not fabricate facts, first-save upgrade,
  version rejection, and one-transaction rollback tests.
- `SqliteTranscriptStore::load` validates the real
  `ProviderStateSnapshot` before returning a cursor; matching metadata loads,
  while endpoint and metadata-version mismatches fail before dispatch.
- Product construction preserves the bounded dispatched request body through
  `build_product(..., Retain)` for usage accounting and `/view-system-prompt`;
  stable `build` strips it, and neither path exposes it through errors or
  telemetry.
- `Agent::set_model` has prepare/commit behavior: failure preserves the old
  model snapshot, and success is visible on the next activation.
- `/model` uses a correlated request/result. A rejected switch leaves the
  agent snapshot, global config, status bar, and persistence prompts unchanged.
- Product Responses routing preserves native compaction for OpenAI, Kimi, and
  custom endpoints while DeepSeek continues to use local summary routing.
- `read_image` and the UI image gate consume the activation-scoped vision fact,
  not mutable global config.
- Product account and discovery paths use explicit `TactProviderContext` and
  `_for` functions, with no process-global provider access.
- Registry publication remains explicitly outside this phase's completion gate
  and is tracked as a release-owner checklist.

## Next Implementation Step

Add `ProviderPolicy`, `ModelTarget`, `ModelLimits`, `LlmBuildError`, and the
explicit `ProviderFactory` while preserving every current public API.
