//! Provider-agnostic stop / finish reason for the agent loop.
//!
//! Adapters map each provider's native signal into this enum. Prefer this type
//! over `anthropic_ai_sdk::types::message::StopReason` in business code so new
//! API values (and OpenAI `finish_reason` strings) do not leak into `agent_loop`.

use serde::{Deserialize, Serialize};

/// Why the model stopped generating this turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural completion (Anthropic `end_turn`, OpenAI `stop`).
    EndTurn,
    /// Hit `max_tokens` / length limit — agent may continue.
    MaxTokens,
    /// Hit a configured stop sequence (or OpenAI `content_filter`).
    StopSequence,
    /// Model requested tool execution.
    ToolUse,
    /// Safety / policy refusal (Anthropic `refusal`).
    Refusal,
    /// Anthropic server-tool loop paused; send assistant message back to continue.
    PauseTurn,
    /// Unrecognized provider value — keep raw for diagnostics.
    Unknown(String),
}

impl StopReason {
    /// Parse Anthropic Messages API `stop_reason` strings.
    pub fn from_anthropic(reason: Option<&str>) -> Option<Self> {
        match reason {
            Some("end_turn") => Some(Self::EndTurn),
            Some("max_tokens") => Some(Self::MaxTokens),
            // Treat context-window fill like truncation so callers can continue.
            Some("model_context_window_exceeded") => Some(Self::MaxTokens),
            Some("stop_sequence") => Some(Self::StopSequence),
            Some("tool_use") => Some(Self::ToolUse),
            Some("refusal") => Some(Self::Refusal),
            Some("pause_turn") => Some(Self::PauseTurn),
            Some(other) => Some(Self::Unknown(other.to_string())),
            None => None,
        }
    }

    /// Parse OpenAI Chat Completions `finish_reason` strings.
    pub fn from_openai(reason: Option<&str>) -> Option<Self> {
        match reason {
            Some("stop") => Some(Self::EndTurn),
            Some("length") => Some(Self::MaxTokens),
            Some("tool_calls") | Some("function_call") => Some(Self::ToolUse),
            Some("content_filter") => Some(Self::StopSequence),
            Some(other) => Some(Self::Unknown(other.to_string())),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StopReason;

    #[test]
    fn anthropic_maps_known_and_unknown() {
        assert_eq!(
            StopReason::from_anthropic(Some("pause_turn")),
            Some(StopReason::PauseTurn)
        );
        assert_eq!(
            StopReason::from_anthropic(Some("refusal")),
            Some(StopReason::Refusal)
        );
        assert_eq!(
            StopReason::from_anthropic(Some("model_context_window_exceeded")),
            Some(StopReason::MaxTokens)
        );
        assert_eq!(
            StopReason::from_anthropic(Some("brand_new")),
            Some(StopReason::Unknown("brand_new".into()))
        );
        assert_eq!(StopReason::from_anthropic(None), None);
    }

    #[test]
    fn openai_maps_known() {
        assert_eq!(
            StopReason::from_openai(Some("tool_calls")),
            Some(StopReason::ToolUse)
        );
        assert_eq!(
            StopReason::from_openai(Some("length")),
            Some(StopReason::MaxTokens)
        );
    }
}
