# tact_agent_sdk

`tact_agent_sdk` is a thin, ergonomic facade over `tact_agent_api` and
`tact_agent_core`. It supplies conservative defaults, static tool composition,
session ownership, cancellation, and optional tracing telemetry.

It is not the full Tact product. It does not contain a provider adapter, SQLite,
MCP, a TUI, or a runtime plugin registry, and it does not re-export `tact_llm`.

## Minimal agent

```rust,no_run
use std::sync::Arc;

use tact_agent_sdk::AgentBuilder;

# async fn run(model: Arc<dyn tact_agent_api::ModelPort>) -> Result<(), Box<dyn std::error::Error>> {
let mut session = AgentBuilder::new(model)
    .system_prompt("You are concise.")
    .max_total_turns(24)
    .build_session("chat-1")?;

let outcome = session.run_text("hello").await?;
println!("{:?}", outcome.stop);
# Ok(())
# }
```

The model is required. Tools, the permission gate, context, persistence, usage,
and events have safe defaults. In particular, no tools are advertised and the
default gate denies every call. An agent that needs tools must inject a
`ToolPort` and an explicit gate such as `AllowAllGate`.

## Tools as static plugins

Plugin style here means constructor injection: implement `ToolHandler`, compose
handlers with `ToolSet`, and pass the result through `AgentBuilder::tools`.
There is no mutable runtime registry.

`ToolHandler::access` defaults to `ResourceAccess::barrier()`. Override it only
when a tool's resource scope is known and concurrency is safe. `ToolSet`
snapshots specs and metadata at insertion and rejects duplicate names.

## Sessions

`Agent` may create multiple independent `AgentSession`s. Each session owns an
`AgentState`; run methods require `&mut self`, which serializes activations.
Session-id uniqueness and durable-store exclusivity are host responsibilities.

When restoring state, `build_session_from_state` requires
`state.initialized == true`. The default `NoopTranscriptStore` keeps no rows;
inject a durable store when the host needs persistence.

## Cancellation

`AgentSession::control()` returns a `SessionControl`. Cancellation applies only
to the active run. Idle cancellation is a no-op, and each run gets a fresh
cancel token.

## Telemetry

The default `telemetry` feature adds tracing spans without installing a global
subscriber:

```rust,no_run
use tact_agent_sdk::TelemetryConfig;

# fn configure(builder: tact_agent_sdk::AgentBuilder) -> tact_agent_sdk::AgentBuilder {
builder.telemetry(TelemetryConfig::default())
# }
```

The host owns subscriber and exporter setup. Content logging is disabled by
default and, when explicitly enabled, each field is truncated to 512 Unicode
scalar values. Disable the feature with `default-features = false` to remove the
`tracing` dependency.

## Context policy

`BasicContextPolicy` forwards every caller input, converts the transcript to
provider-neutral messages, and does not compact. A host that needs context
recovery must inject a custom `ContextPolicy`. A custom policy cannot be combined
with `system_prompt` or `model_options`.

## Provider adapters

Add `tact_llm` directly in the host crate when a concrete provider is needed,
then pass the resulting `ModelPort` to `AgentBuilder::new`. Provider HTTP stacks
stay outside the SDK dependency graph.
