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

The `Local command` form only exposes an executable path, so keep the binary
zero-argument. Tac's configuration is read from the launch working directory /
`~/.tact`; it is not passed through T3's provider settings.

## Protocol scope

- `initialize` negotiates ACP protocol v2 and advertises the baseline session
  capability.
- `session/new`, `session/list`, `session/resume`, `session/close`,
  `session/prompt`, `session/cancel`, and `session/update`.
- `session/prompt` returns as soon as the prompt is inserted; turn completion
  is reported by an idle `state_update` with a stop reason.
- Text and `resource_link` prompt blocks.
- Agent text, thinking, tool calls, tool output, notices, and idle/running/
  requires-action state.
- Tact's single-select permission prompt maps to
  `session/request_permission`; `Allow once` / `Always allow …` / `Deny` map to
  ACP `allow_once` / `allow_always` / `reject_once`.

## Not implemented yet

- Image, audio, and embedded-resource prompts (the agent does not advertise
  those capabilities).
- Client-provided MCP servers (Tact uses its own `.mcp.json` configuration).
- Multi-select `ask_user` questions (they are declined with a notice).
- Session config options and slash-command advertisement.
- History replay on `session/resume`; the Tact session context is resumed, but
  ACP history replay is not emitted (`replay_complete` is sent for a requested
  replay cursor).

## Design notes

- Each ACP session owns one `tact_host::HostSession`. A per-session actor pumps
  `HostEvent`s into ACP `session/update` notifications and forwards
  `UserCommand`s back to the host.
- `cwd` from `session/new` / `session/resume` selects the Tact workspace and
  its `.tact/tact.db`; do not rely on the process launch directory.
- stdout is reserved for JSON-RPC. Logs go to stderr and honour `RUST_LOG`.
- `session/close` closes the host and releases its session lock; stdin EOF
  closes every live host.
