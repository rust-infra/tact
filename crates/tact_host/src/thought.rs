//! The tiers a model offers for "think harder", shared by every frontend.
//!
//! Two providers' models express this differently, and the choice is not a
//! frontend decision: an effort model takes a named tier (`low`, `high`, …),
//! a budget model takes a token count (`0`, `8000`, …). Which one applies is
//! [`tact_llm::model_uses_effort`], the same rule the wire body hooks use, so
//! a frontend that read it differently would offer a control the model cannot
//! honour.
//!
//! This module is deliberately protocol-free: it returns labels and candidate
//! value strings, and each frontend maps them onto its own surface (an ACP
//! `select` config option, a TUI picker, a desktop chip).

use tact::config::ResolvedConfig;
use tact_protocol::ReasoningEffort;

/// Token tiers offered when a budget model has no profile of its own.
///
/// `0` is "thinking off": a budget model that cannot be told to stop thinking
/// has no way to answer a cheap question cheaply.
pub const DEFAULT_THINKING_BUDGETS: [u32; 5] = [0, 8_000, 32_000, 64_000, 128_000];

/// How the active model expresses "think harder".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThoughtKind {
    /// A named tier: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`.
    Effort,
    /// A token count the model may spend thinking.
    Budget,
}

impl ThoughtKind {
    /// The value meaning "no extra thinking", for this kind.
    pub const fn off_value(self) -> &'static str {
        match self {
            Self::Effort => "none",
            Self::Budget => "0",
        }
    }
}

/// Everything one model's thought control needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThoughtTiers {
    pub kind: ThoughtKind,
    /// Display name of the control: `Reasoning effort` or `Thinking budget`.
    pub name: &'static str,
    /// One line explaining what the value does.
    pub description: &'static str,
    /// The value the runtime last reported, as a candidate string.
    pub applied: String,
    /// Every value the control may be set to, in display order.
    pub candidates: Vec<String>,
}

impl ThoughtTiers {
    /// Whether `value` is one of the offered tiers.
    pub fn accepts(&self, value: &str) -> bool {
        self.candidates.iter().any(|candidate| candidate == value)
    }

    /// `value` if it is offered, else the kind's off value.
    ///
    /// Used when a report arrives for a tier the current model no longer
    /// offers (a model switch between an effort and a budget model).
    pub fn clamp(&self, value: &str) -> String {
        if self.accepts(value) {
            value.to_string()
        } else {
            self.kind.off_value().to_string()
        }
    }

    /// Map a reported effort/budget pair onto this control's value.
    ///
    /// `ModelInfo` carries both shapes; only the one this model uses is
    /// meaningful, and an absent value means the model runs with thinking off.
    pub fn reported(&self, budget: Option<u32>, effort: Option<&str>) -> String {
        let value = match self.kind {
            ThoughtKind::Effort => effort.unwrap_or(self.kind.off_value()),
            ThoughtKind::Budget => return budget.unwrap_or(0).to_string(),
        };
        value.to_string()
    }
}

/// Build the thought control for `model`.
///
/// A profile entry (`[llm.model_profiles."<id>"]`) narrows the tiers; without
/// one the provider's default ladder applies. The applied value is taken from
/// config for the kind the model actually uses, so switching a budget model to
/// an effort model cannot leave a token count in an effort control.
pub fn thought_tiers(settings: &ResolvedConfig, model: &str) -> ThoughtTiers {
    let profile = settings.llm.model_profiles.get(model);
    let provider = settings.llm.provider_info();
    if tact_llm::model_uses_effort(model, &provider) {
        let candidates = profile
            .filter(|profile| !profile.reasoning_efforts.is_empty())
            .map(|profile| profile.reasoning_efforts.clone())
            .unwrap_or_else(|| tact_llm::default_effort_tiers(&provider))
            .into_iter()
            .map(|effort| effort.as_str().to_string())
            .collect::<Vec<_>>();
        let applied = settings
            .agent
            .reasoning_effort
            .map(|effort| effort.as_str().to_string())
            .unwrap_or_else(|| ThoughtKind::Effort.off_value().to_string());
        ThoughtTiers {
            kind: ThoughtKind::Effort,
            name: "Reasoning effort",
            description: "How much the model thinks before answering",
            applied,
            candidates,
        }
    } else {
        let candidates = profile
            .filter(|profile| !profile.thinking_budgets.is_empty())
            .map(|profile| {
                profile
                    .thinking_budgets
                    .iter()
                    .map(|budget| budget.to_string())
                    .collect()
            })
            .unwrap_or_else(|| {
                DEFAULT_THINKING_BUDGETS
                    .iter()
                    .map(|budget| budget.to_string())
                    .collect()
            });
        ThoughtTiers {
            kind: ThoughtKind::Budget,
            name: "Thinking budget",
            description: "Tokens the model may spend thinking",
            applied: settings.agent.thinking_budget.to_string(),
            candidates,
        }
    }
}

/// The tiers for a model when no configuration is installed.
///
/// A frontend that renders before config lands still needs a control; the
/// provider default ladder is what it will get once config arrives anyway.
pub fn default_thought_tiers() -> ThoughtTiers {
    ThoughtTiers {
        kind: ThoughtKind::Budget,
        name: "Thinking budget",
        description: "Tokens the model may spend thinking",
        applied: ThoughtKind::Budget.off_value().to_string(),
        candidates: DEFAULT_THINKING_BUDGETS
            .iter()
            .map(|budget| budget.to_string())
            .collect(),
    }
}

/// Parse a reasoning-effort value, rejecting tiers no provider accepts.
///
/// The seven names are the wire vocabulary; `none` and `minimal` are easy to
/// drop because most UI lists start at `low`, and dropping them here is what
/// made a `minimal` selection silently become "not configured".
pub fn parse_effort(value: &str) -> Option<ReasoningEffort> {
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_and_budget_are_distinguished_by_kind() {
        let tiers = ThoughtTiers {
            kind: ThoughtKind::Effort,
            name: "Reasoning effort",
            description: "",
            applied: "none".into(),
            candidates: vec!["low".into(), "high".into()],
        };
        assert!(tiers.accepts("high"));
        assert!(!tiers.accepts("0"));
        assert_eq!(tiers.clamp("0"), "none");
        assert_eq!(tiers.reported(None, Some("high")), "high");
        // A budget report is not an effort tier, and an absent one is off.
        assert_eq!(tiers.reported(Some(32_000), None), "none");
    }

    #[test]
    fn budget_ignores_the_effort_field() {
        let tiers = ThoughtTiers {
            kind: ThoughtKind::Budget,
            name: "Thinking budget",
            description: "",
            applied: "0".into(),
            candidates: vec!["0".into(), "8000".into()],
        };
        assert_eq!(tiers.reported(Some(8_000), Some("high")), "8000");
        assert_eq!(tiers.reported(None, Some("high")), "0");
        assert_eq!(tiers.clamp("high"), "0");
    }

    #[test]
    fn every_wire_effort_name_parses() {
        for name in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            assert!(
                parse_effort(name).is_some(),
                "{name} is in the wire vocabulary"
            );
        }
        assert!(parse_effort("turbo").is_none());
    }

    #[test]
    fn off_values_match_their_kind() {
        assert_eq!(ThoughtKind::Effort.off_value(), "none");
        assert_eq!(ThoughtKind::Budget.off_value(), "0");
        assert_eq!(
            default_thought_tiers().candidates,
            DEFAULT_THINKING_BUDGETS
                .iter()
                .map(|budget| budget.to_string())
                .collect::<Vec<_>>()
        );
    }
}
