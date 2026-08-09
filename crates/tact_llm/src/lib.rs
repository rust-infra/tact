//! LLM provider abstraction.
//!
//! Supports Anthropic (Messages API), OpenAI-compatible providers
//! (Chat Completions API), DeepSeek, and Kimi/Moonshot.

pub mod account;
pub mod anthropic;
pub mod auth;
pub mod client;
pub mod content;
pub mod convert;
pub mod error;
pub mod hook_select;
pub mod inject;
pub mod mock;
pub mod models;
pub mod openai;
pub mod profile;
pub mod provider;
pub mod provider_state;
pub mod transport;
pub mod types;

#[cfg(test)]
mod test_deepseek_reasoning;
#[cfg(test)]
mod test_deepseek_responses;
#[cfg(test)]
mod test_kimi_reasoning;
#[cfg(test)]
mod test_openai;

// Re-export account query APIs at the crate root (existing public surface).
pub use account::{
    query_deepseek_balance, query_deepseek_balance_for, query_kimi_balance, query_kimi_balance_for,
    query_kimi_code_usage, query_kimi_code_usage_for,
};
pub use auth::{ApiKeyProvider, Credential, CredentialProvider};
pub use client::{LlmClient, LlmProvider, LlmRequestBody, LlmResponse};
pub use content::{
    ContentBlock, ContentBlockDelta, ImageSource, Message, MessageContent, MessageKind, Role,
    StreamUsage,
};
pub use error::{LlmError, MessageError};
pub use hook_select::body_hook_for;
pub use mock::MockClient;
pub use models::{
    clear_models_cache_for_tests, ensure_api_model_ids, ensure_api_model_ids_for,
    ensure_api_model_ids_for_provider, is_models_query_supported, merge_model_candidates,
    seed_models_cache_for_tests,
};
pub use openai::OpenAiBodyHook;
pub use profile::ProviderProfile;
pub use provider::{
    Client, ProviderInfo, get_llm_client, get_provider, init_provider,
    init_provider_with_credentials, is_account_query_supported, is_deepseek,
    is_deepseek_balance_supported, is_kimi, is_kimi_balance_supported, is_kimi_coding, is_kimi_k2x,
    is_kimi_k3, is_kimi_k27, is_kimi_usage_supported, model_uses_effort, read_provider,
    supports_vision,
};
pub use provider_state::{
    ProviderConversationState, ProviderStateUpdate, ResponsesConversationState, context_hash,
};
pub use transport::SharedHttpClient;
pub use types::{
    ChatCompletionsDialect, CreateMessageParams, OpenAiProtocol, OpenAiReasoningEffort,
    ProviderKind, RequiredMessageParams, StopReason, Thinking, ThinkingType, Tool, ToolChoice,
};
