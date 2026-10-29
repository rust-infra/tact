//! OpenAI LLM adapters.
//!
//! Two protocol families live under this module:
//!
//! - [`compatible`] — OpenAI-compatible Chat Completions (OpenAI official,
//!   DeepSeek, Kimi, custom OpenAI-compatible endpoints).
//! - [`responses`] — the OpenAI Responses protocol.
//!
//! Public items are re-exported here so callers can keep using
//! `tact_llm::openai::…` paths.

#[cfg(feature = "openai-chat")]
pub mod compatible;
#[cfg(feature = "openai-responses")]
pub mod responses;

#[cfg(feature = "openai-chat")]
pub use compatible::{
    ChatCompletionsAdapter, CompatibleConfig, CreateChatCompletionRequest, OpenAiAdapter,
    body::{BodyHookCtx, DeepSeekBodyHook, KimiBodyHook, OpenAiBodyHook, StandardOpenAiBodyHook},
};
