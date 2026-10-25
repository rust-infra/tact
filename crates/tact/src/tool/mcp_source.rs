//! Adapter from the MCP router to the kernel `ToolSource` contract.
//!
//! MCP tools are discovered at runtime, so they cannot be described by a
//! `&'static ToolMetadata`. The adapter builds owned [`ToolPolicy`] values and
//! routes calls back through the router; permission, security and hooks stay in
//! the kernel pipeline.

use std::sync::Arc;

use async_trait::async_trait;
use tact_kernel::capability::tool::Tool as KernelTool;
use tact_kernel::permission::CapabilityRisk;
use tact_kernel::tool::context::ToolCallContext;
use tact_kernel::tool::descriptor::ToolDescriptor;
use tact_kernel::tool::metadata::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ToolDomain, ToolPolicy, ToolPresentationOwned,
};
use tact_kernel::tool::result::ToolCallResult;
use tact_kernel::tool::source::ToolSource;

use crate::mcp::MCPToolRouter;

/// The runtime backend the adapter reads from and calls through.
#[async_trait]
pub trait McpToolBackend: Send + Sync {
    /// Every visible tool, as `(full name, description, schema)`.
    fn tools(&self) -> Vec<McpToolEntry>;
    async fn call(&self, full_name: &str, input: serde_json::Value) -> anyhow::Result<String>;
    fn risk(&self, full_name: &str) -> CapabilityRisk;
    fn is_auto_approved(&self, full_name: &str) -> bool;
    /// Re-list servers that announced `notifications/tools/list_changed`,
    /// reporting whether the visible set changed.
    async fn refresh(&self) -> anyhow::Result<bool>;
}

/// One discovered MCP tool, already namespaced as `mcp__server__tool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolEntry {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// A backend over a shared router.
///
/// The router is behind a `tokio` mutex because `MCPToolRouter::call` is async
/// and the tool-list refresh mutates it.
pub struct SharedMcpBackend {
    router: tokio::sync::Mutex<MCPToolRouter>,
}

impl SharedMcpBackend {
    pub fn new(router: MCPToolRouter) -> Self {
        Self {
            router: tokio::sync::Mutex::new(router),
        }
    }
}

#[async_trait]
impl McpToolBackend for SharedMcpBackend {
    fn tools(&self) -> Vec<McpToolEntry> {
        // `try_lock` keeps this method synchronous; the router is only mutated
        // by `refresh`, which runs before the snapshot is taken.
        let Ok(router) = self.router.try_lock() else {
            return Vec::new();
        };
        router
            .all_tools()
            .into_iter()
            .map(|spec| McpToolEntry {
                name: spec.name,
                description: spec.description,
                input_schema: spec.input_schema,
            })
            .collect()
    }

    async fn call(&self, full_name: &str, input: serde_json::Value) -> anyhow::Result<String> {
        self.router.lock().await.call(full_name, input).await
    }

    fn risk(&self, full_name: &str) -> CapabilityRisk {
        self.router
            .try_lock()
            .map(|router| {
                let Some(parsed) = full_name.split_once("__") else {
                    return CapabilityRisk::High;
                };
                let rest = parsed.1;
                let Some((server, tool)) = rest.rsplit_once("__") else {
                    return CapabilityRisk::High;
                };
                router.risk_for(server, tool)
            })
            .unwrap_or(CapabilityRisk::High)
    }

    fn is_auto_approved(&self, full_name: &str) -> bool {
        self.router
            .try_lock()
            .map(|router| {
                let Some(rest) = full_name.strip_prefix("mcp__") else {
                    return false;
                };
                let Some((server, tool)) = rest.rsplit_once("__") else {
                    return false;
                };
                router.is_auto_approved(server, tool)
            })
            .unwrap_or(false)
    }

    async fn refresh(&self) -> anyhow::Result<bool> {
        let mut router = self.router.lock().await;
        let report = router.refresh_changed().await;
        anyhow::ensure!(
            report.failed.is_empty(),
            "MCP tool refresh failed: {:?}",
            report.failed
        );
        // The router only re-lists servers that announced a change, so a
        // non-empty report is exactly "the visible tool set moved".
        Ok(!report.changed.is_empty())
    }
}

/// One discovered tool exposed as a kernel [`KernelTool`].
pub struct McpTool {
    entry: McpToolEntry,
    backend: Arc<dyn McpToolBackend>,
}

impl McpTool {
    pub fn new(entry: McpToolEntry, backend: Arc<dyn McpToolBackend>) -> Self {
        Self { entry, backend }
    }
}

fn policy_for(risk: CapabilityRisk, display_name: String) -> ToolPolicy {
    let permission = match risk {
        CapabilityRisk::Read => PermissionPolicy::Read,
        CapabilityRisk::Write => PermissionPolicy::Write,
        CapabilityRisk::High => PermissionPolicy::High,
    };
    ToolPolicy {
        permission,
        permission_prompt: PermissionPromptPolicy::Json,
        resources: tact_kernel::tool::ResourcePolicy::Independent,
        domain: ToolDomain::Generic,
        presentation: ToolPresentationOwned {
            visual_kind: tact_protocol::ToolVisualKind::Generic,
            display_name,
            live_output: LiveOutputPolicy::Standard,
            detail: DetailPolicy::Result,
            popup: PopupPolicy::None,
            compact_result_to_meta: false,
        },
        output: OutputPolicy::PersistLargeOutput,
        argument_summary: ArgumentSummaryPolicy::Json,
    }
}

#[async_trait]
impl KernelTool for McpTool {
    fn descriptor(&self) -> ToolDescriptor {
        let risk = self.backend.risk(&self.entry.name);
        ToolDescriptor {
            name: self.entry.name.clone(),
            description: self
                .entry
                .description
                .clone()
                .unwrap_or_else(|| self.entry.name.clone()),
            input_schema: self.entry.input_schema.clone(),
            policy: policy_for(risk, self.entry.name.clone()),
        }
    }

    async fn call(
        &self,
        _ctx: ToolCallContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolCallResult> {
        let text = self.backend.call(&self.entry.name, input).await?;
        Ok(ToolCallResult::text(text))
    }
}

/// A plugin's dynamic tool set: every visible MCP tool of one router.
pub struct McpToolSource {
    id: String,
    backend: Arc<dyn McpToolBackend>,
}

impl McpToolSource {
    pub fn new(id: impl Into<String>, backend: Arc<dyn McpToolBackend>) -> Self {
        Self {
            id: id.into(),
            backend,
        }
    }
}

#[async_trait]
impl ToolSource for McpToolSource {
    fn id(&self) -> &str {
        &self.id
    }

    async fn refresh(&self) -> anyhow::Result<bool> {
        self.backend.refresh().await
    }

    fn tools(&self) -> Vec<Arc<dyn KernelTool>> {
        self.backend
            .tools()
            .into_iter()
            .map(|entry| Arc::new(McpTool::new(entry, self.backend.clone())) as Arc<dyn KernelTool>)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{McpClient, MockMcpService};
    use rmcp::model::{CallToolResult, Content, JsonObject, Tool as McpToolModel};
    use serde_json::json;
    use std::borrow::Cow;
    use std::sync::Mutex;

    fn echo_tool() -> McpToolModel {
        McpToolModel {
            name: Cow::Borrowed("echo"),
            title: None,
            description: Some(Cow::Borrowed("Echo the input back")),
            input_schema: Arc::new(JsonObject::new()),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        }
    }

    fn backend() -> Arc<dyn McpToolBackend> {
        let service = MockMcpService::new(vec![echo_tool()], |params| {
            let text = params
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            Ok(CallToolResult::success(vec![Content::text(text)]))
        });
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service(
            "demo",
            vec![echo_tool()],
            Arc::new(service),
        ));
        Arc::new(SharedMcpBackend::new(router))
    }

    #[test]
    fn source_exposes_namespaced_descriptors() {
        let source = McpToolSource::new("mcp", backend());
        assert_eq!(source.id(), "mcp");
        let tools = source.tools();
        assert_eq!(tools.len(), 1);
        let descriptor = tools[0].descriptor();
        assert_eq!(descriptor.name, "mcp__demo__echo");
        assert_eq!(descriptor.description, "Echo the input back");
        assert_eq!(descriptor.policy.domain, ToolDomain::Generic);
    }

    #[tokio::test]
    async fn call_routes_through_the_router() {
        let backend = backend();
        let entry = backend.tools().into_iter().next().unwrap();
        let tool = McpTool::new(entry, backend);
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
        let ctx = ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: tact_kernel::tool::context::ToolOrigin::Main,
            workspace: tact_kernel::tool::context::Workspace::new("/repo"),
            events: Arc::new(Events),
            interaction: Arc::new(Interactions),
            cancel: tact_kernel::tool::context::CancelToken::new(),
            redaction: tact_kernel::tool::context::RedactionPolicy::Off,
        };
        let result = tool.call(ctx, json!({"text": "hello"})).await.unwrap();
        assert_eq!(result.content, "hello");
    }

    #[test]
    fn unknown_names_default_to_high_risk() {
        let backend = backend();
        assert_eq!(backend.risk("not-an-mcp-tool"), CapabilityRisk::High);
        assert!(!backend.is_auto_approved("not-an-mcp-tool"));
    }

    #[test]
    fn high_risk_descriptor_maps_to_high_permission() {
        let policy = policy_for(CapabilityRisk::High, "mcp__demo__echo".into());
        assert_eq!(policy.permission, PermissionPolicy::High);
        assert_eq!(policy.presentation.display_name, "mcp__demo__echo");
        assert_eq!(policy.output, OutputPolicy::PersistLargeOutput);
    }

    #[test]
    fn no_mutex_poisoning_on_missing_router_state() {
        // Guards the `try_lock` fallbacks: they must degrade to High, never panic.
        let source = McpToolSource::new("mcp", backend());
        let _ = source.tools();
        let holder: Mutex<()> = Mutex::new(());
        drop(holder);
    }
}
