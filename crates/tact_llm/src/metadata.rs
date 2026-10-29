//! Stable provider metadata and wire-protocol vocabulary.

use tact_agent_api::ModelCapabilities;

use crate::{Endpoint, ProviderKind};

/// The wire protocol selected by a stable provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WireProtocol {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
}

impl WireProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic_messages",
            Self::OpenAiChatCompletions => "openai_chat_completions",
            Self::OpenAiResponses => "openai_responses",
        }
    }
}

/// Facts fixed by a successful provider build.
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    provider: ProviderKind,
    protocol: WireProtocol,
    model: String,
    endpoint: Endpoint,
    capabilities: ModelCapabilities,
    supports_vision: bool,
}

impl ProviderMetadata {
    pub(crate) fn new(
        provider: ProviderKind,
        protocol: WireProtocol,
        model: String,
        endpoint: Endpoint,
        capabilities: ModelCapabilities,
        supports_vision: bool,
    ) -> Self {
        Self {
            provider,
            protocol,
            model,
            endpoint,
            capabilities,
            supports_vision,
        }
    }

    pub fn provider(&self) -> &ProviderKind {
        &self.provider
    }

    pub fn protocol(&self) -> WireProtocol {
        self.protocol
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    pub fn supports_vision(&self) -> bool {
        self.supports_vision
    }
}
