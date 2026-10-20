use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tool_refactor_macros::tool;

use crate::{
    goal::GoalEvent,
    tool::{
        PermissionPolicy, ResourcePolicy, ToolCallResult, ToolContext, ToolEffect, ToolMetadata,
        ToolPresentation,
    },
};
use tact_protocol::ToolVisualKind;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalReadInput {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalCheckpointInput {
    #[schemars(description = "Non-empty progress summary.")]
    pub summary: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalRequestInputInput {
    #[schemars(description = "Non-empty question for the user.")]
    pub question: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalClaimInput {
    pub criterion_id: String,
    pub claim: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalProposeCompletionInput {
    #[schemars(description = "Non-empty completion summary.")]
    pub summary: String,
    /// What the model says it ran to check its own work — e.g.
    /// "cargo test --workspace: 1405 passed". These are recorded as the
    /// model's self-report, never as acceptance evidence.
    #[serde(default)]
    #[schemars(
        description = "Self-reported verification steps, one per entry (for example: \
                       \"cargo test --workspace: all green\"). Recorded verbatim as an \
                       untrusted self-report; they are not acceptance evidence.",
        default
    )]
    pub checks: Vec<String>,
    pub claims: Vec<GoalClaimInput>,
}

const GOAL_DISPLAY: &str = "🎯 Goal";

const fn goal_metadata(
    name: &'static str,
    description: &'static str,
    permission: PermissionPolicy,
    resources: ResourcePolicy,
) -> ToolMetadata {
    ToolMetadata {
        name,
        description,
        permission,
        permission_prompt: crate::tool::PermissionPromptPolicy::Json,
        resources,
        domain: crate::tool::ToolDomain::Generic,
        presentation: ToolPresentation {
            visual_kind: ToolVisualKind::Generic,
            display_name: GOAL_DISPLAY,
            live_output: crate::tool::LiveOutputPolicy::Standard,
            detail: crate::tool::DetailPolicy::Result,
            popup: crate::tool::PopupPolicy::None,
            compact_result_to_meta: false,
        },
        output: crate::tool::OutputPolicy::KeepInline,
        argument_summary: crate::tool::ArgumentSummaryPolicy::Json,
    }
}

pub const GOAL_READ_METADATA: ToolMetadata = goal_metadata(
    "goal_read",
    "Read the currently bound Goal snapshot.",
    PermissionPolicy::Read,
    ResourcePolicy::Independent,
);
pub const GOAL_CHECKPOINT_METADATA: ToolMetadata = goal_metadata(
    "goal_checkpoint",
    "Append a progress checkpoint to the currently bound Goal.",
    PermissionPolicy::Write,
    ResourcePolicy::SharedState { scope: "goal" },
);
pub const GOAL_REQUEST_INPUT_METADATA: ToolMetadata = goal_metadata(
    "goal_request_input",
    "Ask the user a question through the Goal Runner.",
    PermissionPolicy::Write,
    ResourcePolicy::Barrier,
);
pub const GOAL_PROPOSE_COMPLETION_METADATA: ToolMetadata = goal_metadata(
    "goal_propose_completion",
    "Propose Goal completion with untrusted criterion claims and, when you can, \
     self-reported checks of what you ran.",
    PermissionPolicy::Write,
    ResourcePolicy::SharedState { scope: "goal" },
);

fn active_goal_id(ctx: &ToolContext) -> Result<&str> {
    ctx.active_goal_id
        .as_deref()
        .ok_or_else(|| anyhow!("no active Goal is bound to this tool context"))
}

fn non_empty(value: String, field: &str) -> Result<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        bail!("{field} must not be empty")
    }
    Ok(value)
}

fn event(
    goal_id: &str,
    kind: crate::goal::GoalEventKind,
    payload: impl Serialize,
) -> Result<GoalEvent> {
    Ok(GoalEvent {
        id: crate::goal::new_event_id(),
        goal_id: goal_id.to_string(),
        run_id: None,
        kind,
        // Every event this module writes comes from the model.
        source: crate::goal::GoalEventSource::Model,
        status: None,
        outcome: None,
        payload: Some(serde_json::to_string(&payload)?),
        occurred_at: chrono::Utc::now(),
    })
}

fn event_text(goal_id: &str, kind: crate::goal::GoalEventKind, text: String) -> GoalEvent {
    GoalEvent {
        id: crate::goal::new_event_id(),
        goal_id: goal_id.to_string(),
        run_id: None,
        kind,
        // Every event this module writes comes from the model.
        source: crate::goal::GoalEventSource::Model,
        status: None,
        outcome: None,
        payload: Some(text),
        occurred_at: chrono::Utc::now(),
    }
}

#[tool]
pub async fn goal_read(ctx: ToolContext, _input: GoalReadInput) -> Result<String> {
    let goal = ctx.goal_manager.get(active_goal_id(&ctx)?).await?;
    Ok(serde_json::to_string(&goal)?)
}

#[tool]
pub async fn goal_checkpoint(ctx: ToolContext, input: GoalCheckpointInput) -> Result<String> {
    let goal_id = active_goal_id(&ctx)?;
    let summary = non_empty(input.summary, "summary")?;
    ctx.goal_manager
        .append_event(event(
            goal_id,
            crate::goal::GoalEventKind::Checkpoint,
            serde_json::json!({"summary": summary}),
        )?)
        .await?;
    Ok("Checkpoint recorded for the active Goal.".to_string())
}

#[tool]
pub async fn goal_request_input(
    ctx: ToolContext,
    input: GoalRequestInputInput,
) -> Result<ToolCallResult> {
    let goal_id = active_goal_id(&ctx)?;
    let question = non_empty(input.question, "question")?;
    ctx.goal_manager
        .append_event(event_text(
            goal_id,
            crate::goal::GoalEventKind::NeedsUser,
            question,
        ))
        .await?;
    Ok(ToolCallResult {
        content: "User input requested; yield to Runner.".to_string(),
        effects: vec![ToolEffect::GoalYield {
            reason: crate::tool::GoalYieldReason::NeedsUser,
        }],
        image: None,
    })
}

#[tool]
pub async fn goal_propose_completion(
    ctx: ToolContext,
    input: GoalProposeCompletionInput,
) -> Result<String> {
    let goal_id = active_goal_id(&ctx)?;
    let summary = non_empty(input.summary, "summary")?;
    let checks: Vec<String> = input
        .checks
        .into_iter()
        .map(|check| non_empty(check, "checks"))
        .collect::<Result<_>>()?;
    let claims: Vec<_> = input
        .claims
        .into_iter()
        .map(|claim| {
            let criterion_id = non_empty(claim.criterion_id, "criterion_id")?;
            let claim_text = non_empty(claim.claim, "claim")?;
            Ok(serde_json::json!({
                "criterion_id": criterion_id,
                "claim": claim_text,
                "untrusted": true,
            }))
        })
        .collect::<Result<_>>()?;
    ctx.goal_manager
        .append_event(event(
            goal_id,
            crate::goal::GoalEventKind::CompletionProposed,
            serde_json::json!({
                "summary": summary,
                // The model's own account of how it checked its work. Kept
                // next to — never in place of — acceptance evidence.
                "self_reported_checks": checks,
                "claims": claims,
            }),
        )?)
        .await?;
    Ok("Completion proposal recorded; yield to Runner for verification.".to_string())
}

#[cfg(test)]
mod goal_tool_tests {
    use crate::{
        goal::{GoalCreate, GoalCriterion},
        tool::{ToolRouter, test_support::test_context},
    };

    fn goal_input(title: &str) -> GoalCreate {
        GoalCreate {
            title: title.into(),
            objective: format!("objective for {title}"),
            criteria: vec![GoalCriterion::required_with_id("criterion-1", "done")],
        }
    }

    async fn bound_context(name: &str) -> (crate::tool::ToolContext, String) {
        let mut context = test_context(name);
        let goal = context
            .goal_manager
            .create(goal_input("bound"))
            .await
            .unwrap();
        context.active_goal_id = Some(goal.id.clone());
        (context, goal.id)
    }

    #[tokio::test]
    async fn request_input_metadata_is_a_scheduler_barrier() {
        assert_eq!(
            crate::tool::goal::GOAL_REQUEST_INPUT_METADATA.resources,
            crate::tool::ResourcePolicy::Barrier
        );
    }
    #[tokio::test]
    async fn request_input_returns_typed_yield_and_persists_event() {
        let (context, goal_id) = bound_context("goal_tool_typed_yield").await;
        let result = crate::tool::test_support::run_tool_result(
            &context,
            crate::tool::goal::GoalRequestInputTool,
            "goal_request_input",
            serde_json::json!({"question": "what next?"}),
        )
        .await
        .unwrap();

        assert_eq!(
            result.effects,
            vec![crate::tool::ToolEffect::GoalYield {
                reason: crate::tool::GoalYieldReason::NeedsUser,
            }]
        );
        assert_eq!(
            context
                .goal_manager
                .list_events(&goal_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// Everything the Goal tools append is model-authored. The audit trail
    /// distinguishes it from a user decision, which is what makes the
    /// "a model tool cannot forge user acceptance" rule checkable rather than
    /// remembered (2026-10-20).
    #[tokio::test]
    async fn goal_tool_events_are_model_authored() {
        let (context, goal_id) = bound_context("goal_tool_source").await;
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalCheckpointTool)
            .unwrap()
            .route(crate::tool::goal::GoalRequestInputTool)
            .unwrap();
        router
            .call(
                &context,
                "goal_checkpoint",
                serde_json::json!({"summary": "progress"}),
            )
            .await
            .unwrap();
        router
            .call(
                &context,
                "goal_request_input",
                serde_json::json!({"question": "which one?"}),
            )
            .await
            .unwrap();

        let events = context.goal_manager.list_events(&goal_id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .all(|event| event.source == crate::goal::GoalEventSource::Model),
            "every Goal tool event is model-authored: {events:?}"
        );
    }

    #[tokio::test]
    async fn goal_tools_fail_closed_without_binding_and_reject_goal_id_input() {
        let context = test_context("goal_tool_fail_closed");
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalReadTool)
            .unwrap()
            .route(crate::tool::goal::GoalCheckpointTool)
            .unwrap()
            .route(crate::tool::goal::GoalRequestInputTool)
            .unwrap()
            .route(crate::tool::goal::GoalProposeCompletionTool)
            .unwrap();
        for (name, input) in [
            ("goal_read", serde_json::json!({})),
            (
                "goal_checkpoint",
                serde_json::json!({"summary": "progress"}),
            ),
            (
                "goal_request_input",
                serde_json::json!({"question": "which?"}),
            ),
            (
                "goal_propose_completion",
                serde_json::json!({"summary": "done", "claims": []}),
            ),
        ] {
            let error = router.call(&context, name, input).await.unwrap_err();
            assert!(error.to_string().contains("active Goal"), "{name}: {error}");
        }
        let (context, _) = bound_context("goal_tool_no_model_id").await;
        let error = router
            .call(
                &context,
                "goal_read",
                serde_json::json!({"goal_id": "other"}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[tokio::test]
    async fn goal_inputs_reject_unknown_top_level_and_nested_keys() {
        let (context, goal_id) = bound_context("goal_tool_unknown_keys").await;
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalCheckpointTool)
            .unwrap()
            .route(crate::tool::goal::GoalProposeCompletionTool)
            .unwrap()
            .route(crate::tool::goal::GoalRequestInputTool)
            .unwrap();

        let top_level = router
            .call(
                &context,
                "goal_checkpoint",
                serde_json::json!({"summary": "progress", "goal_id": "other"}),
            )
            .await
            .unwrap_err();
        assert!(
            top_level.to_string().contains("unknown field"),
            "{top_level}"
        );
        assert!(
            context
                .goal_manager
                .list_events(&goal_id)
                .await
                .unwrap()
                .is_empty()
        );

        let nested = router
            .call(
                &context,
                "goal_propose_completion",
                serde_json::json!({
                    "summary": "done",
                    "claims": [{
                        "criterion_id": "criterion-1",
                        "claim": "done",
                        "untrusted": false
                    }]
                }),
            )
            .await
            .unwrap_err();
        assert!(nested.to_string().contains("unknown field"), "{nested}");
        assert!(
            context
                .goal_manager
                .list_events(&goal_id)
                .await
                .unwrap()
                .is_empty()
        );

        // The yield trigger is the one call whose effects stop the Runner, so a
        // rejected schema must not leave a `needs_user` event behind either.
        let request_input = router
            .call(
                &context,
                "goal_request_input",
                serde_json::json!({"question": "which?", "goal_id": "other"}),
            )
            .await
            .unwrap_err();
        assert!(
            request_input.to_string().contains("unknown field"),
            "{request_input}"
        );
        assert!(
            context
                .goal_manager
                .list_events(&goal_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn completion_proposal_rejects_blank_claim_fields_without_event() {
        let (context, goal_id) = bound_context("goal_tool_blank_claims").await;
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalProposeCompletionTool)
            .unwrap();

        for claim in [
            serde_json::json!({"criterion_id": "", "claim": "done"}),
            serde_json::json!({"criterion_id": "criterion-1", "claim": "   "}),
        ] {
            let error = router
                .call(
                    &context,
                    "goal_propose_completion",
                    serde_json::json!({"summary": "done", "claims": [claim]}),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("must not be empty"), "{error}");
        }
        assert!(
            context
                .goal_manager
                .list_events(&goal_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn stale_active_goal_mutators_fail_without_events() {
        let context = test_context("goal_tool_stale_binding");
        let mut context = context;
        context.active_goal_id = Some("missing-active-goal".into());
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalCheckpointTool)
            .unwrap()
            .route(crate::tool::goal::GoalRequestInputTool)
            .unwrap()
            .route(crate::tool::goal::GoalProposeCompletionTool)
            .unwrap();

        for (name, input) in [
            (
                "goal_checkpoint",
                serde_json::json!({"summary": "progress"}),
            ),
            (
                "goal_request_input",
                serde_json::json!({"question": "which?"}),
            ),
            (
                "goal_propose_completion",
                serde_json::json!({"summary": "done", "claims": []}),
            ),
        ] {
            let error = router.call(&context, name, input).await.unwrap_err();
            assert!(error.to_string().contains("not found"), "{name}: {error}");
        }
        assert!(
            context
                .goal_manager
                .list_events("missing-active-goal")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn goal_read_returns_only_bound_goal() {
        let (mut context, bound_id) = bound_context("goal_tool_read_bound").await;
        let other = context
            .goal_manager
            .create(goal_input("other"))
            .await
            .unwrap();
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalReadTool)
            .unwrap();
        let output = router
            .call(&context, "goal_read", serde_json::json!({}))
            .await
            .unwrap();
        assert!(output.contains(&bound_id));
        assert!(!output.contains(&other.id));
        context.active_goal_id = Some(other.id.clone());
        let switched = router
            .call(&context, "goal_read", serde_json::json!({}))
            .await
            .unwrap();
        assert!(switched.contains(&other.id));
        assert!(!switched.contains(&bound_id));
    }

    #[tokio::test]
    async fn goal_mutators_append_typed_events_only_to_active_goal() {
        let (context, goal_id) = bound_context("goal_tool_events").await;
        let other = context
            .goal_manager
            .create(goal_input("other"))
            .await
            .unwrap();
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalCheckpointTool)
            .unwrap()
            .route(crate::tool::goal::GoalRequestInputTool)
            .unwrap()
            .route(crate::tool::goal::GoalProposeCompletionTool)
            .unwrap();
        router
            .call(
                &context,
                "goal_checkpoint",
                serde_json::json!({"summary": "made progress"}),
            )
            .await
            .unwrap();
        router
            .call(
                &context,
                "goal_request_input",
                serde_json::json!({"question": "choose a color"}),
            )
            .await
            .unwrap();
        let completion = router.call(&context, "goal_propose_completion", serde_json::json!({"summary": "all done", "claims": [{"criterion_id": "criterion-1", "claim": "it is done"}]})).await.unwrap();
        assert!(completion.contains("yield to Runner"));
        let events = context.goal_manager.list_events(&goal_id).await.unwrap();
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                crate::goal::GoalEventKind::Checkpoint,
                crate::goal::GoalEventKind::NeedsUser,
                crate::goal::GoalEventKind::CompletionProposed,
            ]
        );
        assert!(
            events[0]
                .payload
                .as_deref()
                .unwrap()
                .contains("made progress")
        );
        assert!(
            events[1]
                .payload
                .as_deref()
                .unwrap()
                .contains("choose a color")
        );
        assert!(
            events[2]
                .payload
                .as_deref()
                .unwrap()
                .contains("criterion-1")
        );
        assert!(events[2].payload.as_deref().unwrap().contains("it is done"));
        assert!(
            context
                .goal_manager
                .list_events(&other.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn completion_claims_are_untrusted_and_do_not_change_goal_state() {
        let (context, goal_id) = bound_context("goal_tool_untrusted_claims").await;
        let before = context.goal_manager.get(&goal_id).await.unwrap();
        let router = ToolRouter::new()
            .route(crate::tool::goal::GoalProposeCompletionTool)
            .unwrap();
        router
            .call(
                &context,
                "goal_propose_completion",
                serde_json::json!({
                    "summary": "trust me",
                    "checks": ["cargo test --workspace: all green"],
                    "claims": [{"criterion_id": "criterion-1", "claim": "passed"}]
                }),
            )
            .await
            .unwrap();
        let after = context.goal_manager.get(&goal_id).await.unwrap();
        assert_eq!(after.status, before.status);
        assert_eq!(after.criteria, before.criteria);
        assert_eq!(after.version, before.version);
        let event = context
            .goal_manager
            .list_events(&goal_id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(event.payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload["claims"][0]["claim"], "passed");
        assert_eq!(payload["claims"][0]["untrusted"], true);
        assert_eq!(
            payload["self_reported_checks"][0], "cargo test --workspace: all green",
            "the model's own account is recorded next to — never as — evidence"
        );
    }

    #[test]
    fn goal_tools_are_main_only_and_metadata_is_scoped() {
        let main = crate::tool::toolset();
        let subagent = crate::tool::subagent_toolset();
        for name in [
            "goal_read",
            "goal_checkpoint",
            "goal_request_input",
            "goal_propose_completion",
        ] {
            assert!(main.resolve(name).is_ok(), "missing {name}");
            assert!(subagent.resolve(name).is_err(), "subagent received {name}");
            let metadata = main.resolve(name).unwrap().metadata();
            match name {
                "goal_read" => {
                    assert_eq!(metadata.permission, crate::tool::PermissionPolicy::Read);
                    assert_eq!(metadata.resources, crate::tool::ResourcePolicy::Independent);
                }
                "goal_request_input" => {
                    assert_eq!(metadata.permission, crate::tool::PermissionPolicy::Write);
                    assert_eq!(metadata.resources, crate::tool::ResourcePolicy::Barrier);
                }
                "goal_checkpoint" | "goal_propose_completion" => {
                    assert_eq!(metadata.permission, crate::tool::PermissionPolicy::Write);
                    assert_eq!(
                        metadata.resources,
                        crate::tool::ResourcePolicy::SharedState { scope: "goal" }
                    );
                }
                _ => unreachable!(),
            }
            assert_eq!(metadata.presentation.display_name, "🎯 Goal");
        }
    }

    #[test]
    fn invocation_clone_preserves_goal_binding_and_bootstrap_defaults_to_none() {
        let context = test_context("goal_tool_clone_binding");
        assert!(context.active_goal_id.is_none());
        let mut bound = context.clone();
        bound.active_goal_id = Some("bound".into());
        assert_eq!(
            bound.for_invocation("goal_read").active_goal_id,
            Some("bound".into())
        );
    }
}
