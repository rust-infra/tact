//! Adapter from the legacy `tact::tool::Tool` to the kernel `Tool` contract.
//!
//! Phase 1b uses this to put existing tools behind `ToolExecutor` without
//! rewriting every handler at once. The adapter owns no policy: permission,
//! security, hooks and tickets stay in the kernel.

use std::sync::Arc;

use async_trait::async_trait;
use tact_kernel::capability::tool::Tool as KernelTool;
use tact_kernel::tool::context::{ToolCallContext, ToolOrigin};
use tact_kernel::tool::descriptor::ToolDescriptor;
use tact_kernel::tool::result::ToolCallResult;

use super::{Tool as LegacyTool, ToolContext};

/// Builds the legacy context a wrapped tool still expects.
pub trait ToolContextFactory: Send + Sync {
    fn build(&self, ctx: &ToolCallContext) -> ToolContext;

    /// The skill registry, for a tool that declared only that handle.
    ///
    /// A default so an existing test factory keeps compiling; the real factory
    /// overrides it. Returning `None` is honest — a host with no skills has no
    /// registry — and a tool that declared the handle then reports it is missing.
    fn skill_registry(&self) -> Option<crate::skill::SharedSkillRegistry> {
        None
    }

    /// The teammate manager, on the same terms.
    fn teammate_manager(&self) -> Option<crate::tool::SharedTeammateManager> {
        None
    }

    /// The task manager, on the same terms.
    fn task_manager(&self) -> Option<crate::tool::SharedTaskManager> {
        None
    }

    /// The sink a tool's read-out goes to.
    fn events(&self) -> Option<tact_protocol::UpdateSink> {
        None
    }
}

/// A factory over a base context, mapping the run-scoped fields.
///
/// Domain handles (task/goal/team/worktree/background/memory/skill/session)
/// come from `base`; the adapter overwrites what the kernel call context knows.
///
/// The base is behind a lock because the Agent keeps changing its
/// `ToolContext` *after* this factory exists: `with_ui_channel` swaps the event
/// sink, and each turn stamps `permission_snapshot` and `subagent_results` so a
/// spawned child inherits the parent's mode and can report back in the
/// background. A frozen clone would hand every kernel-dispatched tool the
/// construction-time values — a child would silently lose its parent's
/// permissions, and `run_in_background` would fail with "requires a parent
/// agent runtime". [`Self::refresh`] is called once per turn, before any call.
#[derive(Clone)]
pub struct SharedToolContextFactory {
    base: std::sync::Arc<std::sync::Mutex<ToolContext>>,
}

impl SharedToolContextFactory {
    pub fn new(base: ToolContext) -> Self {
        Self {
            base: std::sync::Arc::new(std::sync::Mutex::new(base)),
        }
    }

    /// Replace the base with the Agent's current context.
    ///
    /// Called once per turn from pre-flight, after the Agent stamps the
    /// permission snapshot and the subagent-results queue.
    pub fn refresh(&self, live: &ToolContext) {
        let mut base = self.base.lock().unwrap_or_else(|error| error.into_inner());
        *base = live.clone();
    }

    /// Run `f` against the current base.
    pub fn with_base<T>(&self, f: impl FnOnce(&ToolContext) -> T) -> T {
        let base = self.base.lock().unwrap_or_else(|error| error.into_inner());
        f(&base)
    }
}

impl ToolContextFactory for SharedToolContextFactory {
    fn build(&self, ctx: &ToolCallContext) -> ToolContext {
        // `for_invocation_with_redaction` is what stamps the per-call progress
        // reporter: without it a background run's sticky row is keyed by the
        // empty base id, so every run lands on the same row (or on none).
        let level = match ctx.redaction {
            tact_kernel::tool::context::RedactionPolicy::Off => {
                crate::security::RedactionLevel::Off
            }
            tact_kernel::tool::context::RedactionPolicy::Basic => {
                crate::security::RedactionLevel::Basic
            }
            tact_kernel::tool::context::RedactionPolicy::Credential => {
                crate::security::RedactionLevel::Credential
            }
        };
        let mut context =
            self.with_base(|base| base.for_invocation_with_redaction(&ctx.tool_use_id, level));
        context.work_dir = ctx.workspace.root().to_path_buf();
        context.session_id = ctx.session_id.clone();
        context.active_goal_id = ctx.active_goal_id.clone();
        context.is_subagent = matches!(ctx.origin, ToolOrigin::Subagent);
        context.cancel_flag = ctx.cancel.shared_flag();
        context
    }

    fn skill_registry(&self) -> Option<crate::skill::SharedSkillRegistry> {
        Some(self.with_base(|base| base.skill_registry.clone()))
    }

    fn teammate_manager(&self) -> Option<crate::tool::SharedTeammateManager> {
        Some(self.with_base(|base| base.teammate_manager.clone()))
    }

    fn task_manager(&self) -> Option<crate::tool::SharedTaskManager> {
        Some(self.with_base(|base| base.task_manager.clone()))
    }

    fn events(&self) -> Option<tact_protocol::UpdateSink> {
        Some(self.with_base(|base| base.events.clone()))
    }
}

/// A legacy handler exposed as a kernel [`KernelTool`].
pub struct LegacyToolAdapter {
    inner: Arc<dyn LegacyTool>,
    contexts: Arc<dyn ToolContextFactory>,
}

impl LegacyToolAdapter {
    pub fn new<T: LegacyTool + 'static>(inner: T, contexts: Arc<dyn ToolContextFactory>) -> Self {
        Self {
            inner: Arc::new(inner),
            contexts,
        }
    }

    /// Wrap a handler that is already boxed.
    pub fn from_boxed(inner: Box<dyn LegacyTool>, contexts: Arc<dyn ToolContextFactory>) -> Self {
        Self {
            inner: Arc::from(inner),
            contexts,
        }
    }

    /// Wrap a handler that is shared with another owner.
    pub fn from_arc(inner: Arc<dyn LegacyTool>, contexts: Arc<dyn ToolContextFactory>) -> Self {
        Self { inner, contexts }
    }
}

#[async_trait]
impl KernelTool for LegacyToolAdapter {
    fn descriptor(&self) -> ToolDescriptor {
        let metadata = self.inner.metadata();
        ToolDescriptor::from_native(
            metadata.name,
            metadata.description,
            self.inner.input_schema(),
            metadata,
        )
    }

    async fn call(
        &self,
        ctx: ToolCallContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolCallResult> {
        // A tool that needs only the workspace says so, and then the adapter does
        // not materialise a whole `ToolContext` to read one field. The check is
        // on the tool, not on the caller: a tool that opts out always gets the
        // full context, so this cannot hand anyone a partial one.
        if let Some(future) = self
            .inner
            .call_in_workspace(ctx.workspace.clone(), input.clone())
        {
            return future.await;
        }
        // A tool that declared domain handles gets exactly those, built without a
        // `ToolContext`. The declaration is what makes "hand it the one thing it
        // asked for" checkable rather than a convention: an undeclared handle
        // arrives as `None`.
        let needs = self.inner.domain_needs();
        if !needs.is_empty() {
            let mut handles = super::DomainHandles::none();
            for need in needs {
                match need {
                    super::DomainHandle::SkillRegistry => {
                        handles.skill_registry = self.contexts.skill_registry();
                    }
                    super::DomainHandle::TeammateManager => {
                        handles.teammate_manager = self.contexts.teammate_manager();
                    }
                    super::DomainHandle::TaskManager => {
                        handles.task_manager = self.contexts.task_manager();
                    }
                    super::DomainHandle::SessionId => {
                        handles.session_id = ctx.session_id.clone();
                    }
                    super::DomainHandle::ActiveGoalId => {
                        handles.active_goal_id = ctx.active_goal_id.clone();
                    }
                    super::DomainHandle::Events => {
                        handles.events = self.contexts.events();
                    }
                }
            }
            if let Some(future) = self.inner.call_in_domain(handles, input.clone()) {
                return future.await;
            }
        }
        let context = self.contexts.build(&ctx);
        self.inner.call(context, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolMetadata;
    use crate::tool::test_support::test_context;
    use serde_json::{Value, json};
    use std::sync::atomic::Ordering;

    struct InspectingLegacyTool {
        seen: Arc<std::sync::Mutex<Option<Snapshot>>>,
    }

    struct Snapshot {
        work_dir: std::path::PathBuf,
        session_id: Option<String>,
        active_goal_id: Option<String>,
        is_subagent: bool,
        cancelled: bool,
        input: Value,
    }

    #[async_trait]
    impl LegacyTool for InspectingLegacyTool {
        fn metadata(&self) -> &'static ToolMetadata {
            static META: ToolMetadata = ToolMetadata::read_json("echo", "Echo", "Echo");
            &META
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn call(&self, context: ToolContext, input: Value) -> anyhow::Result<ToolCallResult> {
            *self.seen.lock().unwrap() = Some(Snapshot {
                work_dir: context.work_dir.clone(),
                session_id: context.session_id.clone(),
                active_goal_id: context.active_goal_id.clone(),
                is_subagent: context.is_subagent,
                cancelled: context.cancel_flag.load(Ordering::SeqCst),
                input: input.clone(),
            });
            Ok(ToolCallResult::text("ok"))
        }
    }

    fn kernel_context(origin: ToolOrigin, cancel: bool) -> ToolCallContext {
        struct Events;
        impl tact_kernel::tool::context::EventSink for Events {
            fn emit(&self, _event: tact_kernel::tool::context::ToolEvent) {}
        }
        struct Interactions;
        #[async_trait]
        impl tact_kernel::tool::context::InteractionBroker for Interactions {
            async fn request(
                &self,
                _request: tact_kernel::tool::context::InteractionRequest,
            ) -> anyhow::Result<tact_kernel::tool::context::InteractionResponse> {
                Ok(tact_kernel::tool::context::InteractionResponse::Rejected)
            }
        }
        let token = tact_kernel::tool::context::CancelToken::new();
        if cancel {
            token.cancel();
        }
        ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: Some("session".into()),
            active_goal_id: Some("goal".into()),
            origin,
            workspace: tact_kernel::tool::context::Workspace::new("/mapped"),
            events: Arc::new(Events),
            interaction: Arc::new(Interactions),
            cancel: token,
            redaction: tact_kernel::tool::context::RedactionPolicy::Basic,
        }
    }

    /// A tool that declared a domain handle gets that handle, and only that one.
    ///
    /// `DomainHandles` is a flat struct of options; the declaration is what keeps
    /// it from becoming `ToolContext` again. The test asserts both halves: the
    /// declared handle arrives, and the undeclared one does not.
    #[tokio::test]
    async fn a_declared_domain_handle_arrives_and_nothing_else_does() {
        use crate::tool::{DomainHandle, DomainHandles, Tool, ToolCallResult, ToolContext};

        struct NeedsSkills;

        #[async_trait::async_trait]
        impl Tool for NeedsSkills {
            fn metadata(&self) -> &'static crate::tool::ToolMetadata {
                static META: crate::tool::ToolMetadata =
                    crate::tool::ToolMetadata::read_json("needs_skills", "Needs", "Needs");
                &META
            }

            fn input_schema(&self) -> serde_json::Value {
                json!({"type": "object"})
            }

            fn domain_needs(&self) -> &'static [DomainHandle] {
                &[DomainHandle::SkillRegistry]
            }

            async fn call(
                &self,
                _context: ToolContext,
                _input: serde_json::Value,
            ) -> anyhow::Result<ToolCallResult> {
                Ok(ToolCallResult::text("full"))
            }

            fn call_in_domain<'a>(
                &'a self,
                domain: DomainHandles,
                _input: serde_json::Value,
            ) -> Option<
                std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = anyhow::Result<ToolCallResult>>
                            + Send
                            + 'a,
                    >,
                >,
            > {
                Some(Box::pin(async move {
                    Ok(ToolCallResult::text(format!("{:?}", domain.carried())))
                }))
            }
        }

        struct CountingFactory {
            builds: Arc<std::sync::atomic::AtomicUsize>,
            context: ToolContext,
        }

        impl ToolContextFactory for CountingFactory {
            fn build(&self, _ctx: &ToolCallContext) -> ToolContext {
                self.builds
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.context.clone()
            }

            fn skill_registry(&self) -> Option<crate::skill::SharedSkillRegistry> {
                Some(self.context.skill_registry.clone())
            }

            fn teammate_manager(&self) -> Option<crate::tool::SharedTeammateManager> {
                Some(self.context.teammate_manager.clone())
            }
        }

        let builds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = LegacyToolAdapter::from_arc(
            Arc::new(NeedsSkills) as Arc<dyn crate::tool::Tool>,
            Arc::new(CountingFactory {
                builds: builds.clone(),
                context: test_context("declared_domain_handle"),
            }),
        );
        let result = tact_kernel::capability::tool::Tool::call(
            &adapter,
            kernel_context(ToolOrigin::Main, false),
            json!({}),
        )
        .await
        .unwrap();
        assert!(
            result.content.contains("skill_registry"),
            "{}",
            result.content
        );
        assert!(
            !result.content.contains("teammate_manager"),
            "an undeclared handle must not be filled: {}",
            result.content
        );
        assert_eq!(
            builds.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the declared handle must not cost a ToolContext"
        );
    }

    /// A tool that only needs the workspace must not make the adapter build a
    /// whole `ToolContext`.
    ///
    /// The counter is the observable: the adapter asks the tool first, and a
    /// handler that opted into the narrow path never sees a context it cannot
    /// use. (The saving is one `PathBuf` and one `String`; the point is the
    /// dependency, not the allocation.)
    #[tokio::test]
    async fn a_workspace_only_tool_skips_the_context_build() {
        struct NeedsWorkspaceOnly {
            builds: Arc<std::sync::atomic::AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl crate::tool::Tool for NeedsWorkspaceOnly {
            fn metadata(&self) -> &'static crate::tool::ToolMetadata {
                static META: crate::tool::ToolMetadata =
                    crate::tool::ToolMetadata::read_json("saw_workspace", "Saw", "Saw");
                &META
            }

            fn input_schema(&self) -> serde_json::Value {
                json!({"type": "object"})
            }

            async fn call(
                &self,
                _context: crate::tool::ToolContext,
                _input: serde_json::Value,
            ) -> anyhow::Result<crate::tool::ToolCallResult> {
                self.builds
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::tool::ToolCallResult::text("full"))
            }

            fn call_in_workspace<'a>(
                &'a self,
                workspace: tact_kernel::tool::context::Workspace,
                _input: serde_json::Value,
            ) -> Option<
                std::pin::Pin<
                    Box<
                        dyn std::future::Future<
                                Output = anyhow::Result<crate::tool::ToolCallResult>,
                            > + Send
                            + 'a,
                    >,
                >,
            > {
                Some(Box::pin(async move {
                    Ok(crate::tool::ToolCallResult::text(format!(
                        "{}",
                        workspace.root().display()
                    )))
                }))
            }
        }

        struct CountingFactory {
            builds: Arc<std::sync::atomic::AtomicUsize>,
        }

        impl ToolContextFactory for CountingFactory {
            fn build(&self, _ctx: &ToolCallContext) -> ToolContext {
                self.builds
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                test_context("workspace_only_factory")
            }
        }

        let builds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool = NeedsWorkspaceOnly {
            builds: builds.clone(),
        };
        let adapter = LegacyToolAdapter::from_arc(
            Arc::new(tool) as Arc<dyn crate::tool::Tool>,
            Arc::new(CountingFactory {
                builds: builds.clone(),
            }),
        );
        let result = tact_kernel::capability::tool::Tool::call(
            &adapter,
            kernel_context(ToolOrigin::Main, false),
            json!({}),
        )
        .await
        .unwrap();
        assert!(result.content.starts_with("/mapped"), "{}", result.content);
        assert_eq!(
            builds.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the narrow path must not build a ToolContext"
        );
    }

    #[tokio::test]
    async fn adapter_maps_context_and_shares_cancel() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let adapter = LegacyToolAdapter::new(
            InspectingLegacyTool { seen: seen.clone() },
            Arc::new(SharedToolContextFactory::new(test_context(
                "legacy_adapter",
            ))),
        );
        let result = adapter
            .call(
                kernel_context(ToolOrigin::Subagent, true),
                json!({"text": "hi"}),
            )
            .await
            .unwrap();
        assert_eq!(result.content, "ok");

        let snapshot = seen.lock().unwrap().take().unwrap();
        assert_eq!(snapshot.work_dir, std::path::PathBuf::from("/mapped"));
        assert_eq!(snapshot.session_id.as_deref(), Some("session"));
        assert_eq!(snapshot.active_goal_id.as_deref(), Some("goal"));
        assert!(snapshot.is_subagent);
        assert!(snapshot.cancelled);
        assert_eq!(snapshot.input, json!({"text": "hi"}));
    }

    #[test]
    fn refreshing_the_factory_reaches_later_calls() {
        // The Agent stamps `permission_snapshot` and `subagent_results` after
        // construction; a frozen base would give every kernel-dispatched tool
        // the construction-time values, so a spawned child would lose its
        // parent's permissions and a background spawn would fail outright.
        let base = test_context("legacy_refresh");
        let factory = SharedToolContextFactory::new(base.clone());
        assert!(factory.with_base(|live| live.session_id.clone()).is_none());

        let mut live = base;
        live.session_id = Some("stamped-later".into());
        factory.refresh(&live);

        assert_eq!(
            factory.with_base(|live| live.session_id.clone()).as_deref(),
            Some("stamped-later")
        );
    }

    #[test]
    fn a_clone_of_the_factory_sees_the_same_refresh() {
        // The executor holds one clone and the Agent another; the refresh must
        // not be visible to only one of them.
        let base = test_context("legacy_refresh_clone");
        let factory = SharedToolContextFactory::new(base.clone());
        let executor_side = factory.clone();

        let mut live = base;
        live.session_id = Some("shared".into());
        factory.refresh(&live);

        assert_eq!(
            executor_side
                .with_base(|live| live.session_id.clone())
                .as_deref(),
            Some("shared")
        );
    }

    #[test]
    fn adapter_descriptor_comes_from_legacy_metadata() {
        let adapter = LegacyToolAdapter::new(
            InspectingLegacyTool {
                seen: Arc::default(),
            },
            Arc::new(SharedToolContextFactory::new(test_context(
                "legacy_descriptor",
            ))),
        );
        let descriptor = adapter.descriptor();
        assert_eq!(descriptor.name, "echo");
        assert_eq!(descriptor.description, "Echo");
        assert_eq!(
            descriptor.policy.permission,
            crate::tool::PermissionPolicy::Read
        );
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    use crate::tool::test_support::test_context;
    use crate::tool::{ToolMetadata, ToolRouter, toolset};
    use serde_json::{Value, json};

    #[tokio::test]
    async fn router_bridges_every_tool_into_the_kernel_catalog() {
        let router = toolset();
        let expected = router.tool_specs().len();
        let catalog = router
            .kernel_catalog(Arc::new(SharedToolContextFactory::new(test_context(
                "router_bridge",
            ))))
            .unwrap();
        let snapshot = catalog
            .descriptor_snapshot(&tact_kernel::capability::CapabilityFilter::default())
            .await
            .unwrap();
        assert_eq!(snapshot.tools.len(), expected);
        assert!(
            snapshot.tools.iter().any(|tool| tool.name == "bash"),
            "the bridge must keep the real tool set"
        );
    }

    #[tokio::test]
    async fn bridged_tool_is_callable_through_the_kernel_contract() {
        struct Echo;
        #[async_trait]
        impl LegacyTool for Echo {
            fn metadata(&self) -> &'static ToolMetadata {
                static META: ToolMetadata = ToolMetadata::read_json("echo", "Echo", "Echo");
                &META
            }

            fn input_schema(&self) -> Value {
                json!({"type": "object"})
            }

            async fn call(
                &self,
                _context: ToolContext,
                input: Value,
            ) -> anyhow::Result<ToolCallResult> {
                Ok(ToolCallResult::text(input["text"].as_str().unwrap_or("")))
            }
        }

        let catalog = ToolRouter::new()
            .route(Echo)
            .unwrap()
            .kernel_catalog(Arc::new(SharedToolContextFactory::new(test_context(
                "router_bridge_call",
            ))))
            .unwrap();
        let tool = catalog.get("echo").cloned().unwrap();
        let ctx = ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: ToolOrigin::Main,
            workspace: tact_kernel::tool::context::Workspace::new("/repo"),
            events: Arc::new(NoEvents),
            interaction: Arc::new(NoInteractions),
            cancel: tact_kernel::tool::context::CancelToken::new(),
            redaction: tact_kernel::tool::context::RedactionPolicy::Off,
        };
        let result = tool.call(ctx, json!({"text": "bridged"})).await.unwrap();
        assert_eq!(result.content, "bridged");
    }

    struct NoEvents;
    impl tact_kernel::tool::context::EventSink for NoEvents {
        fn emit(&self, _event: tact_kernel::tool::context::ToolEvent) {}
    }

    struct NoInteractions;
    #[async_trait]
    impl tact_kernel::tool::context::InteractionBroker for NoInteractions {
        async fn request(
            &self,
            _request: tact_kernel::tool::context::InteractionRequest,
        ) -> anyhow::Result<tact_kernel::tool::context::InteractionResponse> {
            Ok(tact_kernel::tool::context::InteractionResponse::Rejected)
        }
    }
}
