# Tact LLM SDK Implementation Plan

> **For agentic workers:** Follow the tasks in order. Keep each task
> independently reviewable and do not start the next phase until its gate is
> green. Use checkbox syntax (`- [ ]`) to track implementation.

**Goal:** Make `tact_llm` a third-party-ready provider facade that returns a
`ModelPort` plus binding metadata, has one provider-policy owner, supports
explicit credentials and optional tracing, and preserves current Tact behavior
during a staged migration.

**Architecture:** Keep one `tact_llm` crate in this phase. Add a curated facade,
a validated `ModelTarget`, one `ProviderFactory`, and one internal
`ProviderPolicy`. Route legacy constructors through one shared assembly path:
async credential-aware entry points use the factory, while
`ProviderInfo::build_client` uses a synchronous static-key adapter over the same
target, policy, and construction code. Move wire-heavy code behind private
modules and feature gates without changing serialized behavior.

**Tech stack:** Rust 2024, existing `tact_agent_api`, async-trait, Tokio,
serde, secrecy, tracing, existing async-openai forks, wiremock, and checked-in
wire/SSE fixtures.

**Spec:**
[`docs/superpowers/specs/2026-10-28-tact-llm-sdk-design.md`](../specs/2026-10-28-tact-llm-sdk-design.md)

**Current status (2026-10-28):** proposed; no implementation started.

The `target/` directory is machine-local build output. Characterization
goldens, public API inventories, and external-consumer fixtures must be
checked-in repository artifacts so CI and a fresh checkout can reproduce every
gate.

## Global Constraints

- Do not commit or push unless explicitly requested.
- Work on a feature branch. Never commit to `main`.
- Keep small, reviewable diffs and leave the workspace buildable after every
  task.
- Run one Cargo invocation at a time.
- Preserve existing request JSON, headers, SSE handling, response
  normalization, provider state, account APIs, and model discovery behavior.
- Keep `tact_agent_api::ModelPort` as the runtime seam. Do not add a competing
  public model-call trait.
- Keep plugin style static: constructor injection and explicit composition,
  never a runtime provider registry.
- Do not expose wire, transport SDK, body-hook, lock, or normalization types
  from the stable facade.
- Do not make the new factory path read process-global provider or credential
  state.
- Do not claim registry publication until `tact_agent_api` and optional provider
  dependencies are registry-safe.
- Do not register a tracing subscriber from library code.
- Never record prompts, responses, credentials, authorization headers, or
  provider-state payloads in library telemetry.
- Prefer enums for every closed provider, protocol, request-kind, and error
  classification.
- Preserve unrelated untracked files.

## Review Focus

- `ProviderFactory::build`, `build_with_api_key`, and `build_anonymous` are
  async and use one construction path.
- `ProviderInfo::build_client`, `Client::new`, and `get_llm_client` delegate to
  the shared assembly path and contain no provider branching.
  `ProviderInfo::build_client` remains synchronous and cannot await the async
  factory.
- Only `provider/policy.rs` interprets provider/model/endpoint string
  heuristics. `types.rs` may parse typed labels and own static built-in default
  URLs; `profile.rs` compatibility methods and product tests may contain
  literals but cannot duplicate wire decisions.
- A third-party example writes no global initializer.
- A custom endpoint can be anonymous.
- A refreshable credential is resolved per request.
- `Arc<dyn ModelPort>` works directly with `tact_agent_sdk::AgentBuilder`.
- Cancellation is honored before dispatch and during stream consumption.
- `ModelLimits` plus explicit capability overrides become
  `ModelCapabilities` without guessing.
- In stable mode, a request model different from the target model is rejected
  before dispatch; legacy compatibility retains same-family request switching.
- Session id and provider-state binding are represented in `ModelTarget` and
  `FacadeMetadata`, not adapter downcasts.
- Responses internals remain private.
- Feature combinations compile without pulling unrelated protocol SDKs.

## Task 0: Characterize current behavior

**Files:**

- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/provider_model.rs`
- `crates/tact_llm/src/profile.rs`
- `crates/tact_llm/src/hook_select.rs`
- `crates/tact_llm/src/openai/compatible/body.rs`
- `crates/tact_llm/src/openai/compatible/mod.rs`
- `crates/tact_llm/src/openai/responses/`
- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/tests/`
- `crates/tact_llm/tests/redaction_inventory.txt`
- `scripts/check-llm-redaction.sh`
- `fixtures/tact-llm-consumer/`

**Work:**

- [ ] Add a provider-policy matrix test over Anthropic, OpenAI, DeepSeek, Kimi,
  custom Chat Completions, custom Responses, OpenCode, and request-level model
  switches.
- [ ] Check in request JSON and header goldens for every provider family under
  `crates/tact_llm/tests/goldens/`. Do not write characterization baselines to
  `target/`, where a clean checkout and CI cannot reproduce them.
- [ ] Replay every checked-in Responses SSE fixture through the current stream
  path and snapshot terminal content, stop reason, usage, and cursor update.
- [ ] Characterize `stream`, `complete`, and `compact` cancellation at
  pre-dispatch, waiting for the first event, mid-stream, and post-terminal
  points. Include drop-before-poll and drop-mid-stream. Record the current
  behavior, including the detached producer task and any adapter future that is
  still polled after cancellation; these baselines document known defects and
  are not acceptance criteria for the target contract.
- [ ] Add secret-redaction tests for build errors, runtime errors, debug output,
  existing tracing events, HTTP error bodies, SSE parse errors, and tool
  arguments. Use a unique prompt marker, credential marker, and body marker in
  a subscriber-capture test so every diagnosed path is enumerated. Add the
  workspace `tracing-subscriber` dependency to `tact_llm` dev-dependencies;
  otherwise this gate is not runnable from a clean manifest. Capture all
  dependency targets as well as `tact_llm` targets, including malformed
  OpenAI Responses payloads.
- [ ] Record the current account and model-discovery failure paths. Account
  errors currently interpolate `resp.text()`; model discovery soft-fails.
  The former must become structured and body-free without changing successful
  discovery behavior.
- [ ] Add a checked-in redaction inventory or source guard that names every
  existing content-bearing tracing/error field in Anthropic, Chat Completions,
  and Responses. It must fail with the exact source location if a request body,
  response body, raw SSE frame, or tool argument is logged.
- [ ] Add `scripts/check-llm-redaction.sh` and
  `crates/tact_llm/tests/redaction_inventory.txt`; the script scans the named
  source locations for content-bearing tracing fields and fails with the file
  and line number. The inventory is the reviewed allowlist, not a prose claim.
- [ ] Add a `Retry-After` matrix fixture covering 429 and 5xx delta-seconds,
  valid HTTP dates, malformed values, and absence. Record the current loss at
  `neutral::model_error` before changing it.
- [ ] Check in a reviewed inventory of every `LlmError::OpenAi`,
  `LlmError::OpenAiResponses`, `MessageError` construction/match, and
  `LlmError::HttpError` literal. It must include production adapters,
  `neutral.rs`, `tact/src/recovery.rs`, and recovery tests. Task 3 owns the
  breaking replacement; Task 1 must not remove variants before those callers
  migrate.
- [ ] Record the current `ProviderModel` stream lifecycle with a task tracker:
  `stream_params` starts a detached producer eagerly, dropping before the first
  poll can still dispatch, and provider completion can emit `Finished` without
  rechecking cancellation. These are characterization facts, not the Task 3
  target contract.
- [ ] Create the independent fixture manifest at
  `fixtures/tact-llm-consumer/Cargo.toml` with an empty `[workspace]` table and
  a path dependency on `../../crates/tact_llm` with `default-features = false`;
  define fixture-local Anthropic, Chat, and Responses feature aliases so each
  external build selects exactly one provider. Its concrete compile matrix is
  added in Task 4 and Task 7.
- [ ] Record the current public exports with `cargo public-api` if the command
  is available; otherwise check in a reviewed `rg`-generated compatibility
  symbol inventory under `crates/tact_llm/tests/public_api/`. Characterization
  evidence must be reproducible from a clean checkout.

**Gate:**

- [ ] `cargo test -p tact_llm provider_policy` passes.
- [ ] `cargo test -p tact_llm responses` passes.
- [ ] `cargo test -p tact_llm cancellation` passes.
- [ ] `cargo test -p tact_llm retry_after` passes.
- [ ] `scripts/check-llm-redaction.sh` passes and fails when a marker-bearing
  field is added to a test copy.
- [ ] The checked-in legacy error-caller inventory is complete for `tact_llm`,
  `tact`, and `tact-ui` tests at the current revision.
- [ ] Characterization tests pass against the unmodified production path and
  explicitly record current behavior, including known cancellation defects,
  rather than asserting the Task 3 target contract.
- [ ] Removing `target/` does not remove any required golden or compatibility
  inventory.

## Task 1: Add the validated target and typed build errors

**Files:**

- `crates/tact_llm/src/lib.rs`
- `crates/tact_llm/src/build_error.rs`
- `crates/tact_llm/src/error.rs`
- `crates/tact_llm/src/model_target.rs`
- `crates/tact_llm/src/profile.rs`
- `crates/tact_llm/src/types.rs`

**Work:**

- [ ] Define `LlmBuildError` with the typed variants in the spec and mark it
  `#[non_exhaustive]`. No closed error category may be a free-form `String`;
  the only free-form source is an underlying transport-construction error.
- [ ] Define `ProfileError`, `CredentialError`, `CredentialSourceError`,
  `PolicyError`, `TransportSetupStage`, and `LegacyAssemblyStage`; map them
  into typed `LlmBuildError` variants and the request-time `ModelError` path.
- [ ] Define the new safe `HttpErrorClass`, `HttpErrorKind`, `HttpErrorBody`,
  and `ProviderError` types with redacted `Debug`/`Display`, but do not remove
  or re-shape the existing `LlmError`/`MessageError` variants in this task.
  Task 3 performs the atomic caller migration. Do not provide any body accessor
  visible to the separate `tact` or `tact-ui` crates.
- [ ] Define `ModelLimits::try_new` with checked construction rules:
  - `context_window == 0` represents an explicitly unknown window;
  - `max_output_tokens > 0`;
  - when both are known, `max_output_tokens < context_window`.
- [ ] Define `ModelTarget::new` with private fields and accessors. It returns
  `ModelTargetError` for invalid profile/model/token combinations, including
  `UnsupportedProtocol { provider, protocol }` for Anthropic plus explicit
  Responses; capability validation belongs to the fallible override builders.
- [ ] Reject `ModelRequest.max_output_tokens == 0` and values above the built
  target's configured maximum before dispatch. Do not clamp oversized values;
  `ModelLimits.max_output_tokens` is the default capability, not a rewrite of
  caller intent.
- [ ] Define `context_window == 0` as unknown for consumers. It must not enter
  product budget arithmetic as a zero-token window.
- [ ] Add fallible `with_session_id` and infallible
  `with_replay_prior_reasoning`. Validate the session id as an HTTP header
  value. An omitted id stays distinct from an empty id; empty,
  control-character, newline, and header-illegal values return
  `ModelTargetError::InvalidSessionId`.
- [ ] Add `with_chat_dialect` and `with_native_compaction` typed overrides.
  Reject `with_chat_dialect` for Anthropic and Responses profiles. Reject
  `with_native_compaction(true)` for Anthropic and Chat profiles; accept
  `with_native_compaction(false)` there as a no-op and as an explicit disable
  on Responses.
- [ ] Store target overrides as a private `ProviderPolicyOverrides` value and
  pass it into every policy resolution; never infer a custom profile's
  dialect from its display name.
- [ ] Define `FacadeMetadata` with `EndpointClass`, define `EndpointIdentity`
  and `BuiltModel`, and return metadata only from the validated factory.
- [ ] Add `WireProtocol` and use it in `FacadeMetadata` and build errors.
  `OpenAiProtocol` remains configuration-only.
- [ ] Define `CapabilityPreviewError` and make
  `ModelTarget::preview_capabilities()` return
  `Result<ModelCapabilities, CapabilityPreviewError>`. It must report both
  disabled provider features and policy errors, using the same immutable
  inputs and selection checks as factory build. Do not expose a preview that
  succeeds under a feature configuration where the factory would reject the
  target.
- [ ] Define `EndpointIdentity` normalization and serde exactly as specified,
  including default-port removal, lowercase scheme/host, query/fragment
  rejection, and one trailing-slash removal only when the path remains
  non-empty. Root `/` must stay `/`. A base URL with a query or fragment must
  fail rather than collapse two routed endpoints into one identity.
- [ ] Add `EndpointIdentity::parse`, `FromStr`, `as_str`, and typed
  `EndpointError`; deserialization must use the same canonicalization.
- [ ] Add `ProviderProfile` constructors for Anthropic, OpenAI, DeepSeek, Kimi,
  and custom endpoints. The custom signature is
  `custom(name, model, base_url, protocol)`.
- [ ] Add `openai_responses(model)` and
  `openai_with_protocol(model, protocol)` so the stable facade can express an
  official OpenAI Responses target without treating it as a custom endpoint.
- [ ] Validate custom base URLs as absolute `http` or `https` URLs.
- [ ] Reject embedded username/password userinfo in profile and proxy URLs with
  typed errors; do not include raw rejected URLs in error text.
- [ ] Define public `ProfileError` and map it to `LlmBuildError`.
- [ ] Preserve the existing `ProviderProfile` fields and struct literal
  construction during the compatibility window.
- [ ] Re-export the new facade types from `lib.rs`, including the exact model
  contract symbols used by the examples.

**Gate:**

- [ ] `cargo test -p tact_llm model_target` passes.
- [ ] `cargo test -p tact_llm profile` passes.
- [ ] Invalid URL, empty model, invalid token limits, and unsupported
  provider/protocol combinations return typed errors rather than panic.
- [ ] All existing `ProviderProfile` struct literals still compile.
- [ ] A stable request whose `ModelRequest.model` differs from the target
  returns a typed permanent error before dispatch. Legacy compatibility tests
  separately preserve current same-family request switching.
- [ ] Endpoint normalization tests cover equivalent trailing-slash, host-case,
  and default-port variants, plus typed query and fragment rejection.
- [ ] Endpoint tests cover userinfo rejection, IPv6 brackets, uppercase paths,
  percent-encoding preservation, and scheme-relative input. Two endpoints that
  differ only by query must both be rejected, never treated as equal.
- [ ] `EndpointIdentity` round-trips every legacy `ResponsesConversationState`
  base URL to the canonical string and rejects different endpoints.
- [ ] New construction-error tests prove secrets and bodies are absent from
  `Debug`/`Display`; runtime error-chain and body-classification gates run in
  Task 3 after callers migrate.
- [ ] Anthropic-only preview/build tests prove a disabled OpenAI target returns
  `CapabilityPreviewError::ProviderFeatureDisabled` from preview and
  `LlmBuildError::ProviderFeatureDisabled` from build.
- [ ] Invalid dialect combinations return typed errors.
- [ ] Session-id tests cover omitted, valid, empty, newline, control, and
  header-illegal values, plus the exact adapter header behavior.
- [ ] Native-compaction override tests cover Responses true/false and
  Anthropic/Chat true rejection and false no-op.

## Task 2: Introduce the single provider policy

**Files:**

- `crates/tact_llm/src/provider.rs` (move to `provider/mod.rs`)
- `crates/tact_llm/src/provider/mod.rs`
- `crates/tact_llm/src/provider/policy.rs`
- `crates/tact_llm/src/profile.rs`
- `crates/tact_llm/src/hook_select.rs`
- `crates/tact_llm/src/opencode.rs`
- `crates/tact_llm/src/openai/compatible/body.rs`
- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/openai/responses/convert.rs`
- `crates/tact_llm/src/account.rs`
- `scripts/check-provider-policy.sh`

**Work:**

- [ ] Before adding any child module, move `provider.rs` verbatim to
  `provider/mod.rs`, update imports, and run the current provider tests. Do not
  create `provider/policy.rs` while `provider.rs` still exists.
- [ ] Define the internal `ProviderPolicy` with `wire: WireProtocol` and typed
  reasoning, account, discovery, Responses, vision, and body-policy fields. Do
  not introduce an undefined `ProviderWire` type.
- [ ] Add internal `PolicyMode::{StableFacade, LegacyCompatibility}` and require
  it at every `ProviderPolicy::resolve` call. Stable factory/target paths use
  `StableFacade`; deprecated product compatibility paths use
  `LegacyCompatibility` until their callers migrate.
- [ ] Give `ProviderPolicy::resolve` the immutable profile, request model, and
  `ProviderPolicyOverrides` from the built target. Explicit target overrides
  outrank built-in heuristics; custom display names never do.
- [ ] Return `PolicyError` from the internal resolver rather than `LlmError`.
  Factory validation maps `InvalidOverride` to
  `LlmBuildError::InvalidTarget`; request-time resolution maps
  `ModelFamilyMismatch` to a permanent `ModelError` before dispatch.
- [ ] Resolve policy from `ProviderProfile` and the configured model during
  capability preview and factory build. Resolve `EndpointClass` from the
  canonical endpoint: `openai_with_protocol` may be custom, and a custom
  profile may target an exact built-in endpoint.
- [ ] Before publishing the internal built-in endpoint table, capture the
  effective request URL for every adapter. Do not assume a configured base URL
  is the request URL; Anthropic currently appends `/messages` directly. Add a
  wiremock/request-capture test for each built-in default and reconcile path
  joining before marking its canonical pair `BuiltIn`. The intended Anthropic
  default is `https://api.anthropic.com/v1`, producing
  `https://api.anthropic.com/v1/messages`; compatibility config root
  `https://api.anthropic.com` canonicalizes to that effective base.
- [ ] Resolve the effective policy again from the immutable profile and the
  request model at the adapter boundary.
- [ ] Move DeepSeek, Kimi, K2, K2.7, K3, coding-endpoint, official-endpoint,
  vision, and reasoning-mode decisions into `policy.rs`.
- [ ] Treat `types.rs` as typed-label parsing plus static default URLs only.
  Remove model/endpoint heuristics from it rather than moving them here by
  accident.
- [ ] Make `ProviderProfile` heuristic methods delegate to `ProviderPolicy`.
- [ ] Replace direct vendor string checks in body-hook selection with policy
  methods.
- [ ] Replace Responses conversion vendor checks with the resolved policy.
- [ ] Move OpenCode endpoint classification and header policy into
  `provider/policy.rs`; keep `opencode.rs` as a compatibility delegate.
- [ ] Make account eligibility consume the same policy.
- [ ] Keep model-discovery eligibility separate from `EndpointClass` and account
  eligibility. `LegacyCompatibility` must preserve the current explicit
  `ProviderKind::Custom(_)` `/models` behavior; stable model discovery remains
  outside the third-party facade.
- [ ] Keep `hook_for_dialect` and `body_hook_for` as deprecated compatibility
  wrappers.
- [ ] Keep the public compatibility path `openai::compatible` in this phase.
  The internal Chat destination `openai/chat/` may be introduced later; do not
  rename the public path until its callers and tests migrate together.
- [ ] Add stable custom-profile regression tests with neutral/vendor-looking
  names, base URLs, and model ids containing `deepseek`, `kimi`, `k2.7`, and
  `opencode`; without a typed dialect override they must still use standard
  Chat Completions policy.
- [ ] Add legacy-compatibility tests proving current substring inference is
  retained only under `PolicyMode::LegacyCompatibility`.
- [ ] Add `scripts/check-provider-policy.sh` and a checked-in allowlist. Scan
  for provider/model/endpoint heuristics outside `provider/policy.rs`, the
  compatibility delegates, typed default URL labels, product parsing, fixtures,
  and tests. The script fails with the file and line number.

**Gate:**

- [ ] `scripts/check-provider-policy.sh` fails on a deliberately introduced
  vendor-string decision outside the allowlist.
- [ ] The Task 0 policy matrix passes unchanged.
- [ ] Built-in endpoint tests prove the actual request URLs used by Anthropic,
  OpenAI Chat, OpenAI Responses, DeepSeek, and Kimi.
- [ ] Endpoint-policy aliases cover `api.deepseek.com`,
  `api.moonshot.cn/v1`, `api.moonshot.ai/v1`, and
  `api.kimi.com/coding(/v1)` where the corresponding account, usage, or
  discovery helpers currently recognize them.
- [ ] Legacy discovery tests prove explicit custom `/models` support still
  works; account tests prove account queries remain built-in-only.
- [ ] Stable custom names/model ids do not infer a vendor dialect; explicit
  `with_chat_dialect` still wins.
- [ ] Legacy compatibility tests reproduce the pre-refactor heuristics.
- [ ] `cargo test -p tact_llm profile` passes.
- [ ] `cargo test -p tact_llm account` passes.
- [ ] `cargo test -p tact_llm openai` passes.

## Task 3: Add credential variants and the provider factory

**Files:**

- `Cargo.toml`
- `Cargo.lock`
- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/src/auth.rs`
- `crates/tact_llm/src/transport.rs`
- `crates/tact_llm/src/provider/factory.rs`
- `crates/tact_llm/src/provider/mod.rs`
- `crates/tact_llm/src/provider_model.rs`
- `crates/tact_llm/src/client.rs`
- `crates/tact_llm/src/error.rs`
- `crates/tact_llm/src/neutral.rs`
- `crates/tact_llm/src/anthropic/mod.rs`
- `crates/tact_llm/src/openai/compatible/mod.rs`
- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/account.rs`
- `crates/tact_llm/src/models.rs`
- `crates/tact/src/recovery.rs`
- `crates/tact-ui/tests/recovery_compaction.rs`

**Work:**

- [ ] Change `CredentialProvider::resolve` to return
  `Result<Credential, CredentialSourceError>`, preserving API-key behavior and
  adding `Bearer` and `None`. The stable trait must not expose `LlmError`.
- [ ] Add the stable `CredentialSourceError` type. Map build-time source
  failures to `LlmBuildError::CredentialSource` and request-time failures to a
  permanent authentication `ModelError` before dispatch.
- [ ] Make `Credential` an opaque public newtype with `api_key`, `bearer`, and
  `none` constructors. Do not expose `secrecy` types in its public shape.
- [ ] Atomically replace `LlmError`'s SDK-bearing variants,
  `MessageError`'s raw-string payload, and `LlmError::HttpError`'s public
  `body: String` using the checked-in caller inventory from Task 0. Update all
  adapters, `neutral.rs`, `tact/src/recovery.rs`, and recovery tests in the
  same task before any gate runs. The workspace must remain buildable at the
  task boundary.
- [ ] Make credential constructors fallible and reject empty values, control
  bytes, newlines, and illegal HTTP header bytes. Header conversion is
  fallible; no `unwrap()`/`parse().expect()` on a credential.
- [ ] Keep `ApiKeyProvider`; add `BearerProvider` and `NoCredentialProvider`.
- [ ] Ensure `Credential`, all credential providers, and `ProviderInfo`
  implement redacted `Debug`. `ProviderInfo.api_key` remains source-compatible
  but must never be printed.
- [ ] Define typed `AuthRequirement`/`AuthScheme` and test Anthropic, built-in
  OpenAI Chat, built-in OpenAI Responses, built-in DeepSeek/Kimi, custom Chat,
  custom Responses, and account/model-discovery credential behavior. Built-in
  DeepSeek and Kimi require bearer authentication; anonymous mode is limited
  to custom endpoints.
- [ ] Migrate every consumer in `anthropic`, `openai/compatible`,
  `openai/responses`, `account`, and `models` to the new variants.
- [ ] Map Anthropic `ApiKey` to `x-api-key` and `Bearer` to the authorization
  header; map OpenAI variants to bearer authorization.
- [ ] Keep Anthropic header mapping in `anthropic` and OpenAI mapping in the
  protocol adapters. Do not choose a header from a provider string in a shared
  helper.
- [ ] Add the immutable `ProviderFactory`, `ProviderFactoryBuilder`, and
  `BuiltModel`.
- [ ] Build fallible factory-owned HTTP clients for both reqwest generations;
  never call `SharedHttpClient::default()` on the new path.
- [ ] Pin the workspace `async-openai-responses` dependency to the reviewed
  immutable revision
  `12488eb3f6f2dbf35082a1121f2192dd1c2a7c27`, regenerate `Cargo.lock`, and
  verify `cargo metadata --locked` resolves that exact source. Do not modify
  the external fork or use its branch name as a reproducibility guarantee.
- [ ] Add `ProxyUrl` and user-agent builder methods and apply them to both
  generations. Do not expose reqwest's URL type from the facade.
- [ ] Keep HTTP-client construction and both reqwest generations private.
- [ ] Implement one exhaustive provider/protocol construction match.
- [ ] Build `ProviderModel` with capabilities resolved once by the factory from
  `ModelTarget` plus explicit overrides; `capabilities()` must not guess or
  re-resolve policy.
- [ ] Store the configured target model, `PolicyMode`, and optional
  `FacadeMetadata` on each `ProviderModel`. Stable dispatch rejects a different
  `ModelRequest.model` before adapter polling; legacy compatibility keeps
  same-family request switching. Factory-built models retain metadata and
  validate every cursor with the shared Responses binding predicate before
  `stream`, `complete`, or `compact`.
- [ ] Rework stream production into a lazy wrapper: construction performs no
  spawn, the first `poll_next` starts the producer, and drop aborts a live
  producer. Add one terminal state machine `Open -> Finished | Failed |
  Cancelled`. Use a biased producer selection with cancellation checked before
  the provider future. If both are ready, cancellation wins; the
  success/failure branch rechecks cancellation immediately before an atomic
  compare-and-set claim. After yielding any terminal item, clear the inner
  stream and return `None` on the next poll. The first
  `poll_next` rechecks cancellation before spawning the producer or polling the
  adapter; cancellation between `stream()` returning and the first poll yields
  exactly one `Cancelled` outcome and zero adapter calls.
- [ ] Add pre-dispatch cancellation checks to `complete_params` and
  `compact_params`; an already-cancelled token must not poll the adapter future.
- [ ] Reject `compact_params` before polling the adapter when the built
  capabilities report `supports_native_compaction == false`.
- [ ] Add the target-contract tests that Task 0 intentionally does not require:
  drop-before-poll performs no dispatch, drop-mid-stream leaves no live producer
  task, a live stream emits exactly one terminal `Finished`, `Failed`, or
  `Cancelled` outcome, no terminal event follows another, and an
  already-cancelled token dispatches neither `complete` nor `compact`. After
  each terminal kind, an explicit next poll returns `None` and never remains
  pending.
- [ ] Force terminal races in both directions for success and provider error:
  use controlled channels/barriers so provider-first and cancellation-first
  readiness are deterministic. Cancellation readiness observed before the
  outcome claim yields only `Cancelled`; a `Finished` or `Failed` outcome that
  claims first is not followed by a cancellation error. A simultaneously-ready
  case must deterministically choose `Cancelled` under the biased priority.
- [ ] Apply session id and reasoning-replay options at construction.
- [ ] Return `BuiltModel` from the public methods with private `model` and
  `metadata` fields, accessors, `into_model`, and `into_parts`.
- [ ] Add the feature-gated `ProductBuiltModel` and
  `ProviderFactory::build_product` result for Tact. It returns the model,
  metadata, and legacy `LlmProvider` mirror from the same assembly path and
  accepts `ProductRequestBodyPolicy::{Retain, Strip}`. Stable `build` always
  strips request bytes and never exposes the mirror.
- [ ] Implement `build_with_api_key` and `build_anonymous` by delegating to
  `build`.
- [ ] Keep the legacy `LlmProvider` mirror confined to the feature-gated
  `ProductBuiltModel`; no stable method returns the adapter enum.
- [ ] Route `ProviderInfo::build_client`, `Client::new`, and `get_llm_client`
  through the shared provider decision and adapter-assembly helper. Map typed
  assembly failures into the existing `anyhow::Result` signatures at the
  compatibility boundary.
- [ ] Keep `ProviderInfo::build_client()` synchronous and successful on its
  normal path, using `ProviderInfo.api_key` as a static credential snapshot.
  Do not create a nested runtime, block on the async factory, or call a
  credential provider.
- [ ] Keep `Client::new()` and `get_llm_client()` asynchronous and resolve their
  initial credential exactly once before construction.
- [ ] Remove duplicate provider/protocol matches from `provider.rs`.
- [ ] Preserve legacy `set_user_id` behavior through an explicit session-id
  option or a compatibility adapter method.
- [ ] Replace `LlmError::HttpError { status, body: String }` with the safe
  public fields plus private `HttpErrorBody`, and parse `Retry-After` delta
  seconds and HTTP dates on every direct HTTP error path. Treat malformed or
  absent headers as `None`.
- [ ] Add `httpdate` as a direct workspace/`tact_llm` dependency and use one
  shared parser for the HTTP-date branch; do not rely on a transitive crate.
- [ ] Read every direct HTTP error body through a bounded reader of at most
  `MAX_HTTP_ERROR_BODY_BYTES + 1`; limit before allocating, never after
  buffering the full response. Add an oversized-body test for each direct
  adapter path.
- [ ] Treat adding the `retry_after` field as an intentional source-breaking
  in-branch change: migrate every struct literal and constructor, list them in
  the task evidence, and keep this transition outside the facade release
  promise. The inventory includes at least `crates/tact/src/recovery.rs`,
  `crates/tact-ui/tests/recovery_compaction.rs`, and the direct Anthropic,
  Chat Completions, and Responses constructors.
- [ ] Make `LlmError`, `MessageError`, `HttpErrorClass`, and `HttpErrorBody`
  compatibility-only behind `legacy-tact`; stable calls continue to return
  `ModelError`.
- [ ] Match Responses SDK errors at the adapter boundary, extract only
  status/class/retry metadata and typed `HttpErrorKind`/context-overflow
  classification from the SDK `ApiError`, then discard the SDK value before
  returning `LlmError`. No `source()` chain may retain
  `OpenAIError::JSONDeserialize`.
- [ ] Add body-free `HttpErrorKind::ContextOverflow` classification before
  discarding the bounded HTTP body. Map it in `neutral::model_error` to
  `ModelErrorKind::ContextOverflow`, and migrate `crates/tact/src/recovery.rs`
  to consume the typed kind rather than parsing redacted text.
- [ ] Replace Anthropic's body-bearing `MessageError::ApiError(String)` path
  with a structured, bounded HTTP error that preserves status and response
  headers. Do not concatenate the response body into `MessageError`,
  `ModelError.message`, `Debug`, or `Display`.
- [ ] Convert DeepSeek/Kimi account HTTP failures to structured body-free
  errors; never include `resp.text()` in `anyhow`, `Display`, or tracing.
- [ ] Replace `StreamParse(String)` messages that interpolate raw SSE payloads
  or provider error messages with typed error codes and structural positions.
  Remove the Chat Completions `request_body` tracing field and any equivalent
  Responses payload logging.
- [ ] Keep `MAX_HTTP_ERROR_BODY_BYTES = 8 * 1024` for internal compatibility
  diagnostics. Add canary tests for `MessageError`, `LlmError`,
  `ModelError.message`, `Debug`, `Display`, `HttpErrorBody` access, and a
  captured tracing subscriber.
- [ ] Map `LlmError::HttpError.retry_after` into `ModelError.retry_after` in
  `neutral::model_error`; do not infer delay from response text.
- [ ] Ensure the factory-built `ProviderModel` clears
  `LlmResponse.request_body` before constructing `ModelResponse`. Add a direct
  facade test with a unique prompt marker that asserts
  `ModelResponse.request_body.is_none()`.
- [ ] Prove the product path can retain a unique system-prompt request-body
  marker through `build_product(..., Retain)` and that the existing
  `SqliteUsageSink` path persists it, while stable `build` strips the same
  marker from `ModelResponse.request_body`.
- [ ] Keep the Responses fork unchanged. The current `async-openai-local`
  error type drops response headers before `tact_llm` observes them, so
  SDK-backed Responses `stream` and `complete` errors must report
  `retry_after = None`; do not invent a delay or claim header coverage.
- [ ] Audit every executed OpenAI dependency path for retained or logged
  response bodies. `tact_llm` must avoid body-bearing
  `tracing::debug!(... %response_body ...)` and body-carrying
  `JSONDeserialize(_, body)` errors. If a pinned dependency path still leaks
  through an error source or dependency log and cannot be avoided inside
  `tact_llm`, stop and report the exact path as a blocker rather than modifying
  the external fork or claiming redaction.
- [ ] Add a malformed Responses SDK payload test with a unique body marker and a
  captured subscriber that includes dependency targets. Add corresponding
  account-query failure tests; no marker may appear in logs, errors, or source
  chains.
- [ ] Wiremock tests cover 429/5xx with valid delta seconds, valid HTTP date,
  malformed header, and absent header for the direct Anthropic, Chat
  Completions, and Responses native-compaction paths. A separate SDK-backed
  Responses stream/complete 429 test proves `retry_after = None`. A kernel
  retry test proves a parsed direct-path delay is used rather than default
  backoff.

**Gate:**

- [ ] `cargo test -p tact_llm provider` passes.
- [ ] All three public factory methods produce equivalent behavior for the
  same resolved credential.
- [ ] A failing credential provider returns a credential error before the
  first request.
- [ ] A successful build resolves the credential exactly once for validation;
  the first request resolves it again rather than reusing a cached secret.
- [ ] A refreshed credential changes the next Anthropic request header without
  rebuilding the model.
- [ ] Cancellation between `stream()` return and the first `poll_next` produces
  exactly one `Cancelled` outcome and does not poll the adapter.
- [ ] An anonymous custom Chat Completions endpoint builds and dispatches
  without an authorization header.
- [ ] An anonymous custom Responses endpoint follows standard Responses policy
  and does not enable vendor account or discovery behavior. Toggling
  `with_native_compaction` does not change provider-state binding.
- [ ] `compact` on a model whose capabilities disable native compaction fails
  before adapter polling.
- [ ] A `None` credential for a built-in Anthropic, OpenAI, DeepSeek, or Kimi
  endpoint fails before dispatch.
- [ ] `rg` finds one provider/protocol construction match.
- [ ] Legacy product tests that build `ProviderInfo` still pass.
- [ ] `ProviderInfo::build_client` succeeds both inside and outside an active
  Tokio runtime, and a header-capture test proves the static API key is used
  without invoking `CredentialProvider::resolve`.
- [ ] `Retry-After` tests pass for each direct adapter HTTP path, SDK-backed
  Responses stream/complete report `None`, and the kernel consumes a
  propagated direct-path delay.
- [ ] HTTP-date parsing passes through the declared `httpdate` helper.
- [ ] A 400 context-overflow body is classified as
  `ModelErrorKind::ContextOverflow` without appearing in
  `ModelError.message`, `Debug`, `Display`, or `source()`.
- [ ] Recursive error-chain and dependency-subscriber tests cannot recover or
  log a response-body marker through an OpenAI SDK error.
- [ ] A Responses SDK-backed 429 test proves the adapter reports
  `retry_after = None` without changing the pinned fork.
- [ ] Resolve-count tests record all entry points explicitly:
  `ProviderFactory::build` = 1 build resolve + 1 resolve per request;
  `ProviderInfo::build_client` = 0 credential-provider resolves;
  `Client::new` and `get_llm_client` = 1 build resolve + 1 per request.
  Responses normal and compact calls each count separately; account and model
  discovery reject non-`ApiKey` credentials before dispatch.
- [ ] Oversized HTTP error bodies stop at the configured limit, and a
  factory-built `ModelResponse` never carries request bytes.

## Task 4: Curate the public facade

**Files:**

- `crates/tact_llm/src/lib.rs`
- `crates/tact_llm/src/openai/mod.rs`
- `crates/tact_llm/src/openai/compatible/mod.rs`
- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/neutral.rs`
- `crates/tact_llm/src/convert.rs`
- `crates/tact_llm/src/hook_select.rs`
- `crates/tact_llm/src/inject.rs`
- `crates/tact_llm/src/lock.rs`
- `crates/tact_llm/src/opencode.rs`
- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/tests/public_api.rs`
- `crates/tact_llm/tests/facade_privacy.rs`
- `crates/tact_llm/tests/ui/forbidden_openai_responses.rs`
- `crates/tact_llm/tests/ui/forbidden_openai_compatible.rs`
- `crates/tact_llm/tests/ui/forbidden_neutral.rs`
- `crates/tact_llm/tests/ui/forbidden_convert.rs`
- `crates/tact_llm/tests/ui/*.stderr`
- `crates/tact_llm/examples/`
- `fixtures/tact-llm-consumer/Cargo.toml`
- `fixtures/tact-llm-consumer/src/lib.rs`
- `fixtures/tact-llm-consumer/Cargo.lock`

**Work:**

- [ ] Define the stable facade re-exports:
  - provider/profile/target/factory/credential/build-error types;
  - `ProxyUrl`, `WireProtocol`, `EndpointClass`, `EndpointIdentity`, and
    `FacadeMetadata`;
  - `CancelToken`, `ModelCapabilities`, `ModelCursor`, `ModelError`,
    `ModelEvent`, `ModelPort`, `ModelRequest`, and `ModelResponse`;
  - `ChatCompletionsDialect`, `OpenAiProtocol`, and `ProviderKind`, which are
    reachable from public constructor/accessor signatures;
  - `CredentialSourceError`, `CredentialError`, `PolicyError`,
    `CapabilityPreviewError`, `ProviderFeature`, `AuthRequirement`, and
    `AuthScheme`, which appear in stable signatures or non-exhaustive error
    variants;
  - `TransportSetupStage`, `TransportConfigError`, and `LegacyAssemblyStage`
    as nameable construction-error members, without promising them as
    extension points.
- [ ] Do not export `ProviderModel` from the stable facade: its public
  `LlmProvider` constructor leaks legacy adapters. Keep it crate-internal for
  factory construction and compatibility tests.
- [ ] Keep account and model-discovery APIs behind `legacy-tact`; do not put
  them in the stable third-party facade while they still read process globals.
- [ ] Make implementation modules `pub(crate)` where callers allow it.
  `#[doc(hidden)]` is not a privacy boundary and is not accepted for wire DTOs,
  SSE types, body hooks, adapters, or normalization helpers.
- [ ] Before changing visibility, check in and complete a facade-caller
  inventory covering at least `tact_llm::neutral`, `convert`, `opencode`,
  `openai::compatible`, `openai::responses`, product runtime adapters,
  `test_support`, and integration tests. Every internal caller must migrate to
  a facade API or an explicit `#[cfg(feature = "legacy-tact")]` compatibility
  import before the stable privacy gate is enabled.
- [ ] Keep deprecated root paths while migrating internal callers.
- [ ] Add examples for:
  - static API key and agent SDK;
  - custom anonymous endpoint;
  - direct stream and cancellation;
  - refreshable credentials;
  - tracing subscriber integration.
- [ ] Add `tact_agent_sdk = { path = "../agent_sdk" }`,
  `trybuild = "1"`, and `tracing-subscriber` as `tact_llm` dev-dependencies so
  the SDK example and compile-fail/subscriber gates are runnable from a clean
  manifest. Do not add `tact_agent_sdk` as a normal dependency.
- [ ] Add compile tests that import only the facade.
- [ ] Complete the external-consumer fixture:
  `fixtures/tact-llm-consumer/Cargo.toml` has an empty `[workspace]` table and
  `tact_llm = { path = "../../crates/tact_llm", default-features = false }`.
  Define fixture-local aliases `anthropic = ["tact_llm/anthropic"]`,
  `openai-chat = ["tact_llm/openai-chat"]`, and
  `openai-responses = ["tact_llm/openai-responses"]`; `src/lib.rs` imports only
  facade symbols. In-crate examples are not sufficient because they inherit
  the package's normal dependency graph.
- [ ] Add a `trybuild` compile-fail fixture that imports
  `tact_llm::openai::responses::wire`, `openai::compatible::body`,
  `neutral`, and `convert`; each must fail to resolve in the stable feature
  configuration. Gate the `facade_privacy` test target with
  `#![cfg(not(feature = "legacy-tact"))]` and run it in a dedicated
  `--no-default-features` harness. Do not assert these compatibility modules
  are private when `legacy-tact` intentionally makes them available.
- [ ] Remove or metadata-only-reduce every content-bearing request body,
  response body, raw SSE, and tool-argument log before declaring the facade
  stable. Cover Anthropic, Chat Completions, and Responses paths.
- [ ] Add a subscriber-capture test with unique secret, prompt, and body
  markers; assert none appears in emitted events or rendered errors.

**Gate:**

- [ ] `cargo test -p tact_llm --test public_api` passes.
- [ ] `cargo check -p tact_llm --examples` passes for documentation, but the
  external-consumer fixture is the actual `default-features = false` gate.
- [ ] `cargo doc -p tact_llm --no-deps` passes with no broken links.
- [ ] Third-party examples do not call `init_provider`, `get_provider`, or
  `get_llm_client`.
- [ ] Third-party examples do not import `openai::responses`,
  `openai::compatible`, `neutral`, `convert`, or `hook_select`.
- [ ] The checked-in caller inventory has no unmigrated stable-build import of
  an implementation module.
- [ ] `cargo test -p tact_llm --no-default-features
  --features openai-responses --test facade_privacy` fails to compile each
  forbidden import for the expected name-resolution reason. The same test is
  not treated as a privacy gate under `--all-features` or `legacy-tact`.
- [ ] The stable facade does not export account or model-discovery helpers
  unless `legacy-tact` explicitly enables their compatibility surface.
- [ ] The external consumer fixture compiles with
  `cargo check --manifest-path fixtures/tact-llm-consumer/Cargo.toml
  --locked --no-default-features --features <fixture-alias>` for the
  `anthropic`, `openai-chat`, and `openai-responses` aliases.
- [ ] The redaction test finds no secret, prompt, body, raw SSE, or tool-argument
  marker in any library log or public error rendering.

## Task 5: Isolate legacy state and migrate product callers

**Files:**

- `crates/tact_llm/src/provider/legacy.rs`
- `crates/tact_llm/src/provider/mod.rs`
- `crates/tact_llm/src/models.rs`
- `crates/tact/src/lib.rs`
- `crates/tact/src/config/mod.rs`
- `crates/tact/src/config/types.rs`
- `crates/tact/src/config/resolve.rs`
- `crates/tact/src/runtime/composition.rs`
- `crates/tact/src/runtime/model_adapter.rs`
- `crates/tact/src/runtime/store_adapter.rs`
- `crates/tact/src/agent/mod.rs`
- `crates/tact/src/tool/mod.rs`
- `crates/tact/src/tool/read_image.rs`
- `crates/tact/src/tool/subagent.rs`
- `crates/tact/src/store/session_store/mod.rs`
- `crates/tact/src/store/session_store/sqlite.rs`
- `crates/tact/src/utils/lock.rs`
- `crates/protocol/src/agent.rs`
- `crates/agent_tui_kit/src/bridge.rs`
- `crates/agent_tui_kit/src/state/status_bar_state.rs`
- `crates/tact-ui/src/account.rs`
- `crates/tact-ui/src/interactive.rs`
- `crates/tact-ui/src/session_bootstrap.rs`
- `crates/tact-ui/src/driver.rs`
- `crates/tui/src/handlers/select.rs`
- `crates/tui/src/widgets/state/app/agent.rs`

**Work:**

- [ ] Before editing, check in an exhaustive product-caller inventory for
  `LlmProvider`, `ProviderInfo`, `get_provider`, `read_provider`,
  `init_provider`, `get_llm_client`, `tact_llm::lock`, `supports_vision`,
  request-body persistence, and `/model`. It must include at least
  `crates/tact/src/lib.rs`, `config/mod.rs`, `tool/read_image.rs`,
  `utils/lock.rs`, `tact-ui/src/account.rs`, `interactive.rs`,
  `session_bootstrap.rs`, `driver.rs`, and `tui/src/handlers/select.rs`.
  Every production caller must migrate or be explicitly confined to the
  reviewed legacy compatibility surface.
- [ ] Move `PROVIDER`, `CREDENTIALS`, global initializers, and panicking
  accessors into a clearly named legacy module.
- [ ] Keep deprecated root re-exports during migration.
- [ ] Keep the `LockExt`/`RwLockExt` compatibility helpers behind
  `legacy-tact` or move them to a provider-neutral product dependency; make
  `crates/tact/src/utils/lock.rs` stop importing a module that the stable
  facade hides. Preserve poison-tolerant behavior and migrate all product
  imports together.
- [ ] Keep `crates/tact/src/lib.rs::get_llm_client` only as a deprecated
  compatibility wrapper. New product composition and session bootstrap must
  not call it.
- [ ] Define the product-owned explicit `TactProviderContext` so new
  composition, account, and discovery code does not read legacy globals.
  Account and model discovery use the explicit `_for` functions, carry the
  credential-free profile, refreshable credential source, and shared
  transport, and retain built-in-only account eligibility.
- [ ] Define the product carrier: `TactKernelConfig` stores the
  feature-gated `ProductBuiltModel` produced by
  `ProviderFactory::build_product`; `TactModelPort` owns its
  `Arc<dyn ModelPort>` and the same snapshot retains `FacadeMetadata` and the
  legacy compatibility mirror. Session state persists the metadata required
  for provider-state validation.
- [ ] Keep `agent_api::TranscriptStore` provider-neutral. Have
  `SqliteTranscriptStore` own an
  `active_model_metadata: Option<SessionModelMetadata>` field supplied through
  its producer-specific constructor, and pass it to product-specific
  append/replace methods only for
  `CursorUpdate::Replace { cursor: Some(_) }`. Runtime composition in
  `crates/tact/src/runtime/composition.rs` constructs the adapter after the
  active model metadata is known; no global lookup is allowed.
  `Replace { cursor: None }` deletes provider state and metadata; `Unchanged`
  leaves both untouched and does not perform the nullable-row upgrade. The
  first upgrade happens on the first actual provider-state replacement,
  including a replacement whose transcript contents are unchanged.
- [ ] Publish the store metadata snapshot through the same activation boundary
  as the model snapshot: an in-flight activation keeps its original pair, and
  the next activation sees the rebuilt model plus matching metadata.
- [ ] Define the versioned `SessionModelMetadata` serde shape derived from
  `FacadeMetadata` (its own metadata schema version, provider, protocol,
  canonical endpoint, endpoint class) and persist it in the same transaction
  as provider state and transcript replacement. Keep the existing
  `responses_states.schema_version` as the provider-state version; do not
  overload it as the session-metadata version.
- [ ] Add nullable `facade_metadata_json` to both the new-database
  `CREATE TABLE responses_states` definition and the existing-database
  migration. Use the existing idempotent
  `PRAGMA table_info(responses_states)` + `ALTER TABLE ... ADD COLUMN` helper;
  do not infer column presence from `CREATE TABLE IF NOT EXISTS`. The migration
  leaves every pre-existing row `NULL`; it must not reconstruct provider kind,
  protocol, endpoint class, or metadata version from fields that were never
  stored. A `NULL` row is readable only when the legacy provider marker is
  `"openai_responses"` and the canonical legacy `base_url` matches the current
  `FacadeMetadata` under the shared binding predicate.
- [ ] Update every `responses_states` SQL path together:
  - the three current inserts in `append_agent_message`,
    `replace_agent_session_state`, and
    `replace_session_messages_and_provider_state` (currently near
    `sqlite.rs:678`, `:767`, and `:964`);
  - the current `SELECT state_json` reader (currently near `sqlite.rs:878`)
    must read `facade_metadata_json` and legacy marker fields in the same query;
  - tests and triggers that inspect columns.
  Bind nullable metadata as `None` for old rows and include it in every new
  replacement transaction.
- [ ] On the first successful save after a `NULL`-row read, write the current
  versioned metadata and canonical endpoint; reject unknown metadata schema
  versions and mismatches on later loads. Add tests for an indistinguishable
  pre-migration row, first-save upgrade, version rejection, canonical endpoint
  rewrite, and one-transaction rollback. Create a real old-schema SQLite
  database fixture without `facade_metadata_json`, then open it through
  `SqliteSessionStore::new`, read, upgrade, and reopen it. Do not assert a
  provider identity that the old row never contained.
- [ ] Define `ProviderStateSnapshot { state, metadata }` and the product-only
  `LoadProviderStateSnapshot` capability without adding `tact_llm` types to
  `agent_api::TranscriptStore`. Add the product-specific
  `SessionStore::load_provider_state_snapshot` method. `SqliteTranscriptStore::load`
  must call that method, validate the returned metadata against its injected
  active `SessionModelMetadata`, and only then return the cursor. Unsupported
  metadata versions and endpoint mismatches become `StoreError::Load` before
  the kernel observes provider state; a legacy `NULL` row takes only the
  legacy-marker/canonical-endpoint compatibility path.
- [ ] Add an integration test that writes provider state through the real
  SQLite path, constructs a fresh `SqliteTranscriptStore` with active
  metadata, loads the cursor, and proves a matching snapshot survives while a
  mismatched endpoint/version fails during `load`. Add the corresponding
  pre-migration fixture case instead of testing only the raw SQL row.
- [ ] Change `TactKernelConfig.provider: LlmProvider` to the chosen explicit
  model carrier and construct `TactModelPort` from `Arc<dyn ModelPort>`, not a
  provider enum.
- [ ] Migrate `tact-ui::bootstrap_session` to build a model through the
  explicit factory path.
- [ ] Route every Tact model construction through
  `ProviderFactory::build_product(..., ProductRequestBodyPolicy::Retain)`.
  Product code must not call the stable `build` path once the carrier
  migration lands. Preserve the bounded dispatched request body for the
  usage sink and `/view-system-prompt`; add a unique system-prompt marker test
  that persists through `SqliteUsageSink`, reloads through
  `load_latest_request_body`, and renders through
  `system_prompt::assemble_prompt_view`. The stable `build` path continues to
  strip the same marker.
- [ ] Replace product `ProviderInfo` transport with `ProviderProfile` plus
  `ModelTarget` where possible.
- [ ] Construct the model after the session id is known; pass it through
  `ModelTarget::with_session_id`.
- [ ] Replace enum checks with `ModelPort::capabilities()` or explicit product
  capability flags. Do not claim vision is part of `ModelCapabilities`; vision
  stays in typed product model metadata.
- [ ] Resolve `supports_vision` once in the product model snapshot, including
  the per-model config override, and inject it into the activation's immutable
  `ToolContext` and UI task snapshot. Change `read_image` and the driver's
  image-attachment gate to read that activation fact; remove their calls to
  `crate::config::supports_vision()` and `tact_llm::supports_vision()`.
  A running activation must keep its original vision fact even if global
  config changes.
- [ ] Replace provider-state `LlmProvider` downcasts with `FacadeMetadata` and
  `EndpointIdentity`; define one shared binding predicate over
  `(FacadeMetadata, ResponsesConversationState)` and reuse it from load-time
  validation and adapter dispatch.
- [ ] Implement legacy `ResponsesConversationState.base_url` mapping through the
  canonical `EndpointIdentity` rules and rewrite canonical URLs on state
  replacement.
- [ ] Preserve product native-compaction behavior through the explicit product
  resolver: call `with_native_compaction(true)` for Responses targets other
  than DeepSeek, including Kimi and custom Responses targets. Stable custom
  Responses targets keep native compaction disabled by default. Cover OpenAI,
  Kimi, custom, and DeepSeek Responses before and after migration.
- [ ] Preserve `/model` switching by rebuilding and swapping the `ModelPort`
  from the selected product model metadata; do not infer a capability change
  from `ModelRequest`, which carries none. Update the kernel dependency and
  carry transcript, cursor, and compatible provider state forward.
- [ ] For a same-window switch, still rebuild/swap the `ModelPort` so the wire
  model changes. Add a test that setting only `AgentSettings.model` does not
  masquerade as a working switch.
- [ ] Change `Agent::set_model` to an async fallible prepare/commit operation
  (or an internal prepare/commit pair behind an async wrapper). Resolve the
  requested profile/limits and build the replacement `ProductBuiltModel`
  before mutating settings; atomically publish the model, `FacadeMetadata`,
  activation vision fact, session-model metadata, and legacy mirror only after
  the build succeeds. Leave every old snapshot and setting unchanged on build,
  credential, or binding failure.
- [ ] Replace the fire-and-forget `UserCommand::SetModel(String)` path with a
  request-correlated `ModelSwitchRequest` and
  `AgentUpdate::ModelSwitchResult`. The result is `Applied` only after the
  atomic publish and otherwise carries a typed safe `Rejected` reason. Update
  `tact_protocol`, `agent_tui_kit::bridge`, status-bar state, the
  `tact-ui::driver`, and the TUI `/model` flow together; exhaustive matches
  must compile in every host.
- [ ] Update `crates/tact-ui/src/driver.rs` and the TUI command caller to await
  the acknowledged switch. The TUI must not update the global config, status
  bar, or persistence prompt before `Applied`; a `Rejected` response leaves
  the active agent snapshot, global config, status bar, and prompts unchanged.
  Add tests for first activation after a switch, a concurrent activation
  retaining the old snapshot, build failure rollback, publish failure
  rollback, rejected-request UI rollback, and compactor/subagent inheritance.
- [ ] Migrate `Agent::new`, `Agent::set_model`, `TactKernelConfig`, and
  `TactModelPort::from_provider` together; do not leave one owner on
  `LlmProvider` and another on `Arc<dyn ModelPort>`.
- [ ] Replace `validate_provider_state_binding`'s `LlmProvider` downcast with
  the shared `FacadeMetadata` predicate. Do not create a second implementation
  in the product layer.
- [ ] State the kernel boundary explicitly: today
  `build_model_request` uses `ModelPort::capabilities().model`, so changing
  only `AgentSettings.model` is not a model switch. Rebuild the model in this
  phase; extending the kernel request contract is a separately approved future
  change, not an implementation option here.
- [ ] Migrate subagent construction to the same path.
- [ ] Keep test-only global helpers only where they materially simplify current
  tests; mark them `#[cfg(test)]` or `test-support`.

**Gate:**

- [ ] Production code outside `provider/legacy.rs` does not call
  `get_provider`, `read_provider`, or `init_provider`.
- [ ] The checked-in product-caller inventory is fully migrated or explicitly
  allowlisted as deprecated compatibility, including `tact/src/lib.rs`,
  `config/mod.rs`, `tool/read_image.rs`, `utils/lock.rs`,
  `tact-ui/src/account.rs`, `interactive.rs`, `session_bootstrap.rs`,
  `driver.rs`, and `tui/src/handlers/select.rs`.
- [ ] `cargo test -p tact` passes its focused config/runtime tests.
- [ ] `cargo test -p protocol` and `cargo test -p agent_tui_kit` pass their
  model-switch request/result and bridge tests.
- [ ] `cargo test -p tact-ui` passes its focused bootstrap/session tests.
- [ ] `cargo test -p tui` passes its focused model-selection tests.
- [ ] Native compaction selection uses typed capabilities, not an adapter enum
  check.
- [ ] OpenAI, Kimi, and custom Responses product targets explicitly enable
  native compaction; DeepSeek Responses uses local summary routing. Stable
  custom Responses remains disabled by default.
- [ ] Provider-state binding uses `FacadeMetadata`, not an adapter enum check.
- [ ] Provider-state binding is independent of anonymous credentials and the
  native-compaction capability; a test toggles both without changing whether
  matching Responses state is accepted.
- [ ] `/model` switching across different context windows has a behavior test.
- [ ] A same-context-window model switch test proves the wire model changes
  through the chosen mechanism.
- [ ] `/model` test proves rebuilding updates the active `TactModelPort`, the
  compactor's model facts, and subagent inheritance.
- [ ] `/model` request/ack tests prove a rejected switch leaves the agent
  model snapshot, global config, status bar, and persistence prompt unchanged;
  an applied switch commits all four only after the correlated `Applied`
  result.
- [ ] Provider-state save/load tests cover canonical legacy base URLs and reject
  a different `EndpointIdentity`.
- [ ] A real `SqliteTranscriptStore::load` test reads through
  `ProviderStateSnapshot`, validates matching metadata, and rejects an
  endpoint or metadata-version mismatch before returning a cursor.
- [ ] Session schema tests cover nullable metadata, a `NULL`-row compatibility
  read that fabricates no identity, first-save upgrade, metadata-version
  rejection, canonical endpoint rewrite, one-transaction commit, rollback,
  and idempotent reopening of a checked-in old-schema database without
  `facade_metadata_json`.
- [ ] `SqliteTranscriptStore` tests cover metadata write for cursor replacement,
  metadata removal for cursor clear, no write for `Unchanged`, and rollback of
  transcript/provider-state/metadata as one transaction. A legacy row loads
  metadata as `None`; a newly created row contains the exact injected snapshot.
- [ ] Product construction uses `build_product(..., Retain)` and a unique
  system-prompt marker survives `SqliteUsageSink`, `load_latest_request_body`,
  and `/view-system-prompt`; the stable build strips the same marker.
- [ ] `read_image` and the image-attachment gate consume the activation vision
  snapshot; tests prove a running activation is unaffected by a later global
  config change and production code no longer calls global vision helpers.
- [ ] Account and model-discovery tests use `TactProviderContext` and explicit
  `_for` functions, with no production call to legacy account/discovery
  globals.
- [ ] `Agent::set_model` tests cover build failure, publish failure, first
  activation, concurrent activation, compactor inheritance, and subagent
  inheritance.
- [ ] No production provider-state validator or native-compaction selector
  downcasts `LlmProvider`.

## Task 6: Decompose Responses without behavior changes

**Files:**

- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/openai/responses/adapter.rs`
- `crates/tact_llm/src/openai/responses/request.rs`
- `crates/tact_llm/src/openai/responses/events.rs`
- `crates/tact_llm/src/openai/responses/stream.rs`
- `crates/tact_llm/src/openai/responses/normalize.rs`
- `crates/tact_llm/src/openai/responses/state.rs`
- `crates/tact_llm/src/openai/responses/headers.rs`
- `crates/tact_llm/src/openai/responses/wire.rs`
- `crates/tact_llm/src/openai/responses/convert.rs`
- `crates/tact_llm/src/openai/responses/history.rs`
- `crates/tact_llm/src/openai/responses/capabilities.rs`
- `crates/tact_llm/src/openai/responses/request_options.rs`

**Work:**

- [ ] Move lifecycle code into `adapter.rs`.
- [ ] Move request policy and serialization into `request.rs`.
- [ ] Move SSE decoding and unknown-event preservation into `events.rs`.
- [ ] Keep incremental assembly in `stream.rs`.
- [ ] Move terminal response normalization into `normalize.rs`.
- [ ] Move cursor, provider-state, and compaction updates into `state.rs`.
- [ ] Move header construction into `headers.rs`.
- [ ] Assign `history.rs`, `capabilities.rs`, and `request_options.rs` explicit
  ownership; keep conversion features in `convert.rs` with bounded helpers.
- [ ] Keep the existing fixture paths and test names stable where possible.
- [ ] Remove cross-module test-only re-exports by moving tests beside their
  owner or into `crates/tact_llm/tests/`.
- [ ] Split any source file that still exceeds 1,000 lines by owned
  responsibility; do not hide oversized files by leaving large test modules
  inline.
- [ ] Do not change request JSON, event acceptance, unknown-item retention, or
  state serialization.

**Gate:**

- [ ] `cargo test -p tact_llm responses` passes without fixture edits.
- [ ] Checked-in goldens from Task 0 are byte-equivalent.
- [ ] `openai/responses/mod.rs` is a narrow module declaration and re-export
  file.
- [ ] `find crates/tact_llm/src/openai/responses -name '*.rs' -exec wc -l {} +`
  shows no file over 1,000 lines, with a documented definition of whether the
  line count includes tests.
- [ ] No response wire type is public outside the crate.

## Task 7: Add feature gates and trim the default surface

**Files:**

- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/src/lib.rs`
- `crates/tact_llm/src/openai/mod.rs`
- `crates/tact_llm/src/anthropic/mod.rs`
- `crates/tact_llm/src/error.rs`
- `crates/tact_llm/src/profile.rs`
- `crates/tact_llm/src/types.rs`
- `crates/tact_llm/src/provider/mod.rs`
- `crates/tact_llm/src/provider/factory.rs`
- `crates/tact_llm/src/provider/legacy.rs`
- `crates/tact_llm/src/client.rs`
- `crates/tact_llm/src/account.rs`
- `crates/tact_llm/src/models.rs`
- `crates/tact_llm/src/mock.rs`
- `crates/tact-ui/Cargo.toml`
- `crates/tact-ui/src/lib.rs`
- `crates/tact-ui/src/headless_session.rs`
- `crates/tact-ui/src/test_support.rs`
- `crates/tui/Cargo.toml`
- `crates/tui/src/test_support.rs`
- all workspace crates that depend on `tact_llm`

**Work:**

- [ ] Add `anthropic`, `openai-chat`, `openai-responses`, `openai`, `account`,
  `model-discovery`, `telemetry`, `test-support`, and `legacy-tact` features.
  Account and model discovery remain compatibility features, not stable facade
  exports; their existing global-state behavior stays confined to
  `legacy-tact` until an explicit service API replaces it. Define
  `account = ["legacy-tact"]` and `model-discovery = ["legacy-tact"]` so the
  feature graph cannot expose their symbols without the compatibility surface.
- [ ] Gate optional SDK dependencies behind protocol features.
- [ ] Conditionally compile every provider-specific profile import, request
  option, conversion, and compatibility export. SDK errors remain body-free
  internal classifications rather than feature-gated public variants.
- [ ] Keep reqwest generation differences private to the selected adapter.
- [ ] Gate tracing calls behind `telemetry`.
- [ ] Gate `MockClient` behind `test-support`.
- [ ] Add a `test-support` feature to `tact-ui` and `tui`; gate every library
  path that imports `MockClient` with
  `#[cfg(any(test, feature = "test-support"))]`. Enabling `test-support` only
  as a dev-dependency is invalid because `tact-ui` library code uses the mock.
  `tact-ui` currently exports `test_support` unconditionally and depends on
  `tact` with `test-support` unconditionally; `headless_session.rs` also
  imports `MockClient` unconditionally, and `tact_llm::client` imports it in
  its production module. Gate all of these before claiming a
  `--no-default-features` build.
- [ ] Keep transitional defaults broad in this phase, but do not enable
  `test-support` by default.
- [ ] Enable `test-support` in dev-dependencies for `tact`, `tact-ui`, `tui`,
  and `tact_llm` tests that use `MockClient`.
- [ ] Add explicit feature selections to Tact product crates so a later default
  can become minimal without another source migration.
- [ ] Add `test-support` to dev-dependencies only for test targets that import
  `MockClient`; not for production dependency graphs.
- [ ] Add compile checks for every intended feature combination.
- [ ] Add manifest/compile tests for `account`, `model-discovery`, both
  together, and each without `legacy-tact` (the latter must be rejected or
  normalized by the feature definition, never become a stable-only export).
- [ ] Add a feature-to-symbol test matrix. Verify with
  `--no-default-features` that `anthropic`, `openai-chat`, and
  `openai-responses` each expose their admitted adapter symbols. Keep
  `OpenAiProtocol` and `ChatCompletionsDialect` type-visible in every facade
  build because the compatibility profile fields use them. When a provider
  feature is disabled, `ProviderFactory::build` returns typed
  `ProviderFeatureDisabled` instead of constructing that adapter.
- [ ] Exercise the facade fixture itself under each single-protocol feature,
  not just `cargo tree`; dependency presence alone does not prove the facade
  compiles.
- [ ] Add dependency-tree checks that Anthropic-only does not pull
  `async-openai`, and Chat-only or Responses-only pulls only its required
  protocol SDK.

**Gate:**

- [ ] `cargo check -p tact_llm --no-default-features` passes.
- [ ] `cargo check -p tact_llm --no-default-features --features telemetry`
  passes without provider SDKs.
- [ ] `cargo check -p tact_llm --no-default-features --features anthropic`
  passes.
- [ ] `cargo check -p tact_llm --no-default-features --features openai-chat`
  passes.
- [ ] `cargo check -p tact_llm --no-default-features --features openai-responses`
  passes.
- [ ] `cargo check -p tact_llm --no-default-features --features account`
  passes and enables `legacy-tact`.
- [ ] `cargo check -p tact_llm --no-default-features
  --features model-discovery` passes and enables `legacy-tact`.
- [ ] `cargo check -p tact_llm --no-default-features
  --features account,model-discovery` passes.
- [ ] Stable-only public API fixtures do not see account or model-discovery
  symbols; enabling either feature makes the compatibility API visible with
  `legacy-tact`.
- [ ] `cargo check -p tact_llm --no-default-features --features anthropic`
  still exposes `OpenAiProtocol` and `ChatCompletionsDialect`.
- [ ] The anthropic-only fixture build compiles the shared DTO symbols but
  building an OpenAI target returns `ProviderFeatureDisabled`.
- [ ] A feature-to-symbol compile fixture passes for anthropic-only,
  chat-only, and responses-only builds.
- [ ] Disabled-protocol factory builds and examples return typed errors or are
  absent as specified, with no accidental root re-export.
- [ ] `cargo check -p tact_llm --all-features` passes.
- [ ] `cargo tree -p tact_llm --no-default-features --features anthropic`
  contains no `async-openai` crate.
- [ ] The workspace still builds with its explicit feature set.
- [ ] `cargo check -p tact-ui --no-default-features` and the corresponding
  test-support build both pass.

## Task 8: Add telemetry and final third-party documentation

**Files:**

- `crates/tact_llm/src/provider/mod.rs`
- `crates/tact_llm/src/telemetry.rs`
- `crates/tact_llm/src/provider/factory.rs`
- `crates/tact_llm/src/anthropic/`
- `crates/tact_llm/src/openai/compatible/`
- `crates/tact_llm/src/openai/responses/`
- `crates/tact_llm/README.md`
- `ARCHITECTURE.md`

**Work:**

- [ ] Add spans for factory construction, stream, complete, compact, and
  account calls.
- [ ] Record provider, protocol, model, endpoint host, request kind, status
  class, error kind, retry-after presence, elapsed time, and time to first
  stream event.
- [ ] Never record credentials, authorization headers, prompt content,
  response content, or provider-state payloads.
- [ ] Complete the Task 4 redaction baseline before adding telemetry; audit
  remaining content-bearing `tracing::debug!`/`warn!` paths for request bodies,
  response bodies, tool arguments, and raw SSE payloads.
- [ ] Do not expose request/response bodies through `LlmError` rendering or
  logging. The stable facade path always strips `request_body`. Tact's
  feature-gated product path may retain it only through
  `ProviderFactory::build_product(..., ProductRequestBodyPolicy::Retain)` for
  the bounded usage store and `/view-system-prompt`; no library log or public
  error may expose those bytes.
- [ ] Document that the host owns the subscriber.
- [ ] Write the third-party README around `ModelTarget` and
  `ProviderFactory`, not legacy globals.
- [ ] Document custom endpoints, anonymous local endpoints, refreshable
  credentials, direct `ModelPort`, `AgentBuilder`, feature selection, tracing,
  and semver expectations.
- [ ] Add a migration table from legacy imports to facade imports.
- [ ] Update `ARCHITECTURE.md` with the provider-factory flow.
- [ ] Add publish metadata: description, repository, license, README, keywords,
  categories, and feature documentation.
- [ ] Document that v1 is Tokio-based and that custom HTTP stacks require a
  custom `ModelPort`.

**Gate:**

- [ ] `cargo test -p tact_llm telemetry` passes.
- [ ] A test subscriber proves required fields are recorded.
- [ ] A content-bound test proves prompts, responses, and secrets are absent.
- [ ] `cargo doc -p tact_llm --no-deps` passes.
- [ ] README examples compile or pass through the example compile gate.
- [ ] `cargo package -p tact_llm --no-verify --list` includes the README,
  license, examples, and no unintended local-only files; this is an inventory
  check only.
- [ ] `cargo package -p tact_llm --no-verify --list` is not treated as proof
  of registry readiness.

## Task 9: Release-owner publication checklist

This task is a release gate outside the current implementation definition of
done. It does not block the code work above unless the user explicitly requests
registry publication.

**Files:**

- `crates/tact_llm/Cargo.toml`
- `crates/agent_api/Cargo.toml`
- `Cargo.toml`
- `deny.toml` or the repository's dependency policy file

**Work:**

- [ ] Release owner adds a registry-compatible version constraint for
  `tact_agent_api` while retaining the workspace path.
- [ ] Release owner publishes or versions `tact_agent_api` before `tact_llm`.
- [ ] Release owner replaces the Git-only Responses dependency with a published
  fork, or documents that registry publication is blocked.
- [ ] Release owner decides and tests the MSRV.
- [ ] Release owner runs a real package verification and publish dry-run in
  dependency order.

**Gate:**

- [ ] Every publishable feature set passes a real package verification.
- [ ] `cargo publish --dry-run -p tact_llm` succeeds, or the README records the
  exact registry blocker. Registry publication is not part of this phase's
  code-completion claim.

## Verification

- [ ] Run focused tests after each task; do not run Cargo commands in parallel.
- [ ] `cargo fmt -- --check` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test -p tact_llm --all-features` passes.
- [ ] Affected `tact`, `tact-ui`, and `tui` focused tests pass.
- [ ] `ProviderInfo::build_client()` compatibility tests pass inside and
  outside Tokio.
- [ ] The workspace manifest pins `async-openai-responses` to
  `12488eb3f6f2dbf35082a1121f2192dd1c2a7c27`; `Cargo.lock` matches and
  `cargo metadata --locked` succeeds without branch-based resolution.
- [ ] Cancellation, retry-after, redaction, endpoint-canonicalization, and
  feature-symbol matrix tests pass.
- [ ] SDK-backed Responses stream/complete 429 tests report
  `retry_after = None`; direct Anthropic, Chat, and Responses
  native-compaction paths propagate parsed `Retry-After`.
- [ ] The real session-store load path returns `ProviderStateSnapshot`,
  validates metadata before returning a cursor, and rejects endpoint and
  schema-version mismatches.
- [ ] Product request-body retention persists a unique system-prompt marker
  through the usage store and `/view-system-prompt`, while stable `build`
  strips it.
- [ ] `/model` request/ack tests prove rejected switches do not mutate the
  active agent snapshot, global config, status bar, or persistence prompt.
- [ ] Product native-compaction tests cover OpenAI, Kimi, custom, and DeepSeek
  Responses targets with the intended capability and local-compaction routing.
- [ ] `scripts/check-rust.sh` passes once at the end.
- [ ] `python3 scripts/book-check.py book` passes.
- [ ] `git diff --check` passes.
- [ ] `git status --short` contains no unrelated staged files.
- [ ] A Markdown structure check reports balanced code fences and valid
  relative links in the spec and this plan.
- [ ] A clean consumer build uses an independent manifest rather than an
  in-crate example, runs with `--locked`, and resolves the pinned Git source.

## Definition of Done

- [ ] A third-party host can create a model with `ProviderFactory`, compose it
  with `tact_agent_sdk::AgentBuilder`, and run without process-global setup.
- [ ] Static API key, anonymous custom endpoint, refreshable credentials, and
  direct `ModelPort` streaming are documented and compile-tested.
- [ ] One policy module owns all provider/model heuristics.
- [ ] Legacy construction paths delegate to the factory.
- [ ] `ProviderInfo::build_client()` remains synchronous and successful in a
  Tokio runtime.
- [ ] `Retry-After` reaches the kernel through `ModelError.retry_after` on
  direct HTTP paths; SDK-backed Responses stream/complete report an explicit
  safe `None` without modifying the pinned fork.
- [ ] Product provider-state binding no longer downcasts `LlmProvider`.
- [ ] Product model construction uses `build_product(..., Retain)` so the
  bounded usage store and `/view-system-prompt` still receive the dispatched
  system prompt, while stable `build` strips request bytes.
- [ ] `SqliteTranscriptStore::load` validates a real `ProviderStateSnapshot`
  through the product-only load capability before returning a cursor; matching
  metadata loads and endpoint/schema mismatches fail early.
- [ ] `Agent::set_model` publishes only a fully built model, metadata, vision,
  and legacy snapshot. A rejected switch leaves the agent snapshot, global
  config, status bar, and persistence prompts unchanged.
- [ ] No library log or public error rendering contains prompt, response,
  credential, tool-argument, or raw-SSE payloads.
- [ ] The Responses implementation is internally decomposed with unchanged
  behavior.
- [ ] Product Responses targets preserve current native-compaction routing for
  OpenAI, Kimi, and custom endpoints, while DeepSeek keeps local summary
  routing.
- [ ] Feature combinations avoid unrelated protocol SDKs.
- [ ] Public docs expose the facade, not wire or transport internals.
- [ ] The full repository gate passes.
- [ ] The Responses Git dependency is pinned by immutable workspace revision
  and the independent consumer builds with `--locked`.
- [ ] Registry publication is explicitly deferred to the release-owner
  checklist and is not claimed by this implementation.
