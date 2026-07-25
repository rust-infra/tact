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
    /// Catch-all for other LLM failures.
    #[error("llm error: {0}")]
    Other(String),
}
