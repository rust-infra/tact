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

## Protocol scope

- `initialize` negotiates ACP protocol v2, advertises the baseline session
  capability, and records whether the client supports form elicitation.
- `session/new`, `session/list`, `session/resume`, `session/close`,
  `session/prompt`, `session/cancel`, and `session/update`.
- A `model` session config option (`type: "select"`, `category: "model"`) is
  returned by `session/new` and `session/resume`, and updated through
  `session/set_config_option`. The option values come from the configured
  `[llm.providers.<active>].models` list. The response is the source of truth;
  a `config_option_update` follows once the runtime confirms the change.
- `session/prompt` returns as soon as the prompt is inserted; turn completion
  is reported by an idle `state_update` with a stop reason. A prompt accepted
  during a running turn is forwarded to Tact's queue without a second idle.
- Text and `resource_link` prompt blocks.
- Agent text, thinking, tool calls, tool output, notices, and idle/running/
  requires-action state.
- Tact's single-select permission prompt maps to
  `session/request_permission`; `Allow once` / `Always allow …` / `Deny` map to
  ACP `allow_once` / `allow_always` / `reject_once`.
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

## Not implemented yet

- History replay on `session/resume`. The marker is emitted and the Tact
  session context is resumed, but no transcript is reconstructed. T3 renders
  reopened threads from its own projection and drops replay updates anyway.
- Client-provided MCP servers. T3 always sends its stdio bridge; accepting and
  ignoring it is a deliberate compatibility exception because Tact has no
  per-session MCP injection point. `session.mcp` is not advertised.
- Slash-command advertisement (`availableCommands`). Advertising without a
  command parser would be worse than none.
- Permission mode, thinking budget, and reasoning effort as config options.
- URL-based elicitation.
- Image, audio, and embedded-resource prompts (the agent does not advertise
  those capabilities).

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
  reconciles its own view from `ModelInfo`.
- `cwd` from `session/new` / `session/resume` selects the Tact workspace and
  its `.tact/tact.db`; do not rely on the process launch directory.
- stdout is reserved for JSON-RPC. Logs go to stderr and honour `RUST_LOG`.
- `session/close` closes the host and releases its session lock; stdin EOF
  closes every live host.
