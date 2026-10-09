# Commands as Capabilities — design proposal

**Status: PROPOSAL — needs your decision before any code.** Not an approved spec.

## The problem, measured

`UserCommand` (20 variants, `crates/tact_view/src/lib.rs`) is the TUI's command
vocabulary, and it carries `RuntimeCommand` **nested inside it** as
`UserCommand::Runtime(RuntimeCommand::…)`:

- 22 wrap sites, 15 unwrap/match sites, 179 references (`tui` 108, `tact_ui` 65,
  `tact_view` 4, `tact_extensions` 2).
- The production dispatcher is one large `match` in
  `crates/tact_ui/src/driver.rs`.

The architecture already says where these belong — spec §4 lists `Command` as a
plugin **contribution** kind, and Task 10 gives Chat ownership of conversational
commands — so the end state is these commands as **capability invocations**, not
enum variants. That is why this is a design decision and not a sweep: `RuntimeCommand`
(`StartRun`, `CancelRun`, `RespondInteraction`, `Subscribe`, `Resume`) cannot
carry `SetModel` or `HooksTrust`, and nothing today decides who owns them.

## Target

1. The TUI holds **no command vocabulary**. It builds its slash-command list and
   its completion from `CapabilityRouter::describe_all()` filtered to
   `CapabilityKind::Command` — which the spec's §4 contribution kinds already
   provide, and which is what currently has to be hard-coded in the TUI.
2. Invoking a command is `router.invoke("<name>", context, input)`, so every one
   of them passes the Capability Router and the Permission Engine like any other
   capability — today the `match` in `driver.rs` bypasses both.
3. `UserCommand` is deleted; `UserCommand::Runtime(…)` stops existing as a
   wrapper because the runtime commands travel as themselves.

## Mapping

| today | end state | owner |
|---|---|---|
| `SubmitTask(String)` | `RuntimeCommand::StartRun` — one path, not two | Chat |
| `Cancel` | `RuntimeCommand::CancelRun` | Chat |
| `Runtime(RuntimeCommand::…)` | the command itself, wrapper deleted | — |
| `Compact` | Command `chat.compact` | Chat |
| `QueryStats` | Command `session.stats` (ReadOnly) | Session |
| `QueryBalance` | Command `session.balance` (ReadOnly) | Session |
| `QueryBackground(Option<String>)` | Command `tools.background` (ReadOnly) | Tools |
| `CancelSubagent { child_id }` | Command `tools.subagent_cancel` | Tools |
| `SetModel` | Command `config.model` | Config |
| `SetThinkingBudget` | Command `config.thinking_budget` | Config |
| `SetReasoningEffort` | Command `config.reasoning_effort` | Config |
| `SetPermissionMode` | Command `config.permission_mode` | Config |
| `McpList` | Command `mcp.list` (ReadOnly) | MCP |
| `McpAuth { server }` | Command `mcp.auth` | MCP |
| `McpPrompts { server }` | Command `mcp.prompts` (ReadOnly) | MCP |
| `RunMcpPrompt(…)` | Command `mcp.run_prompt` | MCP |
| `HooksList` | Command `hooks.list` (ReadOnly) | Hooks / plugin ext |
| `HooksTrust { all, source }` | Command `hooks.trust` | Hooks |
| `HooksForget` | Command `hooks.forget` | Hooks |
| `SubagentFinishedNotification { … }` | **stays as it is** — see below | — |

Every capability name above is a proposal; the important part is the ownership
split, which follows §4/Task 10 rather than inventing a new one.

### The one row that must not move

`SubagentFinishedNotification` **stays**. Its handler (`driver.rs:168`) renders
nothing: it is a *wake-up* — "retain the wake-up until that turn's `JoinHandle`
completes, otherwise the notification could be lost in the gap between the final
queue drain and turn exit". Its place on the command queue is deliberate
ordering, not a misplaced event, and an earlier plan revision was wrong to
propose converting it. Only its name is misleading.

## The blocker this proposal first missed: capabilities cannot reach live state

Probing the smallest slice (`mcp.list`) showed the mapping table above is
optimistic. Two couplings it does not account for:

1. **The state lives in the Agent, by value.** `Agent::mcp_router` is
   `pub mcp_router: MCPToolRouter` — not shared — and `reload_mcp_router(&mut self)`
   replaces it. The driver's own comment for `/mcp` says it: *"Live view: describe
   what the agent's **current** router holds."* A capability that captured a clone
   at registration would go stale, and one that captured an `Arc<RwLock<…>>`
   would need the Agent to stop owning its router outright.
2. **The rendering lives in the binary crate.** `/mcp`'s table is produced by
   `tact_ui::mcp_cli::render_live_listing`, which `tact_extensions` cannot call.
   The same split will apply to `/background`, `/hooks` and `/stats`.

So a command is not "a capability that does what the match arm did" — it is
state access **plus** presentation, and neither half is in the extension today.

### Options for state access (pick one before implementing)

- **(a) Data in the invocation input.** The caller — which already holds the live
  state — passes it: `invoke("mcp.list", ctx, json!({"servers": [...]}))`, and the
  capability stays pure (describe + emit + ack). Cheapest, no new ownership
  question, but it makes the capability's input a projection of app state and
  only helps for read-only commands.
- **(b) Shared state service.** The Agent hands the extension an
  `Arc<RwLock<MCPToolRouter>>` (or a narrower read-only handle) and the capability
  reads it. Honest for long-lived state, but changes who owns the router and must
  be done per-subsystem (MCP, tasks, config, hooks).
- **(c) Keep those commands where they are.** Accept that *app* commands are the
  TUI's own vocabulary and only the `Runtime(RuntimeCommand)` nesting is
  redundant — i.e. unwrap the one row that is genuinely protocol and leave the 19
  app commands as an enum the TUI owns. This contradicts spec §2's "UserCommand is
  replaced by RuntimeCommand", but it is the reading under which the View keeps a
  private command vocabulary, exactly as it keeps a private rendering model.

**(a) is the recommendation for the read-only group**; (b) is what a full
migration needs; (c) is the honest fallback if the design cost is not worth the
benefit — and it is a legitimate end state, because a View owning its own command
vocabulary is not the same thing as a legacy *protocol* type.

## How a command's output reaches the user

Today `QueryStats` / `McpList` / `HooksList` render by pushing `PopupMarkdown`
or `MdInfo` onto the event stream and then returning. Proposal: **keep that**.
A command capability returns a small JSON ack (success + optional message) and,
when it has something to show, emits the same `RuntimeEvent`s it does now — so
"who renders a capability's result" is not a new question, and the popup
behaviour is unchanged.

## Risks / open decisions for you

1. **Permission cost.** Routing config mutations (`SetModel`, `SetPermissionMode`)
   through the Permission Engine means a slash command may start prompting. Is
   that wanted? Proposal: `ReadOnly` for the query commands, `Medium` for
   mutations, and the existing Auto/Ask ladder decides — a user in an interactive
   TUI normally has Auto for these, so no prompt appears in practice.
2. **Discovery order.** The TUI's command list would depend on register order
   (and on `PluginRegistry` dependencies, which already exist). Proposal: sort by
   capability name, exactly as `CapabilityRouter::describe_all` returns them.
3. **Scope of the first landing.** The whole table is a large change. The
   smallest useful first slice is the read-only queries
   (`session.stats`, `session.balance`, `tools.background`, `mcp.list`,
   `hooks.list`) — they change no state and their rendering is already
   event-driven.
4. **`UserCommand` deletion.** Once every row above is migrated the enum is empty
   apart from the wake-up; at that point the wake-up becomes its own type or a
   `RuntimeCommand` variant, and `UserCommand` goes. Confirm that is the intent.

## Order (if approved)

1. Add the `Command` capabilities + registrations, without touching the TUI.
2. Switch the TUI's `match` arms to `router.invoke` one group at a time
   (queries → tools → config → mcp → hooks).
3. Delete the migrated `UserCommand` variants; then the enum.
4. Build the slash-command list and completion from `describe_all`.
5. Verify: `./scripts/check-rust.sh` plus the TUI command-palette and completion
   tests; a human should exercise `/stats`, `/model` and `/hooks` once.
