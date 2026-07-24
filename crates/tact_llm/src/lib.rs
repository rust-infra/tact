//! LLM provider abstraction.
//!
//! Supports Anthropic (Messages API), OpenAI-compatible providers
//! (Chat Completions API), DeepSeek, and Kimi/Moonshot.

pub mod account;
pub mod anthropic;
pub mod client;
pub mod content;
pub mod convert;
pub mod deepseek;
pub mod error;
pub mod hook_select;
pub mod inject;
pub mod kimi;
pub mod mock;
pub mod models;
pub mod openai;
pub mod provider;
pub mod types;

#[cfg(test)]
mod test_deepseek_reasoning;
#[cfg(test)]
mod test_kimi_reasoning;
#[cfg(test)]
mod test_openai;

// Re-export account query APIs at the crate root (existing public surface).
pub use account::{query_deepseek_balance, query_kimi_balance, query_kimi_code_usage};
pub use client::{LlmClient, LlmProvider, LlmRequestBody};
pub use content::{
    ContentBlock, ContentBlockDelta, ImageSource, Message, MessageContent, Role, StreamUsage,
};
pub use error::{LlmError, MessageError};
pub use hook_select::body_hook_for;
pub use mock::MockClient;
pub use models::{
    clear_models_cache_for_tests, ensure_api_model_ids, is_models_query_supported,
    merge_model_candidates, seed_models_cache_for_tests,
};
pub use openai::{current_reasoning_effort_from_budget, reasoning_effort_from_budget};
pub use provider::{
    ProviderInfo, get_llm_client, get_provider, init_provider, is_account_query_supported,
    is_deepseek, is_kimi, is_kimi_balance_supported, is_kimi_coding, is_kimi_k2x, is_kimi_k27,
    is_kimi_usage_supported, read_provider, set_model, supports_vision,
};
pub use types::{
    CreateMessageParams, OpenAiProtocol, OpenAiReasoningEffort, ProviderKind,
    RequiredMessageParams, StopReason, Thinking, ThinkingType, Tool, ToolChoice,
    effective_reasoning_effort,
};
