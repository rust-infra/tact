# Plan: `mcp get` tool declaration cost

Spec: [2026-10-01-mcp-tool-declaration-cost-design.md](../specs/2026-10-01-mcp-tool-declaration-cost-design.md)

Shipped as `baa1a4f9`. Retroactive record (2026-10-02).

1. **Measure** (`crates/tact_extensions/src/mcp/mod.rs`)
   - `derive_exposed` records each exposed tool's declaration size (name + description + input
     schema) as it rebuilds the specs that are actually sent, sorted by tool name.
   - `McpServerInspection` carries the per-tool bytes; empty when the server is not connected.
   - Doc comments state that this is **not** a policy input — it exists so the human choosing
     `enabled_tools` can see what each tool costs.
2. **Format** (`crates/tact_ui/src/mcp_cli.rs`)
   - `format_bytes` — one decimal (`34.6 KB`).
   - `format_tokens` — `≈8.6k tokens`, labelled an estimate at four bytes per token.
3. **Render** (`crates/tact_ui/src/mcp_cli.rs`)
   - `render_server_detail` sums the per-tool bytes into a per-request estimate, appends each tool's
     own size, and names `enabled_tools` on the header line.
4. **Bundle the leftover** (`crates/tact_ui/src/mcp_cli.rs`, `book/08_chapter_mcp_zh.md`)
   - `suggested_risk_policy` — a paste-ready `tools` block for the tools the server declared
     read-only, and the Ch 08 prose describing it. Drafts only what the server claimed, so pasting
     can only tighten.
5. **Tests**
   - `cargo test -p tact-ui --lib mcp_cli` — `the_detail_view_reports_what_the_tools_cost`,
     `the_detail_view_drafts_a_risk_policy_from_the_servers_own_claim`.
6. **Docs sync**
   - `book/08_chapter_mcp_zh.md`: the cost line and the risk-policy draft.
   - `book/26_chapter_issue_zh.md`: 2026-10-01 entry.
