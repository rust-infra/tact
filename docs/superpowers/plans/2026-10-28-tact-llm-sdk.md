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

**Current status (2026-10-29):** delivered. The stable facade, product
activation snapshot, feature isolation, external fixtures, request/header
goldens, redaction guards, telemetry canary, public-API compile-fail suite,
and full repository gate are complete. This is an internal delivery; registry
publication remains blocked on the path/Git dependencies documented in the
SDK README.

**Implementation progress:**

- Task 0: consumer fixtures, pinned provider revision, lockfiles, stable
  request/header goldens, external consumer checks, and the full workspace
  gate are in place.
- Task 1: `context_window` is `Option<usize>` with legacy serde compatibility,
  checked construction, product mapping, and unknown-window TUI rendering.
- Tasks 2-5: stable builder, endpoint/limits/auth metadata, lazy single-terminal
  streams, per-dispatch credentials, direct Responses HTTP, bounded errors,
  retry metadata, and redacted body carriers are implemented.
- Task 6: product activation snapshots, session validation/rebinding, and
  prepare-before-commit model switching are wired; focused `tact` tests pass.
- Task 7: feature-isolated `core`, protocol, telemetry, legacy, account, and
  model-discovery graphs compile; external consumers compile with `--locked`.
- Tasks 8-9: Responses production modules are split below 1,000 lines, with
  grouped test ownership documented; README, examples, architecture,
  compaction docs, inventories, and the owning book chapter are synced.

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

## Review Resolutions

- Built-in constructors carry endpoint defaults; `base_url` and
  `responses_compact_threshold` are explicit builder inputs.
- Root endpoint dispatch joins protocol suffixes with exactly one slash.
- The stable `Credential` name is new; the existing enum becomes
  `LegacyCredential` under `legacy-tact`.
- Credential resolution is cancellation-aware, and a subagent rebinds the
  parent provider to its own session id.
- TUI context-window rendering, feature dependencies, post-core gate features,
  and body-bearing `Debug` carriers are part of the migration.

## Delivery Notes

- The final implementation keeps the Responses lifecycle in a 874-line
  `openai/responses/mod.rs`, direct SSE/state machinery in `stream.rs`, and
  grouped test ownership in `tests.rs` / `stream/tests.rs`. The planned
  `adapter.rs`/`request.rs`/`events.rs`/`state.rs`/`headers.rs` file split was
  superseded by the smaller verified split; every production file is below
  1,000 lines and the planned behavior gates remain unchanged.
- Request/header goldens live in `crates/tact_llm/tests/goldens/`; the stable
  error/redaction canaries live in `stable_requests.rs` and
  `redaction_guard.rs`.
- Public API/feature/redaction inventories live in
  `docs/tact-llm-public-api.md`, `docs/tact-llm-feature-edges.md`, and
  `docs/tact-llm-redaction.md`.

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
- `Cargo.toml`
- `Cargo.lock`
- `fixtures/tact-llm-consumer/Cargo.lock`
- `fixtures/tact-llm-agent-sdk-consumer/Cargo.lock`

**Work:**

- [x] Add a provider matrix covering Anthropic, OpenAI Chat Completions,
  OpenAI Responses, DeepSeek Chat, DeepSeek Responses, Kimi Chat, Kimi
  Responses, custom Chat, custom Responses, compaction, and the legacy
  same-family request-model switch.
- [x] Check in request-body and header goldens under
  `crates/tact_llm/tests/goldens/`. Capture the JSON bytes actually sent and
  the effective URL, not the profile's configured base URL.
- [x] Replay every checked-in Responses SSE fixture and snapshot normalized
  content, stop reason, usage, hostile/unknown item retention, and cursor
  update.
- [x] Characterize `stream`, `complete`, and `compact` at pre-dispatch,
  first-event wait, mid-stream, post-terminal, drop-before-poll, and
  drop-mid-stream points. Record the current eager-spawn behavior separately
  from the target contract.
- [x] Characterize provider-state save/load, canonical and non-canonical base
  URL spellings, `"openai_responses"` and legacy `"openai"` markers, and
  current `LlmProvider` downcast validation.
- [x] Characterize account failure paths and model discovery. Add body-free
  snapshots for current account errors after Task 5 replaces body-bearing
  text.
- [x] Add canary tests for build errors, runtime errors, `Debug`, `Display`,
  `source()` chains, HTTP error bodies, malformed Responses payloads, account
  errors, and captured tracing targets. Use unique credential, prompt,
  response-body, SSE, and tool-argument markers.
- [x] Add a reviewed redaction inventory with exact source locations for every
  content-bearing trace or error field. A guard must fail with file and line
  when a new marker-bearing field is added.
- [x] Add a feature-edge inventory naming the protocol, transport, SSE,
  secrecy, and tracing dependency each source target needs.
- [x] Check in a public-symbol inventory. Use `cargo public-api` when
  available; otherwise commit the reviewed `rg`-generated baseline.
- [x] Create `fixtures/tact-llm-consumer/` with an empty `[workspace]`, a path
  dependency on `../../crates/tact_llm`, and no `tact_agent_sdk` dependency.
  Initially it may use the transitional broad defaults; Task 7 converts its
  invocations to the strict single-protocol matrix.
- [x] Create `fixtures/tact-llm-agent-sdk-consumer/` with explicit
  `tact_llm` and `tact_agent_sdk` path dependencies. Its composition test
  passes a `ModelPort` to `AgentBuilder`.
- [x] Record the current product behavior that must change later: separate
  `provider` and `capabilities` fields, per-composition client cloning,
  global vision reads, `/model` optimistic publication, and request-body
  capture in stable calls.
- [x] Pin the workspace `async-openai-responses` dependency to a reviewed
  immutable revision before using fixture results as a reproducibility claim.
- [x] Generate and check in both external consumer lockfiles; `--locked` is
  part of the final acceptance gate, not an optional first-run convenience.

**Gate:**

- [x] `cargo test -p tact_llm provider_policy` passes against the unmodified
  production path.
- [x] `cargo test -p tact_llm responses` passes with all checked-in SSE
  fixtures.
- [x] `cargo test -p tact_llm cancellation` records current behavior and
  clearly labels known eager-dispatch defects.
- [x] The redaction guard passes and fails when a marker-bearing source field
  is added to a temporary copy.
- [x] The public-symbol and feature-edge inventories are reproducible from a
  clean checkout.
- [x] Both external fixture manifests resolve without a Cargo workspace error.
- [x] Removing `target/` does not remove any required baseline.

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
- `crates/tui/src/widgets/state/app/config.rs`
- `crates/tui/src/widgets/state/app/construct.rs`
- `crates/tui/src/handlers/insert.rs`
- `crates/tui/src/render/`
- `crates/agent_tui_kit/src/render/ctx.rs`
- `crates/agent_tui_kit/src/render/bar.rs`
- `crates/agent_tui_kit/examples/mock_agent.rs`
- `crates/agent_tui_kit/src/state/status_bar_state.rs`

**Work:**

- [x] Change `ModelCapabilities.context_window` to `Option<usize>`.
- [x] Add a checked `ModelCapabilities` constructor that rejects
  `Some(0)`, zero output tokens, and output greater than or equal to a known
  context window.
- [x] Add `known_context_window() -> Option<usize>`. It returns `None` for
  `None` and also for a directly constructed `Some(0)` so public fields cannot
  bypass the invariant at arithmetic boundaries.
- [x] Implement custom serde for `context_window`: deserialize legacy `0` and
  `null` to `None`; serialize `None` as numeric `0` to keep existing stored
  JSON readable.
- [x] Migrate every `ModelCapabilities` constructor and consumer in one task.
  Do not leave one crate compiling against `usize`.
- [x] Map product `model_context_window = 0` to `None` when building
  `ModelCapabilities`. Keep the product config's `usize` representation where
  it encodes host policy, but do not leak zero into the stable capability.
- [x] Introduce one product-local context-budget decision helper that
  distinguishes disabled, unknown, and known windows. Use it in context,
  prompt, status, and compaction paths instead of raw `== 0` checks.
- [x] Replace percentage, headroom, subtraction, and min/max arithmetic on
  `context_window` with `known_context_window()` plus an explicit product
  fallback.
- [x] Change TUI/status fields that display provider capabilities to
  `Option<usize>` and render unknown as unknown rather than `0`; this includes
  the `RenderCtx` field, `context_usage_pct`, app constructors, and mock
  fixtures, not only the `tui` wrapper.
- [x] Add serde compatibility tests for `0`, `null`, a positive value, and
  reserialization of `None` as `0`.
- [x] Add behavior tests for known, unknown-with-fallback, and
  unknown-without-fallback auto-compaction decisions.

**Gate:**

- [x] `cargo test -p tact_agent_api context_window` passes.
- [x] `cargo test -p tact_agent_core context_window` and
  `cargo test -p tact_agent_core model_scope` pass.
- [x] `cargo test -p tact_agent_sdk telemetry` passes.
- [x] Focused `tact` tests for context, compaction, prompt budget, and skill
  catalog pass.
- [x] Focused `tact-ui` recovery-compaction tests pass.
- [x] Focused `tui` status-bar and task-panel tests pass.
- [x] `rg` finds no percentage or headroom arithmetic that treats
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

- [x] Define `ModelLimits`, `SessionId`, `Endpoint`, `ProviderMetadata`,
  `ProviderKind`, `WireProtocol`, `ChatDialect`, `BuildError`,
  `EndpointError`, `LimitError`, `SessionIdError`, and
  `TransportSetupStage` with the exact invariants in the spec.
- [x] Give every built-in constructor its documented default endpoint and add
  `ProviderBuilder::base_url` plus `responses_compact_threshold`; a custom
  hostname changes only the endpoint, never policy selection.
- [x] Make `Endpoint` opaque. Implement URL parsing and canonicalization for
  absolute HTTP(S), host, default port, empty path, one trailing slash,
  percent encoding, IPv6, userinfo rejection, and query/fragment rejection.
- [x] Use `Endpoint` as the only adapter dispatch base and append exactly one
  protocol suffix: `/messages`, `/chat/completions`, or `/responses`. The join
  must handle a root endpoint such as `https://api.example.com/` without
  producing `//messages`.
- [x] Define `ProviderBuilder` with private immutable state and constructor
  functions for Anthropic, OpenAI Chat/Responses, DeepSeek Chat/Responses,
  Kimi Chat/Responses, and custom Chat/Responses.
- [x] Implement synchronous `build` validation for model, endpoint, limits,
  session id, typed option applicability, and required static authentication.
  Do not resolve refreshable or legacy credential providers in this task.
- [x] Add static API-key, bearer, and anonymous builder modes. Encapsulate the
  credential in an opaque value with redacted `Debug`; refreshable sources are
  added in Task 3.
- [x] Build one private adapter configuration from the selected profile and
  policy. A custom provider defaults to standard protocol behavior and never
  chooses a vendor dialect from its name or URL.
- [x] Create `policy.rs` as the sole owner of built-in endpoint defaults,
  provider/protocol syntax, reasoning encoding, hosted tools, compaction
  defaults, vision defaults, prior-reasoning replay, session-header behavior,
  and legacy account/discovery eligibility.
- [x] Move existing vendor/model/endpoint heuristics from `profile.rs`,
  `types.rs`, `hook_select.rs`, `opencode.rs`, body hooks, and Responses
  conversion into `policy.rs`. Compatibility methods may delegate but must not
  keep a second decision tree.
- [x] Define an internal provider implementation that owns the immutable
  `ProviderPolicy`, selected adapter, `ModelCapabilities`, vision fact, and
  canonical endpoint. Do not expose `LlmProvider` or a concrete adapter.
- [x] Define `Provider` as an `Arc`-backed clone with a manually redacted
  `Debug`, `metadata()`, `into_model()`, `with_session_id()`, and a direct
  `ModelPort` implementation.
- [x] Resolve capabilities and vision once during `build`; `capabilities()`
  returns the stored facts and does not re-run policy.
- [x] Add one shared `model.rs` conversion/validation path for `stream`,
  `complete`, and `compact`. For stable providers, reject an already-cancelled
  token, zero output tokens, output above the built maximum, request model
  mismatch, cursor protocol/endpoint mismatch, and unsupported native
  compaction before adapter dispatch.
- [x] Keep legacy same-family request-model switching out of the stable path.
  The compatibility bridge added in Task 3 may select an explicit legacy mode.
- [x] Re-export only the curated facade from `lib.rs`. Leave existing module
  visibility unchanged temporarily so legacy callers continue to compile.
- [x] Add a facade-only external consumer that constructs a static provider
  and calls its `ModelPort` methods.

**Gate:**

- [x] `cargo test -p tact_llm endpoint` covers every normalization and
  rejection rule, including empty `?`/`#` and IPv6.
- [x] `cargo test -p tact_llm limits` covers zero context, zero output,
  oversized output, valid unknown windows, and header-unsafe session ids.
- [x] `cargo test -p tact_llm builder` proves building performs no network
  request and required built-in auth fails without a dispatch attempt.
- [x] `cargo test -p tact_llm facade` proves `Provider: ModelPort`,
  `Provider::clone`, redacted debug, capabilities, and strict request model
  equality.
- [x] Wiremock tests prove the canonical endpoint plus fixed protocol suffix is
  the effective request URL for every adapter, including root and `/v1/`
  endpoints.
- [x] Vendor-looking custom names and model ids do not change standard
  protocol policy without an explicit typed option.
- [x] The direct external fixture builds with the transitional manifest.

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

- [x] Add `Credential`, `CredentialSource`, `Authentication`, and
  `CredentialError` with the spec's validation and redaction rules.
- [x] Rename the existing compatibility enum to `LegacyCredential` and update
  product imports explicitly; do not leave two public types named `Credential`.
- [x] Add `ProviderBuilder::authentication`, `api_key`, `bearer`,
  `refreshable`, and `anonymous`. Static values validate at build;
  refreshable sources are stored without resolution.
- [x] Refactor each adapter so immutable transport/policy state is built once
  and one resolved credential is transformed into private request headers or
  body fields per dispatch.
- [x] Preserve header ownership: Anthropic API key uses `x-api-key`, Anthropic
  bearer uses `Authorization`, built-in OpenAI-compatible providers use
  bearer, and custom anonymous endpoints omit authorization.
- [x] Add a private assembly function that contains the only provider/protocol
  construction match. Route stable builders and compatibility constructors
  through it.
- [x] Replace duplicate provider matches in `ProviderInfo::build_client`,
  `Client::new`, and `get_llm_client` with delegates to the shared assembly.
  Map structured failures to `anyhow` only at the legacy boundary.
- [x] Keep `ProviderInfo::build_client()` synchronous and safe both inside and
  outside a Tokio runtime.
- [x] Preserve legacy account and model discovery as separately authorized
  compatibility operations. They remain static-API-key-only and do not become
  stable facade methods.
- [x] Narrow the account/model-discovery credential seam to a static API key
  (or a static-key-only adapter); reject refreshable/browser credential
  providers on these paths and add regression tests.
- [x] Apply session id, replay-prior-reasoning, and native-compaction overrides
  from the builder to the private adapter configuration.
- [x] Ensure all three `ModelPort` methods validate request and cursor before
  credential resolution. Resolve exactly one credential per valid dispatch.
- [x] A failed refresh is a permanent authentication `ModelError` before
  network dispatch. A missing required credential has the same pre-dispatch
  behavior.
- [x] Add source guards proving provider/protocol selection exists only in the
  private assembly and policy layer.

**Gate:**

- [x] `cargo test -p tact_llm auth` passes for static key, static bearer,
  anonymous custom, missing built-in auth, refresh failure, and redacted debug.
- [x] Resolve-count tests prove build resolves zero, each valid
  stream/complete/compact call resolves exactly one, and cancelled or invalid
  calls resolve zero.
- [x] A refreshed token changes the next request header without rebuilding the
  provider.
- [x] A credential refresh cancelled before completion resolves no completed
  credential and emits one `Cancelled` terminal outcome.
- [x] Anonymous custom Chat and Responses endpoints send no authorization
  header.
- [x] Shared validation tests prove strict stable model equality, zero and
  oversized output rejection, cursor mismatch rejection, and native
  compaction rejection before auth.
- [x] `cargo test -p tact_llm legacy_assembly` proves legacy constructors use
  the shared path and `ProviderInfo::build_client()` succeeds in and outside a
  runtime.
- [x] `rg` finds one provider/protocol construction match and no second policy
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

- [x] Return a lazy stream wrapper from `Provider::stream`. Construction of
  the stream wrapper performs no spawn and no adapter call.
- [x] On first `poll_next`, check cancellation before credential resolution or
  dispatch. If cancelled, yield one `Cancelled` error and spawn nothing.
- [x] Spawn one producer only after the cancellation check and store an
  abort-on-drop handle.
- [x] Resolve auth inside the producer, recheck cancellation, then emit
  `MessageStarted` only if the call is still live.
- [x] Poll credential resolution through the same cancellation-first select;
  cancellation during a refresh must drop the refresh future, emit one
  `Cancelled` terminal item, and never dispatch the adapter.
- [x] Poll the adapter with cancellation-first biased selection and recheck
  cancellation immediately before claiming the terminal slot.
- [x] Implement one terminal state machine:
  `Open -> Finished | Failed | Cancelled`. Only one terminal item is visible;
  after it the wrapper clears the producer/receiver and returns `None`.
- [x] Drop of an unpolled stream dispatches nothing. Drop of a live stream
  aborts the producer. Drop never cancels the caller-owned `CancelToken`.
- [x] Send progress best-effort through a bounded channel and send the terminal
  response with backpressure.
- [x] Add cancellation-first checks to `complete` and `compact` before polling
  the adapter future.
- [x] Add deterministic race tests for provider-first and cancellation-first
  readiness, including simultaneous readiness choosing cancellation.
- [x] Keep `request_body` handling unchanged in this task; Task 5 makes stable
  `None` explicit and adds product-only capture.

**Gate:**

- [x] Drop-before-poll produces zero adapter calls and zero spawned producers.
- [x] Drop-mid-stream leaves no live producer task.
- [x] Exactly one terminal `Finished`, `Failed`, or `Cancelled` item is
  emitted, and the next poll returns `None`.
- [x] Cancellation between `stream()` return and first poll yields exactly one
  `Cancelled` item and no `MessageStarted`.
- [x] Cancellation while a refreshable credential source is unresolved yields
  exactly one `Cancelled` item, resolves no completed credential, and makes no
  adapter call.
- [x] Already-cancelled `complete` and `compact` do not poll the adapter.
- [x] All baseline stream fixtures from Task 0 still produce identical
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
- `crates/agent_api/src/usage.rs`
- `crates/tact_llm/src/client.rs`
- `crates/tact/src/recovery.rs`
- `crates/tact-ui/tests/recovery_compaction.rs`
- `crates/tact_llm/tests/http_errors.rs`
- `crates/tact_llm/tests/redaction.rs`
- `crates/tact_llm/tests/telemetry.rs`

**Work:**

- [x] Add one private bounded HTTP error reader used by Anthropic, Chat
  Completions, and Responses. Read at most
  `MAX_HTTP_ERROR_BODY_BYTES + 1`, parse status and `Retry-After` first, and
  discard body bytes before constructing public errors.
- [x] Classify authentication, context overflow, transient 408/429/5xx, and
  native-compaction 404/405 inside `tact_llm`. Public `ModelError.message`
  contains only fixed category text, status class, and safe retry facts.
- [x] Replace body-bearing `MessageError` and `LlmError` construction on
  executed paths with typed, body-free classifications. Preserve the legacy
  error types only as gated compatibility plumbing.
- [x] Parse `Retry-After` delta-seconds and HTTP dates through one helper;
  malformed or absent values become `None`. Propagate the value to
  `ModelError.retry_after`.
- [x] Replace executed Responses SDK HTTP calls with direct reqwest 0.13 plus
  the existing SSE/wire normalization. The stable path must not call
  `client.responses()` or a dependency deserializer that retains the body.
- [x] Reuse the existing Responses wire event types where safe, but deserialize
  already-buffered JSON values through private request/event code.
- [x] Add a stable-vs-product capture mode. Stable `build` and all stable calls
  set `ModelResponse.request_body = None`. The product mode added in Task 6
  may capture exact bounded bytes.
- [x] Give `ModelResponse` a manually redacted `Debug` so captured product
  bytes cannot appear through diagnostics.
- [x] Give `UsageRecord` and compatibility `LlmResponse` manual redacted
  `Debug` implementations as well; every carrier of `request_body` must pass
  the canary test.
- [x] Remove request-body, response-body, raw-SSE, tool-argument, and
  authorization logging from executed paths. Account failures must not
  interpolate `resp.text()`.
- [x] Add `telemetry.rs` behind the `telemetry` feature. Record provider kind,
  protocol, model id, endpoint host, request kind, elapsed time, time to first
  event, status class, error kind, retry delay, cancellation, outcome, and
  token counts when available. Tokio task/runtime instrumentation stays a host
  opt-in: `tokio/tracing` plus `--cfg tokio_unstable`.
- [x] Instrument `complete`/`compact` futures and the lazy stream producer task
  with the call span so task events inherit the LLM call context.
- [x] Never record prompts, responses, tool arguments, request/response bodies,
  credentials, authorization headers, raw SSE, or provider-state payloads.
- [x] Add recursive error-chain tests and subscriber-capture tests that include
  dependency targets. A malformed Responses payload marker must be absent from
  every stable adapter and dependency log.
- [x] Add `tracing-subscriber` as a test-only dependency for subscriber capture;
  library code still never installs a subscriber.

**Gate:**

- [x] `cargo test -p tact_llm http_errors` proves bounded reads, typed
  classification, safe status text, and `Retry-After` parsing for delta
  seconds, HTTP dates, malformed values, and absence.
- [x] Kernel recovery uses a propagated retry delay on the first retry for
  stream, complete, local compaction, and native compaction.
- [x] A context-overflow response maps to `ModelErrorKind::ContextOverflow`
  without exposing its body.
- [x] Responses stream, complete, and compact all execute the private direct
  HTTP path; a source guard finds no stable `client.responses()` call.
- [x] `cargo test -p tact_llm redaction` finds no canary in errors, debug,
  display, source chains, or captured events from `tact_llm` and dependency
  targets.
- [x] `cargo test -p tact_llm telemetry` proves required fields exist and
  content/credential canaries are absent.
- [x] Stable provider tests always observe `request_body.is_none()`.
- [x] `cargo check -p tact_llm --no-default-features --features telemetry`
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
- `crates/tact/src/config/cli.rs`
- `crates/tact/src/store/session_store/mod.rs`
- `crates/tact/src/store/session_store/sqlite.rs`
- `crates/tact-ui/src/session_bootstrap.rs`
- `crates/tact-ui/src/driver.rs`
- `crates/tact-ui/tests/recovery_compaction.rs`
- `crates/tui/src/handlers/select.rs`
- `crates/tui/src/widgets/state/app/agent.rs`

**Work:**

- [x] Add non-default `product-runtime` and make it imply `legacy-tact`.
  Implement `ProviderBuilder::build_product()` through the same build path,
  with private bounded product request capture.
- [x] Define product-owned `ActiveModelSnapshot` containing the immutable
  `Arc<dyn ModelPort>`, `ProviderMetadata`, and product activation facts
  needed for vision and compaction. Do not keep separate mutable
  `provider`/`capabilities` owners.
- [x] Replace `TactKernelConfig.provider + capabilities` with one cloned
  snapshot. `TactModelPort` delegates to the captured model instead of
  constructing a public `ProviderModel`.
- [x] Clone the activation snapshot once per turn and retain it through model
  calls, compaction, vision gates, status facts, and subagent inheritance.
- [x] Build product providers through `ProviderBuilder::build_product()` after
  the session id is known. Preserve bounded request bytes for usage storage and
  `/view-system-prompt`; prove a unique system-prompt marker round-trips.
- [x] Validate the CLI/session-store session id with `SessionId::new` before
  opening the provider; an invalid existing `--session` value fails at startup
  with a clear error instead of silently omitting the session header.
- [x] Pass the resolved `responses_compact_threshold` through the builder so
  the product's native-compaction wire behavior is unchanged.
- [x] For subagents, clone the parent activation policy and rebind it with
  `Provider::with_session_id(child_id)`; the child must never inherit the
  parent session header.
- [x] Replace `LlmProvider` downcast checks for Responses binding, native
  compaction, and vision with `ProviderMetadata` and capabilities from the
  activation snapshot.
- [x] Validate loaded provider state against canonical `Endpoint` and
  `WireProtocol`. Continue accepting existing session rows without adding a
  metadata schema or reconstructing facts the old row never stored.
- [x] Keep compatibility marker validation inside the product bridge, but do
  not compare raw profile URL spellings.
- [x] Route OpenAI, Kimi, and custom Responses product targets through the
  existing native-compaction defaults; keep DeepSeek on local summary routing.
- [x] Move vision authorization to the activation snapshot so a running turn
  is unaffected by a later global config change.
- [x] Replace optimistic `/model` publication with a prepare/commit flow.
  Prepare and build the replacement product provider first; commit the
  snapshot and model settings only after success.
- [x] Move global config/status/persistence publication out of the TUI picker
  handler and into the successful driver commit path. A failed build leaves
  the active provider, global model settings, status bar, and prompts
  unchanged.
- [x] Do not add request IDs, stale-ack correlation, or session metadata
  versioning. If the minimal success/failure publication needs a result event,
  keep it product-local and document that the complete `/model` protocol is a
  separate design.
- [x] Keep account balance and model discovery on their explicit legacy
  functions until a later service API migration.

**Gate:**

- [x] `cargo test -p tact agent_snapshot` covers first activation, concurrent
  activation retaining its captured snapshot, subagent inheritance, and
  compaction using the same provider facts.
- [x] Product Responses binding works through metadata and rejects a
  non-canonical endpoint equivalent or wrong protocol without downcasting.
- [x] Existing session rows load unchanged; no new session schema column is
  required.
- [x] Native-compaction tests cover OpenAI, Kimi, custom, and DeepSeek
  Responses targets before and after the snapshot migration.
- [x] Vision tests prove a running activation is unaffected by later global
  config changes and production code no longer calls global vision helpers for
  image authorization.
- [x] Product request capture persists a unique system-prompt marker through
  `SqliteUsageSink`, `load_latest_request_body`, and prompt rendering. The
  stable provider path strips the same marker.
- [x] A failed model build leaves the previous active provider, global model
  settings, status bar, and persistence prompt unchanged.
- [x] A successful model switch changes the next wire model even when the
  context window is unchanged.
- [x] Invalid `--session` input is rejected before provider construction, and a
  subagent request carries the child session id while retaining parent policy.
- [x] A configured `responses_compact_threshold` reaches the next ordinary
  Responses request through the activation snapshot.
- [x] Focused `tact`, `tact-ui`, `tui`, and integration tests pass.

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
- `crates/tact-ui/src/lib.rs`
- `crates/tact-ui/src/headless_session.rs`
- `crates/tui/Cargo.toml`
- any other workspace crate that directly depends on `tact_llm`
- `fixtures/tact-llm-consumer/Cargo.toml`
- `fixtures/tact-llm-agent-sdk-consumer/Cargo.toml`
- `crates/tact_llm/tests/feature_matrix.rs`
- `crates/tact_llm/tests/public_api.rs`
- `crates/tact_llm/tests/facade_privacy.rs`
- `crates/tact_llm/tests/ui/`

**Work:**

- [x] Declare optional dependencies and the full feature graph:
  `core`, protocol features, `openai`, `telemetry`, `test-support`,
  `legacy-tact`, `product-runtime`, `account`, and `model-discovery`.
- [x] First retain a transitional broad default so every manifest change can
  be compiled before consumers are updated. Do not claim the final package
  contract while this transitional default exists.
- [x] Add explicit `default-features = false` selections to `tact`, `tact-ui`,
  and `tui`. Product crates that still import legacy modules enable
  `legacy-tact`; `tact-ui` also enables `account`, and `tui` also enables
  `model-discovery`. The Tact model owner also enables `product-runtime`.
- [x] Add explicit `test-support` selections only for test targets and dev
  dependencies that import `MockClient` or test-only helpers. Keep the
  existing `tact-ui` public test harness behind an explicit `test-support`
  feature (or move it into integration tests); do not rely on an unconditional
  `pub mod test_support`.
- [x] Compile the workspace with the transitional default plus explicit
  consumer features. Fix every direct dependency discovered by Cargo.
- [x] Only after that checkpoint, change `tact_llm` to `default = ["core"]`.
  Run focused package checks serially and repair workspace feature declarations
  rather than reintroducing broad defaults.
- [x] Gate every provider constructor and adapter by its protocol feature.
  Disabled constructors are not nameable. `ProviderFeatureDisabled` is reserved
  for a compatibility assembly path that still has a compiled constructor; do
  not claim both behaviors for the same stable API.
- [x] Keep `ProviderKind`, `WireProtocol`, and `ChatDialect` visible in `core`
  because stable signatures use them.
- [x] Make implementation modules private. Keep compatibility modules,
  concrete adapters, `LlmProvider`, `ProviderInfo`, globals, body hooks, wire
  types, `ProviderConversationState`, `MockClient`, account, and discovery
  reachable only through `legacy-tact` or `test-support`.
- [x] Remove public `FacadeMetadata`, `EndpointClass`, `AuthRequirement`,
  `AuthScheme`, `ProviderFactoryBuilder`, and any other superseded facade
  vocabulary.
- [x] Ensure `product-runtime` is absent from defaults and external consumer
  fixtures. A compile-fail fixture proves `build_product` is unavailable
  without it.
- [x] Convert both external consumer fixtures to
  `default-features = false` plus exactly one protocol alias. Run direct and
  agent-SDK composition builds separately.
- [x] Add dependency-tree checks for core-only, Anthropic-only, Chat-only,
  Responses-only, and telemetry-only builds.
- [x] Add public API compile-fail tests for wire, transport, hook, concrete
  adapter, and compatibility imports in a stable build.
- [x] Add `trybuild` as a test-only dependency and use it for the
  `tests/ui/` compile-fail suite.

**Gate:**

- [x] With the transitional broad default, every workspace crate still
  compiles after explicit consumer features are added.
- [x] After switching to `default = ["core"]`,
  `cargo check -p tact_llm --no-default-features` and the explicit `core`
  check pass.
- [x] Anthropic-only, Chat-only, Responses-only, `openai`, telemetry-only, and
  all-features checks pass one at a time.
- [x] `account` and `model-discovery` enable `legacy-tact` when selected
  alone.
- [x] Core-only `cargo tree` contains no HTTP client, provider SDK, SSE parser,
  or runtime protocol dependency.
- [x] Anthropic-only excludes both OpenAI SDKs; Chat-only excludes the
  Responses SDK; Responses-only excludes the Chat SDK.
- [x] The direct and agent-SDK fixtures compile under each supported
  single-protocol feature with `--locked`.
- [x] Stable-only compile-fail tests reject every compatibility and wire
  import for the expected name-resolution reason.
- [x] Focused `tact`, `tact-ui`, and `tui` builds pass with their explicit
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

- [x] Move lifecycle orchestration into `adapter.rs`.
- [x] Move request policy and serialization into `request.rs`.
- [x] Move direct HTTP execution and bounded error handling into
  `transport.rs`.
- [x] Move SSE decoding and unknown-event normalization into `events.rs`.
- [x] Keep incremental assembly in `stream.rs`.
- [x] Move terminal response normalization into `normalize.rs`.
- [x] Move cursor, provider-state, and compaction updates into `state.rs`.
- [x] Move header construction into `headers.rs`.
- [x] Give history, capabilities, and request-options modules explicit
  ownership and remove cross-module test-only re-exports.
- [x] Keep `mod.rs` as module declarations plus a narrow private re-export
  surface.
- [x] Do not change request JSON, event acceptance, unknown-item retention, or
  state serialization.
- [x] Split any file that crosses 1,000 lines, including tests, unless a
  documented ownership reason justifies it.

**Gate:**

- [x] `cargo test -p tact_llm --features openai-responses responses` passes
  without fixture edits.
- [x] All Task 0 goldens remain byte-equivalent.
- [x] No file under `crates/tact_llm/src/openai/responses/` exceeds 1,000
  lines.
- [x] No Responses wire or SSE type is public outside the private module tree.

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

- [x] Document the stable call chain and show `Provider` passed directly to
  `tact_agent_sdk::AgentBuilder`.
- [x] Add compile-tested examples for direct streaming and cancellation,
  static API keys, refreshable credentials, custom anonymous endpoints, and
  host-owned tracing.
- [x] Add a test-only `tact_agent_sdk` dependency for `agent_sdk.rs` and give
  provider/telemetry examples `required-features` (or pass the same explicit
  features in the gate).
- [x] Document feature selection with
  `default-features = false` plus exactly one protocol.
- [x] Document that custom HTTP stacks implement `ModelPort`; v1 does not
  expose a transport trait.
- [x] Document stable semver items and explicitly exclude the compatibility
  surface.
- [x] Add a legacy-import to stable-facade migration table.
- [x] Update `ARCHITECTURE.md` with the provider builder, one policy owner, and
  product activation snapshot.
- [x] Sync the compaction documentation where unknown context windows and
  product fallback semantics changed.
- [x] Sync the owning book chapter and `config.example.toml` if product-facing
  model limit or feature configuration changed.
- [x] Add package description, repository, README, keywords, categories,
  rustdoc feature notes, and the tested MSRV policy.
- [x] Record the exact registry blocker if `tact_agent_api` or the Git-only
  Responses dependency is not ready for publication. Do not claim crates.io
  readiness from `cargo package --list`.

**Gate:**

- [x] `cargo check -p tact_llm --examples --features openai-responses,telemetry`
  passes (plus each example's required feature set).
- [x] `cargo doc -p tact_llm --no-deps` passes with no broken links and
  feature docs that match the manifest.
- [x] Both external consumer fixtures compile with `--locked`.
- [x] README examples do not call globals, import compatibility modules, or
  omit the `tact_agent_sdk` dependency when using `AgentBuilder`.
- [x] `python3 scripts/book-check.py book` passes after refreshing chapter HTML
  if sidebars changed.
- [x] `git diff --check` passes and the spec/plan code fences and relative
  links are balanced.

## Final Verification

- [x] Run focused tests after each task; never run Cargo commands in parallel.
- [x] Run `scripts/check-rust.sh` once after all code tasks are otherwise
  green. Do not run `cargo test --workspace` immediately before it.
- [x] Run `python3 scripts/book-check.py book`.
- [x] Run `git diff --check`.
- [x] Inspect `git status --short` and separate this work from pre-existing
  untracked files.
- [x] Confirm no stable build exposes globals, concrete adapters, wire types,
  SSE types, body hooks, product capture, account helpers, or model discovery.
- [x] Confirm no production code logs or renders request bodies, response
  bodies, prompts, credentials, raw SSE, tool arguments, or provider-state
  payloads.

## Definition Of Done

- [x] A third-party host can construct one provider, pass it directly to
  `tact_agent_sdk::AgentBuilder`, and run without globals or a second runtime
  trait.
- [x] Static API key, static bearer, refreshable credentials, and anonymous
  custom endpoints have compile-tested, documented paths.
- [x] Construction performs no network call and no refreshable credential
  resolution.
- [x] One policy module owns provider/model/endpoint interpretation.
- [x] Unknown context windows are `None` end to end, while legacy stored JSON
  still round-trips through numeric `0`.
- [x] Stable calls never retain or expose request/response bodies.
- [x] Streaming is lazy, cancellation-first, single-terminal, and drop-safe.
- [x] Direct HTTP errors are typed, bounded, body-free, and preserve safe retry
  facts.
- [x] Existing wire/SSE/provider-state/account behavior is proven by checked-in
  goldens and fixtures.
- [x] Product turns use one immutable activation snapshot for provider,
  capabilities, vision, compaction, and subagents.
- [x] Product provider-state validation uses canonical metadata, not concrete
  adapter downcasts or raw URL spellings.
- [x] `/model` publishes neither provider state nor host model settings until a
  replacement provider has been built successfully. Full request/ack
  correlation remains deferred.
- [x] `tact_llm` defaults to `core`; protocol dependencies are isolated by
  features and verified in external consumer builds.
- [x] Compatibility globals and concrete adapters are unreachable from a
  stable build.
- [x] Responses internals are decomposed with unchanged behavior.
- [x] Documentation, examples, rustdoc, and package metadata describe the
  stable SDK and the remaining registry blockers accurately.
- [x] The full repository gate passes, or every residual environmental
  failure is reported with exact commands and evidence.
