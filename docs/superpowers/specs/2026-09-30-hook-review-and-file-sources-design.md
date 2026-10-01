# Hook layer, Codex parity part 2: file sources, review, and the block contract

Status: **approved** (user asked to continue the C items from the hook-parity review).

Follows `2026-09-30-mcp-tool-policy-design.md` in shape: land the compatibility
surface Tact is missing, keep every new capability reported rather than silent.

## Problem

Three gaps, all measured against Codex 0.157.1 (binary schemas) and Basic
Memory's Codex plugin (its `bm hook install --harness codex` flow):

1. **No file entry point.** Codex loads hooks from `$CODEX_HOME/hooks.json`,
   `<repo>/.codex/hooks.json` and inline `[hooks]` tables; Tact loads hooks only
   from *installed plugin bundles* (`installed_hooks`). So
   `bm hook install --harness codex` — which writes `~/.codex/hooks.json` — is a
   no-op for Tact, and there is nowhere for a user or a repository to declare a
   hook without first building a plugin.
2. **No review.** Installing a plugin is the only gate Tact has: its hooks then
   run arbitrary commands unattended. Codex records a hash per hook definition
   and refuses to run anything new or changed until the user reviews it
   (`Hooks need review`, `1 hook is new or changed.`, `Continue without trusting
   (hooks won't run)`, `trusted_hash` under `[hooks.state]` in `config.toml`).
   A repository-supplied hooks file makes this concrete: cloning a repo must not
   execute its commands.
3. **Exit code 2 is ignored.** Codex blocks on `exit 2` with the reason read
   from **stderr** (`PreToolUse hook exited with code 2 but did not write a
   blocking reason to stderr`, and the same for `PermissionRequest`,
   `PostToolUse` feedback, `Stop`/`SubagentStop` continuation). Tact treats
   every non-zero exit as fail-open, so a policy hook written the simple way
   silently does nothing.

## Design

### 1. Hook sources

One loader, three origins, all matching hooks run (Codex runs user + project +
managed together; a higher layer never replaces a lower one):

| Origin | Path | `${PLUGIN_ROOT}` | `${PLUGIN_DATA}` |
|---|---|---|---|
| user file | `~/.tact/hooks.json` | the file's directory | `~/.tact` |
| project file | `<workdir>/.tact/hooks.json` | the file's directory | `<workdir>/.tact` |
| installed plugin | bundle `hooks/hooks.json` / manifest `hooks` | bundle root | plugin data dir |

Registration order stays deterministic and documented: **plugins, user file,
project file** — plugin hooks keep the order they have today, so this change
cannot reorder an existing user's SessionStart context.

`~/.tact/hooks.json` deliberately mirrors Codex's *schema*, not its filename: a
`bm hook install` user copies one file, and Tact never reads `~/.codex/`.

### 2. Review (fail-closed)

- Identity is the hook **definition**: `source label`, event, matcher, command.
  Its SHA-256 is the key, so editing a command invalidates its approval — the
  same property Codex gets from `trusted_hash`.
- Trust lives in `~/.tact/hooks-state.json`
  (`{"version":1,"trusted":{"<hash>":"<one-line description>"}}`) — user-global,
  like the OAuth credential store, and separate from `config.toml` so a
  hand-edited config cannot silently grant execution.
- An untrusted hook is **skipped, never spawned**, and the load collects it into
  a report. `apply_hook_sources` returns `(Agent, HookLoadReport)`; the report's
  lines go to `AgentUpdate::Info` in the TUI and stderr in headless mode, the
  same two channels the MCP load report already uses.
- Recovery is one command: `tact-ui hooks trust --all` (or `--source <label>`),
  `tact-ui hooks list` to review first, `tact-ui hooks forget --all` to revoke.
- The review gate applies to **every** origin, including plugins, which is what
  Codex does. On a machine whose plugins have no hooks (all of them here) this
  changes nothing; where it does, the notice names the file and the command.

### 3. Exit code 2

`run_process` starts reporting the exit status and stderr, and the block
contract is per event, as Codex defines it:

| Event | exit 2 means |
|---|---|
| `PreToolUse`, `PermissionRequest` | block, `stderr` is the reason |
| `PostToolUse` | the tool result becomes a failure carrying `stderr` |
| `Stop`, `SubagentStop`, `UserPromptSubmit` | block/continue with `stderr` as the prompt |
| everything else | still fail-open, but now *reported* as a failed hook |

JSON output keeps precedence: a hook that prints a valid decision is honoured,
and only a bare `exit 2` falls back to stderr.

## Shipped in the same change (extended after review)

- **`PermissionRequest`**: runs only when Tact was about to ask, with `allow`
  skipping the prompt and `block` denying with its reason. This is what forced
  `HookControl::Allow` — without it a hook could refuse but never permit.
- **`Interrupt`**: observational, fired once per turn when the user cancels.
- **`additionalContextLimit`**: bounds one hook's injected context at the hook
  boundary, before the client-wide cap.

## Non-goals (deliberately not implemented)

- `SessionStart`'s `clear` / `fork` sources: Tact has no history-clear command
  and no session fork (verified by grep), so the variants would be unreachable.
  The vocabulary stays `startup` / `resume` / `compact`. Recorded in
  [Ch 9](../../../book/09_chapter_hook_zh.md) §13.
- Inline `[hooks]` tables in `config.toml`: Tact's config has no hook tables, and
  a second spelling would need its own precedence rules.
- A `bypass_trust` config switch (Codex's `dangerously-bypass-hook-trust`):
  `tact-ui hooks trust --all` is the scriptable equivalent, and a switch that
  disables review is exactly the thing an attacker would set.
- Codex's `mcp_tool` handler type and managed/enterprise hooks.

## Verification

- Unit: source discovery (user + project + plugin, missing file is fine),
  hash stability (same definition → same hash; changed command → new hash),
  fail-closed (untrusted hook is not spawned — asserted by a hook that would
  create a file), trust/forget round-trip through a temp store, notice lines.
- Exit contract: a fake hook exiting 2 with stderr blocks `PreToolUse`, fails
  `PostToolUse`, and does not block `SessionStart`; a JSON decision still wins.
- End-to-end: `tact-ui hooks list` on a temp home with one untrusted and one
  trusted hook; then `trust --all` and the same command again.
