//! The kernel-owned tool invocation pipeline.
//!
//! Preflight is the only path to a handler: it checks the snapshot generation,
//! resolves the descriptor, runs the security guard and PreToolUse hook,
//! rechecks the final input, asks for permission, and mints a one-use ticket.
//! `execute` consumes that ticket before calling the tool.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::capability::CapabilityFilter;
use crate::capability::tool::Tool;
use crate::catalog::ToolCatalog;
use crate::tool::context::{InteractionRequest, InteractionResponse, ToolCallContext};
use crate::tool::descriptor::ToolDescriptor;
use crate::tool::result::ToolCallResult;
use crate::tool::source::ToolDescriptorSnapshot;

/// The meaning of one permission option, in policy-neutral terms.
///
/// A service maps these onto its own rules; the kernel only routes the
/// human's pick back to [`PermissionService::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOptionKind {
    AllowOnce,
    AlwaysAllow,
    AlwaysAllowWide,
    Deny,
}

/// One choice a permission prompt offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub kind: PermissionOptionKind,
}

/// A prompt the kernel shows through [`InteractionRequest::Select`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPrompt {
    pub message: String,
    pub options: Vec<PermissionOption>,
}

/// The human's pick, minted by the kernel from the selected option id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionAnswer {
    pub option_id: String,
    pub kind: PermissionOptionKind,
}

/// A refusal the host must show with a label, not just a message.
///
/// The sensitive-target guard is the only producer today: the cell's label line
/// is the one place a reader learns *what kind* of material was refused, and a
/// bare error string would lose it.
#[derive(Debug)]
pub struct LabeledRefusal {
    pub label: String,
    pub message: String,
}

impl std::fmt::Display for LabeledRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LabeledRefusal {}

/// A port that classifies a concrete input's risk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny(String),
    Ask(PermissionPrompt),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub descriptor: ToolDescriptor,
    pub input: serde_json::Value,
}

#[async_trait]
pub trait PermissionService: Send + Sync {
    async fn decide(&self, request: PermissionRequest) -> anyhow::Result<PermissionDecision>;

    /// Persist or remember an answer the human gave.
    ///
    /// Called only for a real prompt answer, never for `Allow`/`Deny`, so a
    /// service can keep "always allow" semantics (rule text, persistence,
    /// denial counters) entirely on its own side.
    async fn record(
        &self,
        _request: &PermissionRequest,
        _answer: &PermissionAnswer,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A hook's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookControl {
    Continue,
    /// The hook decided the call may proceed without asking.
    ///
    /// A `Block` from any hook still wins: an explicit refusal must not be
    /// undone by another hook's approval.
    Allow,
    Block(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreToolUseEvent {
    pub tool_name: String,
    pub input: serde_json::Value,
    /// Context a hook wants recorded before the next request, not injected into
    /// the tool input. The kernel never interprets it; the caller drains it.
    pub additional_context: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostToolUseEvent {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub result: ToolCallResult,
    pub additional_context: Vec<String>,
}

/// A call that is about to prompt a human.
///
/// Runs only when a prompt would actually appear, so a policy hook can answer
/// it without paying for every call: `Allow` skips the prompt, `Block` denies
/// outright, anything else leaves the decision to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequestEvent {
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[async_trait]
pub trait HookService: Send + Sync {
    async fn pre_tool_use(&self, event: &mut PreToolUseEvent) -> anyhow::Result<HookControl>;
    async fn post_tool_use(&self, event: &mut PostToolUseEvent) -> anyhow::Result<HookControl>;

    /// Answer a prompt that is about to be shown.
    async fn permission_request(
        &self,
        _event: &mut PermissionRequestEvent,
    ) -> anyhow::Result<HookControl> {
        Ok(HookControl::Continue)
    }
}

/// The security guard, run before and after a PreToolUse rewrite.
pub trait SecurityService: Send + Sync {
    fn guard(&self, descriptor: &ToolDescriptor, input: &serde_json::Value) -> anyhow::Result<()>;
}

/// Why preflight authorized a call, in UI-neutral terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightAuthorization {
    /// A policy decided it; no prompt was shown.
    PolicyAllowed,
    /// A PreToolUse hook allowed it, bypassing the prompt.
    HookAllowed,
    /// The human answered a prompt.
    Prompted,
    /// A `PermissionRequest` hook allowed it, so no prompt appeared.
    PermissionRequestHookAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRequest {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub snapshot_generation: u64,
}

/// A call whose handler and descriptor are resolved, and whose *initial* input
/// passed the security guard.
///
/// Exposed so a host that must run its own PreToolUse hooks (one that needs
/// host state the kernel cannot see) can interpose them between `resolve` and
/// [`ToolExecutor::authorize`] without re-implementing the guard or the ticket.
pub struct ResolvedCall {
    tool: Arc<dyn Tool>,
    descriptor: ToolDescriptor,
    input: serde_json::Value,
}

impl ResolvedCall {
    pub fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    pub fn input(&self) -> &serde_json::Value {
        &self.input
    }

    /// The input a hook rewrote. The security guard runs again on it inside
    /// [`ToolExecutor::authorize`], so a rewrite cannot smuggle a credential
    /// path past the guard that already ran.
    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.input = input;
        self
    }
}

/// The policy verdict on a resolved call, before any ticket is minted.
///
/// Split from [`ToolExecutor::authorize`] so a host that must run its own
/// PermissionRequest hooks (ones that need host state the kernel cannot see)
/// can do that between the verdict and the ticket.
pub enum AuthorizedCall {
    /// The call may run; the ticket is already minted.
    Allowed(ToolPreflight),
    /// A prompt must be answered before the call may run.
    NeedsPrompt {
        call: ResolvedCall,
        prompt: PermissionPrompt,
        request: PermissionRequest,
    },
    /// The call is refused, with the message the caller should surface.
    Denied(String),
}

/// A prepared call. Opaque on purpose: only `execute` may consume it.
pub struct ToolPreflight {
    ticket: u64,
    /// Why the call was authorized (`Allow once`, `Allowed by PreToolUse hook`,
    /// …), for the UI. The kernel never interprets it.
    authorization: PreflightAuthorization,
    tool: Arc<dyn Tool>,
    descriptor: ToolDescriptor,
    input: serde_json::Value,
    ctx: ToolCallContext,
}

pub struct ToolExecutor {
    catalog: Arc<ToolCatalog>,
    filter: CapabilityFilter,
    security: Arc<dyn SecurityService>,
    permissions: Arc<dyn PermissionService>,
    hooks: Arc<dyn HookService>,
    tickets: Mutex<HashSet<u64>>,
    next_ticket: Mutex<u64>,
}

impl ToolPreflight {
    /// The tool this call will run, for a host that wants to look before it runs.
    pub fn tool_name(&self) -> &str {
        &self.descriptor.name
    }

    /// A second handle, for a host that must move the call into a spawned task
    /// while keeping the original for bookkeeping.
    ///
    /// Both handles share one ticket: whoever reaches [`ToolExecutor::execute`]
    /// first consumes it, and any later attempt is refused, so this is not a way
    /// to run a call twice.
    pub fn clone_for_host(&self) -> Self {
        Self {
            ticket: self.ticket,
            authorization: self.authorization,
            tool: self.tool.clone(),
            descriptor: self.descriptor.clone(),
            input: self.input.clone(),
            ctx: self.ctx.clone(),
        }
    }
}

impl ToolExecutor {
    pub fn new(
        catalog: Arc<ToolCatalog>,
        filter: CapabilityFilter,
        security: Arc<dyn SecurityService>,
        permissions: Arc<dyn PermissionService>,
        hooks: Arc<dyn HookService>,
    ) -> Self {
        Self {
            catalog,
            filter,
            security,
            permissions,
            hooks,
            tickets: Mutex::new(HashSet::new()),
            next_ticket: Mutex::new(1),
        }
    }

    pub async fn descriptor_snapshot(&self) -> anyhow::Result<ToolDescriptorSnapshot> {
        self.catalog.refresh_sources().await?;
        self.catalog.descriptor_snapshot(&self.filter).await
    }

    pub async fn preflight(
        &self,
        request: ToolRequest,
        ctx: ToolCallContext,
    ) -> anyhow::Result<ToolPreflight> {
        let call = self.resolve(&request).await?;
        let mut event = PreToolUseEvent {
            tool_name: request.tool_name.clone(),
            input: call.input().clone(),
            additional_context: Vec::new(),
        };
        let control = match self.hooks.pre_tool_use(&mut event).await {
            Ok(control) => control,
            Err(error) => anyhow::bail!("PreToolUse hook failed: {error}"),
        };
        if let HookControl::Block(reason) = control {
            anyhow::bail!("Tool blocked by PreToolUse hook: {reason}");
        }
        self.authorize(
            call.with_input(event.input),
            ctx,
            control == HookControl::Allow,
        )
        .await
    }

    /// Resolve a request to a handler, checking the generation and running the
    /// security guard on the input as the model sent it.
    ///
    /// Split out from [`Self::authorize`] so a host that runs its own PreToolUse
    /// hooks (one whose hooks need host state) can interpose them between the
    /// two without re-implementing the guard, the descriptor lookup or the
    /// ticket.
    pub async fn resolve(&self, request: &ToolRequest) -> anyhow::Result<ResolvedCall> {
        let snapshot = self.catalog.descriptor_snapshot(&self.filter).await?;
        anyhow::ensure!(
            snapshot.generation == request.snapshot_generation,
            "stale tool descriptor snapshot: model saw {}, current is {}",
            request.snapshot_generation,
            snapshot.generation
        );

        let descriptor = snapshot
            .tools
            .into_iter()
            .find(|tool| tool.name == request.tool_name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", request.tool_name))?;

        let tool = self
            .catalog
            .handler_for(&request.tool_name)
            .ok_or_else(|| anyhow::anyhow!("tool has no handler: {}", request.tool_name))?;

        self.security.guard(&descriptor, &request.input)?;
        Ok(ResolvedCall {
            tool,
            descriptor,
            input: request.input.clone(),
        })
    }

    /// Re-guard the final input, then ask the permission service for a verdict.
    ///
    /// This is the seam a host with its own hooks uses: run its PreToolUse
    /// before, its PermissionRequest in the `NeedsPrompt` arm, then call
    /// [`Self::finish_authorization`].
    pub async fn authorize_policy(
        &self,
        call: ResolvedCall,
        ctx: ToolCallContext,
        hook_allowed: bool,
    ) -> anyhow::Result<AuthorizedCall> {
        let ResolvedCall {
            tool,
            descriptor,
            input,
        } = call;
        self.security.guard(&descriptor, &input)?;

        let request = PermissionRequest {
            descriptor: descriptor.clone(),
            input: input.clone(),
        };
        let decision = if hook_allowed {
            PermissionDecision::Allow
        } else {
            self.permissions.decide(request.clone()).await?
        };
        Ok(match decision {
            PermissionDecision::Allow => AuthorizedCall::Allowed(self.issue_ticket(
                tool,
                descriptor,
                input,
                ctx,
                if hook_allowed {
                    PreflightAuthorization::HookAllowed
                } else {
                    PreflightAuthorization::PolicyAllowed
                },
            )),
            PermissionDecision::Deny(reason) => {
                AuthorizedCall::Denied(format!("Permission denied: {reason}"))
            }
            PermissionDecision::Ask(prompt) => AuthorizedCall::NeedsPrompt {
                call: ResolvedCall {
                    tool,
                    descriptor,
                    input,
                },
                prompt,
                request,
            },
        })
    }

    /// Turn an answered prompt into a ticket.
    ///
    /// `hook_allowed` marks the case where a PermissionRequest hook answered the
    /// prompt itself; then no human answer is recorded.
    pub async fn finish_authorization(
        &self,
        call: ResolvedCall,
        request: PermissionRequest,
        answer: Option<PermissionAnswer>,
        ctx: ToolCallContext,
        hook_allowed: bool,
    ) -> anyhow::Result<ToolPreflight> {
        if let Some(answer) = &answer {
            if answer.kind == PermissionOptionKind::Deny {
                anyhow::bail!("Permission denied by user for {}", call.descriptor.name);
            }
            self.permissions.record(&request, answer).await?;
        }
        let authorization = if hook_allowed {
            PreflightAuthorization::PermissionRequestHookAllowed
        } else {
            PreflightAuthorization::Prompted
        };
        Ok(self.issue_ticket(call.tool, call.descriptor, call.input, ctx, authorization))
    }

    fn issue_ticket(
        &self,
        tool: Arc<dyn Tool>,
        descriptor: ToolDescriptor,
        input: serde_json::Value,
        ctx: ToolCallContext,
        authorization: PreflightAuthorization,
    ) -> ToolPreflight {
        let ticket = {
            let mut next = self.next_ticket.lock().unwrap();
            let ticket = *next;
            *next += 1;
            self.tickets.lock().unwrap().insert(ticket);
            ticket
        };
        ToolPreflight {
            ticket,
            authorization,
            tool,
            descriptor,
            input,
            ctx,
        }
    }

    /// Authorize a resolved call: re-guard the final input, consult permission
    /// (unless a PreToolUse hook already allowed it), and mint a one-use ticket.
    pub async fn authorize(
        &self,
        call: ResolvedCall,
        ctx: ToolCallContext,
        hook_allowed: bool,
    ) -> anyhow::Result<ToolPreflight> {
        let ResolvedCall {
            tool,
            descriptor,
            input,
        } = call;
        self.security.guard(&descriptor, &input)?;

        let permission_request = PermissionRequest {
            descriptor: descriptor.clone(),
            input: input.clone(),
        };
        let decision = if hook_allowed {
            PermissionDecision::Allow
        } else {
            self.permissions.decide(permission_request.clone()).await?
        };
        let prompted = matches!(decision, PermissionDecision::Ask(_));
        let mut permission_hook_allowed = false;
        match decision {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny(reason) => {
                anyhow::bail!("Permission denied: {reason}")
            }
            PermissionDecision::Ask(prompt) => {
                anyhow::ensure!(
                    !prompt.options.is_empty(),
                    "permission prompt for {} offered no options",
                    descriptor.name
                );
                let mut event = PermissionRequestEvent {
                    tool_name: descriptor.name.clone(),
                    input: input.clone(),
                };
                match self.hooks.permission_request(&mut event).await {
                    // Fail open, like every other hook failure: a broken policy
                    // hook must not block work.
                    Ok(HookControl::Allow) => permission_hook_allowed = true,
                    Ok(HookControl::Block(reason)) => {
                        anyhow::bail!("Permission denied by PermissionRequest hook: {reason}")
                    }
                    Ok(HookControl::Continue) | Err(_) => {}
                }
                if !permission_hook_allowed {
                    let answer = self.ask(ctx.interaction.as_ref(), &prompt).await?;
                    if answer.kind == PermissionOptionKind::Deny {
                        anyhow::bail!("Permission denied by user for {}", descriptor.name);
                    }
                    self.permissions
                        .record(&permission_request, &answer)
                        .await?;
                }
            }
        }

        let authorization = if hook_allowed {
            PreflightAuthorization::HookAllowed
        } else if permission_hook_allowed {
            PreflightAuthorization::PermissionRequestHookAllowed
        } else if prompted {
            PreflightAuthorization::Prompted
        } else {
            PreflightAuthorization::PolicyAllowed
        };
        let ticket = {
            let mut next = self.next_ticket.lock().unwrap();
            let ticket = *next;
            *next += 1;
            self.tickets.lock().unwrap().insert(ticket);
            ticket
        };
        Ok(ToolPreflight {
            ticket,
            authorization,
            tool,
            descriptor,
            input,
            ctx,
        })
    }

    /// Ask the human and translate the answer into a typed verdict.
    async fn ask(
        &self,
        interaction: &dyn crate::tool::context::InteractionBroker,
        prompt: &PermissionPrompt,
    ) -> anyhow::Result<PermissionAnswer> {
        let request = InteractionRequest::Select {
            prompt: prompt.message.clone(),
            options: prompt
                .options
                .iter()
                .map(|option| match &option.detail {
                    Some(detail) => format!("{} ({detail})", option.label),
                    None => option.label.clone(),
                })
                .collect(),
        };
        let response = interaction.request(request).await?;
        let picked = match response {
            InteractionResponse::Selected(labels) => labels,
            InteractionResponse::Approved => Vec::new(),
            InteractionResponse::Rejected | InteractionResponse::Cancelled => {
                anyhow::bail!("Permission cancelled by user")
            }
            InteractionResponse::Text(text) => vec![text],
        };
        let fallback = || {
            prompt
                .options
                .iter()
                .find(|option| option.kind != PermissionOptionKind::Deny)
        };
        let option = if picked.is_empty() {
            fallback().ok_or_else(|| anyhow::anyhow!("permission prompt has no allow option"))?
        } else {
            let first = &picked[0];
            prompt
                .options
                .iter()
                .find(|option| &option.id == first || &option.label == first)
                .or_else(|| {
                    prompt
                        .options
                        .iter()
                        .find(|option| option.label.starts_with(first.as_str()))
                })
                .ok_or_else(|| anyhow::anyhow!("unknown permission option: {first}"))?
        };
        Ok(PermissionAnswer {
            option_id: option.id.clone(),
            kind: option.kind,
        })
    }

    /// Drop a ticket that will never be redeemed.
    ///
    /// A host can authorize more calls than it runs: a cancel between pre-flight
    /// and the execution wave, or a Goal-yield that skips the rest of the turn,
    /// leaves a ticket live forever. Releasing it keeps the invariant honest —
    /// every ticket is either redeemed or released — so any future audit or
    /// expiry on this set starts from a true statement.
    pub fn release(&self, prepared: ToolPreflight) {
        self.tickets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&prepared.ticket);
    }

    /// The hook port, for a host that must run its own hook callbacks.
    pub fn hooks(&self) -> &dyn HookService {
        self.hooks.as_ref()
    }

    /// The catalog this executor dispatches from.
    pub fn catalog(&self) -> &ToolCatalog {
        self.catalog.as_ref()
    }

    /// Why this prepared call was authorized.
    pub fn authorization(&self, prepared: &ToolPreflight) -> PreflightAuthorization {
        prepared.authorization
    }

    pub async fn execute(&self, prepared: ToolPreflight) -> anyhow::Result<ToolCallResult> {
        anyhow::ensure!(
            self.tickets.lock().unwrap().remove(&prepared.ticket),
            "tool execution ticket is missing or already used"
        );

        let call_id = prepared.ctx.tool_use_id.clone();
        prepared
            .ctx
            .events
            .emit(crate::tool::context::ToolEvent::Started {
                call_id: call_id.clone(),
                tool_name: prepared.descriptor.name.clone(),
            });
        let result = prepared
            .tool
            .call(prepared.ctx.clone(), prepared.input.clone())
            .await;
        let mut result = match result {
            Ok(result) => {
                prepared
                    .ctx
                    .events
                    .emit(crate::tool::context::ToolEvent::Finished {
                        call_id,
                        content: result.content.clone(),
                    });
                result
            }
            Err(error) => {
                prepared
                    .ctx
                    .events
                    .emit(crate::tool::context::ToolEvent::Failed {
                        call_id,
                        error: error.to_string(),
                    });
                return Err(error);
            }
        };

        let mut post = PostToolUseEvent {
            tool_name: prepared.descriptor.name,
            input: prepared.input,
            result: result.clone(),
            additional_context: Vec::new(),
        };
        match self.hooks.post_tool_use(&mut post).await? {
            HookControl::Continue | HookControl::Allow => {}
            HookControl::Block(reason) => {
                result.content = format!("Tool blocked by PostToolUse hook: {reason}");
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::tool::Tool;
    use crate::tool::context::{
        CancelToken, EventSink, InteractionBroker, RedactionPolicy, ToolEvent, ToolOrigin,
        Workspace,
    };
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingTool {
        calls: Arc<AtomicUsize>,
        seen_input: Arc<Mutex<serde_json::Value>>,
        fail: bool,
    }

    #[async_trait]
    impl Tool for RecordingTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor::from_native(
                "echo",
                "echo",
                json!({"type": "object"}),
                &ToolMetadata::read_json("echo", "echo", "Echo"),
            )
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.seen_input.lock().unwrap() = input;
            if self.fail {
                anyhow::bail!("tool failed");
            }
            Ok(ToolCallResult::text("ok"))
        }
    }

    #[derive(Clone, Default)]
    struct OrderLog(Arc<Mutex<Vec<&'static str>>>);

    impl OrderLog {
        fn push(&self, value: &'static str) {
            self.0.lock().unwrap().push(value);
        }

        fn take(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    struct RecordingSecurity {
        log: OrderLog,
        seen: Arc<Mutex<Vec<String>>>,
        reject: bool,
    }

    impl SecurityService for RecordingSecurity {
        fn guard(
            &self,
            _descriptor: &ToolDescriptor,
            input: &serde_json::Value,
        ) -> anyhow::Result<()> {
            self.log.push("security");
            self.seen
                .lock()
                .unwrap()
                .push(input["text"].as_str().unwrap_or("").to_string());
            if self.reject {
                anyhow::bail!("blocked by security");
            }
            Ok(())
        }
    }

    struct RecordingPermissions {
        log: OrderLog,
        decision: PermissionDecision,
    }

    #[async_trait]
    impl PermissionService for RecordingPermissions {
        async fn decide(&self, _request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
            self.log.push("permission");
            Ok(self.decision.clone())
        }
    }

    struct RecordingHooks {
        log: OrderLog,
        pre: HookControl,
        rewrite: Option<serde_json::Value>,
        post: HookControl,
    }

    #[async_trait]
    impl HookService for RecordingHooks {
        async fn pre_tool_use(&self, event: &mut PreToolUseEvent) -> anyhow::Result<HookControl> {
            self.log.push("pre");
            if let Some(input) = &self.rewrite {
                event.input = input.clone();
            }
            Ok(self.pre.clone())
        }

        async fn post_tool_use(
            &self,
            _event: &mut PostToolUseEvent,
        ) -> anyhow::Result<HookControl> {
            self.log.push("post");
            Ok(self.post.clone())
        }
    }

    struct RejectAll;
    #[async_trait]
    impl InteractionBroker for RejectAll {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Rejected)
        }
    }

    struct ApproveAll;
    #[async_trait]
    impl InteractionBroker for ApproveAll {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Approved)
        }
    }

    struct RecordingEvents(Arc<Mutex<Vec<ToolEvent>>>);
    impl EventSink for RecordingEvents {
        fn emit(&self, event: ToolEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn context(interaction: Arc<dyn InteractionBroker>) -> ToolCallContext {
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: Workspace::new("/repo"),
            events: Arc::new(RecordingEvents(Arc::default())),
            interaction,
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Basic,
        }
    }

    fn executor(
        tool: Arc<dyn Tool>,
        _log: OrderLog,
        security: RecordingSecurity,
        permissions: RecordingPermissions,
        hooks: RecordingHooks,
    ) -> ToolExecutor {
        let mut catalog = ToolCatalog::default();
        catalog.insert(tool).unwrap();
        ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(security),
            Arc::new(permissions),
            Arc::new(hooks),
        )
    }

    async fn generation(executor: &ToolExecutor) -> u64 {
        executor.descriptor_snapshot().await.unwrap().generation
    }

    fn request(generation: u64) -> ToolRequest {
        ToolRequest {
            call_id: "call".into(),
            tool_name: "echo".into(),
            input: json!({"text": "hello"}),
            snapshot_generation: generation,
        }
    }

    #[tokio::test]
    async fn order_is_security_prehook_security_permission_tool_posthook() {
        let log = OrderLog::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = Arc::new(RecordingTool {
            calls: calls.clone(),
            seen_input: Arc::default(),
            fail: false,
        });
        let executor = executor(
            tool,
            log.clone(),
            RecordingSecurity {
                log: log.clone(),
                seen: Arc::default(),
                reject: false,
            },
            RecordingPermissions {
                log: log.clone(),
                decision: PermissionDecision::Allow,
            },
            RecordingHooks {
                log: log.clone(),
                pre: HookControl::Continue,
                rewrite: None,
                post: HookControl::Continue,
            },
        );
        let generation = generation(&executor).await;
        let prepared = executor
            .preflight(request(generation), context(Arc::new(ApproveAll)))
            .await
            .unwrap();
        executor.execute(prepared).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            log.take(),
            vec!["security", "pre", "security", "permission", "post"]
        );
    }

    #[tokio::test]
    async fn deny_does_not_call_tool_or_ticket() {
        let log = OrderLog::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(
            Arc::new(RecordingTool {
                calls: calls.clone(),
                seen_input: Arc::default(),
                fail: false,
            }),
            log.clone(),
            RecordingSecurity {
                log: log.clone(),
                seen: Arc::default(),
                reject: false,
            },
            RecordingPermissions {
                log: log.clone(),
                decision: PermissionDecision::Deny("nope".into()),
            },
            RecordingHooks {
                log: log.clone(),
                pre: HookControl::Continue,
                rewrite: None,
                post: HookControl::Continue,
            },
        );
        let generation = generation(&executor).await;
        assert!(
            executor
                .preflight(request(generation), context(Arc::new(RejectAll)))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn block_does_not_consult_permission_or_call_tool() {
        let log = OrderLog::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(
            Arc::new(RecordingTool {
                calls: calls.clone(),
                seen_input: Arc::default(),
                fail: false,
            }),
            log.clone(),
            RecordingSecurity {
                log: log.clone(),
                seen: Arc::default(),
                reject: false,
            },
            RecordingPermissions {
                log: log.clone(),
                decision: PermissionDecision::Allow,
            },
            RecordingHooks {
                log: log.clone(),
                pre: HookControl::Block("no".into()),
                rewrite: None,
                post: HookControl::Continue,
            },
        );
        let generation = generation(&executor).await;
        assert!(
            executor
                .preflight(request(generation), context(Arc::new(RejectAll)))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(log.take(), vec!["security", "pre"]);
    }

    #[tokio::test]
    async fn ask_rejected_does_not_call_tool() {
        let log = OrderLog::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(
            Arc::new(RecordingTool {
                calls: calls.clone(),
                seen_input: Arc::default(),
                fail: false,
            }),
            log,
            RecordingSecurity {
                log: OrderLog::default(),
                seen: Arc::default(),
                reject: false,
            },
            RecordingPermissions {
                log: OrderLog::default(),
                decision: PermissionDecision::Ask(PermissionPrompt {
                    message: "allow?".into(),
                    options: vec![PermissionOption {
                        id: "allow_once".into(),
                        label: "Allow once".into(),
                        detail: None,
                        kind: PermissionOptionKind::AllowOnce,
                    }],
                }),
            },
            RecordingHooks {
                log: OrderLog::default(),
                pre: HookControl::Continue,
                rewrite: None,
                post: HookControl::Continue,
            },
        );
        let generation = generation(&executor).await;
        assert!(
            executor
                .preflight(request(generation), context(Arc::new(RejectAll)))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn ticket_is_one_use() {
        let log = OrderLog::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(
            Arc::new(RecordingTool {
                calls: calls.clone(),
                seen_input: Arc::default(),
                fail: false,
            }),
            log.clone(),
            RecordingSecurity {
                log: log.clone(),
                seen: Arc::default(),
                reject: false,
            },
            RecordingPermissions {
                log: log.clone(),
                decision: PermissionDecision::Allow,
            },
            RecordingHooks {
                log: log.clone(),
                pre: HookControl::Continue,
                rewrite: None,
                post: HookControl::Continue,
            },
        );
        let generation = generation(&executor).await;
        let prepared = executor
            .preflight(request(generation), context(Arc::new(ApproveAll)))
            .await
            .unwrap();
        let duplicate = ToolPreflight {
            ticket: prepared.ticket,
            authorization: prepared.authorization,
            tool: prepared.tool.clone(),
            descriptor: prepared.descriptor.clone(),
            input: prepared.input.clone(),
            ctx: prepared.ctx.clone(),
        };
        executor.execute(prepared).await.unwrap();
        let error = executor.execute(duplicate).await.unwrap_err();
        assert!(error.to_string().contains("ticket"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            log.take(),
            vec!["security", "pre", "security", "permission", "post"]
        );
    }

    #[tokio::test]
    async fn stale_generation_fails_before_security() {
        let log = OrderLog::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(
            Arc::new(RecordingTool {
                calls: calls.clone(),
                seen_input: Arc::default(),
                fail: false,
            }),
            log.clone(),
            RecordingSecurity {
                log: log.clone(),
                seen: Arc::default(),
                reject: false,
            },
            RecordingPermissions {
                log: log.clone(),
                decision: PermissionDecision::Allow,
            },
            RecordingHooks {
                log: log.clone(),
                pre: HookControl::Continue,
                rewrite: None,
                post: HookControl::Continue,
            },
        );
        let generation = generation(&executor).await;
        assert!(
            executor
                .preflight(request(generation + 1), context(Arc::new(ApproveAll)))
                .await
                .is_err()
        );
        assert_eq!(log.take(), Vec::<&str>::new());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn prehook_rewrite_is_rechecked_and_used() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = OrderLog::default();
        let executor = executor(
            Arc::new(RecordingTool {
                calls: Arc::new(AtomicUsize::new(0)),
                seen_input: Arc::default(),
                fail: false,
            }),
            log.clone(),
            RecordingSecurity {
                log: log.clone(),
                seen: seen.clone(),
                reject: false,
            },
            RecordingPermissions {
                log: log.clone(),
                decision: PermissionDecision::Allow,
            },
            RecordingHooks {
                log: log.clone(),
                pre: HookControl::Continue,
                rewrite: Some(json!({"text": "rewritten"})),
                post: HookControl::Continue,
            },
        );
        let generation = generation(&executor).await;
        executor
            .preflight(request(generation), context(Arc::new(ApproveAll)))
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().clone(), vec!["hello", "rewritten"]);
    }
}

#[cfg(test)]
mod permission_prompt_tests {
    use super::*;
    use crate::capability::tool::Tool;
    use crate::tool::context::{
        CancelToken, EventSink, RedactionPolicy, ToolCallContext, ToolEvent, ToolOrigin, Workspace,
    };
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoopTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for NoopTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor::from_native(
                "echo",
                "echo",
                json!({"type": "object"}),
                &ToolMetadata::read_json("echo", "echo", "Echo"),
            )
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult::text("ok"))
        }
    }

    struct NoSecurity;
    impl SecurityService for NoSecurity {
        fn guard(
            &self,
            _descriptor: &ToolDescriptor,
            _input: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct NoHooks;
    #[async_trait]
    impl HookService for NoHooks {
        async fn pre_tool_use(&self, _event: &mut PreToolUseEvent) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }

        async fn post_tool_use(
            &self,
            _event: &mut PostToolUseEvent,
        ) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }
    }

    struct Asking {
        recorded: Arc<Mutex<Vec<(String, PermissionOptionKind)>>>,
    }

    #[async_trait]
    impl PermissionService for Asking {
        async fn decide(&self, _request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
            Ok(PermissionDecision::Ask(PermissionPrompt {
                message: "run echo?".into(),
                options: vec![
                    PermissionOption {
                        id: "once".into(),
                        label: "Allow once".into(),
                        detail: None,
                        kind: PermissionOptionKind::AllowOnce,
                    },
                    PermissionOption {
                        id: "always".into(),
                        label: "Always allow echo".into(),
                        detail: Some("records a rule".into()),
                        kind: PermissionOptionKind::AlwaysAllow,
                    },
                    PermissionOption {
                        id: "deny".into(),
                        label: "Deny".into(),
                        detail: None,
                        kind: PermissionOptionKind::Deny,
                    },
                ],
            }))
        }

        async fn record(
            &self,
            _request: &PermissionRequest,
            answer: &PermissionAnswer,
        ) -> anyhow::Result<()> {
            self.recorded
                .lock()
                .unwrap()
                .push((answer.option_id.clone(), answer.kind));
            Ok(())
        }
    }

    struct Pick(String);
    #[async_trait]
    impl crate::tool::context::InteractionBroker for Pick {
        async fn request(
            &self,
            _request: crate::tool::context::InteractionRequest,
        ) -> anyhow::Result<crate::tool::context::InteractionResponse> {
            Ok(crate::tool::context::InteractionResponse::Selected(vec![
                self.0.clone(),
            ]))
        }
    }

    struct Events;
    impl EventSink for Events {
        fn emit(&self, _event: ToolEvent) {}
    }

    fn context(pick: &str) -> ToolCallContext {
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: Workspace::new("/repo"),
            events: Arc::new(Events),
            interaction: Arc::new(Pick(pick.into())),
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Off,
        }
    }

    async fn executor(
        calls: Arc<AtomicUsize>,
        recorded: Arc<Mutex<Vec<(String, PermissionOptionKind)>>>,
    ) -> ToolExecutor {
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(NoopTool(calls))).unwrap();
        ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(NoSecurity),
            Arc::new(Asking { recorded }),
            Arc::new(NoHooks),
        )
    }

    fn request(generation: u64) -> ToolRequest {
        ToolRequest {
            call_id: "call".into(),
            tool_name: "echo".into(),
            input: json!({"text": "hi"}),
            snapshot_generation: generation,
        }
    }

    #[tokio::test]
    async fn selecting_by_option_id_records_the_answer() {
        let calls = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let executor = executor(calls.clone(), recorded.clone()).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let prepared = executor
            .preflight(request(generation), context("always"))
            .await
            .unwrap();
        executor.execute(prepared).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            recorded.lock().unwrap().clone(),
            vec![("always".to_string(), PermissionOptionKind::AlwaysAllow)]
        );
    }

    #[tokio::test]
    async fn selecting_deny_runs_nothing_and_records_nothing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let executor = executor(calls.clone(), recorded.clone()).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let error = executor
            .preflight(request(generation), context("deny"))
            .await
            .err()
            .expect("deny must fail preflight");
        assert!(error.to_string().contains("Permission denied"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn selecting_by_rendered_label_works() {
        let calls = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let executor = executor(calls.clone(), recorded.clone()).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let prepared = executor
            .preflight(request(generation), context("Always allow echo"))
            .await
            .unwrap();
        executor.execute(prepared).await.unwrap();
        assert_eq!(
            recorded.lock().unwrap().clone(),
            vec![("always".to_string(), PermissionOptionKind::AlwaysAllow)]
        );
    }
}

#[cfg(test)]
mod hook_allow_tests {
    use super::*;
    use crate::capability::tool::Tool;
    use crate::tool::context::{
        CancelToken, EventSink, InteractionBroker, InteractionRequest, InteractionResponse,
        RedactionPolicy, ToolCallContext, ToolEvent, ToolOrigin, Workspace,
    };
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for CountingTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor::from_native(
                "bash",
                "bash",
                json!({"type": "object"}),
                &ToolMetadata::read_json("bash", "bash", "Bash"),
            )
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult::text("ran"))
        }
    }

    struct CountingPermission(Arc<AtomicUsize>);
    #[async_trait]
    impl PermissionService for CountingPermission {
        async fn decide(&self, _request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(PermissionDecision::Ask(PermissionPrompt {
                message: "should never be asked".into(),
                options: vec![PermissionOption {
                    id: "deny".into(),
                    label: "Deny".into(),
                    detail: None,
                    kind: PermissionOptionKind::Deny,
                }],
            }))
        }
    }

    struct AllowingHooks;
    #[async_trait]
    impl HookService for AllowingHooks {
        async fn pre_tool_use(&self, _event: &mut PreToolUseEvent) -> anyhow::Result<HookControl> {
            Ok(HookControl::Allow)
        }

        async fn post_tool_use(
            &self,
            _event: &mut PostToolUseEvent,
        ) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }
    }

    struct DenyAllInteractions;
    #[async_trait]
    impl InteractionBroker for DenyAllInteractions {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Rejected)
        }
    }

    struct NoGuard;
    impl SecurityService for NoGuard {
        fn guard(
            &self,
            _descriptor: &ToolDescriptor,
            _input: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _event: ToolEvent) {}
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: Workspace::new("/repo"),
            events: Arc::new(NoEvents),
            interaction: Arc::new(DenyAllInteractions),
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Off,
        }
    }

    async fn executor(calls: Arc<AtomicUsize>, asked: Arc<AtomicUsize>) -> ToolExecutor {
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(CountingTool(calls))).unwrap();
        ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(NoGuard),
            Arc::new(CountingPermission(asked)),
            Arc::new(AllowingHooks),
        )
    }

    #[tokio::test]
    async fn hook_allow_skips_permission_and_runs_the_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let asked = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone(), asked.clone()).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let prepared = executor
            .preflight(
                ToolRequest {
                    call_id: "call".into(),
                    tool_name: "bash".into(),
                    input: json!({"command": "ls"}),
                    snapshot_generation: generation,
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            executor.authorization(&prepared),
            PreflightAuthorization::HookAllowed
        );
        executor.execute(prepared).await.unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 0, "hook allow must not ask");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn policy_allow_is_reported_as_policy_authorized() {
        struct AllowAll;
        #[async_trait]
        impl PermissionService for AllowAll {
            async fn decide(
                &self,
                _request: PermissionRequest,
            ) -> anyhow::Result<PermissionDecision> {
                Ok(PermissionDecision::Allow)
            }
        }

        struct ContinueHooks;
        #[async_trait]
        impl HookService for ContinueHooks {
            async fn pre_tool_use(
                &self,
                _event: &mut PreToolUseEvent,
            ) -> anyhow::Result<HookControl> {
                Ok(HookControl::Continue)
            }

            async fn post_tool_use(
                &self,
                _event: &mut PostToolUseEvent,
            ) -> anyhow::Result<HookControl> {
                Ok(HookControl::Continue)
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let mut catalog = ToolCatalog::default();
        catalog
            .insert(Arc::new(CountingTool(calls.clone())))
            .unwrap();
        let executor = ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(NoGuard),
            Arc::new(AllowAll),
            Arc::new(ContinueHooks),
        );
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let prepared = executor
            .preflight(
                ToolRequest {
                    call_id: "call".into(),
                    tool_name: "bash".into(),
                    input: json!({"command": "ls"}),
                    snapshot_generation: generation,
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            executor.authorization(&prepared),
            PreflightAuthorization::PolicyAllowed
        );
    }
}

#[cfg(test)]
mod resolve_authorize_tests {
    use super::*;
    use crate::capability::tool::Tool;
    use crate::tool::context::{
        CancelToken, EventSink, InteractionBroker, InteractionRequest, InteractionResponse,
        RedactionPolicy, ToolCallContext, ToolEvent, ToolOrigin, Workspace,
    };
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::metadata::{PermissionPolicy, PermissionPromptPolicy};
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixedTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for FixedTool {
        fn descriptor(&self) -> ToolDescriptor {
            let mut metadata = ToolMetadata::read_json("read_file", "Read", "Read");
            metadata.permission = PermissionPolicy::ReadPath { path_field: "path" };
            metadata.permission_prompt = PermissionPromptPolicy::Path { field: "path" };
            ToolDescriptor::from_native("read_file", "Read", json!({"type": "object"}), &metadata)
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult::text("read"))
        }
    }

    struct RecordingGuard;
    impl SecurityService for RecordingGuard {
        fn guard(
            &self,
            _descriptor: &ToolDescriptor,
            input: &serde_json::Value,
        ) -> anyhow::Result<()> {
            if input["path"].as_str() == Some(".ssh/id_ed25519") {
                anyhow::bail!("Refused: credential material");
            }
            Ok(())
        }
    }

    struct AllowAll;
    #[async_trait]
    impl PermissionService for AllowAll {
        async fn decide(&self, _request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
            Ok(PermissionDecision::Allow)
        }
    }

    struct NoHooks;
    #[async_trait]
    impl HookService for NoHooks {
        async fn pre_tool_use(&self, _event: &mut PreToolUseEvent) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }

        async fn post_tool_use(
            &self,
            _event: &mut PostToolUseEvent,
        ) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }
    }

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _event: ToolEvent) {}
    }

    struct NoInteractions;
    #[async_trait]
    impl InteractionBroker for NoInteractions {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Cancelled)
        }
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: Workspace::new("/repo"),
            events: Arc::new(NoEvents),
            interaction: Arc::new(NoInteractions),
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Off,
        }
    }

    async fn executor(calls: Arc<AtomicUsize>) -> ToolExecutor {
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(FixedTool(calls))).unwrap();
        ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(RecordingGuard),
            Arc::new(AllowAll),
            Arc::new(NoHooks),
        )
    }

    fn request(path: &str, generation: u64) -> ToolRequest {
        ToolRequest {
            call_id: "call".into(),
            tool_name: "read_file".into(),
            input: json!({ "path": path }),
            snapshot_generation: generation,
        }
    }

    #[tokio::test]
    async fn resolve_guards_before_a_host_hook_can_rewrite() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let error = executor
            .resolve(&request(".ssh/id_ed25519", generation))
            .await
            .err()
            .expect("the guard runs during resolve");
        assert!(error.to_string().contains("Refused"));
    }

    #[tokio::test]
    async fn authorize_rechecks_the_rewritten_input() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone()).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let resolved = executor
            .resolve(&request("src/main.rs", generation))
            .await
            .unwrap();
        // A host hook rewrites the path to a credential after the first guard.
        let rewritten = resolved.with_input(json!({ "path": ".ssh/id_ed25519" }));
        let error = executor
            .authorize(rewritten, ctx(), false)
            .await
            .err()
            .expect("the second guard must catch the rewrite");
        assert!(error.to_string().contains("Refused"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn hook_allowed_authorization_reports_the_reason() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone()).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let resolved = executor
            .resolve(&request("src/main.rs", generation))
            .await
            .unwrap();
        let prepared = executor.authorize(resolved, ctx(), true).await.unwrap();
        assert_eq!(
            executor.authorization(&prepared),
            PreflightAuthorization::HookAllowed
        );
        executor.execute(prepared).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod permission_request_hook_tests {
    use super::*;
    use crate::capability::tool::Tool;
    use crate::tool::context::{
        CancelToken, EventSink, InteractionBroker, InteractionRequest, InteractionResponse,
        RedactionPolicy, ToolCallContext, ToolEvent, ToolOrigin, Workspace,
    };
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoopTool;
    #[async_trait]
    impl Tool for NoopTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor::from_native(
                "bash",
                "bash",
                json!({"type": "object"}),
                &ToolMetadata::read_json("bash", "bash", "Bash"),
            )
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            Ok(ToolCallResult::text("ran"))
        }
    }

    struct Asking(Arc<AtomicUsize>);
    #[async_trait]
    impl PermissionService for Asking {
        async fn decide(&self, _request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
            Ok(PermissionDecision::Ask(PermissionPrompt {
                message: "run?".into(),
                options: vec![PermissionOption {
                    id: "allow_once".into(),
                    label: "Allow once".into(),
                    detail: None,
                    kind: PermissionOptionKind::AllowOnce,
                }],
            }))
        }

        async fn record(
            &self,
            _request: &PermissionRequest,
            _answer: &PermissionAnswer,
        ) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct PolicyHook(HookControl);
    #[async_trait]
    impl HookService for PolicyHook {
        async fn pre_tool_use(&self, _event: &mut PreToolUseEvent) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }

        async fn post_tool_use(
            &self,
            _event: &mut PostToolUseEvent,
        ) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }

        async fn permission_request(
            &self,
            _event: &mut PermissionRequestEvent,
        ) -> anyhow::Result<HookControl> {
            Ok(self.0.clone())
        }
    }

    struct NoGuard;
    impl SecurityService for NoGuard {
        fn guard(
            &self,
            _descriptor: &ToolDescriptor,
            _input: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _event: ToolEvent) {}
    }

    /// Would refuse any prompt, so a test that passes proves none was asked.
    struct RefusePrompts;
    #[async_trait]
    impl InteractionBroker for RefusePrompts {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Cancelled)
        }
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: Workspace::new("/repo"),
            events: Arc::new(NoEvents),
            interaction: Arc::new(RefusePrompts),
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Off,
        }
    }

    async fn executor(recorded: Arc<AtomicUsize>, hook: HookControl) -> ToolExecutor {
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(NoopTool)).unwrap();
        ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(NoGuard),
            Arc::new(Asking(recorded)),
            Arc::new(PolicyHook(hook)),
        )
    }

    fn request(generation: u64) -> ToolRequest {
        ToolRequest {
            call_id: "call".into(),
            tool_name: "bash".into(),
            input: json!({"command": "ls"}),
            snapshot_generation: generation,
        }
    }

    #[tokio::test]
    async fn allow_skips_the_prompt_and_is_reported() {
        let recorded = Arc::new(AtomicUsize::new(0));
        let executor = executor(recorded.clone(), HookControl::Allow).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let prepared = executor
            .preflight(request(generation), ctx())
            .await
            .expect("a hook allow must not need a human");
        assert_eq!(
            executor.authorization(&prepared),
            PreflightAuthorization::PermissionRequestHookAllowed
        );
        executor.execute(prepared).await.unwrap();
        assert_eq!(recorded.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn block_denies_with_the_legacy_wording() {
        let recorded = Arc::new(AtomicUsize::new(0));
        let executor = executor(recorded.clone(), HookControl::Block("policy".into())).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let error = executor
            .preflight(request(generation), ctx())
            .await
            .err()
            .expect("a blocking policy hook must deny");
        assert!(
            error
                .to_string()
                .starts_with("Permission denied by PermissionRequest hook:"),
            "{error}"
        );
        assert_eq!(recorded.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod host_seam_tests {
    use super::*;
    use crate::capability::tool::Tool;
    use crate::tool::context::{
        CancelToken, EventSink, InteractionBroker, InteractionRequest, InteractionResponse,
        RedactionPolicy, ToolCallContext, ToolEvent, ToolOrigin, Workspace,
    };
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoopTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for NoopTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor::from_native(
                "bash",
                "bash",
                json!({"type": "object"}),
                &ToolMetadata::read_json("bash", "bash", "Bash"),
            )
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult::text("ran"))
        }
    }

    struct Fixed(PermissionDecision);
    #[async_trait]
    impl PermissionService for Fixed {
        async fn decide(&self, _request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
            Ok(self.0.clone())
        }

        async fn record(
            &self,
            _request: &PermissionRequest,
            _answer: &PermissionAnswer,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct NoHooks;
    #[async_trait]
    impl HookService for NoHooks {
        async fn pre_tool_use(&self, _event: &mut PreToolUseEvent) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }

        async fn post_tool_use(
            &self,
            _event: &mut PostToolUseEvent,
        ) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }
    }

    struct NoGuard;
    impl SecurityService for NoGuard {
        fn guard(
            &self,
            _descriptor: &ToolDescriptor,
            _input: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _event: ToolEvent) {}
    }

    struct NoInteractions;
    #[async_trait]
    impl InteractionBroker for NoInteractions {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Cancelled)
        }
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: Workspace::new("/repo"),
            events: Arc::new(NoEvents),
            interaction: Arc::new(NoInteractions),
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Off,
        }
    }

    async fn executor(calls: Arc<AtomicUsize>, decision: PermissionDecision) -> ToolExecutor {
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(NoopTool(calls))).unwrap();
        ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(NoGuard),
            Arc::new(Fixed(decision)),
            Arc::new(NoHooks),
        )
    }

    fn request(generation: u64) -> ToolRequest {
        ToolRequest {
            call_id: "call".into(),
            tool_name: "bash".into(),
            input: json!({"command": "ls"}),
            snapshot_generation: generation,
        }
    }

    fn prompt() -> PermissionPrompt {
        PermissionPrompt {
            message: "run?".into(),
            options: vec![PermissionOption {
                id: "allow_once".into(),
                label: "Allow once".into(),
                detail: None,
                kind: PermissionOptionKind::AllowOnce,
            }],
        }
    }

    #[tokio::test]
    async fn allow_mints_a_ticket_immediately() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone(), PermissionDecision::Allow).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let resolved = executor.resolve(&request(generation)).await.unwrap();
        let AuthorizedCall::Allowed(prepared) = executor
            .authorize_policy(resolved, ctx(), false)
            .await
            .unwrap()
        else {
            panic!("allow must mint a ticket");
        };
        executor.execute(prepared).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ask_returns_the_prompt_without_a_ticket() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone(), PermissionDecision::Ask(prompt())).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let resolved = executor.resolve(&request(generation)).await.unwrap();
        let AuthorizedCall::NeedsPrompt {
            call,
            prompt,
            request,
        } = executor
            .authorize_policy(resolved, ctx(), false)
            .await
            .unwrap()
        else {
            panic!("an ask must not mint a ticket");
        };
        assert_eq!(prompt.options.len(), 1);
        // The host answers it (here: allow once) and finishes authorization.
        let prepared = executor
            .finish_authorization(
                call,
                request,
                Some(PermissionAnswer {
                    option_id: "allow_once".into(),
                    kind: PermissionOptionKind::AllowOnce,
                }),
                ctx(),
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            executor.authorization(&prepared),
            PreflightAuthorization::Prompted
        );
        executor.execute(prepared).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn deny_returns_the_legacy_message_and_no_ticket() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone(), PermissionDecision::Deny("nope".into())).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let resolved = executor.resolve(&request(generation)).await.unwrap();
        let AuthorizedCall::Denied(message) = executor
            .authorize_policy(resolved, ctx(), false)
            .await
            .unwrap()
        else {
            panic!("a deny must be reported");
        };
        assert_eq!(message, "Permission denied: nope");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_hook_answer_records_no_human_choice() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone(), PermissionDecision::Ask(prompt())).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let resolved = executor.resolve(&request(generation)).await.unwrap();
        let AuthorizedCall::NeedsPrompt { call, request, .. } = executor
            .authorize_policy(resolved, ctx(), false)
            .await
            .unwrap()
        else {
            panic!("expected a prompt");
        };
        let prepared = executor
            .finish_authorization(call, request, None, ctx(), true)
            .await
            .unwrap();
        assert_eq!(
            executor.authorization(&prepared),
            PreflightAuthorization::PermissionRequestHookAllowed
        );
        executor.execute(prepared).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn denying_answer_never_reaches_the_tool() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = executor(calls.clone(), PermissionDecision::Ask(prompt())).await;
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let resolved = executor.resolve(&request(generation)).await.unwrap();
        let AuthorizedCall::NeedsPrompt { call, request, .. } = executor
            .authorize_policy(resolved, ctx(), false)
            .await
            .unwrap()
        else {
            panic!("expected a prompt");
        };
        let error = executor
            .finish_authorization(
                call,
                request,
                Some(PermissionAnswer {
                    option_id: "deny".into(),
                    kind: PermissionOptionKind::Deny,
                }),
                ctx(),
                false,
            )
            .await
            .err()
            .expect("a deny answer must fail");
        assert!(error.to_string().contains("Permission denied by user"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;
    use crate::capability::tool::Tool;
    use crate::tool::context::{
        CancelToken, EventSink, InteractionBroker, InteractionRequest, InteractionResponse,
        RedactionPolicy, ToolCallContext, ToolEvent, ToolOrigin, Workspace,
    };
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoopTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for NoopTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor::from_native(
                "bash",
                "bash",
                json!({"type": "object"}),
                &ToolMetadata::read_json("bash", "bash", "Bash"),
            )
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult::text("ran"))
        }
    }

    struct Allow;
    #[async_trait]
    impl PermissionService for Allow {
        async fn decide(&self, _request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
            Ok(PermissionDecision::Allow)
        }
    }

    struct NoHooks;
    #[async_trait]
    impl HookService for NoHooks {
        async fn pre_tool_use(&self, _event: &mut PreToolUseEvent) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }

        async fn post_tool_use(
            &self,
            _event: &mut PostToolUseEvent,
        ) -> anyhow::Result<HookControl> {
            Ok(HookControl::Continue)
        }
    }

    struct NoGuard;
    impl SecurityService for NoGuard {
        fn guard(
            &self,
            _descriptor: &ToolDescriptor,
            _input: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _event: ToolEvent) {}
    }

    struct NoInteractions;
    #[async_trait]
    impl InteractionBroker for NoInteractions {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Cancelled)
        }
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: Workspace::new("/repo"),
            events: Arc::new(NoEvents),
            interaction: Arc::new(NoInteractions),
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Off,
        }
    }

    #[tokio::test]
    async fn releasing_an_unredeemed_ticket_makes_it_unusable() {
        // A host can authorize a call it never runs (a cancel between preflight
        // and the wave). Releasing keeps "every ticket is redeemed or released"
        // true instead of leaking the id for the life of the session.
        let calls = Arc::new(AtomicUsize::new(0));
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(NoopTool(calls.clone()))).unwrap();
        let executor = ToolExecutor::new(
            Arc::new(catalog),
            CapabilityFilter::default(),
            Arc::new(NoGuard),
            Arc::new(Allow),
            Arc::new(NoHooks),
        );
        let generation = executor.descriptor_snapshot().await.unwrap().generation;
        let prepared = executor
            .preflight(
                ToolRequest {
                    call_id: "call".into(),
                    tool_name: "bash".into(),
                    input: json!({"command": "ls"}),
                    snapshot_generation: generation,
                },
                ctx(),
            )
            .await
            .unwrap();
        executor.release(prepared.clone_for_host());
        let Err(error) = executor.execute(prepared).await else {
            panic!("a released ticket must not run the tool");
        };
        assert!(error.to_string().contains("ticket"), "{error}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
