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
            "\n\nApprove with `tact-ui hooks trust --all`, or narrow it \
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

#[cfg(test)]
mod tests {
    use super::*;
    use tact::plugin::HookSummary;

    fn hook(source: &str, event: &str, command: &str) -> HookSummary {
        HookSummary {
            hash: format!("{source}:{event}:{command}"),
            source: source.to_string(),
            event: event.to_string(),
            matcher: None,
            command: command.to_string(),
        }
    }

    #[test]
    fn an_empty_configuration_explains_where_hooks_live() {
        let text = render_hooks_listing(&HookLoadReport::default());
        assert!(text.contains("No command hooks configured"), "{text}");
        assert!(text.contains("~/.tact/hooks.json"), "{text}");
        assert!(text.contains(".tact/hooks.json"), "{text}");
    }

    #[test]
    fn pending_hooks_are_named_before_trusted_ones() {
        let report = HookLoadReport {
            trusted: vec![hook("plugin ponytail", "SessionStart", "run-trusted.sh")],
            pending: vec![hook("~/.tact/hooks.json", "PreToolUse", "run-pending.sh")],
        };

        let text = render_hooks_listing(&report);
        let pending_at = text.find("Needs review").expect("pending section");
        let trusted_at = text.find("Trusted (runs)").expect("trusted section");
        assert!(pending_at < trusted_at, "{text}");
        assert!(text.contains("2 hook(s) configured"), "{text}");
        assert!(text.contains("1 trusted, 1 need review"), "{text}");
        // The command itself is what a reviewer is approving, so it must show.
        assert!(text.contains("run-pending.sh"), "{text}");
        assert!(text.contains("tact-ui hooks trust --all"), "{text}");
    }

    #[test]
    fn an_all_trusted_configuration_has_no_review_section() {
        let report = HookLoadReport {
            trusted: vec![hook("plugin ponytail", "SessionStart", "run.sh")],
            pending: Vec::new(),
        };

        let text = render_hooks_listing(&report);
        assert!(!text.contains("Needs review"), "{text}");
        assert!(text.contains("Trusted (runs)"), "{text}");
    }

    fn live_view(
        name: &str,
        transport: mcp::McpTransportKind,
        status: McpLiveStatus,
    ) -> mcp::McpServerView {
        mcp::McpServerView {
            server: mcp::ConfiguredServer {
                name: name.to_string(),
                transport,
                source: "~/.tact/.mcp.json".to_string(),
                disabled: false,
            },
            status,
        }
    }

    #[test]
    fn live_listing_marks_a_disabled_server() {
        let mut view = live_view(
            "switched-off",
            mcp::McpTransportKind::Stdio {
                command: "/bin/off".to_string(),
            },
            McpLiveStatus::Disabled,
        );
        view.server.disabled = true;

        let text = render_live_mcp_listing(&[view]);
        assert!(text.contains("disabled (enabled: false)"), "{text}");
    }

    #[test]
    fn live_listing_has_a_row_per_server_with_its_status() {
        let views = vec![
            live_view(
                "figma",
                mcp::McpTransportKind::Remote {
                    url: "https://mcp.figma.com/mcp".into(),
                    oauth: true,
                },
                McpLiveStatus::NeedsAuthorization,
            ),
            live_view(
                "local",
                mcp::McpTransportKind::Stdio {
                    command: "node".into(),
                },
                McpLiveStatus::Connected { tools: 4 },
            ),
            live_view(
                "broken",
                mcp::McpTransportKind::Stdio {
                    command: "nope".into(),
                },
                McpLiveStatus::NotConnected,
            ),
        ];

        let text = render_live_mcp_listing(&views);
        assert!(
            text.contains("| Server | Transport | Source | Status |"),
            "{text}"
        );
        assert!(text.contains("connected (4 tools)"), "{text}");
        assert!(
            text.contains("needs authorization — run `/mcp auth figma`"),
            "{text}"
        );
        assert!(text.contains("| broken |"), "{text}");
        assert!(text.contains("not connected"), "{text}");
    }

    #[test]
    fn live_listing_explains_how_to_configure_when_empty() {
        let text = render_live_mcp_listing(&[]);
        assert!(text.contains("No MCP servers configured."), "{text}");
        assert!(text.contains("~/.tact/.mcp.json"), "{text}");
    }

    #[test]
    fn live_listing_shows_a_plugin_server_under_its_short_name() {
        // The `plugin__<id>__` prefix is how the loader keeps plugin servers
        // apart from the user's own; in a listing it is only noise, and the
        // Source column already says which plugin it came from.
        let mut view = live_view(
            "plugin__canva__canva",
            mcp::McpTransportKind::Remote {
                url: "https://mcp.canva.com/mcp".into(),
                oauth: true,
            },
            McpLiveStatus::NeedsAuthorization,
        );
        view.server.source = "plugin openai-curated/canva".to_string();

        let text = render_live_mcp_listing(&[view]);

        assert!(text.contains("| canva |"), "{text}");
        assert!(
            text.contains("needs authorization — run `/mcp auth canva`"),
            "{text}"
        );
        assert!(!text.contains("plugin__canva__canva"), "{text}");
    }

    #[test]
    fn live_listing_escapes_pipes_so_a_source_path_cannot_break_the_table() {
        let mut view = live_view(
            "weird",
            mcp::McpTransportKind::Stdio {
                command: "/bin/echo".into(),
            },
            McpLiveStatus::Connected { tools: 1 },
        );
        view.server.source = "a|b".to_string();

        let text = render_live_mcp_listing(&[view]);
        assert!(text.contains("a\\|b"), "{text}");
    }
}
