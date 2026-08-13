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

pub mod compatible;
pub mod responses;

pub use compatible::{
    ChatCompletionsAdapter, CompatibleConfig, CreateChatCompletionRequest, OpenAiAdapter,
    body::{BodyHookCtx, DeepSeekBodyHook, KimiBodyHook, OpenAiBodyHook, StandardOpenAiBodyHook},
};
