# Tact LLM SDK

`tact_llm` is the provider adapter package used by Tact. The stable path is:

```text
provider constructor -> ProviderBuilder -> Provider: ModelPort
```

The default feature is `core`; protocol adapters are opt-in. A third-party
host should select exactly the protocol it needs:

```toml
tact_llm = {
  version = "2.0.0",
  default-features = false,
  features = ["openai-responses"]
}
```

Until the registry release is available, use the same repository tag:

```toml
tact_llm = {
  git = "https://github.com/rust-infra/tact",
  branch = "codex/agent-kernel-design",
  default-features = false,
  features = ["openai-chat"]
}
```

Use the `v2.0.0` tag after that release is published; the branch form above is
the pre-release path for this tree.

## Build and run

```rust
use tact_llm::{ModelLimits, openai_responses};

let provider = openai_responses("gpt-5.6")
    .api_key(std::env::var("OPENAI_API_KEY")?)?
    .limits(ModelLimits::new(Some(400_000), 16_384)?)
    .build()?;

let model = provider.into_model();
```

`Provider` implements `tact_agent_api::ModelPort`. It does not resolve
refreshable credentials during construction, and it does not install a tracing
subscriber. The `telemetry` feature emits the `tact_llm.call` spans and events
under whatever subscriber the host installs. Use `Provider::with_session_id`
when a child agent must carry a different session identity from its parent.

Every dispatched call reports one `outcome`: `finished`, `failed`, or
`dropped`. Validation failures and unsupported compaction report `failed` even
though no adapter ran. A stream abandoned by its consumer reports `dropped`;
an unpolled stream reports nothing because it dispatched nothing. Failures
carry `error_kind`, and HTTP failures also carry `status_class`
(`authentication`, `context_overflow`, `transient`, `permanent`).

Tokio task/runtime instrumentation is the host's decision, not this crate's.
Enabling `tact_llm/telemetry` does not turn it on and does not modify the
consumer's tokio features. A host that wants it enables `tokio/tracing` in its
own dependency declaration and builds with `--cfg tokio_unstable`, for example:

```toml
[dependencies]
tokio = { version = "1", features = ["tracing"] }
```

```toml
[build]
rustflags = ["--cfg", "tokio_unstable"]
```

Both are required: with the feature but without the cfg, Tokio compiles its
instrumentation out. The cfg also unlocks other unstable Tokio behavior
build-wide.

## Stable and compatibility surfaces

The stable semver surface is the constructors, `ProviderBuilder`, `Provider`,
`ProviderMetadata`, `ModelLimits`, `SessionId`, `Endpoint`, `ProviderKind`,
`WireProtocol`, `ChatDialect`, authentication types, build/validation errors,
and the re-exported `tact_agent_api` model-call vocabulary. The old globals,
`ProviderInfo`, `LlmProvider`, concrete adapters, wire types, body hooks,
account/discovery helpers, and `MockClient` are compatibility-only under
`legacy-tact` or `test-support`.

| Legacy import | Stable replacement |
| --- | --- |
| `LlmProvider` / `ProviderInfo` | `Provider` + `ProviderMetadata` |
| `CredentialProvider` | `CredentialSource` |
| `ProviderProfile` policy heuristics | typed constructor / `ProviderBuilder` |
| `OpenAiResponsesAdapter` downcast | `ProviderMetadata::protocol` |
| global `get_provider()` | pass `Provider` or `Arc<dyn ModelPort>` |

## Release status

All dependencies are registry-resolvable. Publish `tact_agent_api` before
`tact_llm`; the local path in `Cargo.toml` becomes a versioned crates.io
dependency in the packaged crate.

This crate declares Rust 1.88 as its minimum supported version. CI also checks
the stable `openai-chat` and `openai-responses` paths on that toolchain.
