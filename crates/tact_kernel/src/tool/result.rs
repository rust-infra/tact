//! Structured tool results and control-plane effects.
//!
//! The kernel keeps these types neutral: no provider crate, no UI types. The
//! Agent converts [`ToolImage`] to the provider's image source at the message
//! boundary.

/// An image produced by a tool, encoded as base64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolImage {
    pub mime_type: String,
    pub data: String,
}

/// Why a tool yielded the turn back to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YieldReason {
    NeedsUser,
}

/// A control-plane effect a tool asks the Agent to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEffect {
    CompactHistory { focus: Option<String> },
    YieldTurn { reason: YieldReason },
}

/// A tool's structured result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallResult {
    pub content: String,
    pub effects: Vec<ToolEffect>,
    pub image: Option<ToolImage>,
}

impl ToolCallResult {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            effects: Vec::new(),
            image: None,
        }
    }

    pub fn text_image(content: impl Into<String>, image: ToolImage) -> Self {
        Self {
            content: content.into(),
            effects: Vec::new(),
            image: Some(image),
        }
    }
}

/// Converts handler return values into a tool result.
pub trait IntoToolCallResult {
    fn into_tool_call_result(self) -> ToolCallResult;
}

impl IntoToolCallResult for String {
    fn into_tool_call_result(self) -> ToolCallResult {
        ToolCallResult::text(self)
    }
}

impl IntoToolCallResult for &'static str {
    fn into_tool_call_result(self) -> ToolCallResult {
        ToolCallResult::text(self)
    }
}

impl IntoToolCallResult for ToolCallResult {
    fn into_tool_call_result(self) -> ToolCallResult {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_result_has_no_image_or_effects() {
        let result = ToolCallResult::text("ok");
        assert_eq!(result.content, "ok");
        assert!(result.image.is_none());
        assert!(result.effects.is_empty());
    }

    #[test]
    fn text_image_round_trips_owned_image() {
        let image = ToolImage {
            mime_type: "image/png".to_string(),
            data: "AA==".to_string(),
        };
        let result = ToolCallResult::text_image("ok", image.clone());
        assert_eq!(result.image, Some(image));
    }

    #[test]
    fn yield_turn_carries_a_generic_reason() {
        let effect = ToolEffect::YieldTurn {
            reason: YieldReason::NeedsUser,
        };
        assert_eq!(
            effect,
            ToolEffect::YieldTurn {
                reason: YieldReason::NeedsUser
            }
        );
    }

    #[test]
    fn string_converts_to_effect_free_result() {
        assert_eq!("ok".into_tool_call_result().content, "ok");
    }
}
