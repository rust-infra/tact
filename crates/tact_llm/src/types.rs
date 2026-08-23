//! Provider-agnostic LLM types.
//!
//! - [`ProviderKind`] — config / CLI / runtime provider identity
//! - [`CreateMessageParams`] — shared request model (Anthropic Messages shape)
//! - [`StopReason`] — shared stop signal for the agent loop

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::Message;

/// Typed LLM provider identity (config / CLI / runtime).
///
/// [`ProviderKind::Custom`] accepts any name not covered by the built-ins and
/// behaves as an OpenAI-compatible provider (Chat Completions / Responses).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    DeepSeek,
    Kimi,
    /// Arbitrary provider name from config (e.g. "moonshot", "xai", "ollama").
    /// Reuses the OpenAI protocol; `base_url` must be configured explicitly.
    Custom(String),
}

/// Wire protocol used by an OpenAI provider entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiProtocol {
    /// OpenAI-compatible Chat Completions (`/chat/completions`).
    #[default]
    ChatCompletions,
    /// OpenAI Responses API (`/responses`).
    Responses,
}

/// Wire dialect used by a Chat Completions request.
///
/// DeepSeek and Kimi are not separate providers: they share the
/// OpenAI-compatible transport and only differ in the body fields injected by
/// their [`crate::OpenAiBodyHook`] implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatCompletionsDialect {
    Standard,
    DeepSeek,
    Kimi,
}

impl ChatCompletionsDialect {
    /// Whether this dialect injects a top-level `user_id` for KV-cache
    /// isolation. Only DeepSeek currently supports it.
    pub fn supports_user_id(self) -> bool {
        matches!(self, Self::DeepSeek)
    }
}

/// Reasoning effort forwarded to OpenAI reasoning models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl OpenAiReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl FromStr for OpenAiReasoningEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => Err(format!(
                "unknown OpenAI reasoning effort '{other}'; expected none|minimal|low|medium|high|xhigh|max"
            )),
        }
    }
}

impl fmt::Display for OpenAiReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Convert Tact's provider-agnostic effort into the SDK's typed
/// `ReasoningEffort` (vendored `async-openai-local`), so the Responses
/// adapter can build the request with the typed builder instead of injecting
/// `reasoning.effort` through JSON. The vendor adds `Max` so every variant
/// maps one-to-one; the wire strings match `as_str()`.
impl From<OpenAiReasoningEffort> for async_openai_responses::types::responses::ReasoningEffort {
    fn from(effort: OpenAiReasoningEffort) -> Self {
        match effort {
            OpenAiReasoningEffort::None => Self::None,
            OpenAiReasoningEffort::Minimal => Self::Minimal,
            OpenAiReasoningEffort::Low => Self::Low,
            OpenAiReasoningEffort::Medium => Self::Medium,
            OpenAiReasoningEffort::High => Self::High,
            OpenAiReasoningEffort::Xhigh => Self::Xhigh,
            OpenAiReasoningEffort::Max => Self::Max,
        }
    }
}

impl OpenAiProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
        }
    }
}

impl FromStr for OpenAiProtocol {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "chat_completions" => Ok(Self::ChatCompletions),
            "responses" => Ok(Self::Responses),
            other => Err(format!(
                "unknown OpenAI protocol '{other}'; expected chat_completions|responses"
            )),
        }
    }
}

impl fmt::Display for OpenAiProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ProviderKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::Kimi => "kimi",
            Self::Custom(name) => name,
        }
    }

    pub fn default_base_url(&self) -> Option<&'static str> {
        match self {
            Self::Anthropic => None,
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::DeepSeek => Some("https://api.deepseek.com"),
            Self::Kimi => Some("https://api.moonshot.cn/v1"),
            // Custom providers are arbitrary OpenAI-compatible endpoints; the
            // user must configure `base_url` explicitly.
            Self::Custom(_) => None,
        }
    }

    pub fn is_openai_compatible(&self) -> bool {
        !matches!(self, Self::Anthropic)
    }

    /// Whether this provider's models generally support image (vision) input.
    ///
    /// DeepSeek V4 (chat/reasoner/v4/v4-pro) is a text-only model.
    /// Anthropic Claude 3+, OpenAI GPT-4o/V, and Kimi K2.x all support vision.
    /// Custom OpenAI-compatible providers default to vision support.
    pub fn supports_vision(&self) -> bool {
        !matches!(self, Self::DeepSeek)
    }
}

/// Whether a DeepSeek model id is an experimental `*-vision-*` variant that
/// accepts `image_url` parts over the OpenAI-compatible Chat Completions API
/// (e.g. `deepseek-v4-flash-vision-exp`). The V4 text family (chat/reasoner/
/// v4/v4-pro/v4-flash) does not, so callers use this to exempt only vision ids.
pub fn is_deepseek_vision_model(model: &str) -> bool {
    model.contains("deepseek") && model.contains("vision")
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "deepseek" => Ok(Self::DeepSeek),
            "kimi" => Ok(Self::Kimi),
            "" => Err("provider name must not be empty".to_string()),
            // Unknown providers are allowed and reuse the OpenAI protocol.
            other => Ok(Self::Custom(other.to_string())),
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

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
    /// Explicit reasoning effort for this request (openai / deepseek / kimi k3).
    /// `None` = omit from wire (provider default applies); this is per-request,
    /// never read from a global provider state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<OpenAiReasoningEffort>,
    /// Responses-only request fields; ignored by non-Responses adapters.
    #[serde(skip)]
    pub responses_options: Option<crate::openai::responses::ResponsesRequestOptions>,
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

    /// Set the explicit per-request reasoning effort (openai / deepseek / kimi k3).
    pub fn with_reasoning_effort(mut self, effort: Option<OpenAiReasoningEffort>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    /// Set Responses-only request options. Other adapters ignore this field.
    pub fn with_responses_options(
        mut self,
        options: crate::openai::responses::ResponsesRequestOptions,
    ) -> Self {
        self.responses_options = Some(options);
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
    use std::str::FromStr;

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

    #[test]
    fn provider_kind_from_str_round_trip() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::DeepSeek,
            ProviderKind::Kimi,
        ] {
            assert_eq!(ProviderKind::from_str(kind.as_str()).unwrap(), kind);
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn provider_kind_from_str_accepts_unknown_as_custom() {
        for name in ["foo", "moonshot", "xai", "ollama"] {
            let kind = ProviderKind::from_str(name).unwrap();
            assert_eq!(kind, ProviderKind::Custom(name.to_string()));
            assert_eq!(kind.as_str(), name);
            assert_eq!(kind.to_string(), name);
            assert!(kind.is_openai_compatible());
            assert_eq!(kind.default_base_url(), None);
            assert!(kind.supports_vision());
        }
        assert!(ProviderKind::from_str("").is_err());
    }

    #[test]
    fn provider_kind_default_base_urls() {
        assert_eq!(
            ProviderKind::OpenAi.default_base_url(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            ProviderKind::DeepSeek.default_base_url(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(
            ProviderKind::Kimi.default_base_url(),
            Some("https://api.moonshot.cn/v1")
        );
        assert_eq!(ProviderKind::Anthropic.default_base_url(), None);
    }

    #[test]
    fn provider_kind_openai_compatible_flags() {
        assert!(!ProviderKind::Anthropic.is_openai_compatible());
        assert!(ProviderKind::OpenAi.is_openai_compatible());
        assert!(ProviderKind::DeepSeek.is_openai_compatible());
        assert!(ProviderKind::Kimi.is_openai_compatible());
    }

    #[test]
    fn openai_protocol_from_str_round_trip_and_default() {
        for protocol in [OpenAiProtocol::ChatCompletions, OpenAiProtocol::Responses] {
            assert_eq!(
                OpenAiProtocol::from_str(protocol.as_str()).unwrap(),
                protocol
            );
            assert_eq!(protocol.to_string(), protocol.as_str());
        }
        assert_eq!(OpenAiProtocol::default(), OpenAiProtocol::ChatCompletions);
    }

    #[test]
    fn openai_protocol_from_str_rejects_unknown() {
        let error = OpenAiProtocol::from_str("response").unwrap_err();
        assert!(error.contains("chat_completions|responses"));
    }

    #[test]
    fn openai_reasoning_effort_from_str_round_trips_all_values() {
        for raw in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            let effort = OpenAiReasoningEffort::from_str(raw).unwrap();
            assert_eq!(effort.as_str(), raw);
            assert_eq!(effort.to_string(), raw);
            assert_eq!(serde_json::to_value(effort).unwrap(), raw);
        }
    }

    #[test]
    fn openai_reasoning_effort_rejects_unknown() {
        let error = OpenAiReasoningEffort::from_str("extreme").unwrap_err();
        assert!(error.contains("none|minimal|low|medium|high|xhigh|max"));
    }
}
