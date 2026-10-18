//! Evidence-based acceptance decision for Goals.
//!
//! The verifier is a pure decision function: it performs no I/O, touches no
//! store, reads no clock, and never inspects model text. It reads only `Goal`
//! state (`criteria`, `criteria_version`) and returns a decision, per
//! `book/28_chapter_goal_zh.md` §6.

use crate::goal::{Goal, GoalCriterion, GoalVerificationKind};

/// Outcome of checking a Goal's required acceptance criteria against the
/// evidence recorded for the current criteria version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalVerification {
    /// Every required criterion holds valid evidence at the current criteria
    /// version, and no criterion still awaits a human decision — or the Goal
    /// declares no required criteria at all, in which case there is nothing to
    /// gate success on and completion is self-attested.
    ///
    /// Invariant: `Verified` is exactly the condition under which
    /// [`Goal::validate_acceptance`] returns `Ok`.
    Verified,
    /// No required criterion is provably unmet, but at least one required
    /// human criterion still awaits the user's decision.
    NeedsUser { criteria: Vec<String> },
    /// At least one required criterion is provably unmet: an automatic
    /// criterion without valid evidence.
    Rejected { criteria: Vec<String> },
}

/// Decides whether a Goal may be accepted. See `book/28_chapter_goal_zh.md` §6.
///
/// The decision is a pure function of `goal.criteria` and
/// `goal.criteria_version`, evaluated through [`GoalCriterion::has_valid_evidence`]:
///
/// 1. If no criterion is `required`, the Goal has nothing to verify →
///    `Verified` (completion is the model's proposal; the runner records it as
///    self-attested, never as verified evidence).
/// 2. Otherwise, if any required automatic criterion lacks valid evidence →
///    `Rejected` naming those criterion ids, in `goal.criteria` order.
/// 3. Otherwise, if any required human criterion lacks valid evidence →
///    `NeedsUser` naming those criterion ids, in `goal.criteria` order.
/// 4. Otherwise → `Verified`.
///
/// Invariant: `Verified` is exactly the condition under which
/// [`Goal::validate_acceptance`] returns `Ok`.
pub struct GoalVerifier;

impl GoalVerifier {
    pub fn verify(goal: &Goal) -> GoalVerification {
        let version = goal.criteria_version;

        let required: Vec<&GoalCriterion> = goal
            .criteria
            .iter()
            .filter(|criterion| criterion.required)
            .collect();
        if required.is_empty() {
            // Nothing to verify. Automatic criteria cannot appear here in
            // production (creation refuses them), so "no required criteria"
            // really does mean "no gate".
            return GoalVerification::Verified;
        }

        let unmet_automatic: Vec<String> = required
            .iter()
            .filter(|criterion| {
                !criterion.has_valid_evidence(version)
                    && criterion.verification_kind == GoalVerificationKind::Automatic
            })
            .map(|criterion| criterion.id.clone())
            .collect();
        if !unmet_automatic.is_empty() {
            return GoalVerification::Rejected {
                criteria: unmet_automatic,
            };
        }

        let unmet_human: Vec<String> = required
            .iter()
            .filter(|criterion| {
                !criterion.has_valid_evidence(version)
                    && criterion.verification_kind == GoalVerificationKind::Human
            })
            .map(|criterion| criterion.id.clone())
            .collect();
        if !unmet_human.is_empty() {
            return GoalVerification::NeedsUser {
                criteria: unmet_human,
            };
        }

        GoalVerification::Verified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::{Goal, GoalCreate, GoalCriterion, GoalUpdate, GoalVerificationKind};

    /// Build a Goal admitting any criterion list the constructor accepts.
    fn goal_with(criteria: Vec<GoalCriterion>) -> Goal {
        Goal::new(GoalCreate {
            title: "Verifier test goal".to_string(),
            objective: "Exercise the acceptance decision".to_string(),
            constraints: Vec::new(),
            criteria,
        })
        .expect("valid goal")
    }

    fn automatic(id: &str, required: bool) -> GoalCriterion {
        let mut criterion =
            GoalCriterion::new("auto check", required, GoalVerificationKind::Automatic);
        criterion.id = id.to_string();
        criterion
    }

    fn human(id: &str, required: bool) -> GoalCriterion {
        let mut criterion =
            GoalCriterion::new("human review", required, GoalVerificationKind::Human);
        criterion.id = id.to_string();
        criterion
    }

    #[test]
    fn required_automatic_criterion_without_evidence_is_rejected() {
        let goal = goal_with(vec![automatic("auto-1", true)]);
        assert_eq!(
            GoalVerifier::verify(&goal),
            GoalVerification::Rejected {
                criteria: vec!["auto-1".to_string()],
            }
        );
    }

    #[test]
    fn stale_version_evidence_is_rejected() {
        let mut goal = goal_with(vec![automatic("auto-1", true)]);
        let version = goal.criteria_version;
        goal.criteria[0].record_evidence("recorded-at-v1", version);
        assert_eq!(GoalVerifier::verify(&goal), GoalVerification::Verified);

        // Advance the criteria definition/version through the real update path.
        goal.update(GoalUpdate {
            criteria: Some(vec![human("auto-1", true)]),
            ..GoalUpdate::default()
        })
        .unwrap();
        assert_eq!(goal.criteria_version, version + 1);
        assert_eq!(
            GoalVerifier::verify(&goal),
            GoalVerification::NeedsUser {
                criteria: vec!["auto-1".to_string()],
            }
        );

        // Re-evidencing at the new version restores verification.
        let current = goal.criteria_version;
        goal.criteria[0].record_evidence("recorded-at-v2", current);
        assert_eq!(GoalVerifier::verify(&goal), GoalVerification::Verified);
    }

    #[test]
    fn valid_automatic_evidence_verifies() {
        let mut goal = goal_with(vec![automatic("a", true), automatic("b", true)]);
        let version = goal.criteria_version;
        for criterion in &mut goal.criteria {
            criterion.record_evidence("ok", version);
        }
        assert_eq!(GoalVerifier::verify(&goal), GoalVerification::Verified);
    }

    #[test]
    fn required_human_criterion_needs_user() {
        let mut goal = goal_with(vec![automatic("auto", true), human("human-1", true)]);
        let version = goal.criteria_version;
        goal.criteria[0].record_evidence("ok", version);

        assert_eq!(
            GoalVerifier::verify(&goal),
            GoalVerification::NeedsUser {
                criteria: vec!["human-1".to_string()],
            }
        );
    }

    #[test]
    fn unmet_automatic_outranks_unmet_human() {
        let goal = goal_with(vec![automatic("auto-1", true), human("human-1", true)]);
        assert_eq!(
            GoalVerifier::verify(&goal),
            GoalVerification::Rejected {
                criteria: vec!["auto-1".to_string()],
            }
        );
    }

    /// A Goal with nothing required has nothing to gate: it verifies
    /// vacuously, and the runner records the completion as self-attested
    /// instead of claiming evidence.
    #[test]
    fn goal_without_required_criteria_verifies_vacuously() {
        let optional_only = goal_with(vec![human("optional-1", false)]);
        let empty = goal_with(Vec::new());

        for goal in [&optional_only, &empty] {
            assert_eq!(GoalVerifier::verify(goal), GoalVerification::Verified);
            assert!(
                goal.validate_acceptance().is_ok(),
                "nothing required means nothing to fail"
            );
            assert!(!goal.has_required_criteria());
        }
    }

    #[test]
    fn verified_implies_validate_acceptance_ok() {
        let mut verified = goal_with(vec![automatic("a", true)]);
        let version = verified.criteria_version;
        verified.criteria[0].record_evidence("ok", version);

        let goals = vec![
            verified,
            goal_with(vec![automatic("a", true)]),
            goal_with(Vec::new()),
            goal_with(vec![human("h", true)]),
        ];

        for goal in goals {
            match GoalVerifier::verify(&goal) {
                GoalVerification::Verified => assert!(
                    goal.validate_acceptance().is_ok(),
                    "Verified must imply validate_acceptance().is_ok()"
                ),
                GoalVerification::NeedsUser { .. } | GoalVerification::Rejected { .. } => assert!(
                    goal.validate_acceptance().is_err(),
                    "NeedsUser/Rejected must imply validate_acceptance().is_err()"
                ),
            }
        }
    }

    #[test]
    fn non_required_criteria_do_not_block_verification() {
        let mut goal = goal_with(vec![
            automatic("auto", true),
            human("optional-human", false),
        ]);
        let version = goal.criteria_version;
        goal.criteria[0].record_evidence("ok", version);

        assert_eq!(GoalVerifier::verify(&goal), GoalVerification::Verified);
    }
}
