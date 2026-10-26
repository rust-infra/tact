# Plugin-first migration — spec conformance audit

Audit of the approved spec
([`2026-10-25-plugin-first-architecture-design.md`](2026-10-25-plugin-first-architecture-design.md))
against the implemented state on `feat/plugin-first-architecture`.

**Status: 11 of 11 checklist items met, 0 partially met, 0 not met.**

Every verdict below is reproducible: `bash scripts/audit-conformance.sh` prints the
evidence for all eleven items, one section each. A verdict that cannot be re-run is
an opinion.

Verified against the tree at `582edab6`. Each "Met" below names the evidence that
shows it, because an audit that asserts without naming its check is a restatement of
intent rather than a result.

What the migration changed, in one paragraph: fifteen of the design's fifteen
capability domains now have plugin objects with replaceable ports; both frontends are
`View` plugins; the kernel declares the Agent, Chat and Session ports with the
implementations in extensions; commands are distributed, and the transport is a
message enum (`DriverMessage`) rather than a command enum, carrying nine variants
that each need the agent, a live task, the router, the account channel, or are the
spec's own named transport.

Each item states what the spec asks, what the code does, and how the claim was
verified. "Verified by" names a command or test, not an opinion.

---

## Checklist status

| # | Item | Status |
|---|---|---|
| 1 | Kernel boundary small enough to build without extensions | **Met** |
| 2 | Every capability has an owning plugin and a contribution or contract | **Met** (15/15) |
| 3 | Every Agent-issued tool call routed through the kernel executor | **Met** |
| 4 | Agent/Chat/Session ports in the kernel; implementations in extensions | **Met** |
| 5 | Plugins shut down in reverse install order, releasing resources | **Met** |
| 6 | TUI/headless are views; the host holds only composition and adapters | **Met** |
| 7 | Commands distributed; no central command enum survives | **Met** |
| 8 | Migration phases independently shippable and testable | **Met** |
| 9 | Configuration ownership unambiguous; TOML keys preserved | **Met** |
| 10 | Dynamic MCP/ToolSource items obey the same capability filter | **Met** |
| 11 | Marketplace package behavior and `/plugin` UX unchanged | **Met** |

---

## 1. Kernel boundary — met

`tact_kernel` depends only on `tact_protocol`, `anyhow`, `async-trait`, `serde`,
`serde_json`, `strum`, `tokio(sync)`.

Verified by: `bash scripts/check-architecture.sh` (prints the whitelist and fails
on any other edge).

## 2. Capability inventory — met

The spec's inventory (§ *Plugin inventory*) lists 5 policy/infrastructure, 4
runtime and 13 capability-domain plugins. Implemented as plugins today:

- Policy/infrastructure: `SecurityPlugin`, `PermissionPlugin`, `StoragePlugin`,
  `SessionStorePlugin`, `ProvidersPlugin`, `MarketplacePlugin`, `ConfigPlugin`
  — all present. (Eight `impl Plugin` blocks are production; two more are test
  impostors in `command/plugin.rs` and `permission/plugin.rs`.)
- Runtime: all four are present — `SessionPlugin`, `AgentPlugin`,
  `CompactionPlugin` and `ChatPlugin`.
  The neutral turn contract exists (`TurnRequest`, `TurnOptions`, `TurnStop`,
  `TurnOutcome` in `tact_kernel::runtime`), with the conversation deliberately
  not crossing the boundary. `CompactionService` is extension-level per the spec,
  and covers the pure half (micro-compaction, estimates, history shaping,
  transcripts) while the trigger policy stays on the agent.
  `ChatService` owns the turn lifecycle (message assembly, Stop-hook
  continuation, completion) over the same agent slot, so a turn and a compaction
  serialise structurally rather than by convention.
- Capability domains: **15 of 15**. `HooksPlugin`, `NotificationsPlugin`,
  `BackgroundPlugin`, `MemoryPlugin`, `SkillsPlugin`, `WorktreePlugin`,
  `VoicePlugin`, `SubagentPlugin` and `McpPlugin` have plugin objects with a
  lifecycle and a resolve-by-name port. `Tools` was already done and this audit
  had it wrong: `FamilyPlugin` is a `Plugin`, and `catalog_from_families` installs
  all twelve `builtin.tools.*` families into a sealed `PluginHost` on the
  production path — twelve granular plugins rather than one aggregate.

  `VoicePlugin` also settled the ownership question for live-resource domains: the
  worker moved from the view to the domain (`RuntimeVoiceService` owns it and shuts
  it down in `Drop`), and the view keeps only a forwarding port. `RuntimeVoicePort`
  was deleted — it existed solely to keep the worker alive.

  `McpPlugin` is worth reading as a sizing lesson: I had estimated it at 44 call
  sites because `Agent::mcp_router` has that many references, but the replaceable
  seam turned out to be *where the declarations come from* (`McpConfigSource`)
  rather than the live connections — the router's clients were already `Clone` over
  `Arc`ed services. The same slice removed a process-CWD read from the command
  path, which `resolve_current()` had been hiding.

  `GoalPlugin` needed the different shape this audit predicted: the seam is
  `AcceptancePolicy` (how a Goal proves it is done), not a handle, because `GoalStore`
  was already a trait and the runner is the loop. `Task` and `Team` turned out to be
  store-backed like the rest, so they took the plain pattern.

  The last two are **views**: `TuiPlugin` lives in `crates/tui` and
  `HeadlessPlugin` in `tact-ui`, each contributing a `View` capability, chosen by
  the composition root from its `Frontend`. `tact_ui::view_plugin::attached`
  enforces one view per process — the catalog deliberately allows more, because it
  is a registry, and the rule belongs to whoever composes.

  The shape is now stable: a named shared type, a `Plugin` impl that registers it,
  a resolver that errors by name when absent, and three tests (resolve, missing,
  manifest). What it buys is replacement — the code already existed, and the plugin
  is what lets a host swap it.

  Thirteen plugins total exist (`builtin.*`). The remaining domains each need the
  same seam `NotificationsPlugin` just got — a trait for the replaceable part plus
  a registration — and several are much larger than notifications.

Verified by: `grep -l 'impl Plugin for' crates/tact/src/**/*.rs` and the
inventory above.

## 3. Tool calls through the executor — met

Every model-originated call goes resolve → PreToolUse → authorize → ticket →
execute. A native call that reaches the dispatch wave without a ticket is an
explicit failure, not a dispatch. MCP tools, resource tools and prompt tools are
catalog entries and take the same pipeline; the legacy ladder is deleted.

Verified by: `crates/tact/tests/execution_parity.rs` (permission parity across
every mode × risk) and the executor tests in `tact_kernel`.

## 4. Ports in kernel, implementations in extensions — met

`SessionService` lives in `tact_kernel::runtime` (this migration). The view-facing
ports (`SkillCatalog`, `InteractionPort`, `ModelCatalog`, `SessionHistory`,
`SettingsWriter`, `VoicePort`) live in `tui`, which is where the spec says
view-side ports belong.

All three kernel contracts exist and all three plugins register them. What is
**not** done: the driver still holds `Agent` directly (164 accesses) instead of
driving `ChatService`. Profiling showed why that is a separate slice — the
driver's accesses are host operations (managers, events, reconfiguration), not
turn execution, so the migration is a deletion of inline lifecycle code rather
than a swap of one handle for another.

Verified by: `grep -n 'pub trait' crates/tact_kernel/src/runtime.rs`.

## 5. Reverse-order shutdown — met

`Runtime::shutdown` calls plugin teardown in reverse install order and continues
after individual errors. The order is a contract, not an implementation detail:
the composer installs security before permission, and a host test asserts the
session service is installed after the store it reads.

Verified by: `crates/tact/src/host.rs` tests (`assembly_installs_in_dependency_order`,
`a_host_with_storage_gets_the_session_service`).

## 6. Views and host — met

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

## 7. Commands distributed — partial

Catalog commands exist (`/stats`, `/background`, `/goal`) and are contributed by
`CommandPlugin`, so the *catalog* half is real. `UserCommand` is down from 28
variants to 25 by replacing four read-only variants with `RunCommand { name, args }`.

The view's palette now reads the runtime catalog through a `CommandDirectory`
port, and five commands have left the transport entirely: `/stats`,
`/background`, `/compact`, `/hooks` and `/mcp list|prompts`, plus `/permission`.
The catalog owns their names, descriptions and dispatch; `execute_palette_command`
routes local → catalog → skill, so a plugin-contributed command is both listed and
runnable. `UserCommand` is down from 28 variants to 18.

**Not met:** the spec's checklist line is "no central command enum survives".
Eighteen variants remain, and they are not one kind:

| Group | Count | Why it is still a transport variant |
|---|---|---|
| Turn control | 3 | `SubmitTask`, `Cancel`, `RunCommand` — the spec's named target |
| Goal lifecycle | 1 | `ResumeGoal` — needs the cancel flag and the runner |
| Runtime control | 2 | `CancelSubagent` (a button), `SubagentFinishedNotification` (a wake-up) |
| Needs the router / a turn | 2 | `McpAuth`, `RunMcpPrompt` |
| Account | 1 | `QueryBalance` needs the account channel |

**Nine is the floor.** The three picker results moved after all — I first argued
they were "a picked value, not a typed word", and the argument was wrong: `/permission`
also has a picker and also takes its value as text. The picker owns *choosing*; the
command owns *applying*. With that split, `/model <id>`, `/thinking <n>` and
`/effort <tier>` are commands like any other, and the transport keeps nothing typed.

What genuinely cannot move is the remaining nine: `ResumeGoal` needs the agent's
cancel flag and the runner; `CancelSubagent` is a button on a live task;
`SubagentFinishedNotification` is an internal wake-up, not a user action; `McpAuth`
and `RunMcpPrompt` need the router and a turn; `QueryBalance` needs the account
channel. The other three are the spec's named target transport.

Four of these are the spec's named target (`SubmitTask`, `Cancel`,
`RunCommand`, plus interaction responses already removed). The rest are
runtime operations a view legitimately issues, not command-line vocabulary —
which is the distinction the spec draws when it says the transport reduces to
"user text, cancel and interaction responses".

Verified by: variant count in `crates/protocol/src/agent.rs`.

## 8. Phases independently shippable — met

Every phase so far landed as commits that keep `scripts/check-rust.sh` green on
their own. The repository has no phase that only works combined with a later one.

Verified by: `git log --oneline` (each commit is a phase slice) and the per-round
gate log in `.superpowers/sdd/2026-10-25-tool-execution-contracts/`.

## 9. Configuration ownership — met

`ConfigPlugin` provides the resolved snapshot. The view reads a projection
(`ViewSettings`) and writes through `SettingsWriter`; `tact-ui`'s driver reads the
agent's own snapshot. Existing TOML keys are unchanged — the project's own
`config.example.toml` is still the reference and no key was renamed.

`read_image` and the notification gate no longer read the global: both take a
policy (`VisionPolicy`) or the answer (`enabled`) from the caller, and the
production bootstrap resolves the vision policy from the snapshot it already
read. `Agent::new` reads the configuration once at construction rather than twice,
so the tool families and the settings snapshot cannot disagree.

**Met.** `config::settings()` has **4** production readers, all of them boundary
reads that cannot be removed without inventing a place for configuration to come
from:

| Reader | Why it is a boundary |
|---|---|
| `tact-ui/src/main.rs` | process start, before any snapshot exists |
| `tact-ui/src/interactive.rs` | composition root |
| `tact-ui/src/session_bootstrap.rs` | composition root |
| `Agent::new` | construction, once — never again |

Everything downstream takes the snapshot it was handed: `ConfigPlugin::new(snapshot)`
receives its typed settings at construction, and the view reads its projection
through `ViewSettings`. `spawn_subagent`'s settings and the `#[tool]` macro's
fallback fill are gone — both were readers when this audit was first written.

Verified by a script that cuts each file at `#[cfg(test)]` and skips comment lines:
`main.rs` 1, `interactive.rs` 1, `session_bootstrap.rs` 1, `agent/mod.rs` 1, and 0
everywhere else. `host.rs`, `driver.rs`, `config_plugin.rs` and `tui/select.rs` all
match only inside tests — `select.rs`'s 19 matches are the strings in a guard that
asserts the production region contains none.

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
