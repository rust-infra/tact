//! Product implementation of the kernel tool gate.

use std::{fmt::Display, future::Future, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tact_agent_api::{
    AgentContentBlock, CancelToken, GateDecision, ResolvedToolCall, ToolCall, ToolExecutionError,
    ToolExecutionErrorKind, ToolExecutionResult, ToolExecutionStatus, ToolGate, ToolGateContext,
    ToolGateError, ToolResolver,
};
use tact_llm::Message;
use tact_protocol::StepStatus;

use super::{
    activation_view::{ModelIdentity, SessionIdentity, TactActivationView},
    permission_authority::PermissionAuthority,
    services::TactSharedServices,
};
use crate::{
    hook::{
        Hook, HookControl, NotificationContext, NotificationFn, PermissionRequestFn,
        PostToolUseFailureFn, PostToolUseFn, PreToolUseFn, SessionStartSource, ToolResult, ToolUse,
    },
    permission::{CapabilityRisk, PermissionBehavior, PermissionManager, format_permission_prompt},
    security::{
        redact::{level_for_call, redact},
        sensitive::refusal_text_for,
    },
    tool::PermissionPromptPolicy,
    ui_responder::UiResponder,
};

/// What the interactive permission popup's selection means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionChoice {
    AllowOnce,
    AlwaysAllow,
    AlwaysAllowProgram,
    Deny,
}

pub(crate) const PERMISSION_OPTIONS: [&str; 4] = [
    "Allow once",
    "Deny",
    "Always allow this tool",
    "Always allow this pattern",
];

/// The options to show, dropping the program-level one when no rule for it
/// could be built. Removing the last entry keeps every other index stable.
pub(crate) fn permission_options(with_program_choice: bool) -> Vec<String> {
    let count = if with_program_choice {
        PERMISSION_OPTIONS.len()
    } else {
        PERMISSION_OPTIONS.len() - 1
    };
    PERMISSION_OPTIONS[..count]
        .iter()
        .map(|option| (*option).to_string())
        .collect()
}

/// Map a popup selection to a typed decision. Anything unexpected denies.
pub(crate) fn permission_choice_for(selection: Option<usize>) -> PermissionChoice {
    match selection {
        Some(0) => PermissionChoice::AllowOnce,
        Some(2) => PermissionChoice::AlwaysAllow,
        Some(3) => PermissionChoice::AlwaysAllowProgram,
        _ => PermissionChoice::Deny,
    }
}

#[derive(Clone, Default)]
struct TactToolHooks {
    pre_tool: Vec<Arc<dyn PreToolUseFn>>,
    permission_request: Vec<Arc<dyn PermissionRequestFn>>,
    post_tool: Vec<Arc<dyn PostToolUseFn>>,
    post_tool_failure: Vec<Arc<dyn PostToolUseFailureFn>>,
    notification: Vec<Arc<dyn NotificationFn>>,
}

impl TactToolHooks {
    fn from_hooks(hooks: &[Hook]) -> Self {
        let mut selected = Self::default();
        for hook in hooks {
            match hook {
                Hook::PreToolUse(hook) => selected.pre_tool.push(hook.clone()),
                Hook::PermissionRequest(hook) => selected.permission_request.push(hook.clone()),
                Hook::PostToolUse(hook) => selected.post_tool.push(hook.clone()),
                Hook::PostToolUseFailure(hook) => selected.post_tool_failure.push(hook.clone()),
                Hook::Notification(hook) => selected.notification.push(hook.clone()),
                _ => {}
            }
        }
        selected
    }
}

/// Tact's hook, permission, prompt, and redaction policy.
///
/// The gate deliberately cannot execute a model tool or write transcript,
/// usage, cursor, or kernel-event state.
pub(crate) struct TactToolGate {
    services: Arc<TactSharedServices>,
    ui: UiResponder,
    hooks: TactToolHooks,
    transcript: Option<Vec<Message>>,
    model: ModelIdentity,
    session_start_source: SessionStartSource,
    is_subagent: bool,
}

impl TactToolGate {
    pub(crate) fn new(
        services: Arc<TactSharedServices>,
        ui: UiResponder,
        hooks: &[Hook],
        transcript: Option<Vec<Message>>,
        model: ModelIdentity,
        session_start_source: SessionStartSource,
        is_subagent: bool,
    ) -> Self {
        Self {
            services,
            ui,
            hooks: TactToolHooks::from_hooks(hooks),
            transcript,
            model,
            session_start_source,
            is_subagent,
        }
    }

    fn activation_view<'a>(&'a self, context: &ToolGateContext) -> TactActivationView<'a> {
        TactActivationView::new(
            self.services.as_ref(),
            self.transcript.as_deref(),
            SessionIdentity {
                id: Some(context.session.as_str().to_string()),
            },
            self.model.clone(),
            context.total_turns,
            context.remaining_turns,
            self.session_start_source,
        )
    }

    fn permissions(&self) -> &PermissionAuthority {
        self.services.permissions()
    }

    fn deny(reason: impl Into<String>) -> GateDecision {
        GateDecision::Deny {
            reason: reason.into(),
        }
    }

    fn risk_escalates(original: CapabilityRisk, final_risk: CapabilityRisk) -> bool {
        matches!(
            (original, final_risk),
            (
                CapabilityRisk::Read,
                CapabilityRisk::Write | CapabilityRisk::High
            ) | (CapabilityRisk::Write, CapabilityRisk::High)
        )
    }

    fn tool_use(call: &ToolCall) -> ToolUse {
        ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        }
    }

    fn call_from_mutation(original_id: &str, tool: &ToolUse) -> ToolCall {
        ToolCall {
            // The call id is the pairing key between ToolUse and ToolResult.
            // Hooks may rewrite the operation, never the identity.
            id: original_id.to_string(),
            name: tool.name.clone(),
            input: tool.input.clone(),
        }
    }

    fn classify(
        resolved: ResolvedToolCall,
    ) -> Result<Result<(ResolvedToolCall, ToolPolicy), GateDecision>, ToolGateError> {
        let policy = ToolPolicy::from_resolved(&resolved)?;
        if policy
            .sensitive
            .as_ref()
            .is_some_and(ToolPolicy::is_credential)
        {
            let sensitive = policy.sensitive.as_ref().expect("checked above");
            return Ok(Err(Self::deny(refusal_text_for(
                &sensitive.matched,
                &sensitive.kind,
            ))));
        }
        Ok(Ok((resolved, policy)))
    }

    async fn resolve_and_classify(
        call: &ToolCall,
        resolver: &dyn ToolResolver,
    ) -> Result<Result<(ResolvedToolCall, ToolPolicy), GateDecision>, ToolGateError> {
        let resolved = match resolver.resolve(call) {
            Ok(resolved) => resolved,
            Err(error) if error.kind == tact_agent_api::ToolErrorKind::UnknownTool => {
                return Ok(Err(Self::deny(error.message)));
            }
            Err(error) => return Err(ToolGateError::Failed(error.to_string())),
        };
        Self::classify(resolved)
    }

    async fn run_pre_tool(
        &self,
        context: &ToolGateContext,
        tool: &mut ToolUse,
        cancel: &CancelToken,
    ) -> Result<HookControl, ToolGateError> {
        let mut control = HookControl::Continue;
        for hook in &self.hooks.pre_tool {
            let view = self.activation_view(context);
            match await_or_cancel(cancel, hook(&view, tool)).await? {
                HookControl::Continue => {}
                HookControl::Allow => {
                    if matches!(control, HookControl::Continue) {
                        control = HookControl::Allow;
                    }
                }
                HookControl::Block(reason) => return Ok(HookControl::Block(reason)),
            }
        }
        Ok(control)
    }

    async fn run_permission_request(
        &self,
        context: &ToolGateContext,
        tool: &mut ToolUse,
        cancel: &CancelToken,
    ) -> Result<HookControl, ToolGateError> {
        let mut control = HookControl::Continue;
        for hook in &self.hooks.permission_request {
            let view = self.activation_view(context);
            match await_or_cancel(cancel, hook(&view, tool)).await? {
                HookControl::Continue => {}
                HookControl::Allow => {
                    if matches!(control, HookControl::Continue) {
                        control = HookControl::Allow;
                    }
                }
                HookControl::Block(reason) => return Ok(HookControl::Block(reason)),
            }
        }
        Ok(control)
    }

    async fn run_notification(
        &self,
        context: &ToolGateContext,
        notification: NotificationContext,
        cancel: &CancelToken,
    ) -> Result<(), ToolGateError> {
        for hook in &self.hooks.notification {
            let view = self.activation_view(context);
            let control = match await_or_cancel(cancel, hook(&view, &notification)).await {
                Ok(control) => control,
                Err(ToolGateError::Cancelled) => return Err(ToolGateError::Cancelled),
                Err(error) => {
                    self.services
                        .emit_update(tact_protocol::AgentUpdate::Info(format!(
                            "[Notification hook failed] {error}"
                        )));
                    continue;
                }
            };
            match control {
                HookControl::Continue | HookControl::Allow => {}
                HookControl::Block(reason) => {
                    self.services
                        .emit_update(tact_protocol::AgentUpdate::Info(format!(
                            "[Notification hook blocked] {reason}"
                        )))
                }
            }
        }
        Ok(())
    }

    async fn choose_permission(
        &self,
        stable_name: &str,
        risk: CapabilityRisk,
        prompt_policy: PermissionPromptPolicy,
        input: &Value,
        cancel: &CancelToken,
    ) -> Result<PermissionChoice, ToolGateError> {
        let prefix_rule = PermissionManager::prefix_rule_for(stable_name, prompt_policy, input);
        let Some(ui_tx) = self.services.ui_sender() else {
            let approved = self
                .permissions()
                .ask_user(stable_name, risk)
                .map_err(|error| ToolGateError::Failed(error.to_string()))?;
            return Ok(if approved {
                PermissionChoice::AllowOnce
            } else {
                PermissionChoice::Deny
            });
        };

        let mut prompt = format_permission_prompt(stable_name, prompt_policy, input);
        if let Some(rule) = &prefix_rule {
            prompt.push_str(&format!(
                "\n\n\"Always allow this pattern\" would record: {}",
                rule.to_rule_string()
            ));
        }
        let selection = await_or_cancel(
            cancel,
            self.ui.request_select(
                ui_tx,
                prompt,
                permission_options(prefix_rule.is_some()),
                false,
                self.is_subagent,
            ),
        )
        .await?;
        Ok(permission_choice_for(selection))
    }

    async fn run_post_tool(
        &self,
        context: &ToolGateContext,
        tool: &ToolUse,
        result: &mut ToolResult,
        status: StepStatus,
        cancel: &CancelToken,
    ) -> Result<HookControl, ToolGateError> {
        let mut control = HookControl::Continue;
        for hook in &self.hooks.post_tool {
            let view = self.activation_view(context);
            match await_or_cancel(cancel, hook(&view, tool, result, status)).await? {
                HookControl::Continue => {}
                HookControl::Allow => {
                    if matches!(control, HookControl::Continue) {
                        control = HookControl::Allow;
                    }
                }
                HookControl::Block(reason) => return Ok(HookControl::Block(reason)),
            }
        }
        Ok(control)
    }

    async fn run_post_tool_failure(
        &self,
        context: &ToolGateContext,
        tool: &ToolUse,
        error: &str,
        cancel: &CancelToken,
    ) -> Result<(), ToolGateError> {
        for hook in &self.hooks.post_tool_failure {
            let view = self.activation_view(context);
            match await_or_cancel(cancel, hook(&view, tool, error)).await? {
                HookControl::Continue | HookControl::Allow => {}
                HookControl::Block(reason) => {
                    self.services
                        .emit_update(tact_protocol::AgentUpdate::Info(format!(
                            "[PostToolUseFailure hook blocked] {reason}"
                        )))
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ToolGate for TactToolGate {
    async fn before(
        &self,
        call: ToolCall,
        resolver: &dyn ToolResolver,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<GateDecision, ToolGateError> {
        if cancel.is_cancelled() {
            return Err(ToolGateError::Cancelled);
        }

        let original_id = call.id.clone();
        let (_, original_policy) = match Self::resolve_and_classify(&call, resolver).await? {
            Ok(classified) => classified,
            Err(denied) => return Ok(denied),
        };
        let original_risk = original_policy.risk()?;

        let mut tool = Self::tool_use(&call);
        let pre_control = self.run_pre_tool(context, &mut tool, &cancel).await?;
        if let HookControl::Block(reason) = pre_control {
            return Ok(Self::deny(format!(
                "Tool blocked by PreToolUse hook: {reason}"
            )));
        }

        let mutated = Self::call_from_mutation(&original_id, &tool);
        let (resolved, policy) = match Self::resolve_and_classify(&mutated, resolver).await? {
            Ok(classified) => classified,
            Err(denied) => return Ok(denied),
        };

        let risk = policy.risk()?;
        if matches!(pre_control, HookControl::Allow) && !Self::risk_escalates(original_risk, risk) {
            return Ok(GateDecision::Allow {
                call: Box::new(resolved),
                label: Some("Allowed by PreToolUse hook".to_string()),
            });
        }

        let decision = self.permissions().check_with_auto(
            &policy.stable_name,
            risk,
            &tool.input,
            policy.auto_approved,
        );
        match decision.behavior {
            PermissionBehavior::Allow => {
                return Ok(GateDecision::Allow {
                    call: Box::new(resolved),
                    label: None,
                });
            }
            PermissionBehavior::Deny => {
                return Ok(Self::deny(format!(
                    "Permission denied: {}",
                    decision.reason
                )));
            }
            PermissionBehavior::Ask => {}
        }

        let permission_original_risk = risk;
        let permission_control = match self
            .run_permission_request(context, &mut tool, &cancel)
            .await
        {
            Ok(control) => control,
            Err(ToolGateError::Cancelled) => return Err(ToolGateError::Cancelled),
            Err(error) => {
                self.services
                    .emit_update(tact_protocol::AgentUpdate::Info(format!(
                        "[PermissionRequest hook failed] {error}"
                    )));
                HookControl::Continue
            }
        };

        if let HookControl::Block(reason) = permission_control {
            return Ok(Self::deny(format!(
                "Permission denied by PermissionRequest hook: {reason}"
            )));
        }

        let mutated = Self::call_from_mutation(&original_id, &tool);
        let (resolved, policy) = match Self::resolve_and_classify(&mutated, resolver).await? {
            Ok(classified) => classified,
            Err(denied) => return Ok(denied),
        };

        let prompt_risk = policy.risk()?;
        if matches!(permission_control, HookControl::Allow)
            && !Self::risk_escalates(permission_original_risk, prompt_risk)
        {
            return Ok(GateDecision::Allow {
                call: Box::new(resolved),
                label: Some("Allowed by PermissionRequest hook".to_string()),
            });
        }

        let prompt_decision = self.permissions().check_with_auto(
            &policy.stable_name,
            prompt_risk,
            &tool.input,
            policy.auto_approved,
        );
        match prompt_decision.behavior {
            PermissionBehavior::Allow => {
                return Ok(GateDecision::Allow {
                    call: Box::new(resolved),
                    label: None,
                });
            }
            PermissionBehavior::Deny => {
                return Ok(Self::deny(format!(
                    "Permission denied: {}",
                    prompt_decision.reason
                )));
            }
            PermissionBehavior::Ask => {}
        }

        let prompt_policy = policy.prompt_policy()?;
        let notification = NotificationContext {
            notification_type: "permission_prompt".to_string(),
            title: policy.stable_name.clone(),
            message: format_permission_prompt(&policy.stable_name, prompt_policy, &tool.input),
        };
        self.run_notification(context, notification, &cancel)
            .await?;

        if cancel.is_cancelled() {
            return Err(ToolGateError::Cancelled);
        }
        let choice = self
            .choose_permission(
                &policy.stable_name,
                prompt_risk,
                prompt_policy,
                &tool.input,
                &cancel,
            )
            .await?;
        if cancel.is_cancelled() {
            return Err(ToolGateError::Cancelled);
        }

        let label = match choice {
            PermissionChoice::AllowOnce => {
                return Ok(GateDecision::Allow {
                    call: Box::new(resolved),
                    label: Some("Allow once".to_string()),
                });
            }
            PermissionChoice::AlwaysAllowProgram => {
                let _ = self.permissions().allow_tool_as_prefix(
                    &policy.stable_name,
                    prompt_policy,
                    &tool.input,
                );
                Some("Always allow this pattern".to_string())
            }
            PermissionChoice::AlwaysAllow => {
                let _ = self.permissions().allow_tool_with_input(
                    &policy.stable_name,
                    prompt_policy,
                    &tool.input,
                );
                Some("Always allow this tool".to_string())
            }
            PermissionChoice::Deny => {
                return Ok(Self::deny(format!(
                    "Permission denied by user for {}",
                    policy.stable_name
                )));
            }
        };

        Ok(GateDecision::Allow {
            call: Box::new(resolved),
            label,
        })
    }

    async fn after(
        &self,
        call: &ResolvedToolCall,
        result: ToolExecutionResult,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<ToolExecutionResult, ToolGateError> {
        if cancel.is_cancelled() {
            return Err(ToolGateError::Cancelled);
        }

        let policy = ToolPolicy::from_resolved(call)?;
        let original_text = content_text(&result.outcome.content);
        let original_error = result.error.clone();
        let original_status = result.status;
        let directives = result.outcome.directives.clone();
        let metadata = result.outcome.metadata.clone();
        let tool = ToolUse {
            id: call.call_id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        };
        let mut hook_result = ToolResult {
            tool_use_id: call.call_id.clone(),
            content: original_text.clone(),
        };
        let status = step_status(original_status);

        let post_control = match self
            .run_post_tool(context, &tool, &mut hook_result, status, &cancel)
            .await
        {
            Ok(control) => control,
            Err(ToolGateError::Cancelled) => return Err(ToolGateError::Cancelled),
            Err(error) => {
                let failed = failed_hook_result(format!("PostToolUse hook failed: {error}"));
                return Ok(redact_result(failed, &policy, self.permissions()));
            }
        };

        if original_status == ToolExecutionStatus::Failed {
            match self
                .run_post_tool_failure(context, &tool, &original_text, &cancel)
                .await
            {
                Ok(()) => {}
                Err(ToolGateError::Cancelled) => return Err(ToolGateError::Cancelled),
                Err(error) => self
                    .services
                    .emit_update(tact_protocol::AgentUpdate::Info(format!(
                        "[PostToolUseFailure hook failed] {error}"
                    ))),
            }
        }

        let mut final_result = match post_control {
            HookControl::Continue | HookControl::Allow
                if original_status == ToolExecutionStatus::Succeeded =>
            {
                ToolExecutionResult {
                    status: ToolExecutionStatus::Succeeded,
                    outcome: outcome_with_text(
                        hook_result.content,
                        &result.outcome.content,
                        directives,
                        metadata,
                    ),
                    error: None,
                }
            }
            HookControl::Block(reason) => {
                failed_hook_result(format!("Tool blocked by PostToolUse hook: {reason}"))
            }
            HookControl::Continue | HookControl::Allow => ToolExecutionResult {
                status: ToolExecutionStatus::Failed,
                outcome: outcome_with_text(
                    hook_result.content,
                    &result.outcome.content,
                    directives,
                    metadata,
                ),
                error: original_error,
            },
        };
        final_result = redact_result(final_result, &policy, self.permissions());
        Ok(final_result)
    }
}

fn outcome_with_text(
    text: String,
    content: &[AgentContentBlock],
    directives: Vec<tact_agent_api::ToolDirective>,
    metadata: Option<Value>,
) -> tact_agent_api::ToolOutcome {
    tact_agent_api::ToolOutcome {
        content: replace_text_content(text, content),
        directives,
        metadata,
    }
}

fn failed_hook_result(message: String) -> ToolExecutionResult {
    ToolExecutionResult::failure(ToolExecutionError {
        kind: ToolExecutionErrorKind::Adapter,
        message,
    })
}

fn step_status(status: ToolExecutionStatus) -> StepStatus {
    match status {
        ToolExecutionStatus::Succeeded => StepStatus::Success,
        ToolExecutionStatus::Failed => StepStatus::Failed,
    }
}

fn content_text(content: &[AgentContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::Text { text }
            | AgentContentBlock::ToolResult { content: text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn replace_text_content(text: String, original: &[AgentContentBlock]) -> Vec<AgentContentBlock> {
    let mut content = vec![AgentContentBlock::Text { text }];
    content.extend(
        original
            .iter()
            .filter(|block| matches!(block, AgentContentBlock::Image { .. }))
            .cloned(),
    );
    content
}

fn redact_result(
    mut result: ToolExecutionResult,
    policy: &ToolPolicy,
    permissions: &PermissionAuthority,
) -> ToolExecutionResult {
    let redaction = permissions.security_config().redaction;
    let level = level_for_call(
        &redaction,
        policy.sensitive.is_some(),
        policy.redaction_target.as_deref(),
    );
    if level == crate::security::RedactionLevel::Off {
        return result;
    }
    for block in &mut result.outcome.content {
        if let AgentContentBlock::Text { text }
        | AgentContentBlock::ToolResult { content: text, .. } = block
        {
            *text = redact(text, level, &redaction.extra_patterns).into_owned();
        }
    }
    if let Some(error) = &mut result.error {
        error.message = redact(&error.message, level, &redaction.extra_patterns).into_owned();
    }
    result
}

async fn await_or_cancel<T, E>(
    cancel: &CancelToken,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, ToolGateError>
where
    E: Display,
{
    if cancel.is_cancelled() {
        return Err(ToolGateError::Cancelled);
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ToolGateError::Cancelled),
        result = future => result.map_err(|error| ToolGateError::Failed(error.to_string())),
    }
}

#[derive(Debug, Deserialize)]
struct ToolPolicy {
    stable_name: String,
    risk: String,
    permission_prompt: Value,
    #[serde(default)]
    sensitive: Option<SensitiveMetadata>,
    #[serde(default)]
    redaction_target: Option<String>,
    #[serde(default)]
    auto_approved: bool,
}

impl ToolPolicy {
    fn from_resolved(call: &ResolvedToolCall) -> Result<Self, ToolGateError> {
        serde_json::from_value(call.descriptor.metadata.clone()).map_err(|error| {
            ToolGateError::Failed(format!("invalid tool policy metadata: {error}"))
        })
    }

    fn risk(&self) -> Result<CapabilityRisk, ToolGateError> {
        let risk = match self.risk.as_str() {
            "read" => CapabilityRisk::Read,
            "write" => CapabilityRisk::Write,
            "high" => CapabilityRisk::High,
            other => {
                return Err(ToolGateError::Failed(format!("invalid tool risk {other}")));
            }
        };
        Ok(if self.sensitive.is_some() {
            CapabilityRisk::High
        } else {
            risk
        })
    }

    fn prompt_policy(&self) -> Result<PermissionPromptPolicy, ToolGateError> {
        let kind = self
            .permission_prompt
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolGateError::Failed("tool policy metadata has no permission prompt kind".into())
            })?;
        let field = |name: &str| {
            self.permission_prompt
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        match kind {
            "json" => Ok(PermissionPromptPolicy::Json),
            "question" => Ok(PermissionPromptPolicy::Question {
                field: static_prompt_field(kind, field("field"))?,
            }),
            "command" => Ok(PermissionPromptPolicy::Command {
                field: static_prompt_field(kind, field("field"))?,
            }),
            "path" => Ok(PermissionPromptPolicy::Path {
                field: static_prompt_field(kind, field("field"))?,
            }),
            "patch_target" => Ok(PermissionPromptPolicy::PatchTarget {
                patch_field: static_prompt_field(kind, field("patch_field"))?,
            }),
            other => Err(ToolGateError::Failed(format!(
                "unknown permission prompt kind {other}"
            ))),
        }
    }

    fn is_credential(sensitive: &SensitiveMetadata) -> bool {
        sensitive.tier == "credential"
    }
}

// PermissionPromptPolicy owns &'static fields because it describes built-in
// tool metadata. The adapter accepts only the fields those built-ins declare.
fn static_prompt_field(kind: &str, field: Option<String>) -> Result<&'static str, ToolGateError> {
    let field = field.ok_or_else(|| {
        ToolGateError::Failed("tool policy metadata has no permission prompt field".into())
    })?;
    match (kind, field.as_str()) {
        ("question", "question") => Ok("question"),
        ("command", "command") => Ok("command"),
        ("path", "path") => Ok("path"),
        ("patch_target", "patch") => Ok("patch"),
        _ => Err(ToolGateError::Failed(format!(
            "unknown {kind} permission prompt field {field}"
        ))),
    }
}

#[derive(Debug, Deserialize)]
struct SensitiveMetadata {
    tier: String,
    kind: String,
    matched: String,
}

#[cfg(test)]
mod tests;
