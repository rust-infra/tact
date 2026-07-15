//! Provider-agnostic LLM request types.
//!
//! [`CreateMessageParams`] is Tact's shared request model. Adapters translate it
//! to Anthropic Messages or OpenAI Chat Completions wire formats.
//!
use serde::{Deserialize, Serialize};

use crate::Message;

/// Required fields for constructing a message request.
#[derive(Debug, Clone)]
pub struct RequiredMessageParams {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

/// Parameters for creating / streaming a model turn.
///
/// Serde shape matches the Anthropic Messages API so the Anthropic adapter can
/// serialize this struct directly. OpenAI-compatible adapters use `convert.rs`
/// and inject provider-specific fields (e.g. `reasoning_effort`) separately.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CreateMessageParams {
    pub max_tokens: u32,
    pub messages: Vec<Message>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
}

impl From<RequiredMessageParams> for CreateMessageParams {
    fn from(required: RequiredMessageParams) -> Self {
        Self {
            model: required.model,
            messages: required.messages,
            max_tokens: required.max_tokens,
            ..Default::default()
        }
    }
}

impl CreateMessageParams {
    pub fn new(required: RequiredMessageParams) -> Self {
        required.into()
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.stop_sequences = Some(stop_sequences);
        self
    }

    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }

    pub fn with_top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    pub fn with_thinking(mut self, thinking: Thinking) -> Self {
        self.thinking = Some(thinking);
        self
    }
}

/// Tool definition sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// How the model should use tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "any")]
    Any,
    #[serde(rename = "tool")]
    Tool { name: String },
    #[serde(rename = "none")]
    None,
}

/// Configuration for extended thinking / reasoning.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Thinking {
    /// Anthropic-style token budget (OpenAI maps this to `reasoning_effort`).
    pub budget_tokens: usize,
    #[serde(rename = "type")]
    pub type_: ThinkingType,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ThinkingType {
    #[serde(rename = "enabled")]
    Enabled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, Role};

    #[test]
    fn builder_sets_optional_fields() {
        let params = CreateMessageParams::new(RequiredMessageParams {
            model: "m".into(),
            messages: vec![Message::new_text(Role::User, "hi")],
            max_tokens: 100,
        })
        .with_system("sys")
        .with_stream(true)
        .with_thinking(Thinking {
            budget_tokens: 1024,
            type_: ThinkingType::Enabled,
        });

        assert_eq!(params.system.as_deref(), Some("sys"));
        assert_eq!(params.stream, Some(true));
        assert_eq!(params.thinking.as_ref().unwrap().budget_tokens, 1024);
    }

    #[test]
    fn serde_matches_anthropic_thinking_shape() {
        let params = CreateMessageParams::new(RequiredMessageParams {
            model: "claude".into(),
            messages: vec![],
            max_tokens: 10,
        })
        .with_thinking(Thinking {
            budget_tokens: 2048,
            type_: ThinkingType::Enabled,
        });
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["thinking"]["budget_tokens"], 2048);
        assert!(json.get("system").is_none());
    }
}
