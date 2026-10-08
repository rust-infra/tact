# Plugin Protocol

Tact plugins cross the Runtime boundary through the types exported by `tact_protocol`. The protocol is independent of TUI and can travel over an in-process adapter, stdio RPC, IPC, or a WASM host function boundary.

Every request envelope carries a protocol version, request ID, plugin ID, and optional session, run, trajectory, and deadline context. Requests cover handshake, capability registration, invocation, event subscription, cancellation, interaction responses, and shutdown.

Capabilities are declared with a stable name, kind, version, schemas, and risk. Runtime dispatch resolves every capability through `CapabilityRouter`, then checks `PermissionService`, cancellation, and deadline before calling the implementation. Native Rust, MCP, Node.js, and WASM implementations use the same semantic path.

Plugin events must be namespaced as `plugin.<plugin_id>.<event>`. The plugin ID and origin are carried in the event and validated together. Reserved host facts such as permission, tool, run, model, and lifecycle events cannot be forged by a plugin.

The protocol keeps legacy `AgentUpdate` and `UserCommand` exports during migration. New clients use `RuntimeEvent`, `RuntimeCommand`, `InteractionRequest`, and `InteractionResponse`.
