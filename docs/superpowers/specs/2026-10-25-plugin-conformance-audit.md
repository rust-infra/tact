# Plugin-first migration — spec conformance audit

Audit of the approved spec
([`2026-10-25-plugin-first-architecture-design.md`](2026-10-25-plugin-first-architecture-design.md))
against the implemented state on `feat/plugin-first-architecture`.

**Status: 6 of 11 checklist items met, 3 partially met, 2 not met.**
Phases 0, 1, and the view half of Phase 4 are done. The remaining gaps all trace
to one undecided thing — the neutral message model — plus the deletions that wait
behind it.

Each item states what the spec asks, what the code does, and how the claim was
verified. "Verified by" names a command or test, not an opinion.

---

## Checklist status

| # | Item | Status |
|---|---|---|
| 1 | Kernel boundary small enough to build without extensions | **Met** |
| 2 | Every capability has an owning plugin and a contribution or contract | **Not met** |
| 3 | Every Agent-issued tool call routed through the kernel executor | **Met** |
| 4 | Agent/Chat/Session ports in the kernel; implementations in extensions | **Partial** |
| 5 | Plugins shut down in reverse install order, releasing resources | **Met** |
| 6 | TUI/headless are views; the host holds only composition and adapters | **Partial** |
| 7 | Commands distributed; no central command enum survives | **Not met** |
| 8 | Migration phases independently shippable and testable | **Met** |
| 9 | Configuration ownership unambiguous; TOML keys preserved | **Partial** |
| 10 | Dynamic MCP/ToolSource items obey the same capability filter | **Met** |
| 11 | Marketplace package behavior and `/plugin` UX unchanged | **Met** |

---

## 1. Kernel boundary — met

`tact_kernel` depends only on `tact_protocol`, `anyhow`, `async-trait`, `serde`,
`serde_json`, `strum`, `tokio(sync)`.

Verified by: `bash scripts/check-architecture.sh` (prints the whitelist and fails
on any other edge).

## 2. Capability inventory — not met

The spec's inventory (§ *Plugin inventory*) lists 5 policy/infrastructure, 4
runtime and 13 capability-domain plugins. Implemented as plugins today:

- Policy/infrastructure: `SecurityPlugin`, `PermissionPlugin`, `StoragePlugin`,
  `SessionStorePlugin`, `ProvidersPlugin`, `MarketplacePlugin`, `ConfigPlugin`
  — all present. (Eight `impl Plugin` blocks are production; two more are test
  impostors in `command/plugin.rs` and `permission/plugin.rs`.)
- Runtime: `SessionPlugin` present (this migration's addition). `AgentPlugin`,
  `ChatPlugin` and `CompactionPlugin` are **absent**: their contracts
  (`AgentService`, `ChatService`, `CompactionService`) are not defined because
  `AgentService::run` needs a neutral message model that the spec leaves open
  (§ *Host-facing services* gives `TurnRequest` / `TurnOutcome` by name only,
  and forbids `tact_llm` types). Defining that model is a design decision, not
  an extraction.
- Capability domains: tools exist as `ToolFamily` plugins (`FamilyPlugin`), but
  Skills, Memory, Goal, Task, Team, Worktree, Background, Subagent, Voice and
  Notifications have **no plugin object of their own** — they are modules with
  tool families and prompt sections, not plugins with a lifecycle.

Verified by: `grep -l 'impl Plugin for' crates/tact/src/**/*.rs` and the
inventory above.

## 3. Tool calls through the executor — met

Every model-originated call goes resolve → PreToolUse → authorize → ticket →
execute. A native call that reaches the dispatch wave without a ticket is an
explicit failure, not a dispatch. MCP tools, resource tools and prompt tools are
catalog entries and take the same pipeline; the legacy ladder is deleted.

Verified by: `crates/tact/tests/execution_parity.rs` (permission parity across
every mode × risk) and the executor tests in `tact_kernel`.

## 4. Ports in kernel, implementations in extensions — partial

`SessionService` lives in `tact_kernel::runtime` (this migration). The view-facing
ports (`SkillCatalog`, `InteractionPort`, `ModelCatalog`, `SessionHistory`,
`SettingsWriter`, `VoicePort`) live in `tui`, which is where the spec says
view-side ports belong.

`AgentService` and `ChatService` do **not** exist in either crate. Same blocker
as item 2.

Verified by: `grep -n 'pub trait' crates/tact_kernel/src/runtime.rs`.

## 5. Reverse-order shutdown — met

`Runtime::shutdown` calls plugin teardown in reverse install order and continues
after individual errors. The order is a contract, not an implementation detail:
the composer installs security before permission, and a host test asserts the
session service is installed after the store it reads.

Verified by: `crates/tact/src/host.rs` tests (`assembly_installs_in_dependency_order`,
`a_host_with_storage_gets_the_session_service`).

## 6. Views and host — partial

`tui` production code has zero `tact`/`tact_llm` references, and both crates are
optional dependencies behind `test-support`; `scripts/check-architecture.sh`
rejects a non-optional edge from `cargo metadata`.

**Not met:** the kernel `View` trait exists
(`tact_kernel::capability::view::View`, one method: `id()`), but no plugin
implements it — there is no `TuiPlugin` or `HeadlessPlugin`, and both frontends
are still functions the host calls directly. The trait being that thin is part of
the gap: the spec's `ViewContext` (agent/chat/session services, command catalog,
interaction broker, workspace, cancellation) was never defined, because it needs
the runtime services from item 2.

The spec also says "the host contains only composition and adapters"; `tact-ui`
still holds the goal runner and the command driver.

Verified by: `grep -rn 'pub trait View' crates` (found, 2 lines) and
`grep -rn 'struct TuiPlugin\|struct HeadlessPlugin' crates` (nothing).

## 7. Commands distributed — not met

Catalog commands exist (`/stats`, `/background`, `/goal`) and are contributed by
`CommandPlugin`, so the *catalog* half is real. `UserCommand` is down from 28
variants to 25 by replacing four read-only variants with `RunCommand { name, args }`.

**Not met:** the spec's checklist line is "no central command enum survives".
`UserCommand` still carries 25 variants, and the TUI still dispatches through its
own `SlashCommand` enum. Commands that drive the agent loop (`create`/`pause`/
`resume`/`cancel` for Goals, `/model`, `/compact`) cannot move until the loop is a
plugin — the same blocker as item 2.

Verified by: variant count in `crates/protocol/src/agent.rs`.

## 8. Phases independently shippable — met

Every phase so far landed as commits that keep `scripts/check-rust.sh` green on
their own. The repository has no phase that only works combined with a later one.

Verified by: `git log --oneline` (each commit is a phase slice) and the per-round
gate log in `.superpowers/sdd/2026-10-25-tool-execution-contracts/`.

## 9. Configuration ownership — partial

`ConfigPlugin` provides the resolved snapshot. The view reads a projection
(`ViewSettings`) and writes through `SettingsWriter`; `tact-ui`'s driver reads the
agent's own snapshot. Existing TOML keys are unchanged — the project's own
`config.example.toml` is still the reference and no key was renamed.

`read_image` and the notification gate no longer read the global: both take a
policy (`VisionPolicy`) or the answer (`enabled`) from the caller, and the
production bootstrap resolves the vision policy from the snapshot it already
read. `Agent::new` reads the configuration once at construction rather than twice,
so the tool families and the settings snapshot cannot disagree.

**Not met:** `config::settings()` still has 6 production readers — three of them
are the `tact-ui` process-start points (`main`, `interactive`,
`session_bootstrap`), which is what a composition root is for; the rest are
`Agent::new`'s one construction-time read, `spawn_subagent`'s settings, and the
`#[tool]` macro's fallback fill. The spec's target — "each plugin receives its
typed settings at construction" — is true for the view and for the two tools
above, not yet for the extension crate as a whole.

Verified by: `grep -rn 'config::settings()' crates --include=*.rs`, filtered to
the region before `#[cfg(test)]` — 6 production readers (down from 11).

## 10. Dynamic sources filtered — met

`CapabilityFilter` applies at static registration and at every `ToolSource` /
`CommandSource` snapshot and resolve, so an MCP refresh cannot re-enable a
disabled item.

Verified by: catalog filter tests in `tact_kernel` and the MCP source tests.

## 11. Marketplace behavior unchanged — met

The package format, install layout and `/plugin` commands are untouched. The DTOs
moved to `tact_protocol` (item: `marketplace.rs`) and the parser stayed in the
extension crate as `MarketplaceSourceExt`, because it validates URLs through
`reqwest` — which the kernel's dependency whitelist forbids.

Verified by: `crates/tact/src/plugin/` tests (183 passing) and
`crates/protocol/src/marketplace.rs` tests.

---

## Deviations from the spec, and why

Each of these is a place where the implementation does something the spec does
not describe. They are listed so a reviewer can accept or reject them
individually.

### D1. `HostOptions` split into `HostOptions` + `HostExtras`

The spec never describes this struct. It was split so the Agent's own assembly
reads as one call instead of passing three `None`s for infrastructure it never
has, and so a host that *does* own a store describes it once.

### D2. `MarketplaceSource::parse` stays in the extension crate

The spec says the type moves to the protocol crate. It did; the *parser* could
not follow, because it validates URLs through `reqwest`. It is
`MarketplaceSourceExt` — an extension trait, since the orphan rule forbids an
inherent `impl` on a foreign type.

### D3. `view_ports` lives in `tact-ui`, not in a view-ports crate

The spec says `tact-ui` implements the TUI-side ports. It does, in one module
(`crates/tact-ui/src/view_ports.rs`). No new crate was created for them.

### D4. Aggregate values moved to `tact_protocol`

`ReasoningEffort`, `MAX_INPUT_CHARS`, the marketplace DTOs, the in-band context
markers and the voice command/event pair. The spec says extension-level values
stay in extensions; these moved because *both* a view and the runtime need them,
and re-declaring them would be two definitions of one contract.

### D5. The guard checks the view edge from `cargo metadata`

The spec's architecture checks are described as dependency-whitelist and
module-boundary. A third rule was added — `tui` may not take a non-optional
dependency on `tact`/`tact_llm`/`tact-ui` — because the source scans cannot see
the dependency graph.

---

## What remains, in dependency order

1. **Decide the neutral message model** (`TurnRequest` / `TurnOutcome` /
   `UserMessage`). Nothing in Phase 3's remaining half can start before this;
   item 2, 4 and 7 all name the same blocker.
2. `AgentPlugin` / `ChatPlugin` / `CompactionPlugin` on top of that model.
3. `View` trait + `TuiPlugin` / `HeadlessPlugin` (item 6).
4. Move the loop-driving commands to the catalog and delete the surviving
   `UserCommand` variants (item 7).
5. Remove the remaining `config::settings()` readers (item 9).
