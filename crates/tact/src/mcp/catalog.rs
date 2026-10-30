//! Interior-synchronized MCP catalog shared by the tool port and facade.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use serde_json::{Map, Value};
use tokio::sync::RwLock as AsyncRwLock;

use super::{
    MCPToolRouter, McpPromptTool, McpResourceTool, ResolvedMcpTool, ToolListReport, ToolSpec,
    prompt_tool_risk, resource_tool_risk,
};
use crate::permission::{CapabilityRisk, normalize_mcp_capability};

struct CatalogState {
    router: AsyncRwLock<MCPToolRouter>,
    snapshot: RwLock<CatalogSnapshot>,
    generation: AtomicU64,
}

/// Cheap cloneable authority for one MCP router generation.
///
/// Replacing the facade's router creates a new handle. An activation keeps the
/// handle it started with, so a replacement between activations is visible on
/// the next run without changing the tool catalog under an in-flight one.
#[derive(Clone)]
pub struct McpCatalogHandle {
    state: Arc<CatalogState>,
}

impl McpCatalogHandle {
    pub(crate) fn new(router: MCPToolRouter) -> Self {
        let snapshot = snapshot_for(&router, 1);
        Self {
            state: Arc::new(CatalogState {
                router: AsyncRwLock::new(router),
                snapshot: RwLock::new(snapshot),
                generation: AtomicU64::new(1),
            }),
        }
    }

    fn snapshot(&self) -> std::sync::RwLockReadGuard<'_, CatalogSnapshot> {
        self.state
            .snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) async fn refresh_changed(&self) -> ToolListReport {
        let mut router = self.state.router.write().await;
        let report = router.refresh_changed().await;
        // A list change can alter descriptions or schemas without changing its
        // set of names, so the report alone cannot decide whether to rebuild.
        let probe = snapshot_for(&router, 0);
        let mut current = self
            .state
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.same_catalog(&probe) {
            return report;
        }
        let generation = self.state.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *current = snapshot_for(&router, generation);
        report
    }

    pub fn all_tools(&self) -> Vec<ToolSpec> {
        self.snapshot().tools.clone()
    }

    #[allow(dead_code)]
    pub(crate) fn generation(&self) -> u64 {
        self.snapshot().generation
    }

    pub fn resource_tool_specs(&self) -> Vec<ToolSpec> {
        self.snapshot().resource_tools.clone()
    }

    pub fn prompt_tool_specs(&self) -> Vec<ToolSpec> {
        self.snapshot().prompt_tools.clone()
    }

    pub fn resolve_tool(&self, name: &str) -> Result<Option<ResolvedMcpTool>> {
        if !MCPToolRouter::is_mcp_tool(name) {
            return Ok(None);
        }
        let parsed = name.parse::<super::McpToolName>()?;
        Ok(Some(ResolvedMcpTool {
            full_name: name.to_string(),
            server: parsed.server,
            tool: parsed.tool,
        }))
    }

    pub fn is_auto_approved(&self, server: &str, tool: &str) -> bool {
        self.snapshot().is_auto_approved(server, tool)
    }

    pub fn risk_for(&self, server: &str, tool: &str) -> CapabilityRisk {
        self.snapshot()
            .risk_for(server, tool)
            .unwrap_or_else(|| normalize_mcp_capability(server, tool))
    }

    pub(crate) fn resource_risk(&self, tool: McpResourceTool) -> CapabilityRisk {
        resource_tool_risk(tool)
    }

    pub(crate) fn prompt_risk(&self, tool: McpPromptTool) -> CapabilityRisk {
        prompt_tool_risk(tool)
    }

    pub fn output_token_limit(&self, full_name: &str) -> Option<usize> {
        self.snapshot()
            .output_token_limits
            .iter()
            .find_map(|(name, limit)| (name == full_name).then_some(*limit))
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<String> {
        let router = self.state.router.read().await;
        router.call(name, arguments).await
    }

    pub async fn list_resources(&self, server: Option<&str>) -> Result<String> {
        let router = self.state.router.read().await;
        router.list_resources(server).await
    }

    pub async fn list_resource_templates(&self, server: Option<&str>) -> Result<String> {
        let router = self.state.router.read().await;
        router.list_resource_templates(server).await
    }

    pub async fn read_resource(&self, server: &str, uri: &str) -> Result<String> {
        let router = self.state.router.read().await;
        router.read_resource(server, uri).await
    }

    pub async fn list_prompts(&self, server: Option<&str>) -> Result<String> {
        let router = self.state.router.read().await;
        router.list_prompts(server).await
    }

    pub async fn get_prompt(
        &self,
        server: &str,
        name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<String> {
        let router = self.state.router.read().await;
        router.get_prompt(server, name, arguments).await
    }

    pub fn instructions_block(&self) -> String {
        self.snapshot().instructions.clone()
    }

    pub fn server_summaries(&self) -> Vec<(String, usize)> {
        self.snapshot().server_summaries.clone()
    }

    pub async fn disconnect_all(&self) {
        self.state.router.write().await.disconnect_all().await;
    }
}

/// Sync lookup data derived once from a router generation.
#[derive(Clone, Default)]
struct CatalogSnapshot {
    #[allow(dead_code)]
    generation: u64,
    tools: Vec<ToolSpec>,
    resource_tools: Vec<ToolSpec>,
    prompt_tools: Vec<ToolSpec>,
    instructions: String,
    server_summaries: Vec<(String, usize)>,
    auto_approved: Vec<(String, String)>,
    risk: Vec<(String, String, CapabilityRisk)>,
    output_token_limits: Vec<(String, usize)>,
}

impl CatalogSnapshot {
    fn same_catalog(&self, other: &Self) -> bool {
        same_specs(&self.tools, &other.tools)
            && same_specs(&self.resource_tools, &other.resource_tools)
            && same_specs(&self.prompt_tools, &other.prompt_tools)
            && self.instructions == other.instructions
            && self.server_summaries == other.server_summaries
            && self.auto_approved == other.auto_approved
            && self.risk == other.risk
            && self.output_token_limits == other.output_token_limits
    }

    fn is_auto_approved(&self, server: &str, tool: &str) -> bool {
        self.auto_approved
            .iter()
            .any(|(candidate_server, candidate_tool)| {
                candidate_server == server && candidate_tool == tool
            })
    }

    fn risk_for(&self, server: &str, tool: &str) -> Option<CapabilityRisk> {
        self.risk
            .iter()
            .find_map(|(candidate_server, candidate_tool, risk)| {
                (candidate_server == server && candidate_tool == tool).then_some(*risk)
            })
    }
}

fn same_specs(left: &[ToolSpec], right: &[ToolSpec]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.name == right.name
                && left.description == right.description
                && left.input_schema == right.input_schema
        })
}

fn snapshot_for(router: &MCPToolRouter, generation: u64) -> CatalogSnapshot {
    let tools = router.all_tools();
    let resource_tools = router.resource_tool_specs();
    let prompt_tools = router.prompt_tool_specs();
    let instructions = router.instructions_block();
    let server_summaries = router.server_summaries();
    let mut auto_approved = Vec::new();
    let mut risk = Vec::new();
    let mut output_token_limits = Vec::new();
    for spec in &tools {
        let Ok(parsed) = spec.name.parse::<super::McpToolName>() else {
            continue;
        };
        if router.is_auto_approved(&parsed.server, &parsed.tool) {
            auto_approved.push((parsed.server.clone(), parsed.tool.clone()));
        }
        risk.push((
            parsed.server.clone(),
            parsed.tool.clone(),
            router.risk_for(&parsed.server, &parsed.tool),
        ));
        if let Some(limit) = router.output_token_limit(&spec.name) {
            output_token_limits.push((spec.name.clone(), limit));
        }
    }
    CatalogSnapshot {
        generation,
        tools,
        resource_tools,
        prompt_tools,
        instructions,
        server_summaries,
        auto_approved,
        risk,
        output_token_limits,
    }
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, sync::Arc};

    use rmcp::model::{CallToolResult, JsonObject, Tool as McpTool};

    use super::*;
    use crate::mcp::{McpClient, MockMcpService};

    fn tool(name: &'static str) -> McpTool {
        McpTool {
            name: Cow::Borrowed(name),
            title: None,
            description: None,
            input_schema: Arc::new(JsonObject::new()),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        }
    }

    fn router_with_server(name: &str, tools: Vec<McpTool>) -> (MCPToolRouter, Arc<MockMcpService>) {
        let mut router = MCPToolRouter::new();
        let service = Arc::new(MockMcpService::new(tools.clone(), |_| {
            Ok(CallToolResult::success(Vec::new()))
        }));
        router.register_client(McpClient::with_service(name, tools, service.clone()));
        (router, service)
    }

    #[tokio::test]
    async fn clones_share_one_generation_and_a_replacement_handle_is_isolated() {
        let (first_router, _) = router_with_server("first", vec![tool("echo")]);
        let first = McpCatalogHandle::new(first_router);
        let active = first.clone();
        let (replacement_router, _) = router_with_server("replacement", vec![tool("recall")]);
        let replacement = McpCatalogHandle::new(replacement_router);

        assert_eq!(first.generation(), 1);
        assert_eq!(
            active
                .all_tools()
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            ["mcp__first__echo"]
        );
        assert_eq!(
            replacement
                .all_tools()
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            ["mcp__replacement__recall"]
        );
        assert_eq!(first.generation(), 1);
        assert_eq!(replacement.generation(), 1);
    }

    #[tokio::test]
    async fn a_mid_turn_refresh_moves_the_shared_snapshot_and_generation() {
        let (router, service) = router_with_server("moving", vec![tool("one")]);
        let catalog = McpCatalogHandle::new(router);
        let active = catalog.clone();

        service.set_tools(vec![tool("one"), tool("two")]);
        service.announce_tools_changed();

        let report = catalog.refresh_changed().await;
        assert_eq!(report.changed.len(), 1);
        assert_eq!(report.changed[0].1.added, ["two"]);
        assert_eq!(catalog.generation(), 2);
        assert_eq!(
            active
                .all_tools()
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            ["mcp__moving__one", "mcp__moving__two"]
        );

        // Quiet refresh preserves the generation.
        assert!(catalog.refresh_changed().await.is_empty());
        assert_eq!(catalog.generation(), 2);
    }
}
