# Kernel service capabilities and the plugin registry — consumer verdict

**Status:** decided 2026-10-11.
**Context:** `ARCHITECTURE.md` §15 ("Declared but not yet consumed") listed two
open items: ten of the twelve §4 service capabilities have no product-code
caller, and `Agent::runtime_plugins` was written and never read.

The question for each was the same: *connect it to a real consumer, or declare
it reserved and stop carrying it where it does not belong.* This records the
answer per item, so the "unconsumed" table is a decision rather than an
unanswered question.

## 1. The twelve §4 service capabilities

Verdict: **reserved** for the plugin-host / external-View boundary. No in-repo
consumer is faked for them, because the honest consumer does not exist yet.

| Capability | Verdict | Boundary that consumes it |
|---|---|---|
| `runs.start` / `runs.cancel` | **consumed** | the host control plane (`chat.submit`, the driver's cancel arms) |
| `sessions.read` / `sessions.write` | reserved | the Session extension serves them for a plugin host or external View; the two hosts deliberately use `DynSessionStore` directly, and moving them behind the capabilities is the Command/View migration, not this |
| `storage.get` / `storage.set` | reserved | a plugin host managing its own `plugins/<id>` namespace |
| `events.subscribe` / `events.publish` | reserved | a plugin host subscribing to the session stream |
| `trajectory.read` / `trajectory.append_plugin_event` | reserved | a plugin host / external View querying or appending history |
| `permission.request` | reserved | a plugin host asking the host for a decision (fail-closed until a responder exists) |
| `interaction.request` | reserved | a plugin host / View raising a prompt (fail-closed until a responder exists) |

Evidence for "no in-repo consumer is missing": the only code that invokes the
eight `tact::services::register` capabilities is a test
(`session_bootstrap::tests::the_serving_context_serves_the_kernel_capabilities_over_real_backing`),
and each host's own path uses the underlying store/service directly on purpose
— the capabilities are the *plugin* surface over the same backing, not a second
internal API. The three-in-one conclusion is that the intended consumer is the
generic plugin host (`tact_plugin_host` / node / wasm) and the Web/Desktop
Views, none of which have a product caller yet; that is the "real plugin host"
and "Web/Desktop" work, not wiring that can be done here without inventing a
consumer.

## 2. The plugin registry

Verdict: **wrong owner, deleted from the Agent; no reader yet, and the reader is
a product decision.**

`Agent::runtime_plugins: PluginRegistry` was populated by the session bootstrap
and never read. Two things follow:

- The owner was wrong. Extension lifecycle is *session* state; an `Agent` is one
  participant in a session, and a registry describes the whole runtime the
  session serves. The registry moves to the session's serving
  `RuntimeContext` (`RuntimeContext::plugins()`), next to the router and the
  services it belongs with, and `Agent::runtime_plugins` plus
  `Agent::with_plugin_registry` are deleted.
- The reader does not exist. Reporting extension health / supervising plugins is
  a host surface (`ARCHITECTURE.md` §15 already recorded it as "a product
  decision (a `/plugins`-style surface), not wiring"). This change does not
  invent one; it puts the state where such a surface will read it, and the
  bootstrap still registers the official manifests there — which keeps the
  dependency-order validation that is the registration's real startup effect.

## Non-goals

- A `/plugins`-style health surface (product decision).
- Routing the hosts' own session / storage / trajectory access through the
  service capabilities (the Command/View migration).
- A product caller for the generic plugin hosts.

## Update — hosted runtime plugins implemented (2026-10-11)

The "real plugin host" this document named as the consumer of most reserved
capabilities now exists: an installed plugin whose `.codex-plugin/plugin.json`
declares a `runtime` half is spawned at session bootstrap through
`StdioPluginHost`, and the capabilities it registers at handshake are registered
on the serving router (spec
`docs/superpowers/specs/2026-10-11-hosted-runtime-plugins-design.md`).

The verdicts above are unchanged, for a reason worth stating: the *host* exists,
but guest **host calls** are denied by default (`DenyHostCalls`), so a plugin
still cannot reach `storage.*` / `events.*` / `trajectory.*` /
`permission.request` / `interaction.request`. Granting a plugin host calls is the
next decision, not a consequence of this one.

One thing this document got wrong and has since been fixed: a hosted capability
of kind `Tool` **is** a model-visible tool. It does not belong on the serving
router at all — a tool reaching that router bypasses the tool preflight (the
sensitive-target guard and the one-use approval ticket) — so a `Tool` declaration
is registered on the Agent's per-wave tool router, appears in the tool list the
model is sent, and is resolved in preflight with the risk its declaration
carries. The non-`Tool` kinds (`Command` / `App` / `Service`) stay serving
capabilities. See `crates/tact_extensions/src/plugin/runtime.rs` and
`crates/tact_extensions/src/agent/tool_dispatch.rs`.

The registry verdict is also unchanged: the hosted-plugin lifecycle now
registers / starts / stops each plugin's manifest through
`RuntimeContext::plugins()`, but nothing *reads* the state back — a health
surface remains a product decision.

## Verification

`crates/tact_extensions/src/extensions/mod.rs` gains a pin that the official
manifests register on a `RuntimeContext` and are readable back through a clone
of it (the handle a host holds). Whole-workspace `./scripts/check-rust.sh`.
