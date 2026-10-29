//! LLM provider abstraction.
//!
//! Supports Anthropic (Messages API), OpenAI-compatible providers
//! (Chat Completions API), DeepSeek, and Kimi/Moonshot.

#![cfg_attr(
    not(feature = "legacy-tact"),
    allow(dead_code, unused_imports, unused_variables)
)]

#[cfg(feature = "account")]
pub mod account;
#[cfg(all(feature = "anthropic", feature = "legacy-tact"))]
pub mod anthropic;
#[cfg(all(feature = "anthropic", not(feature = "legacy-tact")))]
mod anthropic;
mod assembly;
mod auth;
mod builder;
mod client;
#[cfg(feature = "legacy-tact")]
pub mod content;
#[cfg(not(feature = "legacy-tact"))]
mod content;
#[cfg(feature = "openai-chat")]
mod convert;
mod endpoint;
mod error;
#[cfg(feature = "openai-chat")]
mod hook_select;
#[cfg(feature = "openai-chat")]
mod inject;
mod limits;
#[cfg(feature = "legacy-tact")]
pub mod lock;
mod metadata;
mod mock;
#[cfg(feature = "model-discovery")]
pub mod models;
#[cfg(feature = "legacy-tact")]
pub mod neutral;
#[cfg(not(feature = "legacy-tact"))]
mod neutral;
#[cfg(all(
    any(feature = "openai-chat", feature = "openai-responses"),
    feature = "legacy-tact"
))]
pub mod openai;
#[cfg(all(
    any(feature = "openai-chat", feature = "openai-responses"),
    not(feature = "legacy-tact")
))]
mod openai;
#[cfg(all(
    any(feature = "openai-chat", feature = "openai-responses"),
    feature = "legacy-tact"
))]
pub mod opencode;
#[cfg(all(
    any(feature = "openai-chat", feature = "openai-responses"),
    not(feature = "legacy-tact")
))]
mod opencode;
mod policy;
#[cfg(all(
    any(feature = "openai-chat", feature = "openai-responses"),
    feature = "legacy-tact"
))]
pub mod profile;
#[cfg(all(
    any(feature = "openai-chat", feature = "openai-responses"),
    not(feature = "legacy-tact")
))]
mod profile;
mod progress;
#[cfg(feature = "legacy-tact")]
pub mod provider;
mod provider_model;
mod provider_state;
mod telemetry;
#[cfg(any(
    feature = "anthropic",
    feature = "openai-chat",
    feature = "openai-responses",
    feature = "legacy-tact"
))]
mod transport;
mod types;

#[cfg(all(test, feature = "legacy-tact"))]
mod live_test_support;
#[cfg(all(test, feature = "openai", feature = "legacy-tact"))]
mod test_deepseek_reasoning;
#[cfg(all(test, feature = "openai", feature = "legacy-tact"))]
mod test_deepseek_responses;
#[cfg(all(test, feature = "openai", feature = "legacy-tact"))]
mod test_kimi_reasoning;
#[cfg(all(test, feature = "openai", feature = "legacy-tact"))]
mod test_openai;

// Re-export account query APIs at the crate root (existing public surface).
#[cfg(feature = "account")]
pub use account::{
    BalanceEntry, BalanceInfo, UsageQuotaInfo, UsageQuotaWindow, query_deepseek_balance,
    query_deepseek_balance_for, query_kimi_balance, query_kimi_balance_for, query_kimi_code_usage,
    query_kimi_code_usage_for,
};
#[cfg(feature = "legacy-tact")]
pub use auth::{
    ApiKeyProvider, CredentialKind, CredentialProvider, LegacyCredential, LegacyCredentialSource,
    legacy_credential_source,
};
pub use auth::{Authentication, Credential, CredentialError, CredentialSource};
#[cfg(feature = "anthropic")]
pub use builder::anthropic;
pub use builder::{BuildError, Provider, ProviderBuilder, TransportSetupStage};
#[cfg(feature = "openai-chat")]
pub use builder::{custom_chat, deepseek_chat, kimi_chat, openai_chat};
#[cfg(feature = "openai-responses")]
pub use builder::{custom_responses, deepseek_responses, kimi_responses, openai_responses};
#[cfg(feature = "legacy-tact")]
pub use client::{LlmClient, LlmProvider, LlmRequestBody, LlmResponse};
#[cfg(feature = "legacy-tact")]
pub use content::{
    ContentBlock, ContentBlockDelta, ImageSource, Message, MessageContent, MessageKind, Role,
    StreamUsage,
};
#[cfg(not(feature = "legacy-tact"))]
#[allow(unused_imports)]
pub(crate) use content::{
    ContentBlock, ContentBlockDelta, ImageSource, Message, MessageContent, MessageKind, Role,
    StreamUsage,
};
pub use endpoint::{Endpoint, EndpointError};
#[cfg(feature = "legacy-tact")]
pub use error::{HttpErrorClass, LlmError, MessageError};
#[cfg(feature = "legacy-tact")]
pub use hook_select::body_hook_for;
pub use limits::{LimitError, ModelLimits, SessionId, SessionIdError};
pub use metadata::{ProviderMetadata, WireProtocol};
#[cfg(any(feature = "legacy-tact", feature = "test-support", test))]
pub use mock::MockClient;
#[cfg(feature = "model-discovery")]
pub use models::{
    clear_models_cache_for_tests, ensure_api_model_ids, ensure_api_model_ids_for,
    ensure_api_model_ids_for_provider, is_models_query_supported, merge_model_candidates,
    seed_models_cache_for_tests,
};
#[cfg(all(feature = "openai-chat", feature = "legacy-tact"))]
pub use openai::compatible::body::OpenAiBodyHook;
#[cfg(feature = "legacy-tact")]
pub use profile::ProviderProfile;
#[cfg(all(
    not(feature = "legacy-tact"),
    any(feature = "openai-chat", feature = "openai-responses")
))]
#[allow(unused_imports)]
pub(crate) use profile::ProviderProfile;
#[cfg(feature = "legacy-tact")]
pub use progress::{
    ModelCallParams, NoopProgressSink, ProviderProgress, ProviderProgressSink, ThinkingChunk,
    TokenUsageInfo,
};
#[cfg(not(feature = "legacy-tact"))]
#[allow(unused_imports)]
pub(crate) use progress::{
    ModelCallParams, NoopProgressSink, ProviderProgress, ProviderProgressSink, ThinkingChunk,
    TokenUsageInfo,
};
#[cfg(feature = "legacy-tact")]
pub use provider::{
    Client, ProviderInfo, current_provider_kind, get_llm_client, get_provider, init_provider,
    init_provider_with_credentials, is_account_query_supported, is_deepseek,
    is_deepseek_balance_supported, is_kimi, is_kimi_balance_supported, is_kimi_coding, is_kimi_k2x,
    is_kimi_k3, is_kimi_k27, is_kimi_usage_supported, model_uses_effort, read_provider,
    supports_vision, try_active_credential_provider, try_get_provider, update_provider_info,
};
#[cfg(feature = "legacy-tact")]
pub use provider_model::ProviderModel;
#[cfg(feature = "legacy-tact")]
pub use provider_state::{
    ProviderConversationState, ProviderStateUpdate, ResponsesConversationState, context_hash,
};
pub use tact_agent_api::{
    AgentRole, CancelToken, ModelCallOptions, ModelCapabilities, ModelCursor, ModelError,
    ModelErrorKind, ModelEvent, ModelPort, ModelRequest, ModelResponse, ModelStream,
    ReasoningEffort, TokenUsage,
};
#[cfg(feature = "legacy-tact")]
pub use transport::SharedHttpClient;
pub use types::{ChatCompletionsDialect, ChatDialect, ProviderKind};
#[cfg(feature = "legacy-tact")]
pub use types::{
    CreateMessageParams, OpenAiProtocol, OpenAiReasoningEffort, RequiredMessageParams, StopReason,
    Thinking, ThinkingType, Tool, ToolChoice,
};
#[cfg(not(feature = "legacy-tact"))]
#[allow(unused_imports)]
pub(crate) use types::{
    CreateMessageParams, OpenAiProtocol, OpenAiReasoningEffort, RequiredMessageParams, StopReason,
    Thinking, ThinkingType, Tool, ToolChoice,
};
