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
            // Hosted web search is a Responses-protocol capability — it is
            // injected on every ordinary `/responses` request regardless of
            // the endpoint/provider behind the protocol.
            hosted_tools: BTreeSet::from([ResponsesToolKind::WebSearch]),
        }
    }

    /// Conservative defaults for an arbitrary OpenAI-compatible endpoint.
    pub fn custom_provider() -> Self {
        Self {
            responses: true,
            streaming: true,
            compact: false,
            // Same protocol capability: any endpoint speaking the Responses
            // protocol gets hosted web search, so the capability set matches.
            hosted_tools: BTreeSet::from([ResponsesToolKind::WebSearch]),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{ResponsesCapabilities, ResponsesToolKind};

    #[test]
    fn official_openai_defaults_to_responses_with_hosted_web_search() {
        let capabilities = ResponsesCapabilities::official_openai();
        assert!(capabilities.responses);
        assert!(capabilities.streaming);
        assert!(capabilities.compact);
        assert_eq!(
            capabilities.hosted_tools,
            BTreeSet::from([ResponsesToolKind::WebSearch])
        );
    }

    #[test]
    fn custom_provider_defaults_to_streaming_without_compact_but_with_web_search() {
        let capabilities = ResponsesCapabilities::custom_provider();
        assert!(capabilities.responses);
        assert!(capabilities.streaming);
        assert!(!capabilities.compact);
        assert_eq!(
            capabilities.hosted_tools,
            BTreeSet::from([ResponsesToolKind::WebSearch])
        );
    }

    #[test]
    fn hosted_tool_kind_is_hashable_and_ordered() {
        let mut tools = std::collections::BTreeSet::new();
        tools.insert(ResponsesToolKind::WebSearch);
        tools.insert(ResponsesToolKind::FileSearch);
        assert_eq!(tools.len(), 2);
    }
}
