# Tact LLM SDK Redaction Inventory

Reviewed 2026-10-29. The stable provider never renders request bodies,
response bodies, raw SSE frames, tool arguments, credentials, authorization
headers, or provider-state payloads through `Debug`, `Display`, error chains,
or tracing.

| Boundary | Source | Safe representation |
| --- | --- | --- |
| Stable call span | `crates/tact_llm/src/telemetry.rs` | provider, protocol, model, endpoint host, timing, usage counts, error class |
| HTTP status classification | `crates/tact_llm/src/transport.rs` | status and `HttpErrorClass`; bounded body is dropped after classification |
| Stable errors | `crates/tact_llm/src/builder.rs` | fixed category text from `ModelErrorKind` |
| Compatibility HTTP errors | `crates/tact_llm/src/error.rs` | status, `HttpErrorClass`, retry delay |
| Product request capture | `crates/agent_api/src/model.rs` | `request_body` is rendered as `<redacted>` |
| Product usage capture | `crates/agent_api/src/usage.rs` | `request_body` is rendered as `<redacted>` |
| Compatibility response capture | `crates/tact_llm/src/client.rs` | `request_body` is rendered as `<redacted>` |
| Tool-call parse diagnostics | `crates/tact_llm/src/openai/compatible/mod.rs` | tool name and parser error only; arguments are omitted |
| Account responses | `crates/tact_llm/src/account.rs` | bounded JSON parsing; no response body in errors |

`crates/tact_llm/tests/redaction_guard.rs` fails with the source file and line
if a previously leaking body-bearing field or trace argument returns.
`crates/tact_llm/tests/stable_requests.rs` proves the stable error path and
request capture do not expose canary markers.
