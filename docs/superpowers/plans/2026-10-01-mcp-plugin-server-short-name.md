# Plan: MCP server short names

Spec: [2026-10-01-mcp-plugin-server-short-name-design.md](../specs/2026-10-01-mcp-plugin-server-short-name-design.md)

Shipped as `ebd5a18b`. Retroactive record (2026-10-02).

1. **`crates/tact_extensions/src/mcp/mod.rs`**
   - `split_plugin_server_name(name) -> Option<(&str, &str)>` — strips `plugin__`, splits on the
     first `__`, rejects an empty plugin id or server segment.
   - `display_server_name(name) -> &str` — the short form, `#[must_use]`, identity when the prefix
     is absent.
   - `resolve_server_name(name) -> Result<String>` — reads the configured servers; exact match
     first, then the unique display-name match, then an error naming the candidates.
   - `resolve_name_against(name, configured)` — the pure core, separated so the matching rules are
     testable without reading the working directory.
2. **`crates/tact_ui/src/mcp_cli.rs`**
   - Route every displayed name through `display_server_name`; measure the report's column width on
     the displayed names.
   - `mcp get`: short heading, full names on the tool lines.
   - `mcp login` / `mcp logout` / `mcp get` resolve before acting; `mcp logout` best-effort.
   - Leave `mcp remove` alone.
3. **Hints** — the "needs authorization" line (`notice_lines`, same crate) prints the short name, and
   the command it names is one that resolves.
4. **Tests**
   - `cargo test -p tact --lib mcp` — the five name tests plus
     `pending_authorization_hint_uses_the_short_plugin_name`.
   - `cargo test -p tact-ui --lib mcp_cli` — the four report/detail/listing tests.
5. **Docs sync**
   - `book/08_chapter_mcp_zh.md`: the short-name rule and the full-name identity.
   - `book/26_chapter_issue_zh.md`: 2026-10-01 entry.
