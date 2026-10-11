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
  register capabilities on the serving router and manifests in
  `RuntimeContext::plugins()`, drain findings into the session notices, attach to
  the Agent.
- [x] Teardown: `Agent::shutdown_hosted_plugins` called next to `shutdown_mcp` in
  the interactive driver and in headless.
- [x] Tests: 10 in `plugin::runtime` (parse, discovery, per-package skip,
  registration, invocable through the serving router, duplicate-capability
  isolation, teardown).
- [x] Docs: `ARCHITECTURE.md` §15, 2026-10-08 spec/plan status notes,
  `docs/plugin_protocol.md`, the consumer verdicts, `book/26_chapter_issue_zh.md`.

**Not done (deliberate):** plugin capabilities are not model-visible tools (tool
dispatch is untouched); plugin host calls stay denied, so the Kernel services
stay reserved; no supervision/restart and no health surface.

**Verification:** `./scripts/check-rust.sh` ends with `Rust checks passed.`;
whole-workspace `cargo test --workspace` counts recorded in the book entry.
