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

    /// Authority of the configured endpoint: host, plus a non-default port.
    ///
    /// Deliberately not the whole URL. A self-hosted proxy commonly carries a
    /// per-user path (`/v1/<key>/...`), so a diagnostic that echoed the path
    /// would put a credential in a user-visible error. The host is what
    /// distinguishes "my gateway" from "the vendor", which is the part a user
    /// acts on.
    pub fn endpoint_host(&self) -> String {
        let Ok(url) = url::Url::parse(self.endpoint.as_str()) else {
            return String::new();
        };
        let Some(host) = url.host_str() else {
            return String::new();
        };
        match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        }
    }

    pub fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    pub fn supports_vision(&self) -> bool {
        self.supports_vision
    }
}
