# Installed plugins as hosted runtime extensions — design

**Status:** implemented 2026-10-11.
**Context:** `docs/superpowers/specs/2026-10-08-runtime-plugin-architecture-design.md`
§3 (the `discover -> validate -> handshake -> register -> serve -> drain -> stop`
contract). The hosts were implemented and tested, but no product code started
one: `tact_plugin_host`, `tact_plugin_node` and `tact_plugin_wasm` had no caller,
so the plugin-host boundary — and the capabilities `ARCHITECTURE.md` §15 calls
"reserved for a plugin host" — stayed unreachable in a real session.

## Goal

An **installed plugin** (a marketplace package under `~/.tact/plugins`) may
declare a *runtime half*; the session bootstrap then spawns it through
`StdioPluginHost`, registers the capabilities it declares at handshake on the
session's **serving router** — except capabilities of kind `Tool`, which are
model-visible tools and go on the Agent's per-wave tool router (see the
superseded note under Non-goals) — registers its manifest in the session's
`RuntimeContext::plugins()` registry, and stops it when the session ends.

One generic stdio path serves all three host crates: a Node plugin is
`node index.mjs`, a WASM plugin is the configured runner, and a compiled binary
is itself — the manifest names the command, and the host crates stay thin layers
over the same envelope protocol.

## Non-goals

- A `HostCallService` bridge. **Superseded 2026-10-11 (see below):** host calls
  were `DenyHostCalls` when this was written, so a plugin could not reach the
  Kernel services (`storage.*`, `events.*`, …). A manifest that declares
  `runtime.grants` now gets `GrantedHostCalls`; the reservation recorded in
  `docs/superpowers/specs/2026-10-11-kernel-service-consumers-design.md` is
  updated there.
- Supervision: no restart policy, no health surface. A crashed plugin stays
  failed.
- Marketplace-installed *MCP servers*, which already have a product path.

**Superseded 2026-10-11 (later the same day):** this design originally listed
"making plugin capabilities model-visible tools" as a non-goal, keeping every
declared capability on the serving router. That was wrong for capabilities of
kind `Tool`: a tool is model-visible, and a tool reaching the serving router
bypasses the tool preflight (the sensitive-target guard and the one-use approval
ticket). A `Tool` declaration is now registered on the Agent's **per-wave tool
router** instead, appears in the tool list the model is sent, and is resolved in
preflight with the risk its declaration carries; the serving router keeps the
non-`Tool` kinds (`Command` / `App` / `Service`), which is their product path.
See `crates/tact_extensions/src/plugin/runtime.rs::{HostedPlugins::register,
tool_specs, tool_declaration, register_tools}` and
`crates/tact_extensions/src/agent/tool_dispatch.rs`.

## The manifest

The installed plugin's `.codex-plugin/plugin.json` (already read at install
time) gains an optional `runtime` object:

```json
{
  "name": "echo",
  "runtime": {
    "id": "acme.echo",
    "command": "node",
    "args": ["plugin.mjs"],
    "grants": ["storage.get", "events.publish"]
  }
}
```

- `id` — required; deserialized as a protocol `PluginId`, so its shape is
  validated while reading (no whitespace, no control characters).
- `command` — required; a program on `PATH` or a path, passed to the spawner
  verbatim.
- `args` — optional. An arg is rewritten to `<plugin-root>/<arg>` **only when
  that file exists**, so a package can ship its entry point without depending on
  the session's working directory, while a flag or a bare value (`--port`,
  `serve`) is left alone — no heuristic has to guess which args are paths.
- `grants` — optional, default empty. The capability names the plugin may invoke
  as **host calls** while it handles one of its own invocations. The list is the
  manifest's own bound and nothing more: a granted name still has to be a
  `Service` on the serving router, and the call is permission-checked as the
  plugin. Absent or empty is exactly the behaviour of a plugin that never asked
  for host calls — see *Host calls* below.
- Unknown keys inside `runtime` are rejected (`deny_unknown_fields`), like the
  other protocol-adjacent formats. `grants` is a known key; a typo next to it is
  still a rejection rather than a silently ignored setting.

Absence of `runtime` is the normal case: a plugin that only ships skills,
commands, hooks or MCP servers is untouched. The surrounding manifest keeps its
compatibility keys; only `runtime` is read here.

## Discovery and lifecycle

`crates/tact_extensions/src/plugin/runtime.rs`:

- `PluginRuntimeSpec` / `read_runtime_spec(root) -> Result<Option<PluginRuntimeSpec>>`.
- `discover_runtime_plugins(home) -> (Vec<(PluginRuntimeSpec, PathBuf)>, Vec<String>)`
  — installed plugin roots (`PluginStore::installed_plugin_roots`, which already
  validates the cache path) that declare a runtime half. A package whose
  `runtime` does not parse is **skipped with a message**, never fatal to the
  scan: one mistyped key must not stop the session hosting every other plugin.
- `HostedPlugins` — the session's live hosts:
  - `start(specs)`: `StdioPluginHost::start` (spawn + handshake) per plugin; a
    failure is recorded in `notice_lines()`, never returned.
  - `register(router, registry)`: registers each host's manifest in the registry
    (`register` + `start`) and its non-`Tool` capabilities on the router (via
    `register_host_serving_capabilities_with_router`, the predicate form of the
    generic `register_host_with_router`), caching the `Tool` declarations for the
    spec rebuild and preflight. `register_tools(router)` registers those tools on
    the per-wave tool router. A duplicate plugin id is caught before the router
    is touched; if capability registration then fails (a name already on the
    router), the manifest is `stop`ped + `unregister`ed and the host is stopped,
    so a plugin that serves nothing is not left running.
  - `shutdown()`: `PluginHost::shutdown` + `PluginRegistry::stop` per host.
  - `empty()` / `from_hosts(..)`: the "no plugins" case, and the seam a test
    injects a `PluginHost` double through.

`crates/tact_plugin_host` gained the generic seam so registration is not
stdio-specific:

- `PluginHost` gained three defaulted methods — `request_timeout()`,
  `request_with_context(context, request)` and `interrupt()` — so the router
  handler can address a `dyn PluginHost`. `StdioPluginHost` overrides all three
  (deadline on the wire, `terminate` on interrupt); the defaults are correct for
  an in-process host or a test double.
- `register_host_with_router(Arc<Mutex<dyn PluginHost>>, &CapabilityRouter, Arc<dyn HostCallService>)`
  is public; `StdioPluginHost::register_with_router_and_host_calls` delegates to
  it, and `DenyHostCalls` is public so a direct caller lands on the same
  fail-closed policy. Two predicate entries share the one registration body:
  `register_host_serving_capabilities_with_router` (everything that is not a
  `Tool`, the sync bootstrap path) and the async `register_host_tools_with_router`
  (only `Tool`s, for a router built on the turn's hot path).

## Session wiring

`crates/tact_ui/src/session_bootstrap.rs`, next to the plugin MCP load: discover
`PluginHome::from_environment()` → `HostedPlugins::start` → `register(&serving, serving.plugins())`
→ drain `notice_lines()` into the session notices → `agent.with_hosted_plugins(..)`
(which rebuilds the tool list, so the plugin tools are model-visible from the
first request).

Both frontends stop them at teardown next to `shutdown_mcp`
(`Agent::shutdown_hosted_plugins`): the interactive driver's teardown block and
headless's end, so a plugin process never outlives the session.

## Permission

Non-`Tool` plugin capabilities land on the **serving router**, so
`ServingPermission` governs them exactly as it governs any plugin-invoked
capability: the host-control-plane names stay allow-listed and everything else is
decided by the session's configured policy (fail-closed in `ask`). Nothing is
added to `HostControlPlanePermission`, and a plugin's `runtime.id` cannot shadow
an official extension — the registry refuses a duplicate id.

A `Tool` capability is decided by the **tool** permission ladder instead, in
preflight, from the risk tier its declaration carries — the same ladder a native
or MCP tool goes through, and the one the serving router deliberately does not
reach (see the superseded note above).

## Host calls

A plugin may call **back into** the Kernel while it handles one of its own
invocations: it returns `PluginResponse::HostCall { host_request_id, capability,
input }`, and the host answers with a correlated `PluginRequest::HostCallResult`.
The loop, the recursion guard (a plugin cannot invoke its own capability) and the
64-call cap were already in `HostCapabilityHandler::invoke_plugin`; what grants
add is the policy at the far end and the handshake that tells the plugin the host
will answer at all.

- **Handshake.** `HostedPlugins::start` builds its feature list with
  `handshake_features(&spec.grants)`: `tact_plugin_host::default_handshake_features()`
  plus `host_calls` when the manifest declared any grant. A plugin that declared
  none is handed exactly the default list, which is what
  `StdioPluginHost::start` asked for before grants existed — the fail-closed
  path is byte-for-byte the old handshake.
- **Policy.** `crates/tact_plugin_host/src/runtime.rs::GrantedHostCalls` holds
  the plugin's `PluginId`, its grants as a `BTreeSet` and a clone of the
  session's serving `RuntimeContext`. It refuses an ungranted name by name, then
  reads the declaration off the **serving router** and refuses anything that is
  not a `CapabilityKind::Service`. That one kind check is what keeps a `Tool` on
  its per-wave preflight path (sensitive-target guard, one-use approval ticket)
  and a `Command` / `App` / `View` / `EventHandler` with its owner extension. The
  router is the right source: `tact_plugin_wasm`'s runner keeps a hard-coded
  capability list because it decides *before* a router exists, while here the
  Kernel's services are already registered on the router the call lands on.
- **Invocation.** The granted capability is invoked on the serving router with a
  **fresh** invocation — a new `RequestId` and the plugin's own `PluginId` — so
  the ordinary permission path decides it. The identity is load-bearing: it is
  the namespace `storage.*` authorizes against, and it is not a host identity, so
  `HostControlPlanePermission` refuses `chat.submit` / `runs.*` for it. The
  `router` argument the trait hands over is ignored on purpose: an inner invoke
  on the per-wave tool router would hit `PreflightPermissionGate`, whose one-use
  ticket belongs to the outer tool call.
- **Wiring.** `HostedPlugins::register(&serving, registry)` picks the policy per
  host (grants ⇒ `GrantedHostCalls`, none ⇒ `DenyHostCalls`) and passes the
  *same* `Arc<dyn HostCallService>` to the serving-capability registration and to
  `register_tools` → `register_host_tools_with_router`, so a plugin's tool and
  its command behave alike. A plugin that fails registration keeps no policy;
  `register_tools` falls back to `DenyHostCalls`.

## Tests

`plugin::runtime` (17): manifest parsing (present / absent / unknown key
rejected, with a valid `grants` next to the typo so the grant list is not an
"ignore the rest" escape hatch; the declared grants themselves), the handshake
feature list (a granted plugin gets the default list plus `host_calls`, a plugin
without grants gets exactly the default list), relative-arg rewriting only where
a file exists, discovery over a temp plugin home (only runtime-declaring plugins;
a malformed manifest skipped while the healthy ones are still found), `register`
puts the declared capabilities on the router, a registered plugin capability is
invocable through the serving router, a `Tool` declaration stays off the serving
router while a `Command` one does not, the plugin tool becomes a spec and is
invocable on a per-wave router, a clashing plugin tool is reported without
failing the others, a duplicate capability fails only the later plugin, teardown
stops every started host, and the host-call path end to end twice: a granted
plugin's `HostCall` reaches a real `storage.get` over a recording storage double
and the service's output comes back to the plugin, while a plugin without grants
is handed a `PermissionDenied` result and the service is never entered.
`crates/tact_plugin_host/tests/host_call_policy.rs` (6) pins the policy itself:
a granted service is invoked as the plugin, an ungranted name, a granted `Tool`,
a granted `chat.submit` (refused by the kind check before any policy runs) and a
name the router does not carry are all denied, and a plugin identity is refused
by the host control plane's rule. The Agent side is covered in
`agent::tool_dispatch::tests`
(resolution with the declared risk, the high-risk headless denial, the
sensitive-guard exemption) and `agent::tests` (the tool list plus a full turn
whose mock provider calls the plugin tool). The stdio path itself stays covered
by the existing `tact_plugin_host` / `tact_plugin_node` / `tact_plugin_wasm`
suites.

Not covered by a test: the bootstrap call site and the two teardown call sites
(they are three lines each, and a bootstrap test would read the *real*
`$HOME/.tact/plugins`).

## Docs

`ARCHITECTURE.md` §15, the 2026-10-08 spec/plan status notes,
`docs/plugin_protocol.md`, the consumer verdicts in
`docs/superpowers/specs/2026-10-11-kernel-service-consumers-design.md`, and a
`book/26_chapter_issue_zh.md` entry.
