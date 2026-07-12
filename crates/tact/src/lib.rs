//! Tact — the agent runtime crate.
//!
//! This crate implements the core agent loop: it manages conversation context,
//! dispatches tool calls (native and MCP), enforces permission policies,
//! handles context compaction, and integrates with the TUI frontend for
//! streaming output and user interaction.
//!
//! # Key concepts
//!
//! - [`Agent`] owns the message history, tool router, MCP router, and hooks.
//! - [`AgentRuntime`] carries the Anthropic client, context window state, and
//!   recovery/permission state.
//! - [`Agent::agent_loop`] is the main conversation loop: it sends messages to the LLM,
//!   processes tool-use blocks, applies permissions, and writes results back.
//! - Module [`tool`] defines the [`Tool`] trait, the [`ToolRouter`], and
//!   registers all built-in tools.
//! - Module [`hook`] provides pre/post tool-use and session-start hooks.
//! - Module [`compact`] handles context compaction and transcript persistence.
//! - Module [`permission`] classifies tool risk and enforces approval policies.
//! - Module [`notifications`] sends macOS desktop notifications for task lifecycle events.

pub mod agent;
pub mod background;
pub mod compact;
pub mod config;
pub mod consts;
pub mod cron;
pub mod hook;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod notifications;
pub mod permission;
pub mod prompt;
pub mod recovery;
pub(crate) mod shell;
pub mod skill;
pub mod stats;
pub mod store;
pub mod task;
pub mod team;
pub mod tool;
pub mod worktree;

pub use agent::{Agent, AgentRuntime, AgentSystemPrompt};
pub use tact_llm::Tool as ToolSpec;

use tact_llm::{ContentBlock, LlmProvider, MessageContent};

/// Returns the model name from the active provider's environment variable.
/// Parsed once on first call and cached for the lifetime of the process.
pub fn get_model() -> &'static str {
    tact_llm::get_provider().model.as_str()
}

/// Constructs the active LLM client from the installed configuration.
pub fn get_llm_client() -> anyhow::Result<LlmProvider> {
    tact_llm::get_llm_client()
}

pub type LoopState = Agent;

/// Extracts plain text from a [`MessageContent`] block.
///
/// For `Text` content returns the string directly; for `Blocks` content
/// joins all text blocks with newlines.
pub fn extract_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text { content } => content.clone(),
        MessageContent::Blocks { content } => content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
