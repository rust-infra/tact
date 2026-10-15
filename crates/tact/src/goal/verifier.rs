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
    /// version, and no criterion still awaits a human decision.
    ///
    /// Invariant: `Verified` is exactly the condition under which
    /// [`Goal::validate_acceptance`] returns `Ok`, for every Goal that has at
    /// least one required criterion. The single, intentional divergence is a
    /// Goal with no required criteria (empty, or wholly optional): `verify`
    /// returns `Rejected` while `validate_acceptance` vacuously returns `Ok`,
    /// because such a Goal has nothing to verify and must never auto-succeed.
    Verified,
    /// No required criterion is provably unmet, but at least one required
    /// human criterion still awaits the user's decision.
    NeedsUser { criteria: Vec<String> },
    /// At least one required criterion is provably unmet (automatic criterion
    /// without valid evidence), or the Goal has no usable acceptance criteria.
    Rejected { criteria: Vec<String> },
}

/// Decides whether a Goal may be accepted. See `book/28_chapter_goal_zh.md` §6.
///
/// The decision is a pure function of `goal.criteria` and
/// `goal.criteria_version`, evaluated through [`GoalCriterion::has_valid_evidence`]:
///
/// 1. If `goal.criteria` is empty, or no criterion is `required`, the Goal has
///    nothing to verify and must never auto-succeed → `Rejected { criteria:
///    vec![] }`.
/// 2. Otherwise, if any required automatic criterion lacks valid evidence →
///    `Rejected` naming those criterion ids, in `goal.criteria` order.
/// 3. Otherwise, if any required human criterion lacks valid evidence →
///    `NeedsUser` naming those criterion ids, in `goal.criteria` order.
/// 4. Otherwise → `Verified`.
///
/// Invariant: `Verified` is exactly the condition under which
/// [`Goal::validate_acceptance`] returns `Ok`, except for a Goal with no
/// required criteria (see [`GoalVerification::Verified`]).
pub struct GoalVerifier;

impl GoalVerifier {
    pub fn verify(goal: &Goal) -> GoalVerification {
        let version = goal.criteria_version;

        let required: Vec<&GoalCriterion> = goal
            .criteria
            .iter()
            .filter(|criterion| criterion.required)
            .collect();
        if goal.criteria.is_empty() || required.is_empty() {
            return GoalVerification::Rejected {
                criteria: Vec::new(),
            };
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
            criteria: Some(vec![automatic("auto-1", true)]),
            ..GoalUpdate::default()
        })
        .unwrap();
        assert_eq!(goal.criteria_version, version + 1);
        assert_eq!(
            GoalVerifier::verify(&goal),
            GoalVerification::Rejected {
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

    #[test]
    fn goal_without_required_criteria_is_never_verified() {
        // `Goal::new` admits BOTH an optional-only criteria list and an empty
        // criteria list; neither must ever auto-succeed.
        let optional_only = goal_with(vec![human("optional-1", false)]);
        assert_eq!(
            GoalVerifier::verify(&optional_only),
            GoalVerification::Rejected {
                criteria: Vec::new(),
            }
        );

        let empty = goal_with(Vec::new());
        assert_eq!(
            GoalVerifier::verify(&empty),
            GoalVerification::Rejected {
                criteria: Vec::new(),
            }
        );

        // Executably pin the documented divergence: `validate_acceptance()` is
        // vacuously `Ok` for the optional-only Goal, yet the verifier still
        // refuses to call it Verified. Rule 4 (never verify a Goal that has no
        // required criteria) outranks the one-way invariant
        // `Verified ⇒ validate_acceptance().is_ok()`.
        assert!(
            optional_only.validate_acceptance().is_ok(),
            "an optional-only Goal is vacuously acceptable to validate_acceptance()"
        );
        assert!(
            empty.validate_acceptance().is_err(),
            "a criteria-less Goal is refused by validate_acceptance()"
        );
        assert_ne!(
            GoalVerifier::verify(&optional_only),
            GoalVerification::Verified
        );
        assert_ne!(GoalVerifier::verify(&empty), GoalVerification::Verified);
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
