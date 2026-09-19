//! Markdown rendering for the live MCP server view.
//!
//! `/mcp list` renders the agent's *current* router rather than reloading it,
//! so the listing is a pure function of the summaries the caller already holds.
//! Both front ends share this renderer: the TUI prints it as a Markdown cell
//! and `tact-ui mcp list` prints the same shape.

use tact::mcp::{McpLiveStatus, McpServerView};

/// Render `views` as a Markdown section: a table when servers exist, or the
/// "nothing configured" hint with the declaration paths.
pub fn render_live_listing(views: &[McpServerView]) -> String {
    if views.is_empty() {
        return "## 🔌 MCP Servers\n\nNo MCP servers configured.\n\n\
                Declare servers in `~/.tact/.mcp.json` (user) or `.tact/.mcp.json` (project), \
                then restart or run `/mcp auth <server>` for a remote OAuth server."
            .to_string();
    }

    let mut out = String::from(
        "## 🔌 MCP Servers\n\n| Server | Transport | Source | Status |\n|---|---|---|---|\n",
    );
    for view in views {
        let status = match view.status {
            McpLiveStatus::Connected { tools } => format!("connected ({tools} tools)"),
            McpLiveStatus::NeedsAuthorization => {
                format!("needs authorization — run `/mcp auth {}`", view.server.name)
            }
            McpLiveStatus::NotConnected => "not connected".to_string(),
            McpLiveStatus::Disabled => disabled_label().to_string(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            cell(&view.server.name),
            cell(&view.server.transport.to_string()),
            cell(&view.server.source),
            cell(&status),
        ));
    }
    out
}

/// Shown for a server its own declaration switched off (`enabled: false`), so
/// "configured but doing nothing" is never mistaken for a broken server.
pub fn disabled_label() -> &'static str {
    "disabled (enabled: false)"
}

/// Escapes a value for a Markdown table cell.
///
/// A raw `|` or newline would split the row into extra cells/rows; both are
/// reachable from user-controlled fields (a source path, a URL).
fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}
