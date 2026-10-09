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
