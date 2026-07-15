//! Provider-agnostic conversation content types.
//!
//! [`ContentBlock`], [`Message`], and related types are Tact-owned (same
//! Anthropic Messages *wire shape* for serde). Stream helpers
//! ([`ContentBlockDelta`], [`StreamUsage`]) match the Messages SSE schema so
//! the Anthropic adapter can deserialize events without the upstream SDK.

use serde::{Deserialize, Serialize};

/// Role of a message sender.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Content of a message.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum MessageContent {
    Text { content: String },
    Blocks { content: Vec<ContentBlock> },
}

/// Content block in a message.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
}

/// Source of an image attachment.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub type_: String,
    pub media_type: String,
    pub data: String,
}

/// Message in a conversation.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: Role,
    #[serde(flatten)]
    pub content: MessageContent,
}

impl Message {
    pub fn new_text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: MessageContent::Text {
                content: text.into(),
            },
        }
    }

    pub fn new_blocks(role: Role, blocks: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content: MessageContent::Blocks { content: blocks },
        }
    }
}

/// Incremental update inside a Messages API `content_block_delta` SSE event.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ContentBlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

/// Token usage attached to streaming `message_delta` events.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct StreamUsage {
    #[serde(default)]
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_block_serde_tag() {
        let block = ContentBlock::ToolUse {
            id: "1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["name"], "bash");
    }

    #[test]
    fn message_flatten_text() {
        let msg = Message::new_text(Role::User, "hi");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hi");
    }

    #[test]
    fn content_block_delta_serde() {
        let delta = ContentBlockDelta::TextDelta { text: "hi".into() };
        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "hi");
        let round: ContentBlockDelta = serde_json::from_value(json).unwrap();
        assert_eq!(round, ContentBlockDelta::TextDelta { text: "hi".into() });
    }
}
