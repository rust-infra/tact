# MCP server names — short for display, full for identity

Status: **shipped** (`ebd5a18b`, 2026-10-01).
Retroactive record, written 2026-10-02 from the shipped code, that commit's message, and the
Ch 26 entry.

## Problem

`plugin__canva__canva` is three things at once: the config key, the OAuth credential file name, and
the `mcp__<server>__<tool>` prefix the agent calls. Only the first of those *has* to be the full
name, yet the full name was also what every listing and every hint printed — twice in one line:

```
MCP server plugin__canva__canva needs authorization — run /mcp auth plugin__canva__canva
```

The prefix is not wrong; it is eighteen characters of noise in front of the two facts that line
exists to convey.

## Design

### Two ends, separated

**Display** goes through `display_server_name(name)` (`crates/tact/src/mcp/mod.rs`), which drops the
`plugin__<plugin_id>__` prefix. The split is on the *first* `__` after the prefix, so a server key
may itself contain `__`; only a plugin id may not. A name without the prefix, or with an empty
plugin-id or server segment, is returned unchanged.

Used by: `/mcp list` (TUI and CLI), `mcp get`'s heading, the "needs authorization" hints, and the
Overridden / Filtered / Entry-keys notes. The report's column width is measured on the *displayed*
names, so the table does not reserve room for a prefix it never prints.

**Input** goes through `resolve_server_name(name)`:

- An exact match always wins, so a server the user declared themselves is never reached through a
  plugin's short form.
- Otherwise, the *only* configured server whose display name equals the input.
- No match, or several (two plugins both shipping `canva`), is an error naming the candidates rather
  than a silent pick.

`/mcp auth`, `mcp login`, `mcp get` and `mcp logout` all resolve first, which is what makes a printed
hint a command that actually runs.

### Where the line is drawn

- **`mcp logout` resolves best-effort.** A server no longer in the config still has credentials to
  delete, and they are keyed by the full name — refusing to resolve would strand them.
- **`mcp remove` is deliberately untouched.** No file can delete a plugin server, so there is
  nothing a short name could reach.
- **`mcp get`'s heading is short, but its tool lines print the full name**, because that is literally
  what the agent invokes (`mcp__plugin__canva__canva__<tool>`).

### Identity is unchanged

Config keys, credential paths and the `mcp__…` tool prefix all keep the full name. The agent side
sees nothing; this is a presentation change with one input-side affordance.

## Non-goals

- **No renaming** of plugin-contributed servers.
- **No fuzzy matching.** One exact hit, or an error that names the candidates.
- **No change to `mcp remove`.**

## Verification

- `crates/tact/src/mcp/mod.rs`: `display_server_name_strips_only_the_plugin_prefix`,
  `resolve_name_prefers_an_exact_match_over_a_plugin_short_form`,
  `resolve_name_reaches_a_plugin_server_through_its_short_form`,
  `resolve_name_refuses_to_guess_between_two_plugin_servers`,
  `resolve_name_reports_an_unknown_name`, and
  `pending_authorization_hint_uses_the_short_plugin_name` (the hint is built by `notice_lines` in the
  same crate, so it is pinned where it is produced).
- `crates/tact-ui/src/mcp_cli.rs`: `report_shows_a_plugin_server_under_its_short_name`,
  `report_notes_name_a_plugin_server_short_too`,
  `the_detail_view_heads_a_plugin_server_short_but_keeps_its_tool_prefix`,
  `live_listing_shows_a_plugin_server_under_its_short_name`.
- Docs: [Ch 08](../../../book/08_chapter_mcp_zh.md), [Ch 26](../../../book/26_chapter_issue_zh.md)
  2026-10-01 entry.
