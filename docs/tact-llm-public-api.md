# Tact LLM Stable Public API Inventory

Reviewed 2026-11-08. The stable root is curated in
`crates/tact_llm/src/lib.rs`; compatibility modules are private unless
`legacy-tact` is enabled.

Stable constructors:

- `anthropic`
- `openai_chat`, `openai_responses`
- `deepseek_chat`, `deepseek_responses`
- `kimi_chat`, `kimi_responses`
- `custom_chat`, `custom_responses`

Stable facade and vocabulary:

- `ProviderBuilder`, `Provider`, `BuildError`, `TransportSetupStage`
- `ProviderMetadata`, `WireProtocol`, `Endpoint`, `EndpointError`
- `ModelLimits`, `LimitError`, `DEFAULT_MAX_OUTPUT_TOKENS`, `SessionId`,
  `SessionIdError`
- `Credential`, `CredentialSource`, `Authentication`, `CredentialError`
- `ProviderKind`, `ChatDialect`, `ChatCompletionsDialect`
- `tact_agent_api` model contract re-exports

`ModelErrorKind` is `#[non_exhaustive]`: the vocabulary grows with the
providers it describes, and an exhaustive public enum made every addition a
breaking change for downstream `match` expressions. Consumers need a fallback
arm; the recommended treatment is the same as `Permanent` (do not retry). For
the class label itself, call `ModelError::class_label()` — it maps unknown
kinds to `"permanent"`, so a new kind does not force an edit at the call site.

Compatibility-only names are compiled out of a stable build. The executable
baseline is `crates/tact_llm/tests/public_api.rs` plus `tests/ui/stable-*.rs`;
run it with:

```sh
cargo test -p tact_llm --no-default-features --features openai-responses \
  --test public_api
```

This inventory intentionally excludes `#[doc(hidden)]` items; module privacy
and `cfg` gates are the boundary.
