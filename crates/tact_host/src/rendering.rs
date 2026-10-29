//! Presentation helpers shared by host commands.
//!
//! These remain strings because the current [`tact_protocol::AgentUpdate`]
//! command results are text. Desktop adapters can render the same structured
//! data themselves once a semantic host event replaces `MdInfo`.

use tact::{
    mcp::{self, McpLiveStatus},
    plugin::HookLoadReport,
};

pub fn render_live_mcp_listing(views: &[mcp::McpServerView]) -> String {
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
        let name = mcp::display_server_name(&view.server.name);
        let status = match view.status {
            McpLiveStatus::Connected { tools } => format!("connected ({tools} tools)"),
            McpLiveStatus::NeedsAuthorization => {
                format!("needs authorization — run `/mcp auth {name}`")
            }
            McpLiveStatus::NotConnected => "not connected".to_string(),
            McpLiveStatus::Disabled => "disabled (enabled: false)".to_string(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            cell(name),
            cell(&view.server.transport.to_string()),
            cell(&view.server.source),
            cell(&status),
        ));
    }
    out
}

pub fn render_hooks_listing(report: &HookLoadReport) -> String {
    let total = report.trusted.len() + report.pending.len();
    if total == 0 {
        return "No command hooks configured.\n\n\
                Declare them in `~/.tact/hooks.json` or `.tact/hooks.json` \
                (Codex's `hooks.json` schema), or install a plugin that ships them."
            .to_string();
    }

    let mut out = format!("{total} hook(s) configured ({total} = trusted + pending):");
    out.push_str(&format!(
        "\n  {} trusted, {} need review",
        report.trusted.len(),
        report.pending.len()
    ));

    if !report.pending.is_empty() {
        out.push_str("\n\nNeeds review (not run):\n");
        out.push_str(
            &report
                .pending
                .iter()
                .map(|hook| format!("  {}", hook.describe()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        out.push_str(
            "\n\nApprove with `hooks trust --all`, or narrow it \
             with `--source <label>`.",
        );
    }

    if !report.trusted.is_empty() {
        out.push_str("\n\nTrusted (runs):\n");
        out.push_str(
            &report
                .trusted
                .iter()
                .map(|hook| format!("  {}", hook.describe()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    out
}

fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}
