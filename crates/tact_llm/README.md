# Tact LLM SDK

`tact_llm` is the provider adapter package used by Tact. The stable path is:

```text
provider constructor -> ProviderBuilder -> Provider: ModelPort
```

The default feature is `core`; protocol adapters are opt-in. A third-party
host should select exactly the protocol it needs:

```toml
tact_llm = {
  path = "crates/tact_llm",
  default-features = false,
  features = ["openai-responses"]
}
```

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
subscriber. Use `Provider::with_session_id` when a child agent must carry a
different session identity from its parent.

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

## Release blockers

This package is not yet crates.io-ready. `tact_agent_api` is currently a path
dependency, and the Responses protocol uses the Git-pinned
`async-openai-local` fork at
`12488eb3f6f2dbf35082a1121f2192dd1c2a7c27`. Do not claim registry readiness
until both dependencies have a registry-safe publication plan.

The workspace is tested with the Rust 2024 toolchain used by Tact
(`rustc 1.99.0-nightly` in this workspace). No broader MSRV is claimed.
