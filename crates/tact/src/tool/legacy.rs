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
}

/// A factory over a base context, mapping the run-scoped fields.
///
/// Domain handles (task/goal/team/worktree/background/memory/skill/session)
/// and the event sink stay from `base`; the adapter only overwrites what the
/// kernel call context actually knows.
#[derive(Clone)]
pub struct SharedToolContextFactory {
    base: ToolContext,
}

impl SharedToolContextFactory {
    pub fn new(base: ToolContext) -> Self {
        Self { base }
    }
}

impl ToolContextFactory for SharedToolContextFactory {
    fn build(&self, ctx: &ToolCallContext) -> ToolContext {
        let mut context = self.base.clone();
        context.work_dir = ctx.workspace.root().to_path_buf();
        context.session_id = ctx.session_id.clone();
        context.active_goal_id = ctx.active_goal_id.clone();
        context.is_subagent = matches!(ctx.origin, ToolOrigin::Subagent);
        context.cancel_flag = ctx.cancel.shared_flag();
        context
    }
}

/// A legacy handler exposed as a kernel [`KernelTool`].
pub struct LegacyToolAdapter<T: LegacyTool> {
    inner: T,
    contexts: Arc<dyn ToolContextFactory>,
}

impl<T: LegacyTool> LegacyToolAdapter<T> {
    pub fn new(inner: T, contexts: Arc<dyn ToolContextFactory>) -> Self {
        Self { inner, contexts }
    }
}

#[async_trait]
impl<T: LegacyTool + 'static> KernelTool for LegacyToolAdapter<T> {
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
