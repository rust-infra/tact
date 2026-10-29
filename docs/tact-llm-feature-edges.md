# Tact LLM SDK Feature Edges

Reviewed 2026-10-29.

| Feature | Adds | Stable public surface |
| --- | --- | --- |
| `core` | model contract, auth, limits, metadata, policy, endpoint | builder/facade types only |
| `anthropic` | `reqwest 0.12`, `reqwest-eventsource`, `httpdate` | Anthropic constructor |
| `openai-chat` | `reqwest 0.13`, SSE, `async-openai`, `httpdate` | Chat constructors and `ChatDialect` |
| `openai-responses` | `reqwest 0.13`, SSE, `async-openai-responses`, `secrecy 0.10`, `httpdate` | Responses constructors |
| `openai` | both OpenAI protocols | both OpenAI constructor families |
| `telemetry` | optional `tracing` | no subscriber installation |
| `legacy-tact` | compatibility globals, concrete adapters, provider state, mock, profile, account/discovery hooks | no stable additions |
| `account` | account compatibility APIs (implies `legacy-tact`) | no stable additions |
| `model-discovery` | model-list compatibility APIs (implies `legacy-tact`) | no stable additions |
| `product-runtime` | bounded product request capture (implies `legacy-tact`) | no stable additions |
| `test-support` | test-only legacy helpers | no stable additions |

Reproduce the core dependency boundary with:

```sh
cargo tree -p tact_llm --no-default-features --features core -e normal
```

The command must not contain an HTTP client, provider SDK, SSE parser, or
tracing subscriber. Protocol-only builds add exactly the dependency row above.
