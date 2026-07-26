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
    #[error("openai error: {0}")]
    OpenAi(#[from] async_openai::error::OpenAIError),
    /// OpenAI Responses API error.
    #[error("openai responses error: {0}")]
    OpenAiResponses(#[from] async_openai_responses::error::OpenAIError),
    /// JSON serialization/deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The API responded with an error (HTTP-level).
    #[error("api error ({status}): {body}")]
    HttpError { status: u16, body: String },
    /// Stream parsing error.
    #[error("stream error: {0}")]
    StreamParse(String),
    /// Unsupported response state.
    #[error("unsupported response state: {0}")]
    Unsupported(String),
    /// Unsupported hook for provider.
    #[error("unsupported hook for provider: {0}")]
    UnsupportedHook(String),
    /// Placeholder for test mocks.
    #[error("{0}")]
    Mock(String),
}
