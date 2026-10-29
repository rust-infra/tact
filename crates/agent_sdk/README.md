# tact_agent_sdk

`tact_agent_sdk` is a thin, ergonomic facade over `tact_agent_api` and
`tact_agent_core`. It supplies conservative defaults, static tool composition,
session ownership, cancellation, and optional tracing telemetry.

The 2.0 release makes run and session identity explicit in the API. Hosts that
construct `AgentState`, `UsageRecord`, `ToolExecutionContext`,
`ToolGateContext`, or `AgentEvent` literals should add the new `RunContext` /
session fields. Existing event ports can continue using `emit`; session-aware
ports should override `emit_shared_with_context`.

It is not the full Tact product. It does not contain a provider adapter, SQLite,
MCP, a TUI, or a runtime plugin registry, and it does not re-export `tact_llm`.

## Install

The SDK requires Rust 1.88 or newer. Once the workspace crates are published,
the host needs the SDK plus one provider adapter:

```toml
[dependencies]
tact_agent_sdk = "2.0.0"
tact_llm = { version = "2.0.0", default-features = false, features = ["openai-chat"] }
```

Until the registry release is available, pin the same repository tag:

```toml
[dependencies]
tact_agent_sdk = { git = "https://github.com/rust-infra/tact", branch = "codex/agent-kernel-design" }
tact_llm = {
  git = "https://github.com/rust-infra/tact",
  branch = "codex/agent-kernel-design",
  default-features = false,
  features = ["openai-chat"]
}
```

Use the `v2.0.0` tag after that release is published; the branch form above is
the pre-release path for this tree.

## Minimal agent

```rust,no_run
use std::sync::Arc;

use tact_agent_sdk::prelude::*;

# async fn run(model: Arc<dyn ModelPort>) -> Result<(), Box<dyn std::error::Error>> {
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
`ToolPort` and an explicit gate such as `AllowAllGate`; configuring tools
without a gate fails at build time instead of surfacing later as denied calls.

## Tools as static plugins

Plugin style here means constructor injection: implement `ToolHandler`, compose
handlers with `ToolSet`, and pass the result through `AgentBuilder::tools`.
There is no mutable runtime registry.

`tact_agent_sdk::prelude` re-exports the API types, `async_trait`, and
`serde_json` needed by custom model and tool adapters:

```rust,no_run
use tact_agent_sdk::prelude::*;

#[async_trait]
impl ToolHandler for MyTool {
    fn spec(&self) -> ToolSpec { /* ... */ unimplemented!() }

    async fn execute(
        &self,
        input: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        Ok(ToolExecutionResult::success(ToolOutcome::text(input.to_string())))
    }
}
```

`ToolHandler::access` defaults to `ResourceAccess::barrier()`. Override it only
when a tool's resource scope is known and concurrency is safe. `ToolSet`
snapshots specs and metadata at insertion and rejects duplicate names.

`ToolExecutionContext::run` carries the session and activation IDs. Model
adapters that need the same identity can override `ModelPort`'s
`stream_with_context`, `complete_with_context`, or `compact_with_context`
methods; the default implementations forward to the existing methods.

## Sessions

`Agent` may create multiple independent `AgentSession`s. Each session owns an
`AgentState`; run methods require `&mut self`, which serializes activations.
Session-id uniqueness and durable-store exclusivity are host responsibilities.

When restoring state, `build_session_from_state` requires
`state.initialized == true`. The default `NoopTranscriptStore` keeps no rows;
inject a durable store when the host needs persistence. A state bound to one
session cannot be restored under another session. `AgentSession::into_state`
exports the complete state, including the cumulative turn budget.

Durable `TranscriptStore` implementations can also override
`load_state`/`save_state` to persist cumulative turns and the last token
estimate. The kernel uses those values before the first model dispatch after a
restart; the default methods intentionally retain nothing.

## Cancellation

`AgentSession::control()` returns a `SessionControl`. Cancellation applies only
to the active run. Idle cancellation is a no-op, and each run gets a fresh
cancel token.

`AgentSession::compact(request)` exposes manual/native compaction through the
same activation and cancellation boundary as normal runs.

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

## Events

Use `AgentBuilder::on_event` when the host only needs a callback:

```rust,no_run
# use tact_agent_sdk::AgentBuilder;
# fn configure(builder: AgentBuilder) -> AgentBuilder {
builder.on_event(|event| println!("{event:?}"))
# }
```

For UI or multi-session routing, use `on_event_with_context` and route by
`context.session`:

```rust,no_run
# use tact_agent_sdk::AgentBuilder;
# fn configure(builder: AgentBuilder) -> AgentBuilder {
builder.on_event_with_context(|context, event| {
    println!("{}: {event:?}", context.session.as_str())
})
# }
```

Both callback forms enqueue to a worker thread and never wait for the consumer
on the kernel thread. For custom transports, implement `EventPort` and override
`emit_shared_with_context`; terminal events and usage records also carry
`RunContext`.

## Context policy

`BasicContextPolicy` forwards every caller input, converts the transcript to
provider-neutral messages, and does not compact. A host that needs context
recovery must inject a custom `ContextPolicy`. A custom policy cannot be combined
with `system_prompt` or `model_options`.

## Provider adapters

Add `tact_llm` directly in the host crate when a concrete provider is needed,
then pass the resulting `ModelPort` to `AgentBuilder::new`. Provider HTTP stacks
stay outside the SDK dependency graph.
