# Path Convergence: MCP Config, Plugin State, Skill Roots

Date: 2026-09-10
Status: implemented

## Problem

Tact's on-disk layout accreted three ecosystems' conventions and never
reconciled them:

- **Tact** — `.tact/` (project) and `~/.tact/` (user)
- **Codex** — `.agents/` and `.codex-plugin/`
- **Claude** — `.mcp.json`

The result is a layout where the canonical Tact location exists for some
features but not others, and where foreign conventions occupy the highest
precedence slot rather than the lowest.

Concrete defects, all verified against the current tree:

1. **MCP has no home of its own.** Declaring an MCP server requires a
   `.codex-plugin/plugin.json` manifest (cwd-scoped) or a full
   marketplace → install → cache round trip (global). There is no
   `~/.tact/mcp.json` and no `.tact/mcp.json`. Code: `load_mcp_router`
   (`crates/tact/src/mcp/mod.rs:580`) reads exactly two sources.
2. **`.mcp.json` is read inside plugin roots but not at cwd, and there is no
   project-scoped MCP file.** `PluginLoader::scan` only opens
   `<dir>/.codex-plugin/plugin.json` (`crates/tact/src/mcp/mod.rs:79`), so a
   Claude-format project file is silently ignored, while the same filename *is*
   read inside plugin roots (`collect_plugin_mcp_servers`,
   `crates/tact/src/mcp/mod.rs:174`) — an asymmetric rule with no project-level
   answer at all.
3. **Connection failures are invisible.** A failed `McpClient::try_new` logs
   `tracing::debug!` and the router is returned as if nothing happened
   (`crates/tact/src/mcp/mod.rs:610`). A typo'd `command` produces no user
   feedback at all.
4. **Plugin state and plugin content share a directory.**
   `~/.tact/plugins/` holds `installed.json` + `marketplaces.json` (a few KB)
   next to `cache/` (hundreds of MB). Code: `PluginHome::from_home`
   (`crates/tact/src/consts.rs:26`).
5. **Skill roots are ordered backwards.** `skill_search_dirs` returns
   `[workdir/.tact/skills, ~/.tact/skills, ~/.agents/skills]` and later wins
   (`crates/tact/src/skill/mod.rs:151`). The foreign `~/.agents/skills`
   therefore overrides Tact's own `~/.tact/skills`.
6. **`$HOME` is derived positionally.** `plugin_home_dir` is
   `home.root.parent()?.parent()` (`crates/tact/src/plugin/store.rs:142`),
   which silently breaks if the root layout changes.

## Goals

- Give MCP a first-class config file at both project and user scope, with
  project overriding user by server name.
- Make MCP server names from `mcp.json` the plain map key (no `plugin__`
  prefixes), so tool names are predictable: `mcp__<name>__<tool>`.
- Give project scope exactly one filename (`.tact/mcp.json`), instead of
  adding a second one next to a near-identically named `.mcp.json`.
- Remove cwd-level `.codex-plugin/plugin.json` as an MCP source, so a user has
  exactly two places to look: one user file, one project file.
- Surface MCP connection failures to the user instead of only to `tracing`.
- Separate plugin **state** from plugin **content**, with a safe one-time
  migration and a legacy fallback read.
- Make Tact's own skill root outrank the Codex compatibility root.
- Derive `$HOME` from an explicit field rather than `.parent().parent()`.

## Non-goals

- Removing the Claude official marketplace or Codex *marketplace* discovery.
  Both stay untouched. (Consistent with
  `2026-09-10-codex-marketplace-design.md`, whose non-goals forbid removing the
  Claude official marketplace.)
- Removing installed-plugin MCP support. A plugin is a distributable bundle,
  not a config convention, so it stays a source.
- Note: the two *cwd-level* MCP compatibility sources **are** removed
  (`cwd/.codex-plugin/plugin.json` and `cwd/.mcp.json`). That is deliberate —
  see §2 — and is the one place this change reduces accepted inputs.
- Adding a remote MCP transport (http/sse). Still unsupported.
- Handling `notifications/tools/list_changed`. Still unsupported.
- Per-tool MCP permission granularity. All MCP tools stay
  `CapabilityRisk::High`.
- Relocating the plugin cache or the marketplace catalog directory.

## Design

### 1. MCP configuration file

New file name `mcp.json`, Claude-compatible shape:

```json
{
  "mcpServers": {
    "basic-memory": {
      "command": "/Users/rg/.local/bin/basic-memory",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

Two locations, both optional:

| Scope | Path | Precedence |
|---|---|---|
| User | `~/.tact/mcp.json` | lower |
| Project | `<cwd>/.tact/mcp.json` | higher |

Project entries override user entries **by server name**; non-colliding
entries from both are merged. Within a single file, `mcpServers` is a map, so
duplicates are impossible.

Entries use the same `McpServerConfig` shape already in the tree
(`command` / `args` / `env`), so no new parsing type is strictly needed — but
a dedicated `McpConfigFile` wrapper is introduced to own the error messages.

Malformed `mcp.json` is a hard error: a user-authored config file that cannot
be parsed must not be silently ignored. This is the same class of failure as a
malformed `config.toml`.

### 2. Load order and naming

Full resolution order in `load_mcp_router`, lowest precedence first:

1. `~/.tact/mcp.json` (user)
2. `<cwd>/.tact/mcp.json` (project)
3. installed plugins: `.codex-plugin/plugin.json` + plugin-root `.mcp.json`

Steps 1–2 use the plain map key as the server name. Step 3 keeps its
`plugin__{plugin}__{server}` naming, because a plugin is a distributable bundle
rather than a config convention.

Tact reads **no** cwd-level `.codex-plugin/plugin.json` and **no** cwd
`.mcp.json`. Project scope therefore has exactly one filename, which removes
both the `mcp.json` / `.mcp.json` near-collision and the question of whether a
directory is a "plugin" or a "project". `PluginLoader` is deleted along with the
cwd manifest source — it had no other caller.

A plugin-root `.mcp.json` is still read: that is the plugin *bundle* format, not
a project config convention.

When the same server name appears at several levels, the higher-precedence
entry wins and the shadowed one is recorded in the load report (see §3) so the
override is observable rather than silent.

### 3. Failure and override visibility

`load_mcp_router` gains a report alongside the router:

```rust
pub struct McpLoadReport {
    pub connected: Vec<(String, usize)>,   // server name, tool count
    pub failures: Vec<(String, String)>,   // server name, error
    pub shadowed: Vec<(String, String)>,   // server name, source that lost
    pub skipped_remote: Vec<String>,       // http/sse entries not connected
}
```

`connected` / `failures` / `shadowed` / `skipped_remote` are rendered as a
single startup notice in the TUI when non-empty; a fully clean load prints
nothing. This removes the "typo'd command is silent" failure mode without
adding noise to the common case.

### 4. Plugin state directory

`PluginHome` gains a `state` field and an explicit `home` field:

```rust
pub struct PluginHome {
    pub home: PathBuf,          // NEW: $HOME, explicit
    pub root: PathBuf,          // ~/.tact/plugins
    pub state: PathBuf,         // NEW: ~/.tact/plugins/state
    pub marketplaces: PathBuf,  // ~/.tact/plugins/marketplaces
    pub cache: PathBuf,         // ~/.tact/plugins/cache
}
```

`installed.json` and `marketplaces.json` move to `state/`.
`plugin_home_dir` becomes `home.home.clone()` — no positional parents.

**Migration**: on first read, if `<root>/installed.json` (legacy) exists and
`<root>/state/installed.json` does not, the legacy file is used and then
rewritten to the new location. Reads always prefer the new path and fall back
to the legacy path, so an older Tact binary sharing the same home does not
break. Writes always target the new path. The same rule applies to
`marketplaces.json`.

### 5. Skill root precedence

Corrected order, lowest precedence first:

1. `~/.agents/skills` (Codex compatibility)
2. `~/.tact/skills` (Tact canonical)
3. `<workdir>/.tact/skills` (project)

i.e. `skill_search_dirs` returns `[home_agents, home_tact, workdir_tact]`.
Project still wins over all user roots; Tact now wins over Codex. Configured
`[agent].skill_dirs` are still appended last and still win.

## Migration and compatibility summary

| Artifact | Old location | New location | Read fallback |
|---|---|---|---|
| MCP servers | `.codex-plugin/plugin.json` (cwd) | `~/.tact/mcp.json`, `.tact/mcp.json` | **no** — cwd manifest read removed |
| `.mcp.json` (cwd) | not read | **still not read** (no second project filename) | — |
| `installed.json` | `~/.tact/plugins/` | `~/.tact/plugins/state/` | yes |
| `marketplaces.json` | `~/.tact/plugins/` | `~/.tact/plugins/state/` | yes |
| Skills | `~/.agents/skills` | `~/.tact/skills` | both read, new order |

Plugin state and skill precedence are strictly backward-compatible: legacy
state files are read and migrated, and every skill root is still scanned.

Two MCP compatibility inputs are intentionally dropped, so a configuration that
relied on either must move:

| Dropped input | Migration |
|---|---|
| `cwd/.codex-plugin/plugin.json` `mcpServers` | Move the entries into `.tact/mcp.json`; drop the `{plugin}__` prefix from server names, since a native entry is named by its map key |
| `cwd/.mcp.json` | Already unread before this change; move into `.tact/mcp.json` |

Both are project-scoped single-file moves. Server names change for the first
case, so allow-lists keyed on `mcp__{plugin}__{server}__*` need updating.

## Tests

- `mcp.json` parses; missing file is `None`, not an error.
- Malformed `mcp.json` is a hard error with the file path in the message.
- Project `mcp.json` overrides user by server name; non-conflicting keys merge.
- Server name from `mcp.json` produces tool name `mcp__<name>__<tool>`.
- Shadowed servers appear in `McpLoadReport::shadowed`.
- Connection failure appears in `McpLoadReport::failures` and does not abort
  startup.
- `.mcp.json` at cwd is **not** read; a plugin-root `.mcp.json` still is.
- A cwd `.codex-plugin/plugin.json` neither contributes servers nor can abort
  startup (the `PluginLoader::scan` hard-error path is gone).
- Legacy `installed.json` is read, migrated to `state/`, and the legacy file is
  left in place (or removed only after a successful write).
- `PluginHome::from_home` exposes `state`/`home`; no `.parent().parent()`.
- `skill_search_dirs` order is `[home_agents, home_tact, workdir_tact]`; a skill
  present in both `~/.tact/skills` and `~/.agents/skills` resolves to the
  `~/.tact/skills` body.
- Existing plugin, mcp, and skill tests keep passing.

## Docs to sync

- `book/08_chapter_mcp.md` + `_zh.md` — new config file, load order, naming.
- `book/01_chapter_store*.md` — plugin state path, if the chapter mentions it.
- `book/21_*` (marketplace) — state file location.
- `config.example.toml` — only if a new knob is added (none planned).
- `book/26_chapter_issue.md` + `_zh.md` — issue-log entry (user-visible
  change: silent MCP failures now reported; MCP gets a config file).

## Risks

- **Public contract change**: `installed.json` location. Mitigated by
  read-fallback + migration rather than a hard move.
- **Startup notice noise**: mitigated by printing nothing when the report is
  clean.
- **Skill precedence flip is observable**: a user with the same skill name in
  both roots will see different content after upgrade. This is the intended
  fix, but it must be called out in the Ch 26 entry.

## Resolved questions

1. **Migration deletes the legacy file?** No — the legacy file is left in
   place. A downgrade to an older binary sharing the same home must keep
   working, and the migration write itself is best-effort. Implemented in
   `PluginStore::read_state`.
2. **Startup notice style?** A one-line info notice per fact, delivered as
   `AgentUpdate::Info` (TUI) or stderr (headless), and suppressed entirely when
   the load is clean. Implemented in `McpLoadReport::notice_lines`.
