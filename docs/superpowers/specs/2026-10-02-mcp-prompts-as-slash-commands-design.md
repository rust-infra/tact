# Design: MCP prompts as slash commands

Date: 2026-10-02
Status: approved for implementation

Picks up non-goal #1 of [`2026-10-02-mcp-prompts-design.md`](./2026-10-02-mcp-prompts-design.md) §3, which
deferred this on purpose: *"this change answers 'the model cannot reach them at all' without committing
to a second user-facing surface (completion, i18n, persistence) in the same diff."* The model can reach
them now; the user still cannot.

## 1. Problem

Prompts are the one MCP primitive the spec calls **user**-controlled — the server writes them so a
person can invoke them. Tact wired them for the model only, which has the roles backwards: a user
looking at Basic Memory's `getting_started` prompt has no way to run it. The model can call
`list_mcp_prompts` and `get_mcp_prompt`; the person cannot.

## 2. Design

### 2.1 Under `/mcp`, not first-level

Two leaves join the existing `/mcp` family:

| Input | Behaviour |
|---|---|
| `/mcp prompts [server]` | List the connected servers' prompts — names, descriptions, and each prompt's argument vocabulary. |
| `/mcp prompt <server> <name> [key=value …]` | Fetch one prompt and run it. |

Not first-level commands, for the reason skills are not: `/skill` exists because a wall of
server-supplied names buries the built-ins, and a server's prompts are the same kind of name. The
slash popup completes the two leaves like any other `/mcp` subcommand.

### 2.2 No popup completion for prompt names — and why

The obvious follow-up is completing `/mcp prompt basic-memory <TAB>` against the server's actual
prompt list. That needs the catalogue inside the TUI, and the TUI does not have it: MCP is connected by
the **agent**, which is built *after* the TUI is spawned — the order that makes quitting during startup
instant (`7744a2c3`). Delivering a catalogue into the TUI means a new channel, App state, and a refresh
path for a list that is already reachable two other ways (`list_mcp_prompts` for the model,
`/mcp prompts` for the user).

This increment needs no channel at all: the user names the prompt and the **driver** resolves it
against the live router. Recorded as a non-goal rather than a plan, so the cost of adding completion
later is a decision rather than a discovery.

### 2.3 The listing is the driver's, like `/mcp list`

`UserCommand::McpPrompts { server }` is answered by the driver from the agent's already-connected
router and rendered as markdown into the log — the same pattern, and the same reason, as
`UserCommand::McpList`: only the driver can see the live router, and it must never reconnect to print
a table. The rendering reuses `render_prompt_listing`, so the user and the model read the same text.

### 2.4 Running one is a submit, not a new turn type

`UserCommand::RunMcpPrompt { server, name, arguments }`: the driver fetches with the router, renders
the messages with `render_prompt_messages` — the same function the tool result uses — and submits the
result as an ordinary user turn. A prompt is a starting message, so it becomes one; nothing downstream
learns a new turn shape, and the message the model sees is byte-identical to what `get_mcp_prompt`
would have returned. Failures (unknown server, unknown prompt, a server error) surface as
`AgentUpdate::Error`.

### 2.5 Arguments: `key=value`, refused by name when malformed

Tokens after `<name>` are `key=value`, split on the **first** `=`, so a value may contain `=`. A token
with no `=` is refused in the TUI before any round trip, naming the token. Values cannot contain
spaces — the input box has no quoting, and inventing one would be a language. The documented way to
pass a multi-word value is the model's `get_mcp_prompt`, whose `arguments` is a JSON object.

### 2.6 Arity is the driver's call, not the TUI's

The TUI forwards whatever it parsed; the driver names the missing `server` or `name`. This is not
laziness: `every_declared_subcommand_has_a_handler` dispatches `/{cmd} {path} sample` for every
`takes_value` leaf — one token, not one per value — so a TUI that demanded both tokens would turn a
completable command into the usage hint the invariant exists to catch. The check lives in exactly one
place, next to the router that can also say "no such prompt".

## 3. Non-goals

- **Popup completion for prompt names** (§2.2): needs the catalogue in the TUI, hence a delivery
  channel. Not this diff.
- **Quoted / multi-word argument values** (§2.5).
- **Prompts as first-level `/`-commands** (§2.1).
- **`prompts/list_changed`** — unchanged: prompts are fetched on demand, and every server we can check
  declares `listChanged: false`.

## 4. Tests

- `/mcp prompts` and `/mcp prompts <server>` send the listing command; both are idle-gated like
  `/mcp list`.
- `/mcp prompt <server> <name>` sends the run command; `key=value` tokens become the argument map; a
  token without `=` is refused in the TUI with the token named and no command sent.
- The two new leaves are declared in `MCP_SUBCOMMANDS`, so the popup completes them and
  `every_declared_subcommand_has_a_handler` exercises them.
- The driver: `RunMcpPrompt` renders through the same path as the tool and submits a user turn; a
  missing `name` and an unknown prompt each produce an error, not a silent no-op.

## 5. Docs sync

- `book/08_chapter_mcp_zh.md`: Step 9 gains the two commands; the §10 gap row's prompts clause is
  updated to name what is still missing (completion).
- `book/23_chapter_tui_zh.md` §7: the `/mcp` subcommand list.
- `book/26_chapter_issue_zh.md`: newest-first entry.
