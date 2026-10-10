//! Payload redaction shared by every durable Runtime sink.
//!
//! The Kernel owns the trajectory recorder, so it also owns the policy the
//! recorder applies before a fact is persisted. The engine itself is
//! name-based and heuristic — the real boundary is the sandbox; this exists so
//! a secret a tool printed cannot land in `trajectory_events.payload`
//! verbatim.

mod engine;

pub use engine::{StreamRedactor, level_for_call, redact};

/// How aggressive the redactor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionLevel {
    /// Redaction off. The documented escape hatch for legitimate work that
    /// needs to see a token; global and persisted, never per-call.
    Off,
    /// High-confidence, low-false-positive shapes only. Safe on source code.
    Basic,
    /// [`Self::Basic`] plus structure-aware key/value rules. Only for text that
    /// is already known to be a credential store.
    Credential,
}

impl RedactionLevel {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Some(Self::Off),
            "basic" => Some(Self::Basic),
            "credential" | "strict" => Some(Self::Credential),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Basic => "basic",
            Self::Credential => "credential",
        }
    }
}

/// `permissions.redaction`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionConfig {
    pub enabled: Option<bool>,
    /// `None` = unset; resolved by [`Self::resolved_level`].
    pub level: Option<RedactionLevel>,
    pub extra_patterns: Vec<String>,
    /// Paths whose results never escalate to [`RedactionLevel::Credential`].
    ///
    /// The structural rules are meant for credential stores, and would rewrite
    /// a source file or a test fixture that happens to contain `token = "…"`.
    /// This keeps the guard (`allow` is the switch for that) but caps the
    /// redaction at [`RedactionLevel::Basic`].
    pub basic_only_paths: Vec<String>,
}

impl RedactionConfig {
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// The effective level. An unset level is [`RedactionLevel::Basic`] — never
    /// `Off`, so a typo cannot silently disable redaction.
    #[must_use]
    pub fn resolved_level(&self) -> RedactionLevel {
        self.level.unwrap_or(RedactionLevel::Basic)
    }
}
