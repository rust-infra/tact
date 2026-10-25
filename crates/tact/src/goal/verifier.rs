//! Evidence-based acceptance decision for Goals.
//!
//! The verifier is a pure decision function: it performs no I/O, touches no
//! store, reads no clock, and never inspects model text. It reads only `Goal`
//! state (`criteria`, `criteria_version`) and returns a decision, per
//! `book/28_chapter_goal_zh.html` §6.

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

/// Decides whether a Goal may be accepted. See `book/28_chapter_goal_zh.html` §6.
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

/// The acceptance decision, as a replaceable policy.
///
/// `GoalVerifier::verify` is the built-in one and stays a pure function. The trait
/// exists because *how* a Goal proves it is done is the domain's policy boundary: a
/// host that gates completion on a linter, a test command or an external reviewer
/// installs its own policy instead of forking the runner.
pub trait AcceptancePolicy: Send + Sync {
    /// Decide whether `goal` may be accepted.
    fn verify(&self, goal: &Goal) -> GoalVerification;
}

/// The built-in evidence policy: the decision `GoalVerifier::verify` has always
/// made.
pub struct EvidencePolicy;

impl AcceptancePolicy for EvidencePolicy {
    fn verify(&self, goal: &Goal) -> GoalVerification {
        GoalVerifier::verify(goal)
    }
}

/// Goals, contributed as a plugin.
///
/// The store and the runner are not here: the store is already a trait chosen when
/// the manager is opened, and the runner is the loop. What this replaces is the
/// acceptance policy — the part a host legitimately disagrees about.
pub struct GoalPlugin {
    policy: std::sync::Arc<dyn AcceptancePolicy>,
}

impl GoalPlugin {
    /// A plugin over the given acceptance policy.
    pub fn new(policy: std::sync::Arc<dyn AcceptancePolicy>) -> Self {
        Self { policy }
    }

    /// The built-in evidence policy, which is what a normal host wants.
    pub fn evidence() -> Self {
        Self::new(std::sync::Arc::new(EvidencePolicy))
    }
}

#[async_trait::async_trait]
impl tact_kernel::plugin::Plugin for GoalPlugin {
    fn manifest(&self) -> tact_kernel::plugin::PluginManifest {
        tact_kernel::plugin::PluginManifest::new(
            tact_kernel::plugin::PluginId::new("builtin.goal").expect("a literal id"),
            "Goals",
            env!("CARGO_PKG_VERSION"),
            tact_kernel::plugin::PluginKind::Capability,
            "Durable Goals: criteria, evidence and the acceptance decision",
        )
    }

    fn register(&self, registrar: &mut tact_kernel::plugin::Registrar<'_>) -> anyhow::Result<()> {
        registrar.service(std::sync::Arc::new(AcceptanceHandle(self.policy.clone())))
    }
}

/// The policy a [`GoalPlugin`] provided.
#[derive(Clone)]
pub struct AcceptanceHandle(pub std::sync::Arc<dyn AcceptancePolicy>);

impl std::ops::Deref for AcceptanceHandle {
    type Target = dyn AcceptancePolicy;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

/// The acceptance policy a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `GoalPlugin` was installed. A caller about to accept a
/// Goal must be told it has no policy rather than silently accepting on no
/// evidence.
pub fn acceptance_policy(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<std::sync::Arc<dyn AcceptancePolicy>> {
    services
        .get::<AcceptanceHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no acceptance policy installed; install GoalPlugin"))
}

#[cfg(test)]
mod plugin_tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    /// A policy that rejects everything, to prove replacement reaches the caller.
    struct AlwaysRejects;

    impl AcceptancePolicy for AlwaysRejects {
        fn verify(&self, _goal: &Goal) -> GoalVerification {
            GoalVerification::Rejected {
                criteria: vec!["forced".to_string()],
            }
        }
    }

    fn goal() -> Goal {
        Goal::new(crate::goal::GoalCreate {
            title: "Plugin test goal".to_string(),
            objective: "Exercise the policy port".to_string(),
            criteria: Vec::new(),
        })
        .expect("valid goal")
    }

    /// The plugin registers a policy the host can resolve.
    #[tokio::test]
    async fn the_plugin_provides_a_resolvable_policy() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(GoalPlugin::evidence()).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert!(acceptance_policy(runtime.services()).is_ok());
    }

    /// A host with no plugin is told so by name.
    #[test]
    fn a_missing_plugin_is_an_error_that_names_it() {
        let registry = tact_kernel::service::ServiceRegistry::default();
        let error = match acceptance_policy(&registry) {
            Ok(_) => panic!("no policy was installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("GoalPlugin"), "{error}");
    }

    /// The built-in policy is the pure function.
    ///
    /// Nothing about the trait may change the default decision — that is the whole
    /// point of keeping `GoalVerifier::verify` as the body.
    #[test]
    fn the_builtin_policy_agrees_with_the_pure_function() {
        let goal = goal();
        assert_eq!(
            EvidencePolicy.verify(&goal),
            GoalVerifier::verify(&goal),
            "the default must not drift from the function it wraps"
        );
    }

    /// A replacement policy is what the host gets.
    #[tokio::test]
    async fn a_replacement_policy_is_what_the_host_gets() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(GoalPlugin::new(std::sync::Arc::new(AlwaysRejects)))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();
        let policy = acceptance_policy(runtime.services()).unwrap();
        assert_eq!(
            policy.verify(&goal()),
            GoalVerification::Rejected {
                criteria: vec!["forced".to_string()]
            },
            "the installed policy decides, not the built-in one"
        );
    }

    /// The manifest says what it contributes.
    #[test]
    fn the_manifest_says_what_it_is() {
        use tact_kernel::plugin::Plugin;
        let manifest = GoalPlugin::evidence().manifest();
        assert_eq!(manifest.id.as_str(), "builtin.goal");
        assert_eq!(manifest.kind, tact_kernel::plugin::PluginKind::Capability);
    }
}
