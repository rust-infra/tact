//! Model Context Protocol (MCP) integration.
//!
//! MCP is a protocol that allows external tools (written in any language)
//! to expose capabilities to the agent via a JSON-RPC transport.
//!
//! ## Architecture
//!
//! - [`PluginLoader`] scans `.claude-plugin/plugin.json` manifests in
//!   configured search directories.  Each manifest declares MCP servers.
//! - [`McpClient`] connects to an MCP server over stdio transport using
//!   the [`rmcp`] crate, fetches its tool list, and proxies calls.
//! - [`MCPToolRouter`] aggregates all connected clients and routes
//!   incoming tool calls by name (format: `mcp__<server>__<tool>`).
//! - [`McpToolName::try_from`] parses this namespaced naming convention.
//! - [`load_mcp_router`] is the entry point: scans plugins, connects
//!   servers, and returns a ready-to-use router.

use std::{collections::HashMap, fs, path::PathBuf, process::Stdio, sync::Arc};

use anyhow::{Context, Result, bail};
use futures_util::{
    StreamExt,
    future::{BoxFuture, FutureExt},
    stream::FuturesUnordered,
};
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, RawContent, ResourceContents, Tool as McpTool},
    service::{RunningService, ServiceError},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::process::Command;

use crate::{ToolSpec, tool::copy_tool_spec};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Default)]
pub struct PluginLoader {
    search_dirs: Vec<PathBuf>,
    plugins: HashMap<String, PluginManifest>,
}

impl PluginLoader {
    pub fn new(search_dirs: Vec<PathBuf>) -> Self {
        Self { search_dirs, plugins: HashMap::new() }
    }

    pub fn scan(&mut self) -> Result<Vec<String>> {
        self.plugins.clear();
        let mut loaded = Vec::new();

        for dir in &self.search_dirs {
            let manifest_path = dir.join(".claude-plugin").join("plugin.json");
            if !manifest_path.exists() {
                continue;
            }

            let raw = fs::read_to_string(&manifest_path)
                .with_context(|| format!("failed to read {}", manifest_path.display()))?;
            let manifest: PluginManifest =
                serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", manifest_path.display()))?;

            loaded.push(manifest.name.clone());
            self.plugins.insert(manifest.name.clone(), manifest);
        }

        Ok(loaded)
    }

    pub fn mcp_servers(&self) -> HashMap<String, McpServerConfig> {
        let mut servers = HashMap::new();
        for (plugin_name, manifest) in &self.plugins {
            for (server_name, config) in &manifest.mcp_servers {
                servers.insert(format!("{plugin_name}__{server_name}"), config.clone());
            }
        }
        servers
    }
}

/// Low-level interface exposed by an MCP transport, used so tests can swap in
/// a mock implementation without spawning real child processes.
pub trait McpService: Send + Sync + 'static {
    /// List every tool advertised by the server.
    fn list_all_tools(&self) -> BoxFuture<'_, Result<Vec<McpTool>, ServiceError>>;

    /// Execute a single tool call.
    fn call_tool(&self, params: CallToolRequestParams) -> BoxFuture<'_, Result<CallToolResult, ServiceError>>;

    /// Optional cleanup hook. The default implementation is a no-op.
    fn cancel(&self) -> BoxFuture<'_, ()> {
        std::future::ready(()).boxed()
    }
}

struct RealMcpService(tokio::sync::RwLock<Option<RunningService<RoleClient, ()>>>);

impl RealMcpService {
    fn new(service: RunningService<RoleClient, ()>) -> Self {
        Self(tokio::sync::RwLock::new(Some(service)))
    }
}

impl McpService for RealMcpService {
    fn list_all_tools(&self) -> BoxFuture<'_, Result<Vec<McpTool>, ServiceError>> {
        async move {
            let guard = self.0.read().await;
            match guard.as_ref() {
                Some(service) => service.list_all_tools().await,
                None => Err(ServiceError::TransportClosed),
            }
        }
        .boxed()
    }

    fn call_tool(&self, params: CallToolRequestParams) -> BoxFuture<'_, Result<CallToolResult, ServiceError>> {
        async move {
            let guard = self.0.read().await;
            match guard.as_ref() {
                Some(service) => service.call_tool(params).await,
                None => Err(ServiceError::TransportClosed),
            }
        }
        .boxed()
    }

    fn cancel(&self) -> BoxFuture<'_, ()> {
        async move {
            let mut guard = self.0.write().await;
            if let Some(service) = guard.take() {
                let _ = service.cancel().await;
            }
        }
        .boxed()
    }
}

pub struct McpClient {
    pub server_name: String,
    service: Arc<dyn McpService>,
    tools: Vec<McpTool>,
    tool_specs: Vec<ToolSpec>,
}

impl McpClient {
    pub async fn try_new(server_name: impl Into<String>, config: McpServerConfig) -> Result<Self> {
        let server_name = server_name.into();
        let running = Self::connect(&server_name, config).await?;
        let service: Arc<dyn McpService> = Arc::new(RealMcpService::new(running));
        match Self::fetch_tools(&server_name, service.as_ref()).await {
            Ok(tools) => {
                let tool_specs = build_tool_specs(&server_name, &tools);
                Ok(Self { server_name, service, tools, tool_specs })
            },
            Err(err) => {
                let _ = service.cancel().await;
                Err(err)
            },
        }
    }

    /// Build a client from an arbitrary [`McpService`] implementation.
    ///
    /// This is the entry point for test doubles: construct a [`MockMcpService`],
    /// wrap it, and register it with [`MCPToolRouter`].
    pub fn with_service(server_name: impl Into<String>, tools: Vec<McpTool>, service: Arc<dyn McpService>) -> Self {
        let server_name = server_name.into();
        let tool_specs = build_tool_specs(&server_name, &tools);
        Self { server_name, service, tools, tool_specs }
    }

    pub fn list_tools(&self) -> &[McpTool] {
        &self.tools
    }

    async fn connect(server_name: &str, config: McpServerConfig) -> Result<RunningService<RoleClient, ()>> {
        let command = config.command;
        let args = config.args;
        let env = config.env;
        let transport = TokioChildProcess::builder(Command::new(&command).configure(move |cmd| {
            cmd.args(&args).envs(&env).stderr(Stdio::inherit());
        }))
        .spawn()
        .with_context(|| format!("failed to spawn MCP server {server_name}"))?
        .0;

        ().serve(transport).await.with_context(|| format!("failed to initialize MCP client for server {server_name}"))
    }

    async fn fetch_tools(server_name: &str, service: &dyn McpService) -> Result<Vec<McpTool>> {
        service.list_all_tools().await.with_context(|| format!("failed to list tools from {server_name}"))
    }

    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<String> {
        let arguments = match arguments {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => {
                let mut map = Map::new();
                map.insert("value".to_string(), other);
                Some(map)
            },
        };

        let result = self
            .service
            .call_tool(CallToolRequestParams { meta: None, name: tool_name.to_string().into(), arguments, task: None })
            .await
            .with_context(|| format!("failed to call MCP tool {tool_name}"))?;

        Ok(join_mcp_content(&result.content))
    }

    pub fn agent_tools(&self) -> &[ToolSpec] {
        &self.tool_specs
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub async fn shutdown(self) {
        let _ = self.service.cancel().await;
    }
}

/// Test double for an MCP server.
///
/// Configure it with a tool list and a handler closure; calls are forwarded to
/// the closure so tests can assert inputs and return canned responses.
type McpToolHandler = Arc<dyn Fn(&CallToolRequestParams) -> Result<CallToolResult, ServiceError> + Send + Sync>;

pub struct MockMcpService {
    tools: Vec<McpTool>,
    handler: McpToolHandler,
    calls: std::sync::Mutex<Vec<(String, Value)>>,
}

impl MockMcpService {
    pub fn new<F>(tools: Vec<McpTool>, handler: F) -> Self
    where
        F: Fn(&CallToolRequestParams) -> Result<CallToolResult, ServiceError> + Send + Sync + 'static,
    {
        Self { tools, handler: Arc::new(handler), calls: std::sync::Mutex::new(Vec::new()) }
    }

    /// Return every `(tool_name, arguments)` pair received so far.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl McpService for MockMcpService {
    fn list_all_tools(&self) -> BoxFuture<'_, Result<Vec<McpTool>, ServiceError>> {
        let tools = self.tools.clone();
        std::future::ready(Ok(tools)).boxed()
    }

    fn call_tool(&self, params: CallToolRequestParams) -> BoxFuture<'_, Result<CallToolResult, ServiceError>> {
        let name = params.name.to_string();
        let args = params.arguments.as_ref().map(|m| Value::Object(m.clone())).unwrap_or(Value::Null);
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push((name, args));
        let handler = self.handler.clone();
        std::future::ready(handler(&params)).boxed()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolName {
    pub server: String,
    pub tool: String,
}

impl TryFrom<&str> for McpToolName {
    type Error = anyhow::Error;

    fn try_from(tool_name: &str) -> Result<Self> {
        let Some(rest) = tool_name.strip_prefix("mcp__") else {
            bail!("not an MCP tool name: {tool_name}");
        };
        let Some((server, tool)) = rest.rsplit_once("__") else {
            bail!("invalid MCP tool name: {tool_name}");
        };
        if server.is_empty() || tool.is_empty() {
            bail!("invalid MCP tool name: {tool_name}");
        }

        Ok(Self { server: server.to_string(), tool: tool.to_string() })
    }
}

#[derive(Default)]
pub struct MCPToolRouter {
    clients: HashMap<String, McpClient>,
}

impl MCPToolRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_client(&mut self, client: McpClient) {
        self.clients.insert(client.server_name.clone(), client);
    }

    pub fn is_mcp_tool(tool_name: &str) -> bool {
        tool_name.starts_with("mcp__")
    }

    pub async fn call(&self, tool_name: &str, arguments: Value) -> Result<String> {
        let parsed = McpToolName::try_from(tool_name)?;
        let client =
            self.clients.get(&parsed.server).with_context(|| format!("unknown MCP server {}", parsed.server))?;

        client.call_tool(&parsed.tool, arguments).await
    }

    pub fn all_tools(&self) -> Vec<ToolSpec> {
        self.clients.values().flat_map(|client| client.tool_specs.iter().map(copy_tool_spec)).collect()
    }

    pub fn server_summaries(&self) -> Vec<(String, usize)> {
        let mut summaries =
            self.clients.iter().map(|(name, client)| (name.clone(), client.tool_count())).collect::<Vec<_>>();
        summaries.sort_by(|a, b| a.0.cmp(&b.0));
        summaries
    }

    pub async fn disconnect_all(&mut self) {
        for (_, client) in self.clients.drain() {
            client.shutdown().await;
        }
    }
}

fn build_tool_specs(server_name: &str, tools: &[McpTool]) -> Vec<ToolSpec> {
    tools
        .iter()
        .map(|tool| ToolSpec {
            name: format!("mcp__{server_name}__{}", tool.name),
            description: tool.description.as_ref().map(ToString::to_string),
            input_schema: Value::Object((*tool.input_schema).clone()),
        })
        .collect()
}

pub async fn load_mcp_router() -> Result<MCPToolRouter> {
    let cwd = std::env::current_dir()?;
    let mut loader = PluginLoader::new(vec![cwd]);
    let plugins = loader.scan()?;
    if plugins.is_empty() {
        println!("[Plugins: none]");
    } else {
        println!("[Plugins: {}]", plugins.join(", "));
    }

    let mut router = MCPToolRouter::new();
    let mut connections = FuturesUnordered::new();
    for (server_name, config) in loader.mcp_servers() {
        connections.push(async move {
            let result = McpClient::try_new(server_name.clone(), config).await;
            (server_name, result)
        });
    }
    while let Some((server_name, result)) = connections.next().await {
        match result {
            Ok(client) => {
                println!("[MCP connected: {server_name} ({} tools)]", client.list_tools().len());
                router.register_client(client);
            },
            Err(err) => {
                println!("[MCP connect failed: {server_name}: {err:#}]");
            },
        }
    }

    Ok(router)
}

fn join_mcp_content(content: &[rmcp::model::Content]) -> String {
    let parts = content
        .iter()
        .map(|content| match &content.raw {
            RawContent::Text(text) => text.text.clone(),
            RawContent::Resource(resource) => match &resource.resource {
                ResourceContents::TextResourceContents { text, .. } => text.clone(),
                _ => String::new(),
            },
            other => serde_json::to_string(other).unwrap_or_else(|_| "<non-text content>".into()),
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, sync::Arc};

    use rmcp::{
        ErrorData as McpError, ServerHandler, ServiceExt,
        model::{CallToolResult, Content, JsonObject, ListToolsResult, ServerInfo, Tool as McpTool},
        service::{RequestContext, RoleServer},
    };
    use serde_json::json;

    use super::{
        MCPToolRouter, McpClient, McpServerConfig, McpToolName, MockMcpService, PluginLoader, PluginManifest,
        RealMcpService,
    };

    #[test]
    fn parses_plugin_manifest() {
        let raw = r#"{
          "name": "demo",
          "version": "1.0.0",
          "mcpServers": {
            "echo": {
              "command": "node",
              "args": ["server.js"],
              "env": {"A": "B"}
            }
          }
        }"#;

        let manifest: PluginManifest = serde_json::from_str(raw).unwrap();
        let expected = McpServerConfig {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            env: [("A".to_string(), "B".to_string())].into(),
        };

        assert_eq!(manifest.name, "demo");
        assert_eq!(manifest.version.as_deref(), Some("1.0.0"));
        assert_eq!(manifest.mcp_servers["echo"].command, expected.command);
        assert_eq!(manifest.mcp_servers["echo"].args, expected.args);
        assert_eq!(manifest.mcp_servers["echo"].env, expected.env);
    }

    #[test]
    fn parses_mcp_tool_name_with_plugin_server_prefix() {
        let parsed = McpToolName::try_from("mcp__demo__postgres__query").unwrap();

        assert_eq!(parsed.server, "demo__postgres");
        assert_eq!(parsed.tool, "query");
    }

    fn echo_tool() -> McpTool {
        McpTool {
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

    #[tokio::test]
    async fn mock_mcp_service_routes_calls() {
        let echo_tool = echo_tool();

        let service = MockMcpService::new(vec![echo_tool.clone()], |params| {
            let text = params
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(CallToolResult::success(vec![Content::text(text)]))
        });

        let client = McpClient::with_service("test", vec![echo_tool], Arc::new(service));
        let mut router = MCPToolRouter::new();
        router.register_client(client);

        assert_eq!(router.all_tools().len(), 1);
        let output = router.call("mcp__test__echo", json!({"text": "hello"})).await.unwrap();
        assert_eq!(output, "hello");
    }

    #[test]
    fn plugin_loader_scans_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join(".claude-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{
                "name": "demo",
                "mcpServers": {
                    "echo": { "command": "cat" }
                }
            }"#,
        )
        .unwrap();

        let mut loader = PluginLoader::new(vec![tmp.path().to_path_buf()]);
        let loaded = loader.scan().unwrap();
        assert_eq!(loaded, vec!["demo"]);

        let servers = loader.mcp_servers();
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key("demo__echo"));
        assert_eq!(servers["demo__echo"].command, "cat");
    }

    fn upper_tool() -> McpTool {
        McpTool {
            name: Cow::Borrowed("upper"),
            title: None,
            description: Some(Cow::Borrowed("Uppercase the input")),
            input_schema: Arc::new(JsonObject::new()),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        }
    }

    #[tokio::test]
    async fn router_routes_multiple_servers() {
        let echo = echo_tool();
        let upper = upper_tool();

        let echo_service = MockMcpService::new(vec![echo.clone()], |params| {
            let text = params
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(CallToolResult::success(vec![Content::text(text)]))
        });
        let upper_service = MockMcpService::new(vec![upper.clone()], |params| {
            let text = params
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_uppercase();
            Ok(CallToolResult::success(vec![Content::text(text)]))
        });

        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service("demo", vec![echo], Arc::new(echo_service)));
        router.register_client(McpClient::with_service("other", vec![upper], Arc::new(upper_service)));

        let echo_out = router.call("mcp__demo__echo", json!({"text": "hello"})).await.unwrap();
        let upper_out = router.call("mcp__other__upper", json!({"text": "hello"})).await.unwrap();

        assert_eq!(echo_out, "hello");
        assert_eq!(upper_out, "HELLO");
        assert_eq!(router.server_summaries(), vec![("demo".to_string(), 1), ("other".to_string(), 1)]);
    }

    #[tokio::test]
    async fn mock_service_records_calls() {
        let echo = echo_tool();
        let service = Arc::new(MockMcpService::new(vec![echo.clone()], |params| {
            let text = params
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }));

        let client = McpClient::with_service("test", vec![echo], service.clone());
        let mut router = MCPToolRouter::new();
        router.register_client(client);

        let _ = router.call("mcp__test__echo", json!({"text": "first"})).await.unwrap();
        let _ = router.call("mcp__test__echo", json!({"text": "second"})).await.unwrap();

        let calls = service.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "echo");
        assert_eq!(calls[0].1, json!({"text": "first"}));
        assert_eq!(calls[1].0, "echo");
        assert_eq!(calls[1].1, json!({"text": "second"}));
    }

    #[tokio::test]
    async fn router_handles_concurrent_calls() {
        let echo = echo_tool();
        let service = MockMcpService::new(vec![echo.clone()], |params| {
            let text = params
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(CallToolResult::success(vec![Content::text(text)]))
        });

        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service("test", vec![echo], Arc::new(service)));
        let router = Arc::new(router);

        let mut handles = Vec::new();
        for i in 0..3 {
            let router = router.clone();
            handles.push(tokio::spawn(async move {
                router.call("mcp__test__echo", json!({"text": i.to_string()})).await.unwrap()
            }));
        }

        let results = futures_util::future::join_all(handles).await;
        let mut outputs: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
        outputs.sort();
        assert_eq!(outputs, vec!["0".to_string(), "1".to_string(), "2".to_string()]);
    }

    struct EchoServer {
        tools: Vec<McpTool>,
    }

    impl ServerHandler for EchoServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::default()
        }

        fn get_tool(&self, name: &str) -> Option<McpTool> {
            self.tools.iter().find(|t| t.name == name).cloned()
        }

        fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
            std::future::ready(Ok(ListToolsResult::with_all_items(self.tools.clone())))
        }

        fn call_tool(
            &self,
            request: rmcp::model::CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
            let text = request
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            std::future::ready(Ok(CallToolResult::success(vec![Content::text(text)])))
        }
    }

    #[tokio::test]
    async fn mcp_client_talks_to_real_in_process_server() {
        let tool = echo_tool();
        let server = EchoServer { tools: vec![tool.clone()] };
        let (client_stream, server_stream) = tokio::io::duplex(64);

        let _server_handle = tokio::spawn(async move {
            let running = server.serve(server_stream).await.unwrap();
            // Keep the server alive until the client closes the transport.
            while !running.is_transport_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });

        let running = ().serve(client_stream).await.unwrap();
        let client = McpClient::with_service("fixture", vec![tool], Arc::new(RealMcpService::new(running)));

        let output = client.call_tool("echo", json!({"text": "hello"})).await.unwrap();
        assert_eq!(output, "hello");
    }
}
