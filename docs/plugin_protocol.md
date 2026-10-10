# Plugin Protocol

Tact plugins cross the Runtime boundary through the types exported by `tact_protocol`. The protocol is independent of TUI and can travel over an in-process adapter, stdio RPC, or IPC. Node.js and WASM guests use the same newline-delimited JSON envelopes over stdio.

Every request envelope carries a protocol version, request ID, plugin ID, and optional session, run, trajectory, and deadline context. Requests cover handshake, capability registration, invocation, event subscription, cancellation, interaction responses, and shutdown.

Capabilities are declared with a stable name, kind, version, schemas, and risk. Runtime dispatch resolves every capability through `CapabilityRouter`, then checks `PermissionService`, cancellation, and deadline before calling the implementation. Agent currently retains sequential hook, permission, and resource preflight for its interactive workflow; the router consumes a one-use authorization ticket from that phase before executing native or MCP handlers. Node.js and WASM host calls use the same permissioned router path. Routed tool calls with a run ID publish and append their started and finished facts through the configured event and trajectory services.

When the `host_calls` feature is negotiated, a plugin may return `PluginResponse::HostCall` while handling an invocation. The host dispatches the requested service under the plugin manifest grants and the invocation's permission context, then sends a correlated `PluginRequest::HostCallResult`. Host calls are bounded per invocation, and errors return as protocol errors so the plugin can handle denied requests.

The WASM host spawns a caller-configured runner executable over stdio with fuel, linear-memory size, and execution-timeout arguments. The guest does not inherit environment variables, is not given preopened directories, and is asked to disable WASI TCP/UDP. Storage, clock, event, trajectory, and external-capability requests go through the negotiated protocol host-call path. Note that this repository links no WASM engine — the limits are runner arguments, honoured only if the configured runner enforces them.

Plugin events must be namespaced as `plugin.<plugin_id>.<event>`. The plugin ID and origin are carried in the event and validated together. Reserved host facts such as permission, tool, run, model, and lifecycle events cannot be forged by a plugin.

The interactive host uses `RuntimeCommand::StartRun`, `CancelRun`, and `RespondInteraction` for chat submission and select responses. The TUI consumes `RuntimeEvent` for streamed text, thinking, progress, model and token status, run lifecycle, popup content, and interaction requests, then projects those values into its existing widget state.

Tact-specific slash commands still travel in the View's own `UserCommand` vocabulary (with `UserCommand::Runtime(RuntimeCommand::…)` as the protocol escape hatch); the `AgentUpdate` compatibility type is deleted and the runtime/View event path is protocol-only. The generic plugin hosts and the official Agent, Chat, Session, and Workflow **manifests** use `PluginRegistry`; their capability handlers are not yet registered on the production router, so `runs.*` / `sessions.*` / `workflow.run` are reachable from tests only.
