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

#[async_trait]
pub trait HookService: Send + Sync {
    async fn pre_tool_use(&self, event: &mut PreToolUseEvent) -> anyhow::Result<HookControl>;
    async fn post_tool_use(&self, event: &mut PostToolUseEvent) -> anyhow::Result<HookControl>;
}

/// The security guard, run before and after a PreToolUse rewrite.
pub trait SecurityService: Send + Sync {
    fn guard(&self, descriptor: &ToolDescriptor, input: &serde_json::Value) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRequest {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub snapshot_generation: u64,
}

/// A prepared call. Opaque on purpose: only `execute` may consume it.
pub struct ToolPreflight {
    ticket: u64,
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
            .get(&request.tool_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("tool has no handler: {}", request.tool_name))?;

        self.security.guard(&descriptor, &request.input)?;
        let mut hook_event = PreToolUseEvent {
            tool_name: request.tool_name.clone(),
            input: request.input.clone(),
            additional_context: Vec::new(),
        };
        match self.hooks.pre_tool_use(&mut hook_event).await? {
            HookControl::Continue => {}
            HookControl::Block(reason) => {
                anyhow::bail!("tool blocked by PreToolUse hook: {reason}")
            }
        }
        self.security.guard(&descriptor, &hook_event.input)?;

        let permission_request = PermissionRequest {
            descriptor: descriptor.clone(),
            input: hook_event.input.clone(),
        };
        let decision = self.permissions.decide(permission_request.clone()).await?;
        match decision {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny(reason) => anyhow::bail!("permission denied: {reason}"),
            PermissionDecision::Ask(prompt) => {
                anyhow::ensure!(
                    !prompt.options.is_empty(),
                    "permission prompt for {} offered no options",
                    request.tool_name
                );
                let answer = self.ask(ctx.interaction.as_ref(), &prompt).await?;
                if answer.kind == PermissionOptionKind::Deny {
                    anyhow::bail!("permission denied by user")
                }
                self.permissions
                    .record(&permission_request, &answer)
                    .await?;
            }
        }

        let ticket = {
            let mut next = self.next_ticket.lock().unwrap();
            let ticket = *next;
            *next += 1;
            self.tickets.lock().unwrap().insert(ticket);
            ticket
        };
        Ok(ToolPreflight {
            ticket,
            tool,
            descriptor,
            input: hook_event.input,
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
                anyhow::bail!("permission denied by user")
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
            HookControl::Continue => {}
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
        assert!(error.to_string().contains("denied"));
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
