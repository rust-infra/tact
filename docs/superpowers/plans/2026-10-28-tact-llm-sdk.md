# Tact LLM SDK Implementation Plan

> **For agentic workers:** Follow the tasks in order. Keep every task
> independently reviewable and do not start the next task until its gate is
> green. Use checkbox syntax (`- [ ]`) to track implementation.

**Goal:** Make `tact_llm` a third-party-ready provider SDK whose stable path is
`provider constructor -> ProviderBuilder -> Provider`, with `Provider`
implementing `tact_agent_api::ModelPort`, while preserving current Tact
behavior during a staged migration.

**Architecture:** Keep one `tact_llm` crate in this phase. Build the stable
facade on the existing direct HTTP adapters, move all provider/model/endpoint
interpretation into one private policy owner, and retain product compatibility
behind features during migration. Construction is synchronous and local.
Credentials are resolved once per dispatched call. Streams are lazy and
cancel-safe. Product code switches to one immutable activation snapshot before
legacy globals and concrete adapters become unreachable from a stable build.

**Tech stack:** Rust 2024, `tact_agent_api`, Tokio, `futures-util`, existing
Anthropic/Chat Completions/Responses HTTP paths, reqwest, SSE parsers, serde,
secrecy, tracing, wiremock, and checked-in wire/SSE fixtures.

**Spec:**
[`docs/superpowers/specs/2026-10-28-tact-llm-sdk-design.md`](../specs/2026-10-28-tact-llm-sdk-design.md)

**Current status (2026-10-28):** proposed; no implementation started.

## Migration Order

The tasks are deliberately ordered around two hard constraints:

1. The workspace must remain buildable while compatibility callers still
   import legacy modules.
2. The package default can become `core` only after every workspace consumer
   has explicit feature selections and all product code has moved to an
   activation snapshot.

Do not move Task 7's `default = ["core"]` manifest change earlier. Adding
optional dependencies and explicit consumer features while the transitional
default remains broad is a separate, verifiable checkpoint.

## Global Constraints

- Do not commit or push unless explicitly requested.
- Work on a feature branch. Never commit to `main`.
- Keep diffs small and reviewable. Preserve unrelated untracked files.
- Run one Cargo invocation at a time; parallel commands contend on `target/`.
- Use focused tests while changing code. Run the full gate once through
  `scripts/check-rust.sh` after the affected task is otherwise green.
- Keep `tact_agent_api::ModelPort` as the only public model-call trait.
- Plugin style means constructor injection, small typed contracts, and static
  composition. Do not add a runtime provider registry or dynamic loader.
- `ProviderBuilder::build` must not perform network IO, account discovery, or
  refreshable credential resolution.
- Stable builds must not expose provider SDKs, wire DTOs, SSE frames, body
  hooks, transport clients, concrete adapters, or legacy globals.
- Do not log prompts, responses, request bodies, response bodies, tool
  arguments, raw SSE, credentials, authorization headers, or provider-state
  payloads.
- Library code never installs a tracing subscriber.
- Preserve existing request JSON, headers, SSE behavior, response
  normalization, provider-state bytes, account behavior, and model discovery
  until a task explicitly replaces a documented defect.
- Prefer enums for every closed kind, status, protocol, stage, and error class.
- Do not introduce `#[doc(hidden)]` as a substitute for module privacy.

## Review Focus

- `Provider` is the runtime seam and implements `ModelPort` directly.
- `ProviderBuilder::build` is synchronous and IO-free with respect to
  credentials and network calls.
- Static credentials are validated at build; refreshable credentials are
  resolved once per dispatch, never at build.
- `Endpoint` is both the canonical dispatch base and provider-state binding
  value. Raw profile URL spellings are never compared.
- One `policy.rs` owns built-in endpoint defaults, dialects, vision,
  compaction, reasoning encoding, hosted tools, session headers, and legacy
  eligibility.
- Shared request validation runs before credential resolution and dispatch for
  `stream`, `complete`, and `compact`.
- A stream dispatches nothing before first poll and emits exactly one terminal
  outcome.
- Stable `ModelResponse.request_body` is always `None`; only
  `build_product()` may enable bounded product capture.
- Product code uses an immutable activation snapshot, not `LlmProvider`
  downcasts, for Responses binding, compaction, vision, and subagents.
- The complete `/model` request/ack protocol is not redesigned here. This plan
  only prevents publishing model settings before replacement construction
  succeeds.
- Third-party consumer fixtures use `default-features = false` and exactly one
  selected protocol in the final matrix.

## Task 0: Freeze Behavior And Add External Baselines

**Files:**

- `crates/tact_llm/src/anthropic/mod.rs`
- `crates/tact_llm/src/openai/compatible/`
- `crates/tact_llm/src/openai/responses/`
- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/provider_model.rs`
- `crates/tact_llm/src/neutral.rs`
- `crates/tact_llm/src/error.rs`
- `crates/tact_llm/src/account.rs`
- `crates/tact_llm/src/models.rs`
- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/tests/`
- `crates/agent_api/src/model.rs`
- `crates/agent_core/src/kernel.rs`
- `crates/tact/src/agent/mod.rs`
- `crates/tact/src/runtime/composition.rs`
- `crates/tact/src/store/session_store/sqlite.rs`
- `crates/tact-ui/src/driver.rs`
- `crates/tui/src/handlers/select.rs`
- `fixtures/tact-llm-consumer/`
- `fixtures/tact-llm-agent-sdk-consumer/`

**Work:**

- [ ] Add a provider matrix covering Anthropic, OpenAI Chat Completions,
  OpenAI Responses, DeepSeek Chat, DeepSeek Responses, Kimi Chat, Kimi
  Responses, custom Chat, custom Responses, compaction, and the legacy
  same-family request-model switch.
- [ ] Check in request-body and header goldens under
  `crates/tact_llm/tests/goldens/`. Capture the JSON bytes actually sent and
  the effective URL, not the profile's configured base URL.
- [ ] Replay every checked-in Responses SSE fixture and snapshot normalized
  content, stop reason, usage, hostile/unknown item retention, and cursor
  update.
- [ ] Characterize `stream`, `complete`, and `compact` at pre-dispatch,
  first-event wait, mid-stream, post-terminal, drop-before-poll, and
  drop-mid-stream points. Record the current eager-spawn behavior separately
  from the target contract.
- [ ] Characterize provider-state save/load, canonical and non-canonical base
  URL spellings, `"openai_responses"` and legacy `"openai"` markers, and
  current `LlmProvider` downcast validation.
- [ ] Characterize account failure paths and model discovery. Add body-free
  snapshots for current account errors after Task 5 replaces body-bearing
  text.
- [ ] Add canary tests for build errors, runtime errors, `Debug`, `Display`,
  `source()` chains, HTTP error bodies, malformed Responses payloads, account
  errors, and captured tracing targets. Use unique credential, prompt,
  response-body, SSE, and tool-argument markers.
- [ ] Add a reviewed redaction inventory with exact source locations for every
  content-bearing trace or error field. A guard must fail with file and line
  when a new marker-bearing field is added.
- [ ] Add a feature-edge inventory naming the protocol, transport, SSE,
  secrecy, and tracing dependency each source target needs.
- [ ] Check in a public-symbol inventory. Use `cargo public-api` when
  available; otherwise commit the reviewed `rg`-generated baseline.
- [ ] Create `fixtures/tact-llm-consumer/` with an empty `[workspace]`, a path
  dependency on `../../crates/tact_llm`, and no `tact_agent_sdk` dependency.
  Initially it may use the transitional broad defaults; Task 7 converts its
  invocations to the strict single-protocol matrix.
- [ ] Create `fixtures/tact-llm-agent-sdk-consumer/` with explicit
  `tact_llm` and `tact_agent_sdk` path dependencies. Its composition test
  passes a `ModelPort` to `AgentBuilder`.
- [ ] Record the current product behavior that must change later: separate
  `provider` and `capabilities` fields, per-composition client cloning,
  global vision reads, `/model` optimistic publication, and request-body
  capture in stable calls.
- [ ] Pin the workspace `async-openai-responses` dependency to a reviewed
  immutable revision before using fixture results as a reproducibility claim.

**Gate:**

- [ ] `cargo test -p tact_llm provider_policy` passes against the unmodified
  production path.
- [ ] `cargo test -p tact_llm responses` passes with all checked-in SSE
  fixtures.
- [ ] `cargo test -p tact_llm cancellation` records current behavior and
  clearly labels known eager-dispatch defects.
- [ ] The redaction guard passes and fails when a marker-bearing source field
  is added to a temporary copy.
- [ ] The public-symbol and feature-edge inventories are reproducible from a
  clean checkout.
- [ ] Both external fixture manifests resolve without a Cargo workspace error.
- [ ] Removing `target/` does not remove any required baseline.

## Task 1: Migrate Context Windows To `Option<usize>`

**Files:**

- `crates/agent_api/src/model.rs`
- `crates/agent_api/src/usage.rs`
- `crates/agent_core/src/kernel.rs`
- `crates/agent_core/src/model_scope.rs`
- `crates/agent_core/src/test_support/mod.rs`
- `crates/agent_sdk/examples/minimal.rs`
- `crates/agent_sdk/examples/tool_agent.rs`
- `crates/agent_sdk/src/telemetry/model.rs`
- `crates/agent_sdk/src/telemetry/tests.rs`
- `crates/agent_sdk/src/telemetry_config.rs`
- `crates/tact_llm/src/provider_model.rs`
- `crates/tact/src/agent/mod.rs`
- `crates/tact/src/agent/tests/prompt.rs`
- `crates/tact/src/runtime/composition.rs`
- `crates/tact/src/runtime/model_adapter.rs`
- `crates/tact/src/runtime/context_adapter.rs`
- `crates/tact/src/runtime/context_adapter/tests.rs`
- `crates/tact/src/runtime/prompt_adapter.rs`
- `crates/tact/src/runtime/compaction_adapter.rs`
- `crates/tact/src/runtime/compaction_adapter/tests.rs`
- `crates/tact/src/compact/mod.rs`
- `crates/tact/src/skill/mod.rs`
- `crates/tact-ui/src/interactive.rs`
- `crates/tact-ui/tests/recovery_compaction.rs`
- `crates/tui/src/lib.rs`
- `crates/tui/src/widgets/state/mod.rs`
- `crates/tui/src/widgets/state/app/agent.rs`
- `crates/tui/src/render/`
- `crates/agent_tui_kit/src/state/status_bar_state.rs`

**Work:**

- [ ] Change `ModelCapabilities.context_window` to `Option<usize>`.
- [ ] Add a checked `ModelCapabilities` constructor that rejects
  `Some(0)`, zero output tokens, and output greater than or equal to a known
  context window.
- [ ] Add `known_context_window() -> Option<usize>`. It returns `None` for
  `None` and also for a directly constructed `Some(0)` so public fields cannot
  bypass the invariant at arithmetic boundaries.
- [ ] Implement custom serde for `context_window`: deserialize legacy `0` and
  `null` to `None`; serialize `None` as numeric `0` to keep existing stored
  JSON readable.
- [ ] Migrate every `ModelCapabilities` constructor and consumer in one task.
  Do not leave one crate compiling against `usize`.
- [ ] Map product `model_context_window = 0` to `None` when building
  `ModelCapabilities`. Keep the product config's `usize` representation where
  it encodes host policy, but do not leak zero into the stable capability.
- [ ] Introduce one product-local context-budget decision helper that
  distinguishes disabled, unknown, and known windows. Use it in context,
  prompt, status, and compaction paths instead of raw `== 0` checks.
- [ ] Replace percentage, headroom, subtraction, and min/max arithmetic on
  `context_window` with `known_context_window()` plus an explicit product
  fallback.
- [ ] Change TUI/status fields that display provider capabilities to
  `Option<usize>` and render unknown as unknown rather than `0`.
- [ ] Add serde compatibility tests for `0`, `null`, a positive value, and
  reserialization of `None` as `0`.
- [ ] Add behavior tests for known, unknown-with-fallback, and
  unknown-without-fallback auto-compaction decisions.

**Gate:**

- [ ] `cargo test -p tact_agent_api context_window` passes.
- [ ] `cargo test -p tact_agent_core context_window` and
  `cargo test -p tact_agent_core model_scope` pass.
- [ ] `cargo test -p tact_agent_sdk telemetry` passes.
- [ ] Focused `tact` tests for context, compaction, prompt budget, and skill
  catalog pass.
- [ ] Focused `tact-ui` recovery-compaction tests pass.
- [ ] Focused `tui` status-bar and task-panel tests pass.
- [ ] `rg` finds no percentage or headroom arithmetic that treats
  `context_window == 0` as a known window in the affected crates.

## Task 2: Add The Stable Builder, Endpoint, Metadata, And Policy

**Files:**

- `crates/tact_llm/src/lib.rs`
- `crates/tact_llm/src/builder.rs`
- `crates/tact_llm/src/endpoint.rs`
- `crates/tact_llm/src/error.rs`
- `crates/tact_llm/src/limits.rs`
- `crates/tact_llm/src/metadata.rs`
- `crates/tact_llm/src/model.rs`
- `crates/tact_llm/src/policy.rs`
- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/provider_model.rs`
- `crates/tact_llm/src/profile.rs`
- `crates/tact_llm/src/types.rs`
- `crates/tact_llm/src/hook_select.rs`
- `crates/tact_llm/src/opencode.rs`
- `crates/tact_llm/src/openai/compatible/body.rs`
- `crates/tact_llm/src/openai/responses/convert.rs`
- `crates/tact_llm/tests/facade.rs`
- `crates/tact_llm/tests/endpoint.rs`
- `fixtures/tact-llm-consumer/src/lib.rs`

**Work:**

- [ ] Define `ModelLimits`, `SessionId`, `Endpoint`, `ProviderMetadata`,
  `ProviderKind`, `WireProtocol`, `ChatDialect`, `BuildError`,
  `EndpointError`, `LimitError`, `SessionIdError`, and
  `TransportSetupStage` with the exact invariants in the spec.
- [ ] Make `Endpoint` opaque. Implement URL parsing and canonicalization for
  absolute HTTP(S), host, default port, empty path, one trailing slash,
  percent encoding, IPv6, userinfo rejection, and query/fragment rejection.
- [ ] Use `Endpoint` as the only adapter dispatch base and append exactly one
  protocol suffix: `/messages`, `/chat/completions`, or `/responses`.
- [ ] Define `ProviderBuilder` with private immutable state and constructor
  functions for Anthropic, OpenAI Chat/Responses, DeepSeek Chat/Responses,
  Kimi Chat/Responses, and custom Chat/Responses.
- [ ] Implement synchronous `build` validation for model, endpoint, limits,
  session id, typed option applicability, and required static authentication.
  Do not resolve refreshable or legacy credential providers in this task.
- [ ] Add static API-key, bearer, and anonymous builder modes. Encapsulate the
  credential in an opaque value with redacted `Debug`; refreshable sources are
  added in Task 3.
- [ ] Build one private adapter configuration from the selected profile and
  policy. A custom provider defaults to standard protocol behavior and never
  chooses a vendor dialect from its name or URL.
- [ ] Create `policy.rs` as the sole owner of built-in endpoint defaults,
  provider/protocol syntax, reasoning encoding, hosted tools, compaction
  defaults, vision defaults, prior-reasoning replay, session-header behavior,
  and legacy account/discovery eligibility.
- [ ] Move existing vendor/model/endpoint heuristics from `profile.rs`,
  `types.rs`, `hook_select.rs`, `opencode.rs`, body hooks, and Responses
  conversion into `policy.rs`. Compatibility methods may delegate but must not
  keep a second decision tree.
- [ ] Define an internal provider implementation that owns the immutable
  `ProviderPolicy`, selected adapter, `ModelCapabilities`, vision fact, and
  canonical endpoint. Do not expose `LlmProvider` or a concrete adapter.
- [ ] Define `Provider` as an `Arc`-backed clone with a manually redacted
  `Debug`, `metadata()`, `into_model()`, and a direct `ModelPort`
  implementation.
- [ ] Resolve capabilities and vision once during `build`; `capabilities()`
  returns the stored facts and does not re-run policy.
- [ ] Add one shared `model.rs` conversion/validation path for `stream`,
  `complete`, and `compact`. For stable providers, reject an already-cancelled
  token, zero output tokens, output above the built maximum, request model
  mismatch, cursor protocol/endpoint mismatch, and unsupported native
  compaction before adapter dispatch.
- [ ] Keep legacy same-family request-model switching out of the stable path.
  The compatibility bridge added in Task 3 may select an explicit legacy mode.
- [ ] Re-export only the curated facade from `lib.rs`. Leave existing module
  visibility unchanged temporarily so legacy callers continue to compile.
- [ ] Add a facade-only external consumer that constructs a static provider
  and calls its `ModelPort` methods.

**Gate:**

- [ ] `cargo test -p tact_llm endpoint` covers every normalization and
  rejection rule, including empty `?`/`#` and IPv6.
- [ ] `cargo test -p tact_llm limits` covers zero context, zero output,
  oversized output, valid unknown windows, and header-unsafe session ids.
- [ ] `cargo test -p tact_llm builder` proves building performs no network
  request and required built-in auth fails without a dispatch attempt.
- [ ] `cargo test -p tact_llm facade` proves `Provider: ModelPort`,
  `Provider::clone`, redacted debug, capabilities, and strict request model
  equality.
- [ ] Wiremock tests prove the canonical endpoint plus fixed protocol suffix is
  the effective request URL for every adapter.
- [ ] Vendor-looking custom names and model ids do not change standard
  protocol policy without an explicit typed option.
- [ ] The direct external fixture builds with the transitional manifest.

## Task 3: Add Per-Dispatch Authentication And Shared Legacy Assembly

**Files:**

- `crates/tact_llm/src/auth.rs`
- `crates/tact_llm/src/builder.rs`
- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/transport.rs`
- `crates/tact_llm/src/anthropic/mod.rs`
- `crates/tact_llm/src/openai/compatible/mod.rs`
- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/provider_model.rs`
- `crates/tact_llm/src/client.rs`
- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/account.rs`
- `crates/tact_llm/src/models.rs`
- `crates/tact_llm/tests/auth.rs`
- `crates/tact_llm/tests/legacy_assembly.rs`
- `crates/tact/src/lib.rs`
- `crates/tact-ui/src/session_bootstrap.rs`

**Work:**

- [ ] Add `Credential`, `CredentialSource`, `Authentication`, and
  `CredentialError` with the spec's validation and redaction rules.
- [ ] Add `ProviderBuilder::authentication`, `api_key`, `bearer`,
  `refreshable`, and `anonymous`. Static values validate at build;
  refreshable sources are stored without resolution.
- [ ] Refactor each adapter so immutable transport/policy state is built once
  and one resolved credential is transformed into private request headers or
  body fields per dispatch.
- [ ] Preserve header ownership: Anthropic API key uses `x-api-key`, Anthropic
  bearer uses `Authorization`, built-in OpenAI-compatible providers use
  bearer, and custom anonymous endpoints omit authorization.
- [ ] Add a private assembly function that contains the only provider/protocol
  construction match. Route stable builders and compatibility constructors
  through it.
- [ ] Replace duplicate provider matches in `ProviderInfo::build_client`,
  `Client::new`, and `get_llm_client` with delegates to the shared assembly.
  Map structured failures to `anyhow` only at the legacy boundary.
- [ ] Keep `ProviderInfo::build_client()` synchronous and safe both inside and
  outside a Tokio runtime.
- [ ] Preserve legacy account and model discovery as separately authorized
  compatibility operations. They remain static-API-key-only and do not become
  stable facade methods.
- [ ] Apply session id, replay-prior-reasoning, and native-compaction overrides
  from the builder to the private adapter configuration.
- [ ] Ensure all three `ModelPort` methods validate request and cursor before
  credential resolution. Resolve exactly one credential per valid dispatch.
- [ ] A failed refresh is a permanent authentication `ModelError` before
  network dispatch. A missing required credential has the same pre-dispatch
  behavior.
- [ ] Add source guards proving provider/protocol selection exists only in the
  private assembly and policy layer.

**Gate:**

- [ ] `cargo test -p tact_llm auth` passes for static key, static bearer,
  anonymous custom, missing built-in auth, refresh failure, and redacted debug.
- [ ] Resolve-count tests prove build resolves zero, each valid
  stream/complete/compact call resolves exactly one, and cancelled or invalid
  calls resolve zero.
- [ ] A refreshed token changes the next request header without rebuilding the
  provider.
- [ ] Anonymous custom Chat and Responses endpoints send no authorization
  header.
- [ ] Shared validation tests prove strict stable model equality, zero and
  oversized output rejection, cursor mismatch rejection, and native
  compaction rejection before auth.
- [ ] `cargo test -p tact_llm legacy_assembly` proves legacy constructors use
  the shared path and `ProviderInfo::build_client()` succeeds in and outside a
  runtime.
- [ ] `rg` finds one provider/protocol construction match and no second policy
  table outside reviewed compatibility delegates.

## Task 4: Make Streaming Lazy And Cancellation Single-Terminal

**Files:**

- `crates/tact_llm/src/stream.rs`
- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/provider_model.rs`
- `crates/tact_llm/src/model.rs`
- `crates/tact_llm/src/anthropic/mod.rs`
- `crates/tact_llm/src/openai/compatible/mod.rs`
- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/tests/stream_cancellation.rs`

**Work:**

- [ ] Return a lazy stream wrapper from `Provider::stream`. Construction of
  the stream wrapper performs no spawn and no adapter call.
- [ ] On first `poll_next`, check cancellation before credential resolution or
  dispatch. If cancelled, yield one `Cancelled` error and spawn nothing.
- [ ] Spawn one producer only after the cancellation check and store an
  abort-on-drop handle.
- [ ] Resolve auth inside the producer, recheck cancellation, then emit
  `MessageStarted` only if the call is still live.
- [ ] Poll the adapter with cancellation-first biased selection and recheck
  cancellation immediately before claiming the terminal slot.
- [ ] Implement one terminal state machine:
  `Open -> Finished | Failed | Cancelled`. Only one terminal item is visible;
  after it the wrapper clears the producer/receiver and returns `None`.
- [ ] Drop of an unpolled stream dispatches nothing. Drop of a live stream
  aborts the producer. Drop never cancels the caller-owned `CancelToken`.
- [ ] Send progress best-effort through a bounded channel and send the terminal
  response with backpressure.
- [ ] Add cancellation-first checks to `complete` and `compact` before polling
  the adapter future.
- [ ] Add deterministic race tests for provider-first and cancellation-first
  readiness, including simultaneous readiness choosing cancellation.
- [ ] Keep `request_body` handling unchanged in this task; Task 5 makes stable
  `None` explicit and adds product-only capture.

**Gate:**

- [ ] Drop-before-poll produces zero adapter calls and zero spawned producers.
- [ ] Drop-mid-stream leaves no live producer task.
- [ ] Exactly one terminal `Finished`, `Failed`, or `Cancelled` item is
  emitted, and the next poll returns `None`.
- [ ] Cancellation between `stream()` return and first poll yields exactly one
  `Cancelled` item and no `MessageStarted`.
- [ ] Already-cancelled `complete` and `compact` do not poll the adapter.
- [ ] All baseline stream fixtures from Task 0 still produce identical
  progress and terminal content.

## Task 5: Bound HTTP Errors, Redact Content, And Add Host-Owned Tracing

**Files:**

- `crates/tact_llm/Cargo.toml`
- `Cargo.lock`
- `crates/tact_llm/src/transport.rs`
- `crates/tact_llm/src/error.rs`
- `crates/tact_llm/src/neutral.rs`
- `crates/tact_llm/src/telemetry.rs`
- `crates/tact_llm/src/builder.rs`
- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/anthropic/mod.rs`
- `crates/tact_llm/src/openai/compatible/mod.rs`
- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/openai/responses/stream.rs`
- `crates/tact_llm/src/account.rs`
- `crates/tact_llm/src/models.rs`
- `crates/agent_api/src/model.rs`
- `crates/tact/src/recovery.rs`
- `crates/tact-ui/tests/recovery_compaction.rs`
- `crates/tact_llm/tests/http_errors.rs`
- `crates/tact_llm/tests/redaction.rs`
- `crates/tact_llm/tests/telemetry.rs`

**Work:**

- [ ] Add one private bounded HTTP error reader used by Anthropic, Chat
  Completions, and Responses. Read at most
  `MAX_HTTP_ERROR_BODY_BYTES + 1`, parse status and `Retry-After` first, and
  discard body bytes before constructing public errors.
- [ ] Classify authentication, context overflow, transient 408/429/5xx, and
  native-compaction 404/405 inside `tact_llm`. Public `ModelError.message`
  contains only fixed category text, status class, and safe retry facts.
- [ ] Replace body-bearing `MessageError` and `LlmError` construction on
  executed paths with typed, body-free classifications. Preserve the legacy
  error types only as gated compatibility plumbing.
- [ ] Parse `Retry-After` delta-seconds and HTTP dates through one helper;
  malformed or absent values become `None`. Propagate the value to
  `ModelError.retry_after`.
- [ ] Replace executed Responses SDK HTTP calls with direct reqwest 0.13 plus
  the existing SSE/wire normalization. The stable path must not call
  `client.responses()` or a dependency deserializer that retains the body.
- [ ] Reuse the existing Responses wire event types where safe, but deserialize
  already-buffered JSON values through private request/event code.
- [ ] Add a stable-vs-product capture mode. Stable `build` and all stable calls
  set `ModelResponse.request_body = None`. The product mode added in Task 6
  may capture exact bounded bytes.
- [ ] Give `ModelResponse` a manually redacted `Debug` so captured product
  bytes cannot appear through diagnostics.
- [ ] Remove request-body, response-body, raw-SSE, tool-argument, and
  authorization logging from executed paths. Account failures must not
  interpolate `resp.text()`.
- [ ] Add `telemetry.rs` behind the `telemetry` feature. Record provider kind,
  protocol, model id, endpoint host, request kind, elapsed time, time to first
  event, status class, error kind, retry delay, cancellation, outcome, and
  token counts when available.
- [ ] Never record prompts, responses, tool arguments, request/response bodies,
  credentials, authorization headers, raw SSE, or provider-state payloads.
- [ ] Add recursive error-chain tests and subscriber-capture tests that include
  dependency targets. A malformed Responses payload marker must be absent from
  every stable adapter and dependency log.

**Gate:**

- [ ] `cargo test -p tact_llm http_errors` proves bounded reads, typed
  classification, safe status text, and `Retry-After` parsing for delta
  seconds, HTTP dates, malformed values, and absence.
- [ ] Kernel recovery uses a propagated retry delay on the first retry for
  stream, complete, local compaction, and native compaction.
- [ ] A context-overflow response maps to `ModelErrorKind::ContextOverflow`
  without exposing its body.
- [ ] Responses stream, complete, and compact all execute the private direct
  HTTP path; a source guard finds no stable `client.responses()` call.
- [ ] `cargo test -p tact_llm redaction` finds no canary in errors, debug,
  display, source chains, or captured events from `tact_llm` and dependency
  targets.
- [ ] `cargo test -p tact_llm telemetry` proves required fields exist and
  content/credential canaries are absent.
- [ ] Stable provider tests always observe `request_body.is_none()`.
- [ ] `cargo check -p tact_llm --no-default-features --features telemetry`
  passes after the feature is declared; provider dependencies stay absent.

## Task 6: Publish One Product Activation Snapshot

**Files:**

- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/src/builder.rs`
- `crates/tact_llm/src/provider.rs`
- `crates/tact_llm/src/neutral.rs`
- `crates/agent_api/src/model.rs`
- `crates/tact/src/agent/mod.rs`
- `crates/tact/src/runtime/activation_view.rs`
- `crates/tact/src/runtime/composition.rs`
- `crates/tact/src/runtime/model_adapter.rs`
- `crates/tact/src/runtime/store_adapter.rs`
- `crates/tact/src/runtime/compaction_adapter.rs`
- `crates/tact/src/runtime/usage_adapter.rs`
- `crates/tact/src/tool/read_image.rs`
- `crates/tact/src/tool/subagent.rs`
- `crates/tact/src/config/mod.rs`
- `crates/tact/src/config/resolve.rs`
- `crates/tact/src/store/session_store/mod.rs`
- `crates/tact/src/store/session_store/sqlite.rs`
- `crates/tact-ui/src/session_bootstrap.rs`
- `crates/tact-ui/src/driver.rs`
- `crates/tact-ui/tests/recovery_compaction.rs`
- `crates/tui/src/handlers/select.rs`
- `crates/tui/src/widgets/state/app/agent.rs`

**Work:**

- [ ] Add non-default `product-runtime` and make it imply `legacy-tact`.
  Implement `ProviderBuilder::build_product()` through the same build path,
  with private bounded product request capture.
- [ ] Define product-owned `ActiveModelSnapshot` containing the immutable
  `Arc<dyn ModelPort>`, `ProviderMetadata`, and product activation facts
  needed for vision and compaction. Do not keep separate mutable
  `provider`/`capabilities` owners.
- [ ] Replace `TactKernelConfig.provider + capabilities` with one cloned
  snapshot. `TactModelPort` delegates to the captured model instead of
  constructing a public `ProviderModel`.
- [ ] Clone the activation snapshot once per turn and retain it through model
  calls, compaction, vision gates, status facts, and subagent inheritance.
- [ ] Build product providers through `ProviderBuilder::build_product()` after
  the session id is known. Preserve bounded request bytes for usage storage and
  `/view-system-prompt`; prove a unique system-prompt marker round-trips.
- [ ] Replace `LlmProvider` downcast checks for Responses binding, native
  compaction, and vision with `ProviderMetadata` and capabilities from the
  activation snapshot.
- [ ] Validate loaded provider state against canonical `Endpoint` and
  `WireProtocol`. Continue accepting existing session rows without adding a
  metadata schema or reconstructing facts the old row never stored.
- [ ] Keep compatibility marker validation inside the product bridge, but do
  not compare raw profile URL spellings.
- [ ] Route OpenAI, Kimi, and custom Responses product targets through the
  existing native-compaction defaults; keep DeepSeek on local summary routing.
- [ ] Move vision authorization to the activation snapshot so a running turn
  is unaffected by a later global config change.
- [ ] Replace optimistic `/model` publication with a prepare/commit flow.
  Prepare and build the replacement product provider first; commit the
  snapshot and model settings only after success.
- [ ] Move global config/status/persistence publication out of the TUI picker
  handler and into the successful driver commit path. A failed build leaves
  the active provider, global model settings, status bar, and prompts
  unchanged.
- [ ] Do not add request IDs, stale-ack correlation, or session metadata
  versioning. If the minimal success/failure publication needs a result event,
  keep it product-local and document that the complete `/model` protocol is a
  separate design.
- [ ] Keep account balance and model discovery on their explicit legacy
  functions until a later service API migration.

**Gate:**

- [ ] `cargo test -p tact agent_snapshot` covers first activation, concurrent
  activation retaining its captured snapshot, subagent inheritance, and
  compaction using the same provider facts.
- [ ] Product Responses binding works through metadata and rejects a
  non-canonical endpoint equivalent or wrong protocol without downcasting.
- [ ] Existing session rows load unchanged; no new session schema column is
  required.
- [ ] Native-compaction tests cover OpenAI, Kimi, custom, and DeepSeek
  Responses targets before and after the snapshot migration.
- [ ] Vision tests prove a running activation is unaffected by later global
  config changes and production code no longer calls global vision helpers for
  image authorization.
- [ ] Product request capture persists a unique system-prompt marker through
  `SqliteUsageSink`, `load_latest_request_body`, and prompt rendering. The
  stable provider path strips the same marker.
- [ ] A failed model build leaves the previous active provider, global model
  settings, status bar, and persistence prompt unchanged.
- [ ] A successful model switch changes the next wire model even when the
  context window is unchanged.
- [ ] Focused `tact`, `tact-ui`, `tui`, and integration tests pass.

## Task 7: Activate Features And Close The Stable Surface

**Files:**

- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/src/lib.rs`
- `crates/tact_llm/src/account.rs`
- `crates/tact_llm/src/anthropic/mod.rs`
- `crates/tact_llm/src/client.rs`
- `crates/tact_llm/src/error.rs`
- `crates/tact_llm/src/mock.rs`
- `crates/tact_llm/src/models.rs`
- `crates/tact_llm/src/openai/mod.rs`
- `crates/tact_llm/src/openai/compatible/mod.rs`
- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/provider.rs`
- `crates/tact/Cargo.toml`
- `crates/tact-ui/Cargo.toml`
- `crates/tui/Cargo.toml`
- any other workspace crate that directly depends on `tact_llm`
- `fixtures/tact-llm-consumer/Cargo.toml`
- `fixtures/tact-llm-agent-sdk-consumer/Cargo.toml`
- `crates/tact_llm/tests/feature_matrix.rs`
- `crates/tact_llm/tests/public_api.rs`
- `crates/tact_llm/tests/facade_privacy.rs`
- `crates/tact_llm/tests/ui/`

**Work:**

- [ ] Declare optional dependencies and the full feature graph:
  `core`, protocol features, `openai`, `telemetry`, `test-support`,
  `legacy-tact`, `product-runtime`, `account`, and `model-discovery`.
- [ ] First retain a transitional broad default so every manifest change can
  be compiled before consumers are updated. Do not claim the final package
  contract while this transitional default exists.
- [ ] Add explicit `default-features = false` selections to `tact`, `tact-ui`,
  and `tui`. Product crates that still import legacy modules enable
  `legacy-tact`; the Tact model owner also enables `product-runtime`.
- [ ] Add explicit `test-support` selections only for test targets and dev
  dependencies that import `MockClient` or test-only helpers.
- [ ] Compile the workspace with the transitional default plus explicit
  consumer features. Fix every direct dependency discovered by Cargo.
- [ ] Only after that checkpoint, change `tact_llm` to `default = ["core"]`.
  Run focused package checks serially and repair workspace feature declarations
  rather than reintroducing broad defaults.
- [ ] Gate every provider constructor and adapter by its protocol feature.
  Disabled constructors are not nameable; building a target whose selected
  protocol is unavailable returns `ProviderFeatureDisabled` where a
  constructor is still type-visible.
- [ ] Keep `ProviderKind`, `WireProtocol`, and `ChatDialect` visible in `core`
  because stable signatures use them.
- [ ] Make implementation modules private. Keep compatibility modules,
  concrete adapters, `LlmProvider`, `ProviderInfo`, globals, body hooks, wire
  types, `ProviderConversationState`, `MockClient`, account, and discovery
  reachable only through `legacy-tact` or `test-support`.
- [ ] Remove public `FacadeMetadata`, `EndpointClass`, `AuthRequirement`,
  `AuthScheme`, `ProviderFactoryBuilder`, and any other superseded facade
  vocabulary.
- [ ] Ensure `product-runtime` is absent from defaults and external consumer
  fixtures. A compile-fail fixture proves `build_product` is unavailable
  without it.
- [ ] Convert both external consumer fixtures to
  `default-features = false` plus exactly one protocol alias. Run direct and
  agent-SDK composition builds separately.
- [ ] Add dependency-tree checks for core-only, Anthropic-only, Chat-only,
  Responses-only, and telemetry-only builds.
- [ ] Add public API compile-fail tests for wire, transport, hook, concrete
  adapter, and compatibility imports in a stable build.

**Gate:**

- [ ] With the transitional broad default, every workspace crate still
  compiles after explicit consumer features are added.
- [ ] After switching to `default = ["core"]`,
  `cargo check -p tact_llm --no-default-features` and the explicit `core`
  check pass.
- [ ] Anthropic-only, Chat-only, Responses-only, `openai`, telemetry-only, and
  all-features checks pass one at a time.
- [ ] `account` and `model-discovery` enable `legacy-tact` when selected
  alone.
- [ ] Core-only `cargo tree` contains no HTTP client, provider SDK, SSE parser,
  or runtime protocol dependency.
- [ ] Anthropic-only excludes both OpenAI SDKs; Chat-only excludes the
  Responses SDK; Responses-only excludes the Chat SDK.
- [ ] The direct and agent-SDK fixtures compile under each supported
  single-protocol feature with `--locked`.
- [ ] Stable-only compile-fail tests reject every compatibility and wire
  import for the expected name-resolution reason.
- [ ] Focused `tact`, `tact-ui`, and `tui` builds pass with their explicit
  product features.

## Task 8: Decompose The Responses Adapter Without Behavior Changes

**Files:**

- `crates/tact_llm/src/openai/responses/mod.rs`
- `crates/tact_llm/src/openai/responses/adapter.rs`
- `crates/tact_llm/src/openai/responses/request.rs`
- `crates/tact_llm/src/openai/responses/events.rs`
- `crates/tact_llm/src/openai/responses/transport.rs`
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

- [ ] Move lifecycle orchestration into `adapter.rs`.
- [ ] Move request policy and serialization into `request.rs`.
- [ ] Move direct HTTP execution and bounded error handling into
  `transport.rs`.
- [ ] Move SSE decoding and unknown-event normalization into `events.rs`.
- [ ] Keep incremental assembly in `stream.rs`.
- [ ] Move terminal response normalization into `normalize.rs`.
- [ ] Move cursor, provider-state, and compaction updates into `state.rs`.
- [ ] Move header construction into `headers.rs`.
- [ ] Give history, capabilities, and request-options modules explicit
  ownership and remove cross-module test-only re-exports.
- [ ] Keep `mod.rs` as module declarations plus a narrow private re-export
  surface.
- [ ] Do not change request JSON, event acceptance, unknown-item retention, or
  state serialization.
- [ ] Split any file that crosses 1,000 lines, including tests, unless a
  documented ownership reason justifies it.

**Gate:**

- [ ] `cargo test -p tact_llm responses` passes without fixture edits.
- [ ] All Task 0 goldens remain byte-equivalent.
- [ ] No file under `crates/tact_llm/src/openai/responses/` exceeds 1,000
  lines.
- [ ] No Responses wire or SSE type is public outside the private module tree.

## Task 9: Complete Third-Party Documentation And Release Readiness

**Files:**

- `crates/tact_llm/README.md`
- `crates/tact_llm/Cargo.toml`
- `crates/tact_llm/examples/direct_stream.rs`
- `crates/tact_llm/examples/agent_sdk.rs`
- `crates/tact_llm/examples/refreshable_auth.rs`
- `crates/tact_llm/examples/custom_endpoint.rs`
- `crates/tact_llm/examples/tracing.rs`
- `ARCHITECTURE.md`
- `docs/compaction.md`
- `book/05-architecture_zh.html` or the current Tact chapter that owns the
  provider/runtime boundary
- `config.example.toml`

**Work:**

- [ ] Document the stable call chain and show `Provider` passed directly to
  `tact_agent_sdk::AgentBuilder`.
- [ ] Add compile-tested examples for direct streaming and cancellation,
  static API keys, refreshable credentials, custom anonymous endpoints, and
  host-owned tracing.
- [ ] Document feature selection with
  `default-features = false` plus exactly one protocol.
- [ ] Document that custom HTTP stacks implement `ModelPort`; v1 does not
  expose a transport trait.
- [ ] Document stable semver items and explicitly exclude the compatibility
  surface.
- [ ] Add a legacy-import to stable-facade migration table.
- [ ] Update `ARCHITECTURE.md` with the provider builder, one policy owner, and
  product activation snapshot.
- [ ] Sync the compaction documentation where unknown context windows and
  product fallback semantics changed.
- [ ] Sync the owning book chapter and `config.example.toml` if product-facing
  model limit or feature configuration changed.
- [ ] Add package description, repository, README, keywords, categories,
  rustdoc feature notes, and the tested MSRV policy.
- [ ] Record the exact registry blocker if `tact_agent_api` or the Git-only
  Responses dependency is not ready for publication. Do not claim crates.io
  readiness from `cargo package --list`.

**Gate:**

- [ ] `cargo check -p tact_llm --examples` passes.
- [ ] `cargo doc -p tact_llm --no-deps` passes with no broken links and
  feature docs that match the manifest.
- [ ] Both external consumer fixtures compile with `--locked`.
- [ ] README examples do not call globals, import compatibility modules, or
  omit the `tact_agent_sdk` dependency when using `AgentBuilder`.
- [ ] `python3 scripts/book-check.py book` passes after refreshing chapter HTML
  if sidebars changed.
- [ ] `git diff --check` passes and the spec/plan code fences and relative
  links are balanced.

## Final Verification

- [ ] Run focused tests after each task; never run Cargo commands in parallel.
- [ ] Run `scripts/check-rust.sh` once after all code tasks are otherwise
  green. Do not run `cargo test --workspace` immediately before it.
- [ ] Run `python3 scripts/book-check.py book`.
- [ ] Run `git diff --check`.
- [ ] Inspect `git status --short` and separate this work from pre-existing
  untracked files.
- [ ] Confirm no stable build exposes globals, concrete adapters, wire types,
  SSE types, body hooks, product capture, account helpers, or model discovery.
- [ ] Confirm no production code logs or renders request bodies, response
  bodies, prompts, credentials, raw SSE, tool arguments, or provider-state
  payloads.

## Definition Of Done

- [ ] A third-party host can construct one provider, pass it directly to
  `tact_agent_sdk::AgentBuilder`, and run without globals or a second runtime
  trait.
- [ ] Static API key, static bearer, refreshable credentials, and anonymous
  custom endpoints have compile-tested, documented paths.
- [ ] Construction performs no network call and no refreshable credential
  resolution.
- [ ] One policy module owns provider/model/endpoint interpretation.
- [ ] Unknown context windows are `None` end to end, while legacy stored JSON
  still round-trips through numeric `0`.
- [ ] Stable calls never retain or expose request/response bodies.
- [ ] Streaming is lazy, cancellation-first, single-terminal, and drop-safe.
- [ ] Direct HTTP errors are typed, bounded, body-free, and preserve safe retry
  facts.
- [ ] Existing wire/SSE/provider-state/account behavior is proven by checked-in
  goldens and fixtures.
- [ ] Product turns use one immutable activation snapshot for provider,
  capabilities, vision, compaction, and subagents.
- [ ] Product provider-state validation uses canonical metadata, not concrete
  adapter downcasts or raw URL spellings.
- [ ] `/model` publishes neither provider state nor host model settings until a
  replacement provider has been built successfully. Full request/ack
  correlation remains deferred.
- [ ] `tact_llm` defaults to `core`; protocol dependencies are isolated by
  features and verified in external consumer builds.
- [ ] Compatibility globals and concrete adapters are unreachable from a
  stable build.
- [ ] Responses internals are decomposed with unchanged behavior.
- [ ] Documentation, examples, rustdoc, and package metadata describe the
  stable SDK and the remaining registry blockers accurately.
- [ ] The full repository gate passes, or every residual environmental
  failure is reported with exact commands and evidence.
