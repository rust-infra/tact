# `/hooks` — review command hooks where they fire

Status: **approved** (one of the remaining TODOs after the Codex hook-parity work).

## Problem

Hook review shipped CLI-only: `tact-ui hooks list|trust|forget`. That is enough to be *correct* —
an unreviewed hook is never registered — but not enough to be *usable*:

- The only moment a user cares is the moment they see the notice. The TUI tells them
  `1 hook needs review and was not run`, then leaves them to quit the session, run a CLI command,
  and start a new one. `/mcp` already solved exactly this shape; `/hooks` did not.
- The notice names no way forward inside the app. `hooks_cli::pending_summary` names the count, and
  the follow-up is prose in a book chapter.
- Reviewing is a security decision, so the in-app path must not be *easier to get wrong* than the
  CLI. The CLI requires `--all` or `--source <label>`; nothing may approve everything by accident.

## Design

### Command surface (mirrors the CLI exactly)

| TUI | CLI equivalent | Effect |
|---|---|---|
| `/hooks`, `/hooks list` | `hooks list` | Render every configured hook and its review status |
| `/hooks trust --all` | `hooks trust --all` | Approve every pending definition |
| `/hooks trust --source <label>` | `hooks trust --source <label>` | Approve the pending definitions of one source |
| `/hooks forget --all` | `hooks forget --all` | Revoke every approval |
| anything else | — | Usage hint, `--all` / `--source` spelled out |

A bare `/hooks trust` therefore shows usage instead of approving: the safety rule lives in
`trust_hooks` (it bails before touching the store) and the parser never constructs a call that
violates it.

### Parsing is prefix-based, not whitespace-split

A source label is free text with spaces (`plugin ponytail`, `~/.tact/hooks.json`).
`/mcp`-style `split_whitespace` would cut `--source plugin ponytail` in half and the narrowed
approval could never select the hooks it names, so `/hooks` strips the `/hooks` prefix and parses
the remainder as a string.

### Where the work happens

The driver, for the same reason as `/mcp list`: the hook sources and the review store
(`~/.tact/hooks-state.json`) are only reachable from the `tact` crate, and the TUI crate does not
depend on it. Three protocol commands (`HooksList`, `HooksTrust`, `HooksForget`) map to
`survey_hooks` / `trust_hooks` / `forget_hook_trust`, and the result is reported on the channels
the rest of the app already uses (`MdInfo` for the listing, `Info` for the confirmation, `Error`
for a failure).

### Idle gate, and the honest "next session" line

`/hooks list` is idle-only, like `/mcp list`: the driver serializes non-fast commands behind an
in-flight turn, so a listing that appears after a long turn reads as unrelated to the command.
`trust` and `forget` are state changes effective from the next session, so they are allowed to
queue. The confirmation says so out loud — hooks are registered when the agent is built, so
"approved" without "not yet running" would be a lie about the current session.

### Reuse, not paraphrase

The listing is `hooks_cli::render_hooks_listing`, the same function the CLI prints, so the two
surfaces cannot drift into describing the same hooks differently.

## Non-goals

- **No live hook re-registration.** `apply_hook_sources` appends to the agent's hook vectors, so
  re-running it mid-session would run already-registered hooks twice. Approval stays
  next-session, which is also what the CLI documents.
- **No numbered selection** (`/hooks trust 3`). It would need a stable index across two renderings
  and would make "approve the third thing you saw" possible; `--source` is the narrowing knob
  both surfaces share.
- **No hooks editor.** Nothing here writes a `hooks.json`.

## Verification

- `crates/tui/src/handlers/hooks.rs`: parsing for every accepted form, usage for bare `trust` /
  bare `forget` / unknown subcommand, `--source` with a space-containing label, and the idle gate
  on `list`.
- `crates/tact_ui/src/driver.rs`: `HooksList` emits an `MdInfo` listing (read-only), and
  `HooksTrust { all: false, source: None }` reports the refusal — the one trust path that cannot
  write a developer's real review store.
- `cargo test -p tui --lib` (palette-count-sensitive tests now derive the palette size instead of
  hardcoding it).
