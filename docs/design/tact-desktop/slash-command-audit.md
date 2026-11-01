# Slash Command Interaction Audit

Reference implementation: `crates/tui/src/widgets/state/slash.rs` and the
handlers under `crates/tui/src/handlers/`.

The desktop prototype uses one interaction model:

- `Enter` opens a subcommand tree or executes the selected command.
- `Tab` completes the selected path without executing it.
- Argument-taking verbs expose their real values in the same popup.
- Free-form arguments open a desktop form instead of leaving an inert command
  string in the composer.
- Results open in a shared command sheet; managers open in the full Extensions
  Manager.

## Top-Level Commands

| Command | Desktop interaction | Status |
|---|---|---|
| `/theme` | Toggle Light / Dark | Complete |
| `/model` | Main model picker | Complete |
| `/model-subagent` | Subagent model sheet | Complete |
| `/permission` | Permission-mode picker | Complete |
| `/view-system-prompt` | Assembled prompt reader | Complete |
| `/save` | Download transcript + saved-path feedback | Complete |
| `/compact` | Animated compaction result sheet | Complete |
| `/cancel` | Cancel the active run | Complete |
| `/subagent_cancel` | Running-agent value picker + stop action | Complete |
| `/quit` | Persisted-session quit confirmation | Complete |
| `/help` | Keyboard shortcut reader | Complete |
| `/history` | Submitted-task history | Complete |
| `/skill` | Skills command tree | Complete |
| `/plugin` | Plugins command tree | Complete |
| `/mcp` | MCP command tree | Complete |
| `/hooks` | Hook review command tree | Complete |
| `/hook-output` | Toggle hook output visibility | Complete |
| `/balance` | Provider balance and reset detail | Complete |
| `/lang` | Language picker | Complete |
| `/stats` | Session statistics reader | Complete |
| `/tasks-dag` | Task dependency graph | Complete |
| `/background` | Background-task popover | Complete |
| `/goal` | Goal command tree | Complete |

## Declared Subcommands

| Command tree | Desktop interaction |
|---|---|
| `/skill list` | Catalog sheet with hide/show actions |
| `/skill reload` | Reload progress/result |
| `/skill enable <name>` | Skill value picker + catalog update |
| `/skill disable <name>` | Skill value picker + catalog update |
| `/plugin list` | Installed-plugin sheet |
| `/plugin reload` | Reload result |
| `/plugin uninstall <name>` | Plugin value picker + confirmation |
| `/plugin update <name>` | Plugin value picker + update result |
| `/plugin marketplace list` | Registered marketplace sheet |
| `/mcp auth <server>` | Server value picker + authorization result |
| `/mcp login <server>` | Alias of `/mcp auth` |
| `/mcp list` | MCP Extensions Manager |
| `/mcp prompts` | Published-prompt sheet |
| `/mcp prompt ...` | Server/prompt/argument form |
| `/hooks list` | Hook source review with Trust/Forget actions |
| `/hooks trust --all` | Trust every hook source |
| `/hooks trust --source <label>` | Source value picker + Trust action |
| `/hooks forget --all` | Revoke every approval |
| `/goal create <objective>` | Goal creation form |
| `/goal list [--all]` | Active/all Goal sheet |
| `/goal show <goal-id>` | Goal detail sheet |
| `/goal steer <goal-id> <text>` | Goal value picker + text argument |
| `/goal pause <goal-id>` | Goal value picker + state update |
| `/goal resume <goal-id>` | Goal value picker + state update |
| `/goal cancel <goal-id>` | Goal value picker + state update |
| `/goal accept <goal-id> <text>` | Goal value picker + acceptance note |
| `/goal archive <goal-id>` | Goal value picker + archive |
| `/goal unarchive <goal-id>` | Archived Goal picker + restore |

## Fixed Interaction Bugs

- `/hooks list` previously only filled `/hooks list ` into the composer and
  showed a generic toast. It now opens the Hook review sheet.
- All declared leaf subcommands now execute instead of returning inert text.
- Nested paths such as `/hooks trust --source` and `/plugin marketplace list`
  are navigable from the slash popup.
- Argument values are selected from live lists rather than typed blind.
- Enter on a free-form command opens its form; Enter after typed arguments
  submits the command instead of inserting a newline.
