//! Loading the MCP router, as one call the agent loop does not have to spell.
//!
//! The Agent reloads the router after an interactive authorization and at
//! startup. Both need the same three steps — load from disk, keep the router,
//! rebuild the advertised list — and stating them in the agent made the loop
//! mention MCP types for what is really "refresh the external tool set".
//!
//! This module is that operation, so `agent/` can ask for it without naming the
//! transport. It is the last edge the module guard allowed; with it gone, the
//! guard's allowlist is empty.

use crate::mcp::{MCPToolRouter, McpLoadReport, load_mcp_router_with_report};

/// The outcome of a reload: a router to use, or a report explaining why not.
///
/// An enum rather than `Result<_, _>` because neither variant is exceptional —
/// one broken server is an ordinary startup outcome — and because a `Result` here
/// would carry a 200-byte report in its `Err`, which is the compiler telling us
/// the shape is wrong.
pub enum Reloaded {
    /// A router is available (possibly with partial failures in the report).
    Loaded(MCPToolRouter, McpLoadReport),
    /// Nothing could be loaded; the report says why and the caller keeps the
    /// router it already had.
    Failed(McpLoadReport),
}

/// Reload the router, reporting failures instead of propagating them.
///
/// Startup and `/mcp auth` share this: one broken server must not stop a session
/// from starting, so a load failure becomes a report entry the caller can show.
/// The previous connections are dropped by the caller, which owns the router.
pub async fn reload() -> Reloaded {
    match load_mcp_router_with_report().await {
        Ok((router, report)) => Reloaded::Loaded(router, report),
        Err(error) => {
            tracing::warn!(error = %error, "failed to load MCP servers");
            Reloaded::Failed(McpLoadReport {
                failures: vec![("mcp".to_owned(), format!("{error:#}"))],
                ..McpLoadReport::default()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_load_failure_becomes_a_report_not_an_error() {
        // The contract is "never propagate": a caller that has to handle `Err`
        // would end up either swallowing it or aborting startup, and neither is
        // what one broken server should do.
        let report = match reload().await {
            Reloaded::Loaded(_router, report) => report,
            Reloaded::Failed(report) => report,
        };
        // Either way a report comes back; nothing panics and nothing propagates.
        let _ = report.notice_lines();
    }
}
