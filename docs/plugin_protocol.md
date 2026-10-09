# Plugin Protocol

Tact plugins cross the Runtime boundary through the types exported by `tact_protocol`. The protocol is independent of TUI and can travel over an in-process adapter, stdio RPC, or IPC. Node.js and WASM guests use the same newline-delimited JSON envelopes over stdio.

Every request envelope carries a protocol version, request ID, plugin ID, and optional session, run, trajectory, and deadline context. Requests cover handshake, capability registration, invocation, event subscription, cancellation, interaction responses, and shutdown.

Capabilities are declared with a stable name, kind, version, schemas, and risk. Runtime dispatch resolves every capability through `CapabilityRouter`, then checks `PermissionService`, cancellation, and deadline before calling the implementation. Agent currently retains sequential hook, permission, and resource preflight for its interactive workflow; the router consumes a one-use authorization ticket from that phase before executing native or MCP handlers. Node.js and WASM host calls use the same permissioned router path. Routed tool calls with a run ID publish and append their started and finished facts through the configured event and trajectory services.

When the `host_calls` feature is negotiated, a plugin may return `PluginResponse::HostCall` while handling an invocation. The host dispatches the requested service under the plugin manifest grants and the invocation's permission context, then sends a correlated `PluginRequest::HostCallResult`. Host calls are bounded per invocation, and errors return as protocol errors so the plugin can handle denied requests.

The WASM host launches a Wasmtime CLI runner with configured fuel, linear-memory size, and execution timeout. The guest receives stdio for the protocol, does not inherit environment variables, is not given preopened directories, and has WASI TCP/UDP disabled. Storage, clock, event, trajectory, and external-capability requests go through the negotiated protocol host-call path.

Plugin events must be namespaced as `plugin.<plugin_id>.<event>`. The plugin ID and origin are carried in the event and validated together. Reserved host facts such as permission, tool, run, model, and lifecycle events cannot be forged by a plugin.

The protocol keeps legacy `AgentUpdate` and `UserCommand` exports during migration. New clients use `RuntimeEvent`, `RuntimeCommand`, `InteractionRequest`, and `InteractionResponse`.
