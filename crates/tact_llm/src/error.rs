//! LLM error types.

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
    ///
    /// Boxed because the raw variant is too large to pass through every
    /// `Result<_, LlmError>` in this crate by value: with the GUI crates in
    /// the same build feature unification enables `serde_json/preserve_order`,
    /// which grows this variant past 128 bytes and trips
    /// `clippy::result_large_err` on 46 signatures.
    #[error("openai error: {0}")]
    OpenAi(Box<async_openai::error::OpenAIError>),
    /// OpenAI Responses API error. Boxed for the same reason as
    /// [`LlmError::OpenAi`].
    #[error("openai responses error: {0}")]
    OpenAiResponses(Box<async_openai_responses::error::OpenAIError>),
    /// JSON serialization/deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The API responded with an error (HTTP-level).
    #[error("api error ({status}): {body}")]
    HttpError { status: u16, body: String },
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
impl From<async_openai::error::OpenAIError> for LlmError {
    fn from(error: async_openai::error::OpenAIError) -> Self {
        Self::OpenAi(Box::new(error))
    }
}

impl From<async_openai_responses::error::OpenAIError> for LlmError {
    fn from(error: async_openai_responses::error::OpenAIError) -> Self {
        Self::OpenAiResponses(Box::new(error))
    }
}
