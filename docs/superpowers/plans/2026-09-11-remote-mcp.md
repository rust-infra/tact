# Plan: Remote MCP — Streamable HTTP + OAuth 2.0

Date: 2026-09-11
Status: implemented

Spec: `docs/superpowers/specs/2026-09-11-remote-mcp-design.md`

## Result

Shipped as designed. Deviations from the original work items:

- Item 10 ships `/mcp auth <server>` plus the `tact-ui mcp list` / `tact-ui mcp
  auth <server>` CLI pair. `mcp list` covers the `/mcp status` work item (the
  TUI still relies on the startup `pending_auth` / `failures` notices).
- `load_mcp_router_with_report` returns a **boxed future**: the nested transport
  futures made the `Send` bound overflow the trait solver at `tokio::spawn`
  call sites without the box.
- An expired, non-refreshable token is classified as `pending_auth` (via a
  downcastable `AuthorizationRequired` marker error), not as a connection
  failure. A server that answers 401 without declaring `auth` is upgraded the
  same way (`is_auth_required_error`).
- `McpLoadReport.configured` was added so `mcp list` can show every resolved
  server (name, transport, source) — not just the problem lists.
- Follow-up: the CLI grew the full management surface — `mcp get <name>` (one
  server, connects only to it, prints qualified tool names), `mcp remove <name>
  [--user]`, `mcp logout <server>`, and `mcp add <name> --url|--command …`.
  Config writes live in the new `crates/tact/src/mcp/edit.rs` and edit the raw
  JSON document so unknown keys survive; credential deletion lives in
  `remote::forget_credentials`. `mcp login` is the new name for `mcp auth`
  (kept as an alias, and `/mcp login` is accepted in the TUI). Server names are
  now validated as path components, so no command can be pointed at a file
  outside `~/.tact/mcp/oauth/`.

## Work Items

1. **Dependencies** — workspace `rmcp` features `+transport-streamable-http-client-reqwest`,
   `+auth`; `crates/tact` `+http = "1"`. Verify `cargo check -p tact` still resolves one
   reqwest family per consumer.
2. **Config model** (`mcp/config.rs` or in `mod.rs`) — `McpRemoteConfig { url, headers,
   auth }`; `McpAuthConfig::Oauth { client_id, scopes, callback_port }`; parse in
   `McpProjectConfig`; `to_transport()` returning `McpTransportConfig`.
3. **Transport enum + resolver** — `McpTransportConfig::{Stdio(McpServerConfig),
   Remote(McpRemoteConfig)}`; `resolve_servers` keeps remote; `skipped_remote` now only
   unsupported/incomplete transports. Log at parse + resolve.
4. **Remote connect** — `McpClient::connect_remote` builds `StreamableHttpClientTransport`
   (`auth_header` from static header or OAuth token, `custom_headers` from `headers`),
   then the same `serve(transport)` + `fetch_tools` path. Dispatch from `try_new`.
5. **OAuth storage** — `FileCredentialStore` implementing rmcp `CredentialStore`, file
   `~/.tact/mcp/oauth/<server>.json`; `TactPath` helper for the dir; 0600 perms.
6. **OAuth connect-time** — `resolve_remote_token(config)`: stored creds → access token
   (auto-refresh) → connect; no creds → `AuthRequired` marker surfaced as report
   `pending_auth`.
7. **OAuth interactive flow** — loopback `TcpListener` on `127.0.0.1:<port>`, discovery,
   dynamic registration, auth URL, callback parse (`code` + `state`), token exchange,
   persist, return access token. Timeout + cancellation. Log every step.
8. **Report** — `McpLoadReport.pending_auth: Vec<String>`; `notice_lines()` line
   `MCP server <name> needs authorization — run /mcp auth <name>`; `is_quiet()` account.
9. **Reload** — `Agent::reload_mcp_router()` (disconnect old, load new, rebuild
   `cached_tool_specs`); factor the provider-kind spec filter out of `Agent::new`.
10. **Command plumbing** — `UserCommand::McpAuth { server }` in `crates/protocol`;
    driver handles it via the agent-taking path; `/mcp auth <server>` + `/mcp status`
    parse in `crates/tui/src/handlers/mcp.rs`; i18n strings (EN + ZH).
11. **Tests** — config parse, resolver, transport dispatch (mock service), credential
    store round-trip, callback parsing/state check, report lines, reload.
12. **Docs** — Ch 08 (EN+ZH) remote+OAuth section; Ch 21 (EN+ZH) config fields; Ch 26
    (EN+ZH) newest-first entry; update the stale "stdio only" limitations.

## Verification

- `cargo test -p tact --lib mcp::` (one invocation at a time).
- `cargo test -p tui --lib handlers::mcp` and `-p tact-ui driver::`.
- `cargo fmt` + `cargo clippy` on touched crates.
- Manual smoke: temp `mcp.json` with a `url` server reports connect/auth-required rather
  than "skipped"; `/mcp auth` completes against a local test server if available.

## Notes / decisions

- Startup never blocks on OAuth; pending servers are reported and authorized later.
- `/mcp auth` hot-reloads the MCP router so no restart is needed.
- Browser is not opened automatically in v1; the URL is printed.
- Never log token values or header secrets — log names/paths/URLs only.
