use std::collections::BTreeSet;

/// Responses tool families recognized by the wire adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResponsesToolKind {
    WebSearch,
    FileSearch,
    CodeInterpreter,
    ImageGeneration,
    Computer,
    LocalShell,
    Shell,
    Custom,
    Namespace,
    ApplyPatch,
    ToolSearch,
    RemoteMcp,
}

/// Capabilities available on a Responses endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsesCapabilities {
    pub responses: bool,
    pub streaming: bool,
    pub compact: bool,
    pub hosted_tools: BTreeSet<ResponsesToolKind>,
}

impl ResponsesCapabilities {
    /// Capabilities currently implemented for the official OpenAI adapter.
    pub fn official_openai() -> Self {
        Self {
            responses: true,
            streaming: true,
            compact: true,
            hosted_tools: BTreeSet::new(),
        }
    }

    /// Conservative defaults for an arbitrary OpenAI-compatible endpoint.
    pub fn custom_provider() -> Self {
        Self {
            responses: true,
            streaming: true,
            compact: false,
            hosted_tools: BTreeSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ResponsesCapabilities, ResponsesToolKind};

    #[test]
    fn official_openai_defaults_to_core_responses_only() {
        let capabilities = ResponsesCapabilities::official_openai();
        assert!(capabilities.responses);
        assert!(capabilities.streaming);
        assert!(capabilities.compact);
        assert!(capabilities.hosted_tools.is_empty());
    }

    #[test]
    fn custom_provider_defaults_to_core_streaming_without_hosted_tools() {
        let capabilities = ResponsesCapabilities::custom_provider();
        assert!(capabilities.responses);
        assert!(capabilities.streaming);
        assert!(!capabilities.compact);
        assert!(capabilities.hosted_tools.is_empty());
    }

    #[test]
    fn hosted_tool_kind_is_hashable_and_ordered() {
        let mut tools = std::collections::BTreeSet::new();
        tools.insert(ResponsesToolKind::WebSearch);
        tools.insert(ResponsesToolKind::FileSearch);
        assert_eq!(tools.len(), 2);
    }
}
