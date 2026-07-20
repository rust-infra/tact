//! LLM error types.

use std::fmt;

/// Anthropic / Messages-adapter failures (HTTP, parse, API body).
#[derive(Debug)]
pub enum MessageError {
    ApiError(String),
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::ApiError(msg) = self;
        f.write_str(msg)
    }
}

impl std::error::Error for MessageError {}

impl From<String> for MessageError {
    fn from(error: String) -> Self {
        MessageError::ApiError(error)
    }
}

/// Unified error type for LLM operations.
#[derive(Debug)]
pub enum LlmError {
    Anthropic(MessageError),
    OpenAi(async_openai::error::OpenAIError),
    OpenAiResponses(async_openai_responses::error::OpenAIError),
    Other(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Anthropic(e) => write!(f, "Anthropic error: {e}"),
            LlmError::OpenAi(e) => write!(f, "OpenAI error: {e}"),
            LlmError::OpenAiResponses(e) => write!(f, "OpenAI Responses error: {e}"),
            LlmError::Other(s) => write!(f, "LLM error: {s}"),
        }
    }
}

impl std::error::Error for LlmError {}

impl From<MessageError> for LlmError {
    fn from(e: MessageError) -> Self {
        LlmError::Anthropic(e)
    }
}

impl From<async_openai::error::OpenAIError> for LlmError {
    fn from(e: async_openai::error::OpenAIError) -> Self {
        LlmError::OpenAi(e)
    }
}

impl From<async_openai_responses::error::OpenAIError> for LlmError {
    fn from(e: async_openai_responses::error::OpenAIError) -> Self {
        LlmError::OpenAiResponses(e)
    }
}
