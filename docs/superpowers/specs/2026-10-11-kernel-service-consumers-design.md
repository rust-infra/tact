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
| `runs.start` / `runs.cancel` | **consumed** | the host control plane (`chat.submit`, the driver's cancel arms); a plugin may grant the *name*, but the kind check and the host control plane both refuse it (see *Update 2* below) |
| `sessions.read` / `sessions.write` | reserved (reachable with a grant) | the Session extension serves them (`Service` kind) for a plugin host or external View; the two hosts deliberately use `DynSessionStore` directly, and moving them behind the capabilities is the Command/View migration, not this |
| `storage.get` / `storage.set` | reserved (reachable with a grant) | a plugin host managing its own `plugins/<id>` namespace |
| `events.subscribe` / `events.publish` | reserved (reachable with a grant) | a plugin host subscribing to the session stream |
| `trajectory.read` / `trajectory.append_plugin_event` | reserved (reachable with a grant) | a plugin host / external View querying or appending history |
| `permission.request` | reserved; answered by the interactive host's broker, fail-closed otherwise (see *Update 3*) | a plugin host asking the host for a decision |
| `interaction.request` | reserved; answered by the interactive host's broker, fail-closed otherwise (see *Update 3*) | a plugin host / View raising a prompt |

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

When this was written the *host* existed but guest **host calls** were denied by
default (`DenyHostCalls`), so a plugin still could not reach `storage.*` /
`events.*` / `trajectory.*` / `permission.request` / `interaction.request`.
Granting a plugin host calls was the next decision, not a consequence of this
one — and it has since been taken (see *Update 2* below).

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

## Update 2 — a declared grant makes the service capabilities reachable (2026-10-11)

An installed plugin's `runtime` object gained `grants`: a list of capability
names the plugin may invoke as **host calls** while it handles one of its own
invocations. A plugin that declares none keeps `DenyHostCalls` and is not offered
the `host_calls` handshake feature — the fail-closed default above is unchanged
for it. A plugin that declares some is registered under
`tact_plugin_host::GrantedHostCalls`, which answers a host call by invoking the
named capability **on the serving router, as the plugin**:

- the granted name must also be a `CapabilityKind::Service` on that router — the
  one kind check that keeps a `Tool` on its per-wave preflight path and a
  `Command` / `App` with its owner extension — so the per-capability verdicts
  above become *reachable on the ordinary permission path*: `storage.*`,
  `events.*`, `trajectory.*` and the Session extension's `sessions.*`;
- `permission.request` / `interaction.request` stay fail-closed *in a policy built
  without an interaction surface*, so the call is refused rather than hung. (The
  interactive host now builds its policy *with* one — see *Update 3*; the
  headless host keeps the shape described here);
- `runs.start` / `runs.cancel` are `App` / `Command` on the serving router, so
  the kind check refuses them before anything else — and were they `Service`, the
  plugin's identity is not one of `HOST_CONTROL_PLANE_IDENTITIES`, so
  `HostControlPlanePermission` would refuse them anyway.

The verdicts above are therefore amended in exactly one direction: the eight
Kernel-owned service capabilities and `sessions.*` have a **wired consumer
surface** (a granted hosted plugin) without any in-repo caller changing. Nothing
here invents a consumer for them, and a plugin still cannot reach anything its
manifest did not name.

See `docs/superpowers/specs/2026-10-11-hosted-runtime-plugins-design.md` § *Host
calls*, `crates/tact_plugin_host/src/runtime.rs::GrantedHostCalls`,
`crates/tact_extensions/src/plugin/runtime.rs::host_call_policy`, and the policy
tests in `crates/tact_plugin_host/tests/host_call_policy.rs`.

## Update 3 — the interactive host installs the interaction broker (2026-10-11)

The two rows above that read "fail-closed until a responder exists" now have
that responder *in the interactive host*. The frontend creates a
`tact::InteractionBroker`, `bootstrap_session` installs it as the session's
interaction service **and** as the serving policy's `PermissionResponder`, and
the driver routes `RuntimeCommand::RespondInteraction` to it before falling back
to the in-process `UiResponder`. So on that host:

- `interaction.request` is answered by whatever client is subscribed to the
  broker;
- `permission.request` is answered the same way — it asks the **permission**
  service, not the interaction service, so the broker alone would not cover it;
  the policy's responder is what makes the two capabilities symmetric;
- `InteractionBroker::withdraw` lets a client abandon a prompt the waiting
  invocation then sees as `PluginUnavailable` ("client disconnected"), and
  `pending_count()` / `snapshot()` report what is still outstanding.

`permission.request` is answered only where the capability's own permission
*check* allows the call, because `CapabilityRouter::invoke` checks before it
dispatches: in `default` (ask) mode a plugin-initiated `permission.request` is
denied before its handler runs, so the broker is never asked. The responder
answers the prompt the handler raises; it does not turn "ask" into "show a
dialog". `interaction.request` has the same check and the same shape — in `ask`
mode neither capability reaches its handler.

Headless is unchanged and still fail-closed, deliberately: with no `UiWiring`,
`build_serving_context` keeps the no-op interaction service and the policy keeps
no responder, so both capabilities are refused rather than hung.

Two limits are worth stating rather than implying:

- **No shipped client subscribes to the broker yet.** The interactive host
  installs one but the TUI is not a broker client: its popup reconciles from
  `UiResponder::snapshot()` and is keyed by that responder's decimal `u64` id,
  and it renders only the select family. So a Kernel prompt raised in an
  interactive session is refused (`PluginUnavailable`, "no interaction client is
  subscribed") — still fail-closed, no longer `PermissionDenied`. Turning broker
  requests into the popup (arbitrary `RequestId`s plus the `Confirm` / `Input` /
  `Permission` kinds) is the remaining View-adapter work, and it is what would
  make `tact_extensions::ui_responder::UiResponder` the tool-prompt path alone.
- **A host call is run-less, and it is bounded now**, so a *subscribed* client
  that never answers no longer leaves the invocation waiting until the run is
  cancelled or the process exits: `GrantedHostCalls` puts the host's
  `DEFAULT_REQUEST_TIMEOUT` on the inner Kernel-service invoke, so a plugin's
  `interaction.request` fails on that bound. It still records nothing —
  `RuntimeEvent::TimedOut` needs a run, and a host call has none. The §7 timeout
  gap this note used to leave open is closed elsewhere: a hosted plugin's *tool*
  call is both deadline-bearing and run-carrying, so it publishes `TimedOut` for
  its run (see the 2026-10-08 spec / plan). A host call recording nothing stays
  a property of this wiring *and* of the boundary decision, not of the broker.

## Verification

`crates/tact_extensions/src/extensions/mod.rs` gains a pin that the official
manifests register on a `RuntimeContext` and are readable back through a clone
of it (the handle a host holds). Whole-workspace `./scripts/check-rust.sh`.
