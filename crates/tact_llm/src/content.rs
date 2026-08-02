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

/// Kind / origin of a message cell.
///
/// `Normal` is the default for real turns. `Summary` marks a system-generated
/// compaction handoff so it can be detected by type instead of by string
/// matching, and rendered / handled specially by callers (TUI, summarizer,
/// rebuild filters).
///
/// The kind is **in-memory only**: the field is `#[serde(skip)]`, so the wire
/// format (Anthropic messages, OpenAI conversion, JSONL transcripts) never
/// carries it. Persisted sessions reload as `Normal` and fall back to the
/// `SUMMARY_PREFIX` / `<context-handoff>` string detection in
/// `crates/tact/src/compact`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Real user / assistant turn.
    #[default]
    Normal,
    /// System-generated compaction handoff, not a real user turn.
    Summary,
}

/// Message in a conversation.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: Role,
    #[serde(flatten)]
    pub content: MessageContent,
    /// In-memory cell marker; never serialized (see [`MessageKind`]).
    #[serde(skip)]
    pub kind: MessageKind,
}

impl Message {
    pub fn new_text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: MessageContent::Text {
                content: text.into(),
            },
            kind: MessageKind::Normal,
        }
    }

    pub fn new_blocks(role: Role, blocks: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content: MessageContent::Blocks { content: blocks },
            kind: MessageKind::Normal,
        }
    }

    /// Marks this cell with a [`MessageKind`] (builder style).
    pub fn with_kind(mut self, kind: MessageKind) -> Self {
        self.kind = kind;
        self
    }

    /// Returns the cell's [`MessageKind`].
    pub fn kind(&self) -> MessageKind {
        self.kind
    }

    /// True for system-generated compaction handoff cells.
    pub fn is_summary(&self) -> bool {
        self.kind == MessageKind::Summary
    }

    /// Returns true if this message contains any `ContentBlock::Image`.
    ///
    /// Useful for gating vision-only features before the LLM rejects them.
    pub fn has_images(&self) -> bool {
        match &self.content {
            MessageContent::Text { .. } => false,
            MessageContent::Blocks { content } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. })),
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
        assert!(json.get("kind").is_none());
    }

    #[test]
    fn message_kind_is_in_memory_only_and_defaults_to_normal_on_wire() {
        let summary = Message::new_text(Role::User, "handoff").with_kind(MessageKind::Summary);
        assert!(summary.is_summary());
        assert_eq!(summary.kind(), MessageKind::Summary);

        // The kind marker must never leak onto the wire / transcript format.
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "handoff");
        assert!(json.get("kind").is_none());

        // Reloading from a serialized form yields a Normal cell; callers fall
        // back to content-based detection (e.g. SUMMARY_PREFIX / tags).
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back.kind(), MessageKind::Normal);
        assert!(!back.is_summary());
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
