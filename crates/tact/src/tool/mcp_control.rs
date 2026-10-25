//! Tact's own MCP control tools as kernel tools.
//!
//! `list_mcp_resources` / `read_mcp_resource` / `list_mcp_prompts` /
//! `get_mcp_prompt` belong to Tact, not to a server, so their risk comes from
//! `[mcp]` settings rather than a plugin's descriptor. Exposing them as kernel
//! tools is what lets the Agent drop its last hand-written permission ladder:
//! the pipeline asks `PermissionService`, and this file only answers "what risk
//! does this declaration carry" and "how do I run it".

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
use tact_protocol::ToolVisualKind;

use crate::mcp::{MCPToolRouter, McpPromptTool, McpResourceTool};

/// Which family of Tact-owned MCP tool this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpControl {
    Resource(McpResourceTool),
    Prompt(McpPromptTool),
}

impl McpControl {
    /// Every control tool, in stable order.
    pub fn all() -> Vec<Self> {
        McpResourceTool::ALL
            .into_iter()
            .map(Self::Resource)
            .chain(McpPromptTool::ALL.into_iter().map(Self::Prompt))
            .collect()
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Resource(tool) => tool.name(),
            Self::Prompt(tool) => tool.name(),
        }
    }

    pub fn risk(self) -> CapabilityRisk {
        match self {
            Self::Resource(tool) => crate::mcp::resource_tool_risk(tool),
            Self::Prompt(tool) => crate::mcp::prompt_tool_risk(tool),
        }
    }

    fn spec(self) -> crate::ToolSpec {
        match self {
            Self::Resource(tool) => tool.spec(),
            Self::Prompt(tool) => tool.spec(),
        }
    }

    fn description(self) -> String {
        self.spec()
            .description
            .clone()
            .unwrap_or_else(|| self.name().to_string())
    }

    fn input_schema(self) -> serde_json::Value {
        self.spec().input_schema
    }
}

/// The router-backed backend these tools run against.
#[async_trait]
pub trait McpControlBackend: Send + Sync {
    async fn run(&self, control: McpControl, input: serde_json::Value) -> anyhow::Result<String>;
    /// Whether the tool is advertised at all: with no server connected these
    /// would be tools that can only answer "no MCP servers are connected".
    fn is_available(&self) -> bool;
}

/// A backend over an immutable snapshot of the router.
///
/// The snapshot owns cloned clients (cheap: the services are `Arc`ed), so no lock
/// is taken and a reload of the live router cannot block or be blocked by a
/// resource read. The host swaps a new snapshot in after every load.
pub struct SnapshotMcpControlBackend {
    router: MCPToolRouter,
}

impl SnapshotMcpControlBackend {
    pub fn new(router: &MCPToolRouter) -> Self {
        Self {
            router: router.snapshot(),
        }
    }
}

#[async_trait]
impl McpControlBackend for SnapshotMcpControlBackend {
    async fn run(&self, control: McpControl, input: serde_json::Value) -> anyhow::Result<String> {
        let router = &self.router;
        let server = input.get("server").and_then(|value| value.as_str());
        match control {
            McpControl::Resource(McpResourceTool::List) => router.list_resources(server).await,
            McpControl::Resource(McpResourceTool::Templates) => {
                router.list_resource_templates(server).await
            }
            McpControl::Resource(McpResourceTool::Read) => {
                let server = server.ok_or_else(|| {
                    anyhow::anyhow!("Error invoking read_mcp_resource: `server` is required")
                })?;
                let uri = input
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Error invoking read_mcp_resource: `uri` is required")
                    })?;
                router.read_resource(server, uri).await
            }
            McpControl::Prompt(McpPromptTool::List) => router.list_prompts(server).await,
            McpControl::Prompt(McpPromptTool::Get) => {
                let server = server.ok_or_else(|| {
                    anyhow::anyhow!("Error invoking get_mcp_prompt: `server` is required")
                })?;
                let name = input
                    .get("name")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Error invoking get_mcp_prompt: `name` is required")
                    })?;
                let arguments = input
                    .get("arguments")
                    .and_then(|value| value.as_object())
                    .cloned();
                router.get_prompt(server, name, arguments).await
            }
        }
    }

    fn is_available(&self) -> bool {
        // No lock, no fallback: the snapshot either has servers or it does not.
        !self.router.server_summaries().is_empty()
    }
}

/// One MCP control tool.
pub struct McpControlTool {
    control: McpControl,
    backend: Arc<dyn McpControlBackend>,
}

impl McpControlTool {
    pub fn new(control: McpControl, backend: Arc<dyn McpControlBackend>) -> Self {
        Self { control, backend }
    }
}

fn policy_for(control: McpControl, display_name: String) -> ToolPolicy {
    let permission = match control.risk() {
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
            visual_kind: ToolVisualKind::Generic,
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
impl KernelTool for McpControlTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.control.name().to_string(),
            description: self.control.description(),
            input_schema: self.control.input_schema(),
            policy: policy_for(self.control, self.control.name().to_string()),
        }
    }

    async fn call(
        &self,
        _ctx: ToolCallContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolCallResult> {
        Ok(ToolCallResult::text(
            self.backend.run(self.control, input).await?,
        ))
    }
}

/// The five control tools as one dynamic source.
pub struct McpControlSource {
    backend: Arc<dyn McpControlBackend>,
}

impl McpControlSource {
    pub fn new(backend: Arc<dyn McpControlBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl ToolSource for McpControlSource {
    fn id(&self) -> &str {
        "mcp-control"
    }

    async fn refresh(&self) -> anyhow::Result<bool> {
        // The control tools never change shape; only their availability does,
        // and that is read in `tools()`, which the snapshot already calls.
        Ok(false)
    }

    fn tools(&self) -> Vec<Arc<dyn KernelTool>> {
        if !self.backend.is_available() {
            return Vec::new();
        }
        McpControl::all()
            .into_iter()
            .map(|control| {
                Arc::new(McpControlTool::new(control, self.backend.clone())) as Arc<dyn KernelTool>
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixed {
        available: bool,
        calls: std::sync::Mutex<Vec<(McpControl, serde_json::Value)>>,
    }

    #[async_trait]
    impl McpControlBackend for Fixed {
        async fn run(
            &self,
            control: McpControl,
            input: serde_json::Value,
        ) -> anyhow::Result<String> {
            self.calls.lock().unwrap().push((control, input));
            Ok("ok".to_string())
        }

        fn is_available(&self) -> bool {
            self.available
        }
    }

    fn backend(available: bool) -> Arc<Fixed> {
        Arc::new(Fixed {
            available,
            calls: std::sync::Mutex::new(Vec::new()),
        })
    }

    #[test]
    fn every_control_tool_has_a_descriptor() {
        let source = McpControlSource::new(backend(true));
        let tools = source.tools();
        assert_eq!(tools.len(), McpControl::all().len());
        for tool in &tools {
            let descriptor = tool.descriptor();
            assert!(!descriptor.name.is_empty());
            assert!(!descriptor.description.is_empty());
            assert!(descriptor.input_schema.is_object());
        }
    }

    #[test]
    fn no_server_means_no_tools() {
        let source = McpControlSource::new(backend(false));
        assert!(source.tools().is_empty());
    }

    #[test]
    fn risk_comes_from_the_mcp_settings_helper() {
        let source = McpControlSource::new(backend(true));
        for tool in source.tools() {
            let descriptor = tool.descriptor();
            let control = McpControl::all()
                .into_iter()
                .find(|control| control.name() == descriptor.name)
                .unwrap();
            let expected = match control.risk() {
                CapabilityRisk::Read => PermissionPolicy::Read,
                CapabilityRisk::Write => PermissionPolicy::Write,
                CapabilityRisk::High => PermissionPolicy::High,
            };
            assert_eq!(descriptor.policy.permission, expected);
        }
    }

    #[tokio::test]
    async fn a_call_reaches_the_backend_with_its_input() {
        let fixed = backend(true);
        let tool = McpControlTool::new(McpControl::Resource(McpResourceTool::Read), fixed.clone());
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
                Ok(tact_kernel::tool::context::InteractionResponse::Cancelled)
            }
        }
        let ctx = ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "call".into(),
            session_id: None,
            active_goal_id: None,
            origin: tact_kernel::tool::context::ToolOrigin::Main,
            workspace: tact_kernel::tool::context::Workspace::new("/repo"),
            events: Arc::new(NoEvents),
            interaction: Arc::new(NoInteractions),
            cancel: tact_kernel::tool::context::CancelToken::new(),
            redaction: tact_kernel::tool::context::RedactionPolicy::Off,
        };
        let result = tool
            .call(ctx, json!({"server": "s", "uri": "u"}))
            .await
            .unwrap();
        assert_eq!(result.content, "ok");
        assert_eq!(
            fixed.calls.lock().unwrap().clone(),
            vec![(
                McpControl::Resource(McpResourceTool::Read),
                json!({"server": "s", "uri": "u"})
            )]
        );
    }
}
