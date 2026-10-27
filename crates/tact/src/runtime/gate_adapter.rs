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
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use serde_json::json;
    use tact_agent_api::{
        AgentContentBlock, ResourceAccess, SessionRef, ToolCall, ToolDescriptor, ToolDirective,
        ToolError, ToolExecutionError, ToolExecutionErrorKind, ToolExecutionResult,
        ToolExecutionStatus, ToolGate, ToolGateContext, ToolOutcome, ToolResolver,
    };
    use tact_llm::{Message, Role};
    use tact_protocol::{AgentUpdate, UiResponse, UpdateSink};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;
    use crate::{
        mcp::{MCPToolRouter, McpCatalogHandle},
        permission::{AllowOutcome, PermissionManager, PermissionMode},
        runtime::{permission_authority::PermissionAuthority, services::TactSharedServices},
        stats::SessionStats,
    };

    struct PolicyResolver;

    #[test]
    fn permission_popup_indices_map_to_their_choices() {
        assert_eq!(permission_choice_for(Some(0)), PermissionChoice::AllowOnce);
        assert_eq!(permission_choice_for(Some(1)), PermissionChoice::Deny);
        assert_eq!(
            permission_choice_for(Some(2)),
            PermissionChoice::AlwaysAllow
        );
        assert_eq!(
            permission_choice_for(Some(3)),
            PermissionChoice::AlwaysAllowProgram
        );
        assert_eq!(permission_choice_for(None), PermissionChoice::Deny);
        assert_eq!(permission_choice_for(Some(99)), PermissionChoice::Deny);
    }

    #[test]
    fn the_pattern_choice_is_appended_and_can_be_withheld() {
        let offered = permission_options(true);
        let withheld = permission_options(false);
        assert_eq!(offered.len(), PERMISSION_OPTIONS.len());
        assert_eq!(withheld.len(), PERMISSION_OPTIONS.len() - 1);
        assert_eq!(withheld, offered[..withheld.len()].to_vec());
        assert_eq!(
            offered.last().map(String::as_str),
            Some("Always allow this pattern")
        );
        for index in 0..withheld.len() {
            assert_ne!(
                permission_choice_for(Some(index)),
                PermissionChoice::AlwaysAllowProgram
            );
        }
        assert_eq!(
            permission_choice_for(Some(withheld.len())),
            PermissionChoice::AlwaysAllowProgram
        );
    }

    #[test]
    fn every_permission_option_maps_back_to_its_choice() {
        let expected = [
            PermissionChoice::AllowOnce,
            PermissionChoice::Deny,
            PermissionChoice::AlwaysAllow,
            PermissionChoice::AlwaysAllowProgram,
        ];
        assert_eq!(PERMISSION_OPTIONS.len(), expected.len());
        for (index, choice) in expected.iter().enumerate() {
            assert_eq!(permission_choice_for(Some(index)), *choice);
        }
    }

    impl ToolResolver for PolicyResolver {
        fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError> {
            if call.name == "missing" {
                return Err(ToolError::new(
                    tact_agent_api::ToolErrorKind::UnknownTool,
                    "unknown tool: missing",
                ));
            }

            let path = call
                .input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let credential = path.contains(".ssh/") || path.ends_with("id_ed25519");
            let risk = if call.name == "high_risk" || call.name.starts_with("mcp__danger__") {
                "high"
            } else if call.name == "write_file" {
                "write"
            } else {
                "read"
            };
            let sensitive = credential.then(|| {
                json!({
                    "tier": "credential",
                    "kind": "private_key",
                    "matched": path,
                })
            });
            let metadata = json!({
                "kind": "native",
                "stable_name": call.name,
                "risk": risk,
                "permission_prompt": {"kind": "path", "field": "path"},
                "sensitive": sensitive,
                "redaction_target": path,
                "auto_approved": false,
                "arg_full": call.input.to_string(),
                "arg_summary": path,
                "presentation": {
                    "kind": "generic",
                    "label": call.name,
                    "detail": "result",
                },
            });
            Ok(ResolvedToolCall {
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
                access: ResourceAccess::independent(),
                descriptor: ToolDescriptor {
                    name: call.name.clone(),
                    description: "test policy tool".into(),
                    metadata,
                },
            })
        }
    }

    struct Fixture {
        gate: TactToolGate,
        services: Arc<TactSharedServices>,
        responder: UiResponder,
        ui: Option<UnboundedReceiver<AgentUpdate>>,
    }

    fn fixture(mode: PermissionMode, hooks: Vec<Hook>, with_ui: bool) -> Fixture {
        let (updates, ui) = if with_ui {
            let (tx, rx) = unbounded_channel();
            (UpdateSink::for_ui(tx), Some(rx))
        } else {
            (UpdateSink::default(), None)
        };
        let permissions = PermissionAuthority::new(PermissionManager::try_new(mode).unwrap());
        let services = Arc::new(TactSharedServices::new(
            updates,
            Arc::new(std::sync::RwLock::new(SessionStats::default())),
            permissions,
            Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
            McpCatalogHandle::new(MCPToolRouter::new()),
        ));
        let responder = UiResponder::new();
        let gate = TactToolGate::new(
            services.clone(),
            responder.clone(),
            &hooks,
            None,
            ModelIdentity {
                name: "test-model".into(),
            },
            SessionStartSource::Startup,
            false,
        );
        Fixture {
            gate,
            services,
            responder,
            ui,
        }
    }

    fn context() -> ToolGateContext {
        ToolGateContext {
            session: SessionRef::new("session"),
            total_turns: 1,
            remaining_turns: Some(2),
            metadata: Value::Null,
        }
    }

    fn call(name: &str, path: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: name.into(),
            input: json!({"path": path}),
        }
    }

    fn transcript_message() -> Message {
        Message::new_text(Role::User, "hello")
    }

    #[tokio::test]
    async fn original_credential_call_fails_closed_before_any_hook() {
        let hook_ran = Arc::new(AtomicBool::new(false));
        let ran = hook_ran.clone();
        let fixture = fixture(
            PermissionMode::Auto,
            vec![Hook::PreToolUse(Arc::new(move |_state, _tool| {
                ran.store(true, Ordering::Relaxed);
                Box::pin(async { Ok(HookControl::Continue) })
            }))],
            false,
        );

        let decision = fixture
            .gate
            .before(
                call("read_file", "~/.ssh/id_ed25519"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();

        assert!(matches!(decision, GateDecision::Deny { .. }));
        assert!(!hook_ran.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn pre_tool_rewrite_is_fully_re_resolved_and_reclassified() {
        let hook_ran = Arc::new(AtomicBool::new(false));
        let ran = hook_ran.clone();
        let fixture = fixture(
            PermissionMode::Auto,
            vec![Hook::PreToolUse(Arc::new(move |_state, tool| {
                ran.store(true, Ordering::Relaxed);
                tool.name = "write_file".into();
                tool.input = json!({"path": "~/.ssh/id_ed25519"});
                Box::pin(async { Ok(HookControl::Continue) })
            }))],
            false,
        );

        let decision = fixture
            .gate
            .before(
                call("read_file", "notes.txt"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();

        assert!(hook_ran.load(Ordering::Relaxed));
        assert!(matches!(decision, GateDecision::Deny { .. }));
    }

    #[tokio::test]
    async fn permission_request_rewrite_is_rechecked_before_allow() {
        let fixture = fixture(
            PermissionMode::Default,
            vec![Hook::PermissionRequest(Arc::new(|_state, tool| {
                tool.input = json!({"path": "~/.ssh/id_ed25519"});
                Box::pin(async { Ok(HookControl::Allow) })
            }))],
            false,
        );

        let decision = fixture
            .gate
            .before(
                call("write_file", "notes.txt"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();

        assert!(matches!(decision, GateDecision::Deny { .. }));
    }

    #[tokio::test]
    async fn mcp_rewrite_is_reclassified_before_auto_approval() {
        let fixture = fixture(
            PermissionMode::Default,
            vec![Hook::PreToolUse(Arc::new(|_state, tool| {
                tool.name = "mcp__danger__write".into();
                Box::pin(async { Ok(HookControl::Allow) })
            }))],
            false,
        );

        let decision = fixture
            .gate
            .before(
                call("mcp__safe__read", "notes.txt"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();

        assert!(
            matches!(decision, GateDecision::Deny { .. }),
            "a hook Allow cannot preserve the original low-risk MCP approval: {decision:?}"
        );
    }

    #[tokio::test]
    async fn facade_mode_and_grant_changes_reach_the_next_gate_call() {
        let fixture = fixture(PermissionMode::Default, Vec::new(), false);
        let facade = fixture.services.permissions().clone();
        assert!(facade.shares_manager_with(fixture.services.permissions()));

        facade.set_mode(PermissionMode::Plan);
        let denied = fixture
            .gate
            .before(
                call("write_file", "notes.txt"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(
            denied,
            GateDecision::Deny { reason } if reason.contains("Plan mode")
        ));

        facade.set_mode(PermissionMode::Auto);
        let allowed = fixture
            .gate
            .before(
                call("write_file", "notes.txt"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(allowed, GateDecision::Allow { .. }));

        facade.set_mode(PermissionMode::Default);
        assert_eq!(
            facade.allow_tool_with_input(
                "write_file",
                PermissionPromptPolicy::Path { field: "path" },
                &json!({"path": "notes.txt"}),
            ),
            AllowOutcome::Recorded
        );
        let granted = fixture
            .gate
            .before(
                call("write_file", "notes.txt"),
                &PolicyResolver,
                &context(),
                CancelToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(granted, GateDecision::Allow { label: None, .. }));
    }

    #[tokio::test]
    async fn gate_denials_are_visible_through_the_shared_facade_authority() {
        let fixture = fixture(PermissionMode::Default, Vec::new(), false);
        let facade = fixture.services.permissions().clone();

        for _ in 0..3 {
            let decision = fixture
                .gate
                .before(
                    call("high_risk", "notes.txt"),
                    &PolicyResolver,
                    &context(),
                    CancelToken::new(),
                )
                .await
                .unwrap();
            assert!(matches!(decision, GateDecision::Deny { .. }));
        }

        assert_eq!(facade.consecutive_denials(), 3);
    }

    #[tokio::test]
    async fn always_allow_popup_selection_is_visible_to_the_next_gate_call() {
        let Fixture {
            gate,
            services,
            responder,
            ui,
        } = fixture(PermissionMode::Default, Vec::new(), true);
        let mut ui = ui.expect("UI fixture");
        let facade = services.permissions().clone();
        let gate = Arc::new(gate);
        let task_gate = gate.clone();
        let resolver = PolicyResolver;
        let context = context();
        let task = tokio::spawn(async move {
            task_gate
                .before(
                    call("write_file", "notes.txt"),
                    &resolver,
                    &context,
                    CancelToken::new(),
                )
                .await
        });
        let AgentUpdate::RequestSelect {
            request_id,
            options,
            ..
        } = ui.recv().await.expect("permission popup")
        else {
            panic!("expected RequestSelect");
        };
        assert_eq!(options, permission_options(false));
        responder.handle_response(UiResponse::Select {
            request_id,
            choice: Some(2),
        });
        let decision = task.await.unwrap().unwrap();
        assert!(matches!(
            decision,
            GateDecision::Allow {
                label: Some(label),
                ..
            } if label == "Always allow this tool"
        ));
        assert_eq!(
            facade
                .check_with_auto(
                    "write_file",
                    CapabilityRisk::Write,
                    &json!({"path": "notes.txt"}),
                    false,
                )
                .behavior,
            PermissionBehavior::Allow
        );
    }

    #[tokio::test]
    async fn cancellation_wins_while_the_permission_popup_is_pending() {
        let fixture = fixture(PermissionMode::Default, Vec::new(), true);
        let mut ui = fixture.ui.expect("UI fixture");
        let gate = fixture.gate;
        let cancel = CancelToken::new();
        let resolver = PolicyResolver;
        let context = context();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            gate.before(
                call("write_file", "notes.txt"),
                &resolver,
                &context,
                task_cancel,
            )
            .await
        });

        let _popup = ui.recv().await.expect("permission popup");
        cancel.cancel();
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("popup cancellation must be bounded")
            .unwrap()
            .unwrap_err();
        assert_eq!(error, ToolGateError::Cancelled);
    }

    #[tokio::test]
    async fn post_tool_redacts_rewritten_output_and_preserves_the_original_error() {
        let fixture = fixture(
            PermissionMode::Auto,
            vec![Hook::PostToolUse(Arc::new(
                |_state, _tool, result, _status| {
                    result.content = "visible sk-abcdefghijklmnopqrstuvwxyz012345".into();
                    Box::pin(async { Ok(HookControl::Continue) })
                },
            ))],
            false,
        );
        let resolved = PolicyResolver
            .resolve(&call("read_file", "notes.txt"))
            .unwrap();
        let error = ToolExecutionError {
            kind: ToolExecutionErrorKind::Tool,
            message: "original failure".into(),
        };
        let result = ToolExecutionResult {
            status: ToolExecutionStatus::Failed,
            outcome: ToolOutcome {
                content: vec![AgentContentBlock::Text { text: "raw".into() }],
                directives: vec![ToolDirective::CompleteRun],
                metadata: None,
            },
            error: Some(error.clone()),
        };

        let after = fixture
            .gate
            .after(&resolved, result, &context(), CancelToken::new())
            .await
            .unwrap();

        assert_eq!(after.status, ToolExecutionStatus::Failed);
        assert_eq!(after.error, Some(error));
        let text = content_text(&after.outcome.content);
        assert!(text.contains("[redacted:api-key]"));
        assert!(!text.contains("sk-abcdefghijklmnopqrstuvwxyz012345"));
    }

    #[tokio::test]
    async fn post_tool_hooks_cannot_join_a_secret_across_rewrites() {
        const SECRET: &str = "sk-abcdefghijklmnopqrstuvwxyz012345";

        let fixture = fixture(
            PermissionMode::Auto,
            vec![
                Hook::PostToolUse(Arc::new(|_state, _tool, result, _status| {
                    result.content = "visible sk-abcdefgh".into();
                    Box::pin(async { Ok(HookControl::Continue) })
                })),
                Hook::PostToolUse(Arc::new(|_state, _tool, result, _status| {
                    result.content.push_str("ijklmnopqrstuvwxyz012345");
                    Box::pin(async { Ok(HookControl::Continue) })
                })),
            ],
            false,
        );
        let resolved = PolicyResolver
            .resolve(&call("read_file", "notes.txt"))
            .unwrap();
        let result = ToolExecutionResult::success(ToolOutcome {
            content: vec![AgentContentBlock::Text {
                text: "benign".into(),
            }],
            directives: Vec::new(),
            metadata: None,
        });

        let after = fixture
            .gate
            .after(&resolved, result, &context(), CancelToken::new())
            .await
            .unwrap();

        let text = content_text(&after.outcome.content);
        assert!(text.contains("[redacted:api-key]"), "{text}");
        assert!(
            !text.contains(SECRET),
            "hook output leaked the token: {text}"
        );
    }

    #[test]
    fn activation_view_accepts_read_only_transcript_data() {
        let fixture = fixture(PermissionMode::Default, Vec::new(), false);
        let transcript = [transcript_message()];
        let view = fixture.gate.activation_view(&context());
        assert!(view.last_assistant_message().is_none());

        let view = TactActivationView::new(
            fixture.services.as_ref(),
            Some(&transcript),
            SessionIdentity {
                id: Some("session".into()),
            },
            ModelIdentity {
                name: "test-model".into(),
            },
            1,
            Some(2),
            SessionStartSource::Startup,
        );
        assert!(view.last_assistant_message().is_none());
    }
}
