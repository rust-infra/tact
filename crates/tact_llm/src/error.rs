//! LLM error types.

use std::time::Duration;

use thiserror::Error;

/// Anthropic / Messages-adapter failures (HTTP, parse, API body).
#[derive(Debug, Error)]
pub enum MessageError {
    #[error("{0}")]
    ApiError(String),
}

impl From<String> for MessageError {
    fn from(error: String) -> Self {
        Self::ApiError(error)
    }
}

/// Unified error type for LLM operations.
#[derive(Debug, Error)]
pub enum LlmError {
    /// Anthropic adapter error.
    #[error("anthropic error: {0}")]
    Anthropic(#[from] MessageError),
    /// OpenAI chat completions error.
    #[cfg(feature = "openai-chat")]
    #[error("openai error: {0}")]
    OpenAi(#[from] async_openai::error::OpenAIError),
    /// OpenAI Responses API error.
    #[cfg(feature = "openai-responses")]
    #[error("openai responses error: {0}")]
    OpenAiResponses(#[from] async_openai_responses::error::OpenAIError),
    /// JSON serialization/deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The API responded with an error (HTTP-level).
    #[error("api error ({status}): {class:?}")]
    HttpError {
        status: u16,
        class: HttpErrorClass,
        retry_after: Option<Duration>,
    },
    /// Request transport failure (send, connection, or response read).
    #[error("request error: {0}")]
    Request(String),
    /// Stream parsing error.
    #[error("stream error: {0}")]
    StreamParse(String),
    /// Unsupported response state.
    #[error("unsupported response state: {0}")]
    Unsupported(String),
    /// Unsupported hook for provider.
    #[error("unsupported hook for provider: {0}")]
    UnsupportedHook(String),
    /// Credential resolution / authentication failure.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Placeholder for test mocks.
    #[error("{0}")]
    Mock(String),
}

/// Safe classification derived from an HTTP status and bounded body markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpErrorClass {
    Authentication,
    ContextOverflow,
    Transient,
    Permanent,
}

impl HttpErrorClass {
    /// Stable, body-free label for user-facing text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Authentication => "authentication failed",
            Self::ContextOverflow => "context too large",
            Self::Transient => "temporarily unavailable",
            Self::Permanent => "request rejected",
        }
    }
}

impl LlmError {
    /// Whether resending the same request can plausibly succeed unchanged.
    ///
    /// This is the recovery decision, not the diagnostic: a `400` stays false
    /// even when its body reads like a rate limit, and a `429` stays true.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::HttpError { class, .. } => *class == HttpErrorClass::Transient,
            Self::Request(_) => true,
            _ => false,
        }
    }

    /// Body-free description that is safe to show a user.
    ///
    /// The provider's own text never appears here: an error body can echo the
    /// prompt, a credential, or a gateway's HTML page, and a transport error's
    /// `Display` embeds the request URL (which a proxy may key on). What is
    /// kept is the part that is metadata and that a user acts on — the HTTP
    /// status and its class. Variants whose text Tact authors itself
    /// (`Auth`, `Unsupported`, `Mock`) are passed through, because those
    /// strings contain no provider payload and dropping them would leave the
    /// user with nothing to fix.
    pub fn user_message(&self) -> String {
        match self {
            Self::HttpError { status, class, .. } => {
                format!("provider returned HTTP {status} ({})", class.label())
            }
            Self::Anthropic(_) => "provider error (invalid or interrupted response)".to_string(),
            #[cfg(feature = "openai-chat")]
            Self::OpenAi(_) => "provider error (invalid or interrupted response)".to_string(),
            #[cfg(feature = "openai-responses")]
            Self::OpenAiResponses(_) => {
                "provider error (invalid or interrupted response)".to_string()
            }
            Self::Serialization(_) => "provider response was not valid JSON for this protocol; \
                 the endpoint may not be an API"
                .to_string(),
            Self::Request(_) => "connection to the provider failed or was interrupted".to_string(),
            Self::StreamParse(_) => "provider stream was malformed or ended early".to_string(),
            Self::Unsupported(message) | Self::UnsupportedHook(message) => message.clone(),
            Self::Auth(message) => message.clone(),
            Self::Mock(message) => message.clone(),
        }
    }
}
