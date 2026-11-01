//! Stable construction-time model limits and session identity.

/// Output cap applied when a caller does not state one.
///
/// Matches the CLI's `[agent].max_tokens` default (8K), so an SDK host and the
/// app agree on how long a single turn may run.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8_192;

/// Token limits fixed when a provider is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLimits {
    context_window: Option<usize>,
    max_output_tokens: u32,
}

impl Default for ModelLimits {
    /// The fallback for callers that cannot state a model's limits.
    ///
    /// The context window stays **unknown** rather than guessed: a made-up
    /// window would silently drive auto-compaction on a number nobody
    /// verified. Output is capped at [`DEFAULT_MAX_OUTPUT_TOKENS`].
    fn default() -> Self {
        Self {
            context_window: None,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
        }
    }
}

impl ModelLimits {
    pub fn new(
        context_window: impl Into<Option<usize>>,
        max_output_tokens: u32,
    ) -> Result<Self, LimitError> {
        let context_window = context_window.into();
        if context_window == Some(0) {
            return Err(LimitError::ZeroContextWindow);
        }
        if max_output_tokens == 0 {
            return Err(LimitError::ZeroOutputTokens);
        }
        if context_window.is_some_and(|window| max_output_tokens as usize >= window) {
            return Err(LimitError::OutputExceedsContext);
        }
        Ok(Self {
            context_window,
            max_output_tokens,
        })
    }

    pub fn context_window(self) -> Option<usize> {
        self.context_window
    }

    pub fn max_output_tokens(self) -> u32 {
        self.max_output_tokens
    }
}

/// A validated, header-safe session identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Result<Self, SessionIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SessionIdError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(SessionIdError::ControlCharacter);
        }
        if !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err(SessionIdError::InvalidHeaderValue);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SessionId {
    /// A fresh random id for callers that do not track sessions themselves.
    ///
    /// Random rather than constant: the id doubles as the provider's cache
    /// `user_id` and the OpenCode `x-opencode-session` header, so independent
    /// builds must not share one lane.
    fn default() -> Self {
        Self(format!("tact-{}", uuid::Uuid::new_v4()))
    }
}

/// Invalid model-limit values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LimitError {
    #[error("context window must be positive when known")]
    ZeroContextWindow,
    #[error("max output tokens must be positive")]
    ZeroOutputTokens,
    #[error("max output tokens must be smaller than the known context window")]
    OutputExceedsContext,
}

/// Invalid session identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionIdError {
    #[error("session id must not be empty")]
    Empty,
    #[error("session id contains a control character")]
    ControlCharacter,
    #[error("session id is not a valid HTTP header value")]
    InvalidHeaderValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_reject_invalid_values() {
        assert_eq!(
            ModelLimits::new(Some(0), 1),
            Err(LimitError::ZeroContextWindow)
        );
        assert_eq!(ModelLimits::new(None, 0), Err(LimitError::ZeroOutputTokens));
        assert_eq!(
            ModelLimits::new(Some(100), 100),
            Err(LimitError::OutputExceedsContext)
        );
        assert!(ModelLimits::new(None, 100).is_ok());
        assert!(ModelLimits::new(Some(101), 100).is_ok());
    }

    #[test]
    fn default_limits_state_no_window_and_the_shared_output_cap() {
        let limits = ModelLimits::default();
        assert_eq!(limits.context_window(), None);
        assert_eq!(limits.max_output_tokens(), DEFAULT_MAX_OUTPUT_TOKENS);
        assert_eq!(DEFAULT_MAX_OUTPUT_TOKENS, 8_192);
        // The default has to survive the same validation a caller-set value
        // does, or `ProviderBuilder` would build capabilities nobody checks.
        assert!(ModelLimits::new(limits.context_window(), limits.max_output_tokens()).is_ok());
    }

    #[test]
    fn default_session_ids_are_unique_and_header_safe() {
        let first = SessionId::default();
        let second = SessionId::default();
        assert_ne!(first, second);
        assert!(first.as_str().starts_with("tact-"));
        assert!(SessionId::new(first.as_str()).is_ok());
    }

    #[test]
    fn session_ids_are_header_safe() {
        assert!(SessionId::new("chat-1").is_ok());
        assert_eq!(SessionId::new(""), Err(SessionIdError::Empty));
        assert_eq!(
            SessionId::new("chat\n1"),
            Err(SessionIdError::ControlCharacter)
        );
        assert_eq!(
            SessionId::new("chat 1"),
            Err(SessionIdError::InvalidHeaderValue)
        );
    }
}
