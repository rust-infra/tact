# Plan: MCP tool risk granularity

Spec: [2026-09-30-mcp-tool-risk-design.md](../specs/2026-09-30-mcp-tool-risk-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

1. **Facet A — `High` consults the session allow-list** (`crates/tact/src/permission/mod.rs`)
   - In `check_with_auto`, replace the unconditional `risk == High → ask` with: allow-listed
     (tool **and** input) → allow; otherwise → ask.
   - Rewrite the numbered decision list in the doc comment above `check` so steps renumber.
   - Tests: a High tool that is allow-listed is allowed without asking; High still asks when it is
     not; plan mode still blocks; `ask_user` still denies.
2. **Facet B — the `risk` axis**
   - `ToolRisk { Read, Write, High }` in `crates/tact/src/mcp/mod.rs` beside `ApprovalMode`:
     `parse` / `as_str` / `to_capability`, plus `parse_tool_risk(value, field)` warning on an
     unknown value the way `parse_approval_mode` does.
   - `McpToolConfig.risk: Option<String>` (modelled, so serde stops dropping it silently) and
     `McpProjectConfig.default_tool_risk: Option<String>`; update `Default`.
   - `McpToolPolicy.risk: Option<CapabilityRisk>`; `McpServerPolicy::risk_for(tool)` = override then
     default; `MCPToolRouter::risk_for(server, tool)` defaulting to
     `normalize_mcp_capability` (whose doc gains "the default when no entry declares one").
   - `crates/tact/src/agent/tool_dispatch.rs`: the `ResolvedTool::Mcp` arm asks the router instead of
     calling `normalize_mcp_capability` directly.
3. **Facet C — annotations as display-only evidence**
   - `McpClient::assemble` collects exposed tools with `annotations.read_only_hint == Some(true)` into
     a sorted, deduped `declared_read_only: Vec<String>`; accessor `declared_read_only()`.
   - `McpServerInspection.declared_read_only: Vec<String>`, filled by `inspect_server`.
   - `crates/tact-ui/src/mcp_cli.rs`: `render_server_detail` marks those tools and names the effective
     risk per tool.
4. **Tests**
   - `cargo test -p tact --lib permission::` , then `--lib mcp::`.
   - `cargo test -p tact-ui --lib mcp_cli::`.
   - The declared-tier end-to-end assertion lands at the router
     (`mcp::tests::the_router_resolves_a_declared_tier_and_defaults_the_rest_to_high`) rather than
     in `agent::tool_dispatch`: the dispatch site is a one-line delegation with no logic, and
     reaching it there would mean making `McpServerPolicy::from_config` public for a test.
5. **Docs sync**
   - `book/08_chapter_mcp.md` + `_zh.md`: two field-table rows, a "Per-tool risk" subsection under
     "Per-server tool policy" carrying the plan-mode table, and the §10 gap row replaced.
   - `config.example.toml`: the `[mcp]` comment block gains `default_tool_risk` and `tools.<name>.risk`
     with the tier warning.
   - `book/26_chapter_issue.md` + `_zh.md`: two newest-first entries — the facet-A bugfix, then the
     facet-B/C feature.
