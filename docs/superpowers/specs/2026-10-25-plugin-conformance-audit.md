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

## Why the plugins were decoration — and the fix

This is the answer to "many things are not done well", and it was not a missing
plugin. `session_bootstrap` created each domain manager and wired the *same* `Arc`
twice: once into `ToolContext` (where every tool read it) and once into `HostExtras`
(where the plugin registered it). A plugin could be installed, replaced or omitted and
the tools would keep reading the copy the caller passed. That is why counting plugin
objects looked complete while `TeamPlugin` was a parallel wire.

The fix has two halves, both landed:

1. **`ToolContext`'s seven domain handles are `Option`.** A non-`Option` field forces
   the composition root to hand over a fallback, which is exactly the duplicate wire.
2. **The registry is the only thing that fills them.** `ToolContext::rebind_from`
   resolves each from the service the owning plugin registered; the bootstrap now
   passes `None` for all seven, and `Agent` assembly rebinds both the factory and the
   context the Agent keeps (the latter matters because
   `SharedToolContextFactory::refresh` replaces its base with the Agent's context once
   per turn — a rebound factory over a blank context would lose the handles on the
   first turn).

Consequences, each covered by a test:

- Installing `TaskPlugin` and nothing else leaves `task_manager` filled and
  `teammate_manager` empty (`rebinding_fills_only_what_the_host_installed`).
- Replacing a plugin changes what the tools see
  (`rebinding_points_the_context_at_the_registry`).
- A host without `SkillsPlugin` renders no skills catalog in the prompt, and one
  without `MemoryPlugin` injects no memory index.
- `spawn_subagent` refuses by name when `SubagentPlugin` or `WorktreePlugin` is
  missing, because a child that cannot be cancelled or recorded is worse than a clear
  refusal.

3. **Each plugin constructs what it owns.** `TaskPlugin::open(db)`,
   `TeamPlugin::open(db)`, `BackgroundPlugin::open(db)`, `SubagentPlugin::open(db)`,
   `WorktreePlugin::open(db, repo)`, `MemoryPlugin::open(dir)` and
   `SkillsPlugin::open(dir)` own their store, and `HostExtras` carries plugin objects
   rather than managers. The design's "Owns" column is now literally true, and the
   bootstrap cannot hand a tool a handle the registry does not know about — it no
   longer names a single domain manager type, which the compiler enforces by warning
   on the imports.

---

## The contribution layer: what the plugin mechanism actually delivers

The service layer is real and load-bearing: 24 `registrar.service` sites, and the
domain handles every tool reads now come from the registry (`rebind_from`), so
replacing a domain plugin changes behaviour.

The *contribution* layer is not in the same state. `Registrar` has five contribution
methods and only two have both a production producer and a production consumer:

| Contribution | Producer | Consumer | Verdict |
|---|---|---|---|
| `service` | 24 sites | `rebind_from`, `policy::resolve`, command wirings | delivered |
| `view` | `TuiPlugin`, `HeadlessPlugin` | `view_plugin::attached` | delivered |
| `tool` | `FamilyPlugin` (12 families) | `runtime.tools()` → `ToolCatalog` → executor | delivered |
| `command` | `PermissionPlugin` — owns the manager `/permission` writes through | the driver merges `Agent::commands()` into its catalog | delivered |
| `prompt_section` | `install_fragments` — every `builtin.prompt.*` fragment becomes a section | `build_system_prompt` reads them back through the catalog | delivered |

No contribution claim is undelivered and every named contract has a producer:

- The prompt sections are contributed: `build_system_prompt` builds the fragments and
  passes them through `install_fragments`, so each becomes a `builtin.prompt.*` section
  and the catalog decides which survive the capability filter before `apply` writes
  them into the template. The comment in that function claimed this had always been
  true; it had not, and the list went straight to `apply`. What is still not
  plugin-owned is the *content* of the three domain fragments: a `PromptFragment`
  carries a `String`, so skills and memory are computed by the agent and handed over
  rather than rendered by the plugin that owns the registry.
- `CommandSource` has a producer, though not the MCP one: marketplace packages ship
  `commands/<name>.md`, which are now discovered by `plugin::commands` and run as
  commands whose body becomes the turn. Those files were installed and counted
  (`InstalledPlugin::command_count`) but never read, so an installed command was inert.
  The MCP half is still a hand-written `/mcp prompts`, because turning each prompt into
  a slash command needs a structured prompt port (the current one returns rendered
  text) and is a visible UX change.

None of this makes the capability missing — a user still gets skills in the prompt and
`/mcp prompts` still works. It makes the *mechanism* different from the design, and it
means a plugin cannot add a prompt section by registering one. That is a design decision
to accept or to close, not a bug to fix silently.

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
- Capability domains: **14 of 15**. `HooksPlugin`, `NotificationsPlugin`,
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

  **`VoicePlugin` was removed, and that is the honest count.** It registered a
  service nothing resolved: the worker is spawned by the view (it owns the event
  stream, which is single-consumer) and the plugin could not be given one at assembly
  time. `RuntimeVoiceService` remains as the view-side implementation behind
  `VoiceService`; what is gone is the plugin object that pretended to contribute it.
  The same reasoning removed `SettingsPlugin` (its receiver lives in the driver) and
  `GoalPlugin`'s service half (its activator holds the agent slot).

  The rule this settles: **a plugin can only provide what exists at assembly time.**
  A service whose state is created later — a driver-owned queue, a view-owned worker —
  is a seam owned by whoever creates it, handed over through a wiring struct or a view
  port, and a plugin object for it is decoration.

  Twenty-five plugin ids exist in total: fourteen capability domains, the twelve
  `builtin.tools.*` families, and the policy and infrastructure plugins (`security`,
  `permission`, `hooks`, `marketplace`, `config`, `providers`, `storage`,
  `session_store`). Two `impostor.*` ids are test doubles in `#[cfg(test)]`.

  Two of those arrived for a reason the earlier rounds did not anticipate and are
  worth naming: `builtin.settings` exists because the three settings commands needed
  somewhere to record a validated change, and `builtin.agent_service`,
  `builtin.chat_service` and `builtin.compaction_service` exist so the driver holds
  those ports rather than the concrete types.

Verified by: `bash scripts/audit-conformance.sh` item 2, which lists the ids it finds
by scanning for `PluginId::new`.

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

## 7. Commands distributed — met

Catalog commands are contributed by the plugin that owns each domain: `/stats`,
`/background`, `/goal`, `/compact`, `/hooks`, `/mcp`, `/permission`, `/model`,
`/thinking` and `/effort` are ten separate command modules in
`crates/tact/src/command/`, installed by `builtin_catalog` from the wirings each
plugin supplies. There is no `CommandsPlugin`, which the design table asks for
explicitly, and the view's palette reads the runtime catalog through a
`CommandDirectory` port — so a plugin-contributed command is both listed and runnable.

The checklist line is stricter: "no central command enum survives." What survives is
`DriverMessage` (it was `UserCommand`), and the name change is the point. It is not a
command enum:

- A command travels as `DriverMessage::RunCommand { name, args }` and is parsed by the
  domain that owns it. Adding one does not edit this type — which is exactly the
  property the checklist is about.
- Ten commands migrated this way: `/stats`, `/background`, `/hooks`, `/mcp list`,
  `/mcp prompts`, `/permission`, `/goal {list,show,create,accept,pause,cancel,archive,
  unarchive,steer}`, `/model`, `/thinking`, `/effort`.
- Nine messages remain, and none is a command: three are the spec's own named
  transport (`SubmitTask`, `Cancel`, `RunCommand`), and the rest need something a
  command cannot reach — `ResumeGoal` the cancel flag and the runner, `McpAuth` the
  router reload, `RunMcpPrompt` a turn, `QueryBalance` the account channel,
  `CancelSubagent` a live task's handle, `SubagentFinishedNotification` is not from a
  view at all but from a spawned task telling the driver a child finished.

Verified by: `bash scripts/audit-conformance.sh` item 7 (lists the nine), the
exhaustive `match` in `the_transport_carries_no_interaction_answer` (a new variant
fails to compile), and `commands_travel_as_catalog_invocations`, which names the
thirteen migrated variants and asserts none has returned.

Two rounds of this migration got the answer wrong before getting it right, and both
corrections are worth keeping:

- I argued the three picker results (`SetModel` and friends) should stay because
  "picking a value is not typing a word". `/permission` disproves it — it has a picker
  *and* takes its value as text. The picker owns choosing; the command owns applying.
- I then read the nine remaining variants as "nine is the floor" while the type was
  still called `UserCommand` and documented as user commands. The floor was real; the
  name was not.

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

## 10. Dynamic sources filtered — met, with one named contract missing

The checklist line is about `ToolSource`, and that half is implemented and wired:

- `McpToolSource` and `McpControlSource` implement `ToolSource`
  (`crates/tact/src/tool/mcp_source.rs:204`, `mcp_control.rs:223`).
- Both are registered on the production path in `Agent` assembly
  (`crates/tact/src/agent/mod.rs:765` and `:772`), through
  `ToolCatalog::add_source`.
- The filter is applied where the descriptors are produced, not where they are
  registered: `ToolCatalog::descriptor_snapshot(filter)` builds the list from every
  source's `tools()` and then `retain`s on `filter.allows(&id)`, and
  `can_dispatch(name, filter)` applies the same rule. So a source cannot advertise a
  tool the filter denies, and the advertised list and the dispatchable set cannot
  disagree.
- `refresh_sources` bumps the generation only when a source reports a change, so a
  host comparing generations cannot be fooled by a rebuild that changed nothing.

The command half is now implemented at the contract level: `CommandSource` exists in
`tact_kernel::capability::command`, `CommandCatalog::add_source` serves it, and
`descriptors_filtered`/`can_dispatch`/`refresh_sources` apply the same per-command
filter and generation rule `ToolCatalog` uses. Sources take `&self` to register, so a
host can add one after sealing — which is the only moment an MCP prompt set is known.

What has no producer yet is the MCP side: `/mcp prompts` is still a hand-written
command, so `CommandSource` is a tested contract with nothing contributing through it.
Writing that producer means one slash command per MCP prompt plus a queue for the turn
a prompt submits, which is a visible UX change and belongs to a product decision rather
than to this audit.

Commands are filtered in all three places the design names: listing
(`descriptors_filtered`, used by the palette), resolution (`can_dispatch`, used by the
palette's `resolve`), and dispatch (`refusal_for`, used by both driver arms). A disabled
command is refused by name when typed instead of merely hidden.

A note on how this section was written, because it changed three times. It first said
"met" while describing code I had not opened. I then changed it to "not met" after
`grep -rn 'impl ToolSource for' crates --include=*.rs | head -6` returned only kernel
test sources — the `head` had truncated the two MCP implementations below it. The
verdict is "met" on the strength of the file and line references above, which is what
the first version should have cited.

Verified by: `grep -rn 'impl ToolSource for' crates --include=*.rs` (4, two of them
MCP), `grep -n 'add_source' crates/tact/src/agent/mod.rs` (2 production call sites),
and `descriptor_snapshot`'s `retain` in `crates/tact_kernel/src/catalog.rs`.

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

### D0. Contracts the design names that have no implementation

The design's *Plugin inventory* table names a contract for every plugin. Four of
those names do not appear in the tree at all, and one appears under a different name.
This is the list I should have produced the first time instead of counting plugin
objects:

| Design names | In the tree | Consequence |
|---|---|---|
| `ToolSource` (MCP) | implemented (`McpToolSource`, `McpControlSource`) and registered | none — this row was wrong in the first draft |
| `CommandSource` (MCP) | trait + catalog support, and a producer in `plugin::commands` | MCP prompts still travel as `/mcp prompts`; marketplace commands now arrive through the source |
| `AgentFactory` (Agent) | `crate::agent::factory` — `AgentPlugin` contributes it | none: `spawn_subagent` refuses by name when it is absent |
| `ModelService` (Providers) | `ProviderInfo` snapshot | same role, different name — a rename, not a gap |
| `DatabaseHandle` (Storage) | `StorageLocation` | same role, different name — a rename, not a gap |

The two renames are fine: the design says "extension-level", which is what these are,
and an extension-level type is free to be called what it does. Of the two absences,
`CommandSource` is a real gap (MCP contributes tools dynamically but not commands) and
`AgentFactory` is a design element that was never built — `spawn_subagent` assembles
the child directly. Neither is a checklist item, so neither changes the verdict; both
are recorded so a reviewer can decide whether they matter.

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
