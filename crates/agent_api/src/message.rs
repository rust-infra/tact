use serde::{Deserialize, Serialize};

/// Stable logical identity for one assistant model response.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

impl MessageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Provider-neutral sender role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    User,
    Assistant,
}

/// Why a logical message exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageOrigin {
    User,
    Assistant,
    ToolResult,
    InjectedContext { source: String },
    CompactionSummary,
    GoalContext,
    SubagentResult,
    Continuation,
}

impl MessageOrigin {
    pub fn is_user_turn(&self) -> bool {
        matches!(self, Self::User)
    }
}

/// Source of an image attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentImageSource {
    #[serde(rename = "type")]
    pub type_: String,
    pub media_type: String,
    pub data: String,
}

/// Provider-neutral content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AgentImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

/// Logical transcript message. `origin` is persistence metadata and never
/// changes the provider-neutral role used for model conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: AgentRole,
    pub origin: MessageOrigin,
    pub content: Vec<AgentContentBlock>,
}

impl AgentMessage {
    pub fn new(role: AgentRole, origin: MessageOrigin, content: Vec<AgentContentBlock>) -> Self {
        Self {
            role,
            origin,
            content,
        }
    }

    pub fn text(role: AgentRole, origin: MessageOrigin, text: impl Into<String>) -> Self {
        Self::new(
            role,
            origin,
            vec![AgentContentBlock::Text { text: text.into() }],
        )
    }

    pub fn tool_use_ids(&self) -> impl Iterator<Item = &str> {
        self.content.iter().filter_map(|block| match block {
            AgentContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
    }

    pub fn tool_result_ids(&self) -> impl Iterator<Item = &str> {
        self.content.iter().filter_map(|block| match block {
            AgentContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
    }

    pub fn is_tool_result_only(&self) -> bool {
        let mut has_result = false;
        let grouped = self.content.iter().all(|block| match block {
            AgentContentBlock::ToolResult { .. } => {
                has_result = true;
                true
            }
            AgentContentBlock::Image { .. } => true,
            _ => false,
        });
        grouped && has_result
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AgentContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_origin_round_trips() {
        let origins = [
            MessageOrigin::User,
            MessageOrigin::Assistant,
            MessageOrigin::ToolResult,
            MessageOrigin::InjectedContext {
                source: "hook".into(),
            },
            MessageOrigin::CompactionSummary,
            MessageOrigin::GoalContext,
            MessageOrigin::SubagentResult,
            MessageOrigin::Continuation,
        ];
        for origin in origins {
            let json = serde_json::to_string(&origin).unwrap();
            let back: MessageOrigin = serde_json::from_str(&json).unwrap();
            assert_eq!(back, origin);
        }
    }

    #[test]
    fn message_tracks_tool_pairing_and_origin() {
        let message = AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![AgentContentBlock::ToolUse {
                id: "call_1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }],
        );
        assert_eq!(message.tool_use_ids().collect::<Vec<_>>(), ["call_1"]);
        assert!(!message.is_tool_result_only());

        let results = AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            vec![
                AgentContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "done".into(),
                },
                AgentContentBlock::Image {
                    source: AgentImageSource {
                        type_: "base64".into(),
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                },
            ],
        );
        assert_eq!(results.tool_result_ids().collect::<Vec<_>>(), ["call_1"]);
        assert!(results.is_tool_result_only());
    }

    #[test]
    fn persisted_message_boundary_round_trips_with_origin_and_blocks() {
        let message = AgentMessage::new(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: "hook".into(),
            },
            vec![
                AgentContentBlock::Text {
                    text: "context".into(),
                },
                AgentContentBlock::Image {
                    source: AgentImageSource {
                        type_: "base64".into(),
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                },
            ],
        );
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<AgentMessage>(&json).unwrap(),
            message
        );
    }
}
