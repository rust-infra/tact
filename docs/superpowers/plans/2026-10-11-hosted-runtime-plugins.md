# Hosted runtime plugins — plan

**Spec:** `docs/superpowers/specs/2026-10-11-hosted-runtime-plugins-design.md`
**Status:** implemented 2026-10-11.

- [x] `tact_plugin_host`: defaulted `PluginHost::{request_timeout, request_with_context, interrupt}`
  and a public generic `register_host_with_router(Arc<Mutex<dyn PluginHost>>, …)`
  that `StdioPluginHost::register_with_router_and_host_calls` delegates to;
  `DenyHostCalls` made public. This is the seam that lets a test double register
  through the production path.
- [x] `crates/tact_extensions/src/plugin/runtime.rs`: `PluginRuntimeSpec` +
  `read_runtime_spec` + `discover_runtime_plugins` (per-package skip on a
  malformed manifest) + `HostedPlugins::{start, empty, from_hosts, register,
  notice_lines, shutdown}`.
- [x] `session_bootstrap`: discover from `PluginHome::from_environment()`, start,
  register the non-`Tool` capabilities on the serving router (a `Tool` capability
  is a model-visible tool and goes on the Agent's per-wave tool router instead)
  and manifests in `RuntimeContext::plugins()`, drain findings into the session
  notices, attach to the Agent.
- [x] Teardown: `Agent::shutdown_hosted_plugins` called next to `shutdown_mcp` in
  the interactive driver and in headless.
- [x] Tests: 13 in `plugin::runtime` (parse, discovery, per-package skip,
  registration, a `Tool` staying off the serving router, the plugin tool as a
  spec and invocable on a per-wave router, a clashing plugin tool reported
  without failing the others, invocable through the serving router,
  duplicate-capability isolation, teardown) plus the Agent-level plugin-tool
  tests in `agent::tests` / `agent::tool_dispatch::tests`.
- [x] Docs: `ARCHITECTURE.md` §15, 2026-10-08 spec/plan status notes,
  `docs/plugin_protocol.md`, the consumer verdicts, `book/25_chapter_issue_zh.md`.

**Superseded 2026-10-11 (later the same day):** the "plugin capabilities are not
model-visible tools" non-goal was wrong for capabilities of kind `Tool`. A `Tool`
declaration is a model-visible tool: it is now registered on the Agent's per-wave
tool router (never the serving router, which would bypass the tool preflight),
appears in the tool list the model is sent, and is resolved in preflight with the
risk its declaration carries.

**Not done (deliberate):** no supervision/restart and no health surface.

**Follow-up 2026-10-11 (shipped):** plugin *host calls* are no longer always
denied. `PluginRuntimeSpec` gained `#[serde(default)] grants`, a plugin with
grants is started with the `host_calls` handshake feature
(`handshake_features`, on top of `tact_plugin_host::default_handshake_features`)
and registered under `tact_plugin_host::GrantedHostCalls`, which invokes the
granted `Service` on the serving router as the plugin. `HostedPlugins::register`
now takes `&RuntimeContext` and passes one policy to both the serving- and the
tool-registration paths. Spec:
`docs/superpowers/specs/2026-10-11-hosted-runtime-plugins-design.md` § *Host
calls*; policy tests in `crates/tact_plugin_host/tests/host_call_policy.rs`; the
end-to-end host-call loop is covered in `plugin::runtime`'s tests.

**Verification:** `./scripts/check-rust.sh` ends with `Rust checks passed.`;
whole-workspace `cargo test --workspace` counts recorded in the book entry
(2915 passed before the grant work, 2925 after).
