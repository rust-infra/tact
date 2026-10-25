//! How a tool name is classified before the catalog is consulted.
//!
//! The dispatch path has to decide *which* resolver to ask before it can ask any:
//! a name could be a built-in, an MCP server tool, or one of Tact's own MCP
//! resource/prompt tools. That decision reads only the name, and it used to be
//! spelled with the MCP types directly — which is what kept an `agent` → `mcp`
//! edge alive for a question about strings.
//!
//! This module is the neutral answer: a closed enum, derived from the name alone.

/// Which family a tool name belongs to, before any lookup.
///
/// Ordered by specificity: a name can look like an MCP tool (`mcp__server__tool`)
/// and also be one of the control tools, so the control names are checked first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolNameClass {
    /// One of Tact's MCP resource tools (`list_mcp_resources`, …).
    McpResource,
    /// One of Tact's MCP prompt tools (`list_mcp_prompts`, …).
    McpPrompt,
    /// A server-provided MCP tool (`mcp__<server>__<tool>`).
    McpServerTool,
    /// Anything else: the built-in router's business.
    Builtin,
}

impl ToolNameClass {
    /// Classify `name`.
    ///
    /// A pure function of the string, so the dispatch path can ask it without
    /// holding a router — and a test can pin it without one either.
    #[must_use]
    pub fn of(name: &str) -> Self {
        // The control tools first: their names are plain (`read_mcp_resource`),
        // so a later check could not tell them apart from a built-in.
        if crate::mcp::McpResourceTool::from_name(name).is_some() {
            return Self::McpResource;
        }
        if crate::mcp::McpPromptTool::from_name(name).is_some() {
            return Self::McpPrompt;
        }
        if name.starts_with("mcp__") {
            return Self::McpServerTool;
        }
        Self::Builtin
    }

    /// Whether this class is served through the MCP router.
    #[must_use]
    pub fn is_mcp(self) -> bool {
        !matches!(self, Self::Builtin)
    }
}

/// Whether a control tool addresses one server, so its work can be scoped to it.
///
/// A listing (`list_mcp_resources`, `list_mcp_prompts`) may touch every server and
/// is therefore a barrier; a read/get names one. The dispatch path needs the
/// answer to schedule work, which is why it lives beside the classifier rather
/// than in the tool implementation.
#[must_use]
pub fn scopes_to_one_server(name: &str) -> bool {
    matches!(name, "read_mcp_resource" | "get_mcp_prompt")
}

/// Classify `name` without naming the MCP types at the call site.
#[must_use]
pub fn classify(name: &str) -> ToolNameClass {
    ToolNameClass::of(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_tool_is_recognised_before_its_prefix_is_considered() {
        // `read_mcp_resource` has no `mcp__` prefix, so only the control lookup
        // can find it; getting this order wrong would classify it as a built-in.
        assert_eq!(classify("read_mcp_resource"), ToolNameClass::McpResource);
        assert_eq!(classify("list_mcp_prompts"), ToolNameClass::McpPrompt);
    }

    #[test]
    fn a_server_tool_is_recognised_by_its_prefix() {
        assert_eq!(classify("mcp__demo__echo"), ToolNameClass::McpServerTool);
        assert!(classify("mcp__demo__echo").is_mcp());
    }

    #[test]
    fn everything_else_belongs_to_the_builtin_router() {
        for name in ["bash", "read_file", "compact", "mcp_", "mcp"] {
            assert_eq!(classify(name), ToolNameClass::Builtin, "{name}");
            assert!(!classify(name).is_mcp(), "{name}");
        }
    }

    #[test]
    fn only_a_read_or_a_get_is_scoped_to_one_server() {
        // A listing spans every server, so scheduling it as one server's work
        // would run it concurrently with a listing of a different one.
        assert!(scopes_to_one_server("read_mcp_resource"));
        assert!(scopes_to_one_server("get_mcp_prompt"));
        assert!(!scopes_to_one_server("list_mcp_resources"));
        assert!(!scopes_to_one_server("list_mcp_prompts"));
        assert!(!scopes_to_one_server("bash"));
    }

    #[test]
    fn every_mcp_class_reports_itself_as_mcp() {
        for (name, expected) in [
            ("read_mcp_resource", ToolNameClass::McpResource),
            ("get_mcp_prompt", ToolNameClass::McpPrompt),
            ("mcp__s__t", ToolNameClass::McpServerTool),
        ] {
            assert_eq!(classify(name), expected, "{name}");
            assert!(classify(name).is_mcp(), "{name}");
        }
    }
}
