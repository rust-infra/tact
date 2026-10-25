//! Reasoning effort tiers, as a protocol-level value.
//!
//! The tiers are a closed set the user picks from and the wire carries verbatim
//! (`"none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"`). They
//! live here rather than in the model library because both sides of the
//! protocol need them: the runtime forwards them, and a view renders the picker.
//! Keeping them in the library forced the view to depend on it for an enum.
//!
//! `tact_llm` re-exports this type, so existing imports keep working.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Reasoning effort forwarded to OpenAI reasoning models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    /// Every tier, easiest first.
    ///
    /// Ordered because a picker renders them in this order and a "nearest tier"
    /// lookup compares positions.
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    /// The wire spelling.
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

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|effort| effort.as_str() == value)
            .ok_or_else(|| {
                format!(
                    "unknown OpenAI reasoning effort '{value}'; expected none|minimal|low|medium|high|xhigh|max"
                )
            })
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tier round-trips through its wire spelling.
    #[test]
    fn every_tier_round_trips() {
        for effort in ReasoningEffort::ALL {
            assert_eq!(
                effort.as_str().parse::<ReasoningEffort>(),
                Ok(effort),
                "{effort:?} must parse back from its own spelling"
            );
            assert_eq!(effort.to_string(), effort.as_str());
        }
    }

    /// The wire spelling is the documented lowercase form.
    #[test]
    fn the_wire_spelling_is_lowercase() {
        let json = serde_json::to_string(&ReasoningEffort::Xhigh).unwrap();
        assert_eq!(json, "\"xhigh\"");
        assert_eq!(
            serde_json::from_str::<ReasoningEffort>("\"max\"").unwrap(),
            ReasoningEffort::Max
        );
    }

    /// An unknown tier is an error that names the alternatives.
    ///
    /// The error text is what a user sees when their config has a typo, so it
    /// has to say what was accepted rather than just that it failed.
    #[test]
    fn an_unknown_tier_names_the_alternatives() {
        let error = "turbo".parse::<ReasoningEffort>().unwrap_err();
        assert!(error.contains("turbo"), "{error}");
        assert!(error.contains("xhigh"), "{error}");
    }

    /// The order is easiest-first, which a picker relies on.
    #[test]
    fn the_tiers_are_ordered() {
        let names: Vec<&str> = ReasoningEffort::ALL.iter().map(|e| e.as_str()).collect();
        assert_eq!(
            names,
            vec!["none", "minimal", "low", "medium", "high", "xhigh", "max"]
        );
    }
}
