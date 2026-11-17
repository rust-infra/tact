# tact-acp

ACP (Agent Client Protocol) v2 frontend for Tact. It turns a Tact session into
an ACP agent over stdio so an ACP client can drive it with its own chat UI.

T3 Code is the first consumer: its **Settings → Providers → Add provider → ACP**
flow can launch any local command that speaks ACP v2.

## Build

```bash
cargo build -p tact-acp --release
```

The resulting binary is `target/release/tact-acp`. It reads the same
`~/.tact/config.toml` (or project `.tact/config.toml`) as `tact-ui`, so model,
provider, permission, MCP, skill, and hook configuration are shared.

## T3 Code setup

1. Open **Settings → Providers → Add provider**.
2. Choose **ACP** (the driver is labelled `ACP Registry`).
3. Set **ACP source** to `Local command`.
4. Set **Executable** to the absolute path of `tact-acp`.
5. Add the provider, then create a thread and select it.

The form only exposes an executable path. T3 stores `commandArgs` in its
provider settings and passes them through, but the form does not show them.
To pass an explicit config file, set `commandArgs` to
`["--config", "/home/you/.tact/t3-config.toml"]` in those settings, or use a
small wrapper script. Without `--config`, the normal config search paths apply.

**Settings → Providers → <instance> → Native sessions** lists the Tact
sessions of that workspace. Import resumes one as a T3 thread and Delete
removes it. A session's lock is what keeps two hosts from writing it at once,
so importing a session a live host still holds (a running T3 thread, `tact-ui`,
`--session`) fails `session/resume`; T3 then falls back to a fresh
`session/new`, and the imported thread starts with no history.

## Protocol scope

- `initialize` negotiates ACP protocol v2, advertises the baseline session
  capability, and records whether the client supports form elicitation.
  Advertising `capabilities.session` at all is what turns on `session/list`,
  `session/resume`, and `session/close`: v2 has no separate flags for them,
  and T3 expands the non-null object into `list`/`resume`/`close` itself. In
  T3 that object is also `canResumeAfterRestart`, so dropping it would stop a
  thread from resuming its Tact session after the provider restarts.
- `session/new`, `session/list`, `session/resume`, `session/close`,
  `session/delete`, `session/prompt`, `session/cancel`, and `session/update`.
  T3 surfaces `session/list` as Settings → Providers → Native sessions, with
  Import (`session/resume`) and Delete (`session/delete`) per row.
- Three session config options (`type: "select"`) are returned by `session/new`
  and `session/resume`, and updated through `session/set_config_option`:
  - `model` (`category: "model"`) — values from the configured
    `[llm.providers.<active>].models` list.
  - `permission_mode` (`category: "mode"`) — `default`, `plan`, `auto`. T3
    treats a mode-category select as its plan-mode toggle, and on ACP v2 this
    is the only way that toggle can be set.
  - `thought_level` (`category: "thought_level"`) — reasoning-effort tiers for
    effort models, thinking-budget tiers for budget models. Which one applies is
    `tact_llm::model_uses_effort`, the same rule the TUI's model picker uses, so
    the two frontends cannot disagree about a model's tiers.
  The response is the source of truth; a `config_option_update` follows once the
  runtime confirms the change.
- `session/prompt` returns as soon as the prompt is inserted; turn completion
  is reported by an idle `state_update` with a stop reason. A prompt accepted
  during a running turn is forwarded to Tact's queue without a second idle.
- Text, `resource_link`, and image prompt blocks. An image is written into the
  workspace (`.tact-acp-images/`) and referenced by relative path, which is how
  Tact already treats an attached image: the model reads the pixels with
  `read_image` only when it needs them.
- Agent text, thinking, tool calls, tool output, notices, and idle/running/
  requires-action state.
- `available_commands` is both returned by `session/new` / `session/resume` and
  pushed as an `available_commands_update` notification. The notification is
  what a client's `/` menu listens for; T3 reads the setup response only when
  probing a provider. The list holds the commands this adapter acts on
  (`compact-context`, `stats`, `mcp`, `background`, `goal`, `model-select`,
  `permission-mode`, `thought`) plus every enabled skill as a `$name` entry. A client sends the chosen name
  back as `/name args` or `$name args`; the adapter routes those as session
  actions, and an unrecognized name goes to the model as ordinary text. A
  command is one line: text after it in the same prompt runs the command and is
  then submitted as an ordinary turn. A command turn settles with its own idle,
  because a client waits for one after every prompt — including one the adapter
  answers on its own (a `/model` it applies locally, an unknown `$skill`): a
  notice or a config update is not a turn boundary, so the adapter emits the
  idle itself. It stays silent when a turn is already running, since that
  turn's own idle is the boundary.
- T3 reserves the exact `/compact` prompt for its own compaction driver and
  rejects it before the provider starts. The adapter therefore advertises the
  same operation as `/compact-context`; a `/compact` that arrives from another
  ACP client is still routed to Tact. The command turn stays open until the
  compaction reaches a terminal state (`completed`, `failed`, or `cancelled`),
  so a client does not finalize the turn while the summary is still being
  generated — see [Compaction](#compaction).
- T3 also owns the bare `/model` and `/permission` composer commands. The
  adapter therefore advertises `/model-select` and `/permission-mode` (while
  still accepting the bare spellings from other clients); the T3 model picker
  and `permission_mode` config option remain the primary UI.
- A `/goal` command is answered through the same host commands the TUI uses:
  `create` (or a bare objective), `list`, `show`, `steer`, `pause`, `resume`,
  `cancel`, `accept`, `archive`, and `unarchive`. ACP has no Goal panel, so a
  Goal snapshot change becomes a deduplicated `notice` (`Goals`) with status,
  task counts, and wait reason; `/goal list` answers from the snapshot even
  when a periodic reconcile arrives first. Creating or resuming a Goal owns the
  turn until the runner's `TaskIdle`.
- T3 prepends `<t3_code_instructions>…<user_request>…</user_request>` on a
  thread's first turn and whenever the interaction mode changes, and it skips
  that wrapper only for prompts starting with `/`. So a `$skill` first turn
  arrives wrapped: the adapter reads the `<user_request>` body to route it, and
  forwards the whole wrapper unchanged when the body is ordinary text. Nothing
  about this is required by ACP; a bare client works identically.
- Live state: `usage_update` from the reported token total and the configured
  context window, and `plan_update` from Tact's persistent task list. A client's
  context meter and todo panel therefore track the same numbers the TUI shows.
  When the task list empties, the plan is *removed* (`plan_removed`) rather
  than replaced by an empty list: an update is a wholesale replacement, so
  without the removal the panel would keep drawing the last checklist as still
  in progress. A list that is empty before anything was published stays silent,
  since there is no plan to remove.
- Client-provided MCP servers (`stdio` and `http`) are connected for the session.
  They are merged into the workspace's `.mcp.json` resolution as if declared
  last, and never written to a config file.
- `session/delete` deletes the stored session, closing a live host first.
- Tact's single-select permission prompt maps to
  `session/request_permission`; `Allow once` / `Always allow …` / `Deny` map to
  ACP `allow_once` / `allow_always` / `reject_once`.
- `requires_action` is announced only when a permission or elicitation is
  still pending after a short grace period. Clients that auto-approve answer
  immediately, so their timeline does not keep a stale "Action required" row.
- Tool completions repeat the call's name, title, and kind, and retain text the
  call streamed before finishing. Generic tools also send their text as
  `rawOutput`, which T3 renders directly instead of showing the ACP content
  envelope as JSON.
- File tools report `ToolCallLocation`s, and `edit_file` / `write_file` attach a
  `ToolCallContent::Diff` with renderable git-patch text.
- Background tasks are reported as ACP terminals (`terminal_update` plus
  `BackgroundWork`) and appear in every `state_update.background` snapshot;
  detached subagents are listed there too.
- The first prompt publishes a `session_info_update` title, and later turns
  refresh its `updatedAt`. Usage updates carry the context window; cost is
  omitted because Tact has no per-session cost source.
- Compaction is sent as `compaction_update` for its whole lifecycle
  (`in_progress` → `completed` / `failed` / `cancelled`), driven by the typed
  `AgentUpdate::Compaction` the kernel emits, not by matching notice text. The
  completed update carries Tact's summary; the existing `Info` log lines stay
  for the TUI and the desktop. See [Compaction](#compaction).
- Goals are a notice whose `description` is the plain-text read-out and whose
  `_meta` carries the same `Vec<GoalSnapshot>` as JSON under `tact/goals`. ACP
  has no Goal update, and a `PlanEntry` (three statuses, one line of text)
  cannot hold a Goal (ten statuses, objective, criteria, acceptance, budget).
  The text stands alone for clients that ignore `_meta`; a client that reads
  the key can draw a native panel without a second protocol surface. See
  [Goal notices](#goal-notices).
- `session/fork` copies the durable transcript into a new session and resumes
  it. `session/resume` with `replayFrom: start` reconstructs user/assistant
  messages, thoughts, and tool calls/results before `replay_complete`.
- Embedded text resources are accepted in prompts. Audio and binary embedded
  resources remain unsupported.
- `spawn_subagent` is announced as a standard ACP v2 child session through
  `subagent_update`, and the parent tool call carries a `Session` content
  reference. Child transcript entries are routed under the child session ID,
  and the final summary is a `session_message` back to the parent. The ordinary
  tool card is the fallback for clients that do not render child sessions, and
  it is a complete record of the same call.
- T3 is such a client, so it is *not* sent any of this: naming itself
  `t3-code` in `initialize.info` switches child sessions off for that
  connection, and its subagents show the plain tool card. ACP has no capability
  for child sessions and T3 sends no `_meta` that could carry one, so the
  implementation name is the only signal. Withholding is deliberate rather than
  merely conservative: T3 normalizes a fixed set of `sessionUpdate` kinds and
  renders every other one as an `Unsupported ACP update: …` tool card in the
  transcript, and `subagent_update` is not in that set for a non-Claude driver.
  Delete the gate in `client_renders_child_sessions` when that changes.
- Single- and multi-select `ask_user` questions map to ACP form elicitation
  (`elicitation/create`) when the client advertised
  `capabilities.elicitation.form`. Option values are the Tact option indices,
  so repeated labels stay distinguishable. Decline, cancel, errors, and
  malformed content all answer Tact as a declined request.
- `session/cancel` answers every pending Tact UI request first (through the
  host's `UiResponder`), withdraws the matching ACP permission and elicitation
  dialogs with `$/cancel_request`, and then asks Tact to cancel. The turn
  settles from Tact's own cancellation event; the adapter does not force a fake
  idle.
- `session/resume` accepts `replayFrom: { "type": "start" }` and rejects other
  cursors with `-32602`. It sends `replay_complete` and an idle before the
  response. A live session's model options are read back from its actor.

### Compaction

Tact's kernel emits one typed `AgentUpdate::Compaction` per lifecycle step
(`started`, then exactly one of `completed`, `failed`, `cancelled`), and the
adapter maps it onto ACP's `compaction_update`. The event is data, not prose:
attaching to notice text could not close the card when a `/compact` failed
(the failure arrives as a generic `Error`) and misread compaction whose log
lines are worded differently.

- One compaction is one `compaction_id`. ACP patches an entity in place by id
  and the first update fixes its timeline position, so minting a new id per
  status would draw a second card. `CompactionCard` in `lib.rs` is the whole
  state machine, and it is unit-tested without a connection.
- `completed` carries Tact's summary as `summary: [text]`, so a client can show
  what replaced the history it never sees. Native provider compaction replaces
  history provider-side and has no local text; there `summary` is omitted
  rather than faked.
- `failed` carries the error text. `cancelled` covers a `PreCompact` hook veto
  and a user cancellation, and sets `error` to the reason — ACP has no
  separate field, and a cancelled compaction is not a failure.
- The existing `Info` lines (`[auto compact]`, `Compaction complete (Auto).`)
  still go out for the TUI and the desktop. Two views of one fact on purpose:
  dropping the text would blank a log row users already read.
- A `/compact` command turn settles on the terminal typed event, so a vetoed or
  failed compaction no longer waits out the command settle timer.

### Goal notices

ACP v2 has no Goal update, so `AgentUpdate::GoalsChanged` becomes one `notice`.
The notice carries the snapshot twice:

- `title` is `"Goals"` and `description` is one line per Goal
  (`▸ Title — waiting for permission · tasks 1/3 · waiting: <reason>`). This is
  the read-out every client renders, and it is what `/goal list` answers with.
- `_meta["tact/goals"]` is the same `Vec<GoalSnapshot>` serialized as JSON.
  It carries what the text cannot: `objective`, `criteria`, `acceptance`,
  `run_budget`, `phase`, and `archived`.

An empty list still sets the key to `[]`, so a client that drew a panel from an
earlier snapshot can clear it. Notices are not session history and a client may
ignore them entirely — do not treat `_meta` as durable storage; `session/resume`
replays real transcript entries, not notices.

Goals are deliberately **not** published as a second `planId`. `PlanEntry` has
`content`, `priority`, and three statuses; a Goal has ten statuses and five
nested fields. Collapsing one into the other would show `Verifying`,
`WaitingPermission`, and `Failed` as the same "in progress" row, and the `tasks`
plan already owns the plan surface.

## Not implemented yet

- `compaction_summary_chunk` streaming. The summary arrives once, on the
  `completed` update; ACP's streaming chunk is not emitted. Tact's compaction
  produces the summary in one model response (nothing to chunk) and the
  facade's own `[compact summary n/m]` progress lines are `Info` text for the
  TUI, not content the adapter may forward as summary chunks.
- A native Goal surface. Goals are notices; the structured snapshot is in
  `_meta` rather than a protocol type of its own. T3's own Goal panel belongs to
  its driver, and its notice decoder keeps `_meta` but its `NoticeReceived`
  renderer drops it, so a panel from `tact/goals` needs client work this
  adapter cannot do from its side.
- URL-based elicitation.
- Audio and binary embedded-resource prompts.
- Prompt suggestions and usage cost, which have no Tact data source.

## Design notes

- Each ACP session owns one `tact_host::HostSession` and one actor. The actor is
  the single writer of per-session state (model selection and turn phase); the
  manager stores only an immutable handle plus a command sender.
- Model changes are applied to Tact only at an idle boundary. `SetModel` lands
  in the driver's catch-all behind an in-flight turn, where it would also park
  the UI answers and cancellation a blocked turn needs.
- `HostSession::pending_requests()` is the authoritative pending-request
  snapshot. `AgentUpdate` request hints are reconciled against it, so a lost
  hint cannot leave a question unanswerable. `UiResponder` remains the only
  registry that answers Tact.
- A model switch is process-global inside Tact (`UserCommand::SetModel` rewrites
  the shared config). Sibling ACP sessions are not model-isolated; each
  reconciles its own view from `ModelInfo`. The permission mode is per-session
  and has no runtime report, so the actor settles it as soon as the host accepts
  the command.
- MCP configuration is resolved per session against the session's `cwd`, not
  the process directory, so two sessions in different workspaces see their own
  `.mcp.json`. A frontend may add servers for one session through
  `HostSessionConfig::extra_mcp_servers`; they win a name collision, like a
  later declaration always does.
- `cwd` from `session/new` / `session/resume` selects the Tact workspace and
  its `.tact/tact.db`; do not rely on the process launch directory.
- stdout is reserved for JSON-RPC. Logs go to stderr and honour `RUST_LOG`.
- `session/close` closes the host and releases its session lock; stdin EOF
  closes every live host.
