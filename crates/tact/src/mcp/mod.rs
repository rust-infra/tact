//! Model Context Protocol (MCP) integration.
//!
//! MCP is a protocol that allows external tools (written in any language)
//! to expose capabilities to the agent via a JSON-RPC transport.
//!
//! ## Architecture
//!
//! - [`McpConfigFile`] reads Tact's native `mcp.json` (project
//!   `<workdir>/.tact/mcp.json`, user `~/.tact/mcp.json`) and is the only way
//!   a user is expected to declare a server.
//! - [`installed_plugin_mcp_servers`] reads servers contributed by installed
//!   marketplace plugins.
//! - [`McpClient`] connects to an MCP server over stdio transport using
//!   the [`rmcp`] crate, fetches its tool list, and proxies calls.
//! - [`MCPToolRouter`] aggregates all connected clients and routes
//!   incoming tool calls by name (format: `mcp__<server>__<tool>`).
//! - [`McpToolName::try_from`] parses this namespaced naming convention.
//! - [`load_mcp_router`] is the entry point: scans every source, connects
//!   servers, and returns a ready-to-use router.
//!
//! ## Source precedence
//!
//! Lowest to highest; a later entry overrides an earlier one **by server
//! name**, and every override is recorded in [`McpLoadReport::shadowed`]:
//!
//! 1. `~/.tact/mcp.json` (user)
//! 2. `<workdir>/.tact/mcp.json` (project)
//! 3. installed plugins (`plugin__<plugin>__<server>`)
//!
//! There is exactly one config file per scope. Tact intentionally reads **no**
//! cwd-level Codex manifest and **no** cwd `.mcp.json`: project servers live in
//! `.tact/mcp.json` and nowhere else, so "where does this project declare its
//! servers?" has a single answer. Installed plugins remain a source because a
//! plugin is a distributable bundle, not a config convention — their servers
//! keep manifest-prefixed names and are never how a user is told to configure
//! MCP directly.

use std::{collections::HashMap, fs, path::Path, process::Stdio, sync::Arc};

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
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::{
    ToolSpec,
    consts::{PluginHome, TactPath},
    plugin::{PluginRoot, PluginStore},
    tool::copy_tool_spec,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Tact's native MCP configuration file (`mcp.json`), using the same
/// Claude-compatible shape every MCP client accepts:
///
/// ```json
/// {
///   "mcpServers": {
///     "basic-memory": { "command": "/path/to/basic-memory", "args": ["mcp"] }
///   }
/// }
/// ```
///
/// A server declared here is named by its map key verbatim, so the resulting
/// agent tool names are exactly `mcp__<key>__<tool>` — no manifest prefix.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigFile {
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpProjectConfig>,
}

impl McpConfigFile {
    /// Reads the config at `path`. A missing file is `Ok(None)`; an unreadable
    /// or unparseable file is an error naming the path, because a
    /// user-authored config that cannot be understood must not be ignored.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read MCP config {}", path.display()))?;
        let parsed = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse MCP config {}", path.display()))?;
        Ok(Some(parsed))
    }
}

/// What happened while resolving every configured MCP server.
///
/// A clean load leaves every field empty; callers render a notice only when at
/// least one is non-empty, so the common case stays quiet.
#[derive(Debug, Clone, Default)]
pub struct McpLoadReport {
    /// Server name and tool count for each successful connection.
    pub connected: Vec<(String, usize)>,
    /// Server name and error for each failed connection.
    pub failures: Vec<(String, String)>,
    /// Server name and the lower-precedence source it displaced.
    pub shadowed: Vec<(String, String)>,
    /// Server names skipped because they declare a remote transport.
    pub skipped_remote: Vec<String>,
}

impl McpLoadReport {
    /// True when nothing noteworthy happened (no failures, overrides, or
    /// skipped servers). `connected` alone does not count as noteworthy.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.failures.is_empty() && self.shadowed.is_empty() && self.skipped_remote.is_empty()
    }

    /// One-line-per-fact summary lines for display. Empty when quiet.
    #[must_use]
    pub fn notice_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (server, error) in &self.failures {
            lines.push(format!("MCP server {server} failed to connect: {error}"));
        }
        for (server, source) in &self.shadowed {
            lines.push(format!("MCP server {server} overrides {source}"));
        }
        if !self.skipped_remote.is_empty() {
            lines.push(format!(
                "MCP servers skipped (remote transport unsupported): {}",
                self.skipped_remote.join(", ")
            ));
        }
        lines
    }
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

/// Claude project-level MCP configuration (`.mcp.json`) entry.
///
/// Tact only connects stdio servers today; `http` / `url` entries are skipped
/// with a warning (the client has no remote transport yet).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProjectConfig {
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl McpProjectConfig {
    /// Whether this entry declares a transport Tact cannot speak.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        matches!(self.server_type.as_deref(), Some("http") | Some("sse")) || self.url.is_some()
    }

    /// Converts a stdio-style entry to the internal server config; returns
    /// `None` for remote (`http`/`url`) or malformed entries.
    #[must_use]
    pub fn to_stdio(&self) -> Option<McpServerConfig> {
        if self.is_remote() {
            return None;
        }
        Some(McpServerConfig {
            command: self.command.clone()?,
            args: self.args.clone(),
            env: self.env.clone(),
        })
    }
}

/// Scans the installed-plugin cache for MCP servers declared by plugins:
/// `.codex-plugin/plugin.json` `mcpServers` and a project-style `.mcp.json`
/// at the plugin root. Returns `(server_name, config)` pairs where
/// `server_name = "plugin__<plugin_id>__<server>"`.
///
/// This is the only remaining multi-file bundle source. A plugin is a
/// distributable package, so its bundle-relative `.mcp.json` and manifest are
/// read here — but never at the working directory, where `.tact/mcp.json` is
/// the single answer.
pub fn installed_plugin_mcp_servers(home: &PluginHome) -> Result<Vec<(String, McpServerConfig)>> {
    let store = PluginStore::new(home.clone());
    let mut servers = Vec::new();
    for root in store.installed_plugin_roots()? {
        collect_plugin_mcp_servers(&root, &mut servers)?;
    }
    Ok(servers)
}

fn collect_plugin_mcp_servers(
    root: &PluginRoot,
    servers: &mut Vec<(String, McpServerConfig)>,
) -> Result<()> {
    let prefix = |name: &str| format!("plugin__{}__{}", root.plugin_id, name);

    let manifest_path = root.root.join(".codex-plugin").join("plugin.json");
    if manifest_path.is_file() {
        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&raw) {
            for (name, config) in manifest.mcp_servers {
                servers.push((prefix(&name), config));
            }
        }
    }

    // A plugin-root `.mcp.json` is part of the plugin *bundle* format, not a
    // project config convention. Tact deliberately does not read a `.mcp.json`
    // at the working directory — that role belongs to `<workdir>/.tact/mcp.json`.
    let mcp_path = root.root.join(".mcp.json");
    if mcp_path.is_file() {
        let raw = fs::read_to_string(&mcp_path)
            .with_context(|| format!("failed to read {}", mcp_path.display()))?;
        let configs: HashMap<String, McpProjectConfig> = serde_json::from_str(&raw)?;
        for (name, config) in configs {
            match config.to_stdio() {
                Some(server) => servers.push((prefix(&name), server)),
                None => tracing::debug!(
                    "plugin {} MCP server {} uses an unsupported transport (http/url); skipped",
                    root.plugin_id,
                    name
                ),
            }
        }
    }

    Ok(())
}

/// Low-level interface exposed by an MCP transport, used so tests can swap in
/// a mock implementation without spawning real child processes.
pub trait McpService: Send + Sync + 'static {
    /// List every tool advertised by the server.
    fn list_all_tools(&self) -> BoxFuture<'_, Result<Vec<McpTool>, ServiceError>>;

    /// Execute a single tool call.
    fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> BoxFuture<'_, Result<CallToolResult, ServiceError>>;

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

    fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> BoxFuture<'_, Result<CallToolResult, ServiceError>> {
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

/// Drain every line from a captured MCP server stderr until the pipe closes
/// (the server has exited). MCP servers commonly write progress and startup
/// logs to stderr; those bytes must be consumed to avoid backpressure, but
/// must not be forwarded to the interactive terminal.
async fn drain_mcp_stderr<R>(reader: R)
where
    R: AsyncBufRead + Unpin,
{
    let mut lines = reader.lines();
    while lines.next_line().await.ok().flatten().is_some() {}
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
                Ok(Self {
                    server_name,
                    service,
                    tools,
                    tool_specs,
                })
            }
            Err(err) => {
                let _ = service.cancel().await;
                Err(err)
            }
        }
    }

    /// Build a client from an arbitrary [`McpService`] implementation.
    ///
    /// This is the entry point for test doubles: construct a [`MockMcpService`],
    /// wrap it, and register it with [`MCPToolRouter`].
    pub fn with_service(
        server_name: impl Into<String>,
        tools: Vec<McpTool>,
        service: Arc<dyn McpService>,
    ) -> Self {
        let server_name = server_name.into();
        let tool_specs = build_tool_specs(&server_name, &tools);
        Self {
            server_name,
            service,
            tools,
            tool_specs,
        }
    }

    pub fn list_tools(&self) -> &[McpTool] {
        &self.tools
    }

    async fn connect(
        server_name: &str,
        config: McpServerConfig,
    ) -> Result<RunningService<RoleClient, ()>> {
        let command = config.command;
        let args = config.args;
        let env = config.env;
        // Capture and drain the server's stderr instead of inheriting it to
        // the terminal. Many stdio MCP servers log incidental progress (e.g.
        // index/recovery "Reconstruction complete") there; forwarding those
        // lines through tracing would still pollute the TUI when verbose logs
        // are enabled.
        let (transport, stderr) =
            TokioChildProcess::builder(Command::new(&command).configure(move |cmd| {
                cmd.args(&args).envs(&env);
            }))
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn MCP server {server_name}"))?;

        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                drain_mcp_stderr(BufReader::new(stderr)).await;
            });
        }

        ().serve(transport)
            .await
            .with_context(|| format!("failed to initialize MCP client for server {server_name}"))
    }

    async fn fetch_tools(server_name: &str, service: &dyn McpService) -> Result<Vec<McpTool>> {
        service
            .list_all_tools()
            .await
            .with_context(|| format!("failed to list tools from {server_name}"))
    }

    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<String> {
        let arguments = match arguments {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => {
                let mut map = Map::new();
                map.insert("value".to_string(), other);
                Some(map)
            }
        };

        let result = self
            .service
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool_name.to_string().into(),
                arguments,
                task: None,
            })
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
type McpToolHandler =
    Arc<dyn Fn(&CallToolRequestParams) -> Result<CallToolResult, ServiceError> + Send + Sync>;

pub struct MockMcpService {
    tools: Vec<McpTool>,
    handler: McpToolHandler,
    calls: std::sync::Mutex<Vec<(String, Value)>>,
}

impl MockMcpService {
    pub fn new<F>(tools: Vec<McpTool>, handler: F) -> Self
    where
        F: Fn(&CallToolRequestParams) -> Result<CallToolResult, ServiceError>
            + Send
            + Sync
            + 'static,
    {
        Self {
            tools,
            handler: Arc::new(handler),
            calls: std::sync::Mutex::new(Vec::new()),
        }
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

    fn call_tool(
        &self,
        params: CallToolRequestParams,
    ) -> BoxFuture<'_, Result<CallToolResult, ServiceError>> {
        let name = params.name.to_string();
        let args = params
            .arguments
            .as_ref()
            .map(|m| Value::Object(m.clone()))
            .unwrap_or(Value::Null);
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((name, args));
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

        Ok(Self {
            server: server.to_string(),
            tool: tool.to_string(),
        })
    }
}

/// Resolved MCP tool identity — parsed once, reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMcpTool {
    pub full_name: String,
    pub server: String,
    pub tool: String,
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

    /// Resolve a tool name without connecting to servers.
    /// Returns `Ok(Some(ResolvedMcpTool))` for valid MCP-prefixed names,
    /// `Ok(None)` for non-MCP names, or an error for misformatted MCP names.
    pub fn resolve_tool(&self, name: &str) -> Result<Option<ResolvedMcpTool>> {
        if !Self::is_mcp_tool(name) {
            return Ok(None);
        }
        let parsed = McpToolName::try_from(name)?;
        Ok(Some(ResolvedMcpTool {
            full_name: name.to_string(),
            server: parsed.server.clone(),
            tool: parsed.tool.clone(),
        }))
    }

    /// MCP server name for scheduling.
    pub fn server_name_for(&self, name: &str) -> Option<String> {
        if !Self::is_mcp_tool(name) {
            return None;
        }
        McpToolName::try_from(name).ok().map(|p| p.server)
    }

    pub async fn call(&self, tool_name: &str, arguments: Value) -> Result<String> {
        let parsed = McpToolName::try_from(tool_name)?;
        let client = self
            .clients
            .get(&parsed.server)
            .with_context(|| format!("unknown MCP server {}", parsed.server))?;

        client.call_tool(&parsed.tool, arguments).await
    }

    pub fn all_tools(&self) -> Vec<ToolSpec> {
        self.clients
            .values()
            .flat_map(|client| client.tool_specs.iter().map(copy_tool_spec))
            .collect()
    }

    pub fn server_summaries(&self) -> Vec<(String, usize)> {
        let mut summaries = self
            .clients
            .iter()
            .map(|(name, client)| (name.clone(), client.tool_count()))
            .collect::<Vec<_>>();
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

/// One server declaration together with the file it came from, used to make
/// override decisions observable.
#[derive(Debug)]
struct SourcedServer {
    name: String,
    source: String,
    config: McpProjectConfig,
}

/// Reads every MCP source in ascending precedence order.
///
/// Native `mcp.json` files are read first (user, then project) so a later
/// compatibility source can never silently outrank them.
fn collect_sourced_servers(cwd: &Path) -> Result<Vec<SourcedServer>> {
    let mut servers: Vec<SourcedServer> = Vec::new();

    let push_file = |path: &Path, servers: &mut Vec<SourcedServer>| -> Result<()> {
        let Some(file) = McpConfigFile::read(path)? else {
            return Ok(());
        };
        let source = path.display().to_string();
        for (name, config) in file.mcp_servers {
            servers.push(SourcedServer {
                name,
                source: source.clone(),
                config,
            });
        }
        Ok(())
    };

    // 1. Native user config, then 2. native project config.
    if let Some(path) = TactPath::home_mcp_config_path() {
        push_file(&path, &mut servers)?;
    }
    push_file(&TactPath::new(cwd).mcp_config_path(), &mut servers)?;

    // 3. Installed plugins. A plugin is the only remaining multi-file bundle
    // source; there is no cwd-level `.codex-plugin/plugin.json` read, because
    // a project declares its MCP servers in `.tact/mcp.json` and nowhere else.
    if let Some(home) = PluginHome::from_environment() {
        for (name, config) in installed_plugin_mcp_servers(&home)? {
            servers.push(SourcedServer {
                name,
                source: format!("installed plugin ({})", home.root.display()),
                config: McpProjectConfig {
                    server_type: None,
                    command: Some(config.command),
                    args: config.args,
                    env: config.env,
                    url: None,
                },
            });
        }
    }

    Ok(servers)
}

/// Outcome of layering every declared source into one connect list.
struct ResolvedServers {
    /// Servers to connect, in declaration order.
    servers: Vec<(String, McpServerConfig)>,
    /// Overridden server name and the source it displaced.
    shadowed: Vec<(String, String)>,
    /// Servers dropped for an unsupported or incomplete transport.
    skipped_remote: Vec<String>,
}

/// Resolves declared servers into the final connect list.
///
/// Later declarations win by server name; every displaced declaration is
/// recorded so the override is visible rather than silent. Remote-transport
/// entries are dropped here with a report entry, never a hard error.
fn resolve_servers(servers: Vec<SourcedServer>) -> ResolvedServers {
    let mut order: Vec<(String, McpServerConfig, String)> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut shadowed: Vec<(String, String)> = Vec::new();
    let mut skipped_remote: Vec<String> = Vec::new();

    for SourcedServer {
        name,
        source,
        config,
    } in servers
    {
        if config.is_remote() {
            skipped_remote.push(name);
            continue;
        }
        let Some(stdio) = config.to_stdio() else {
            // A stdio entry without a `command`: report, do not abort.
            skipped_remote.push(name);
            continue;
        };
        match index_of.get(&name) {
            Some(&existing) => {
                shadowed.push((name.clone(), order[existing].2.clone()));
                order[existing] = (name, stdio, source);
            }
            None => {
                index_of.insert(name.clone(), order.len());
                order.push((name, stdio, source));
            }
        }
    }

    skipped_remote.sort();
    skipped_remote.dedup();
    ResolvedServers {
        servers: order
            .into_iter()
            .map(|(name, config, _source)| (name, config))
            .collect(),
        shadowed,
        skipped_remote,
    }
}

/// Loads every configured MCP server and reports what happened.
///
/// Connection failures are collected, never propagated: one broken server must
/// not prevent the agent from starting.
pub async fn load_mcp_router_with_report() -> Result<(MCPToolRouter, McpLoadReport)> {
    let cwd = std::env::current_dir()?;
    let resolved = resolve_servers(collect_sourced_servers(&cwd)?);

    let mut report = McpLoadReport {
        shadowed: resolved.shadowed,
        skipped_remote: resolved.skipped_remote,
        ..McpLoadReport::default()
    };

    let mut router = MCPToolRouter::new();
    let mut connections = FuturesUnordered::new();
    for (server_name, config) in resolved.servers {
        connections.push(async move {
            let result = McpClient::try_new(server_name.clone(), config).await;
            (server_name, result)
        });
    }
    while let Some((server_name, result)) = connections.next().await {
        match result {
            Ok(client) => {
                let tools = client.list_tools().len();
                tracing::debug!(mcp_server = %server_name, tools, "MCP server connected");
                report.connected.push((server_name.clone(), tools));
                router.register_client(client);
            }
            Err(err) => {
                tracing::warn!(mcp_server = %server_name, error = %err, "MCP server connection failed");
                report.failures.push((server_name, format!("{err:#}")));
            }
        }
    }

    report.connected.sort_by(|a, b| a.0.cmp(&b.0));
    report.failures.sort_by(|a, b| a.0.cmp(&b.0));
    report.shadowed.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((router, report))
}

/// Loads every configured MCP server, discarding the load report.
///
/// Prefer [`load_mcp_router_with_report`] in interactive entry points so
/// failures and overrides can be surfaced to the user.
pub async fn load_mcp_router() -> Result<MCPToolRouter> {
    let (router, _report) = load_mcp_router_with_report().await?;
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
    use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

    use rmcp::{
        ErrorData as McpError, ServerHandler, ServiceExt,
        model::{
            CallToolResult, Content, JsonObject, ListToolsResult, ServerInfo, Tool as McpTool,
        },
        service::{RequestContext, RoleServer},
    };
    use serde_json::json;

    use super::{
        MCPToolRouter, McpClient, McpConfigFile, McpLoadReport, McpProjectConfig, McpServerConfig,
        McpToolName, MockMcpService, PluginManifest, RealMcpService, SourcedServer,
        collect_sourced_servers, drain_mcp_stderr, installed_plugin_mcp_servers, resolve_servers,
    };
    use crate::{
        consts::PluginHome,
        plugin::{InstalledPlugin, InstalledState, PluginStore},
    };

    #[tokio::test]
    async fn drain_mcp_stderr_consumes_every_line() {
        use tokio::io::BufReader;

        drain_mcp_stderr(BufReader::new(
            &b"Reconstruction complete 1\nindexing note A\n"[..],
        ))
        .await;
    }

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
        let output = router
            .call("mcp__test__echo", json!({"text": "hello"}))
            .await
            .unwrap();
        assert_eq!(output, "hello");
    }

    #[test]
    fn parses_project_mcp_config_stdio_and_http() {
        let raw = r#"{
            "local": { "type": "stdio", "command": "node", "args": ["srv.js"] },
            "remote": { "type": "http", "url": "https://mcp.example.com/api" }
        }"#;
        let configs: std::collections::HashMap<String, McpProjectConfig> =
            serde_json::from_str(raw).unwrap();

        let local = configs["local"].to_stdio().expect("stdio server");
        assert_eq!(local.command, "node");
        assert_eq!(local.args, vec!["srv.js"]);
        assert!(
            configs["remote"].to_stdio().is_none(),
            "http must be skipped"
        );
    }

    #[test]
    fn installed_plugin_mcp_servers_scans_cache_roots() {
        let home = tempfile::tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let plugin_root = plugin_home.cache.join("acme/demo/abc123");
        std::fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        std::fs::write(
            plugin_root.join(".codex-plugin/plugin.json"),
            r#"{ "name": "demo", "mcpServers": { "fromManifest": { "command": "cat" } } }"#,
        )
        .unwrap();
        std::fs::write(
            plugin_root.join(".mcp.json"),
            r#"{
                "fromDotMcp": { "type": "stdio", "command": "node", "args": ["srv.js"] },
                "remote": { "type": "http", "url": "https://mcp.example.com/api" }
            }"#,
        )
        .unwrap();
        let store = PluginStore::new(plugin_home.clone());
        store
            .commit_install(
                &InstalledState {
                    plugins: BTreeMap::from([(
                        "acme/demo".to_owned(),
                        InstalledPlugin {
                            id: "demo".to_owned(),
                            marketplace: "acme".to_owned(),
                            revision: "abc123".to_owned(),
                            cache_path: plugin_root.clone(),
                            skill_count: 0,
                            command_count: 0,
                            has_hooks: false,
                            has_mcp: true,
                        },
                    )]),
                },
                &plugin_root,
            )
            .unwrap();

        let servers = installed_plugin_mcp_servers(&plugin_home).unwrap();

        let names: Vec<_> = servers.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"plugin__demo__fromManifest"));
        assert!(names.contains(&"plugin__demo__fromDotMcp"));
        assert!(
            !names.contains(&"plugin__demo__remote"),
            "http server must be skipped"
        );
        let (_, from_manifest) = servers
            .iter()
            .find(|(name, _)| name == "plugin__demo__fromManifest")
            .unwrap();
        assert_eq!(from_manifest.command, "cat");
        let (_, from_dot_mcp) = servers
            .iter()
            .find(|(name, _)| name == "plugin__demo__fromDotMcp")
            .unwrap();
        assert_eq!(from_dot_mcp.command, "node");
        assert_eq!(from_dot_mcp.args, vec!["srv.js"]);
    }

    #[test]
    fn installed_plugin_mcp_servers_returns_empty_without_plugins() {
        let home = tempfile::tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());

        assert!(
            installed_plugin_mcp_servers(&plugin_home)
                .unwrap()
                .is_empty()
        );
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
        router.register_client(McpClient::with_service(
            "demo",
            vec![echo],
            Arc::new(echo_service),
        ));
        router.register_client(McpClient::with_service(
            "other",
            vec![upper],
            Arc::new(upper_service),
        ));

        let echo_out = router
            .call("mcp__demo__echo", json!({"text": "hello"}))
            .await
            .unwrap();
        let upper_out = router
            .call("mcp__other__upper", json!({"text": "hello"}))
            .await
            .unwrap();

        assert_eq!(echo_out, "hello");
        assert_eq!(upper_out, "HELLO");
        assert_eq!(
            router.server_summaries(),
            vec![("demo".to_string(), 1), ("other".to_string(), 1)]
        );
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

        let _ = router
            .call("mcp__test__echo", json!({"text": "first"}))
            .await
            .unwrap();
        let _ = router
            .call("mcp__test__echo", json!({"text": "second"}))
            .await
            .unwrap();

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
        router.register_client(McpClient::with_service(
            "test",
            vec![echo],
            Arc::new(service),
        ));
        let router = Arc::new(router);

        let mut handles = Vec::new();
        for i in 0..3 {
            let router = router.clone();
            handles.push(tokio::spawn(async move {
                router
                    .call("mcp__test__echo", json!({"text": i.to_string()}))
                    .await
                    .unwrap()
            }));
        }

        let results = futures_util::future::join_all(handles).await;
        let mut outputs: Vec<String> = results.into_iter().map(|r| r.unwrap()).collect();
        outputs.sort();
        assert_eq!(
            outputs,
            vec!["0".to_string(), "1".to_string(), "2".to_string()]
        );
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
        ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_
        {
            std::future::ready(Ok(ListToolsResult::with_all_items(self.tools.clone())))
        }

        fn call_tool(
            &self,
            request: rmcp::model::CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_
        {
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
        let server = EchoServer {
            tools: vec![tool.clone()],
        };
        let (client_stream, server_stream) = tokio::io::duplex(64);

        let _server_handle = tokio::spawn(async move {
            let running = server.serve(server_stream).await.unwrap();
            // Keep the server alive until the client closes the transport.
            while !running.is_transport_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });

        let running = ().serve(client_stream).await.unwrap();
        let client = McpClient::with_service(
            "fixture",
            vec![tool],
            Arc::new(RealMcpService::new(running)),
        );

        let output = client
            .call_tool("echo", json!({"text": "hello"}))
            .await
            .unwrap();
        assert_eq!(output, "hello");
    }

    // ── Native `mcp.json` sources ───────────────────────────────────────

    fn sourced(name: &str, source: &str, command: &str) -> SourcedServer {
        SourcedServer {
            name: name.to_owned(),
            source: source.to_owned(),
            config: McpProjectConfig {
                server_type: None,
                command: Some(command.to_owned()),
                args: Vec::new(),
                env: Default::default(),
                url: None,
            },
        }
    }

    #[test]
    fn mcp_config_file_reads_servers_and_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        assert!(McpConfigFile::read(&path).unwrap().is_none());

        std::fs::write(
            &path,
            r#"{"mcpServers":{"basic-memory":{"command":"/bin/echo","args":["mcp"]}}}"#,
        )
        .unwrap();
        let file = McpConfigFile::read(&path).unwrap().unwrap();
        let stdio = file
            .mcp_servers
            .get("basic-memory")
            .unwrap()
            .to_stdio()
            .unwrap();
        assert_eq!(stdio.command, "/bin/echo");
        assert_eq!(stdio.args, vec!["mcp".to_owned()]);
    }

    #[test]
    fn mcp_config_file_parse_error_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, "{ not json").unwrap();

        let message = format!("{:#}", McpConfigFile::read(&path).unwrap_err());
        assert!(message.contains("mcp.json"), "{message}");
    }

    #[test]
    fn later_source_overrides_earlier_by_server_name() {
        let resolved = resolve_servers(vec![
            sourced("shared", "~/.tact/mcp.json", "/bin/user"),
            sourced("shared", "./.tact/mcp.json", "/bin/project"),
        ]);

        assert_eq!(resolved.servers.len(), 1);
        assert_eq!(resolved.servers[0].1.command, "/bin/project");
        assert_eq!(
            resolved.shadowed,
            vec![("shared".to_owned(), "~/.tact/mcp.json".to_owned())]
        );
        assert!(resolved.skipped_remote.is_empty());
    }

    #[test]
    fn non_conflicting_sources_merge() {
        let resolved = resolve_servers(vec![
            sourced("user-only", "~/.tact/mcp.json", "/bin/user"),
            sourced("project-only", "./.tact/mcp.json", "/bin/project"),
        ]);

        let names: Vec<&str> = resolved.servers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["user-only", "project-only"]);
        assert!(resolved.shadowed.is_empty());
    }

    #[test]
    fn remote_and_commandless_entries_are_skipped_not_fatal() {
        let remote = SourcedServer {
            name: "hosted".to_owned(),
            source: "~/.tact/mcp.json".to_owned(),
            config: McpProjectConfig {
                server_type: Some("http".to_owned()),
                command: None,
                args: Vec::new(),
                env: Default::default(),
                url: Some("https://example.invalid/mcp".to_owned()),
            },
        };
        let commandless = SourcedServer {
            name: "typo".to_owned(),
            source: "~/.tact/mcp.json".to_owned(),
            config: McpProjectConfig {
                server_type: None,
                command: None,
                args: Vec::new(),
                env: Default::default(),
                url: None,
            },
        };

        let resolved = resolve_servers(vec![remote, commandless]);

        assert!(resolved.servers.is_empty());
        assert_eq!(
            resolved.skipped_remote,
            vec!["hosted".to_owned(), "typo".to_owned()]
        );
    }

    #[test]
    fn native_config_key_is_the_server_name_without_a_prefix() {
        // The whole point of `mcp.json`: a plain key, so the agent-side tool
        // name is exactly `mcp__<key>__<tool>`.
        let resolved = resolve_servers(vec![sourced(
            "basic-memory",
            "~/.tact/mcp.json",
            "/bin/echo",
        )]);
        let client = McpClient::with_service(
            resolved.servers[0].0.clone(),
            Vec::new(),
            std::sync::Arc::new(MockMcpService::new(Vec::new(), |_| {
                Ok(rmcp::model::CallToolResult::success(Vec::new()))
            })),
        );

        assert_eq!(client.server_name, "basic-memory");
        assert_eq!(
            format!("mcp__{}__read_note", client.server_name),
            "mcp__basic-memory__read_note"
        );
    }

    #[test]
    fn load_report_is_quiet_only_without_failures_overrides_or_skips() {
        let mut report = McpLoadReport::default();
        assert!(report.is_quiet());
        report.connected.push(("ok".to_owned(), 3));
        assert!(report.is_quiet(), "successful connections alone stay quiet");

        report
            .failures
            .push(("broken".to_owned(), "spawn failed".to_owned()));
        assert!(!report.is_quiet());
        let lines = report.notice_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("broken"), "{lines:?}");
    }

    #[test]
    fn load_report_renders_overrides_and_skipped_servers() {
        let report = McpLoadReport {
            connected: Vec::new(),
            failures: Vec::new(),
            shadowed: vec![("shared".to_owned(), "~/.tact/mcp.json".to_owned())],
            skipped_remote: vec!["hosted".to_owned()],
        };

        let lines = report.notice_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("overrides"), "{lines:?}");
        assert!(lines[1].contains("hosted"), "{lines:?}");
    }

    #[test]
    fn project_mcp_json_is_read_and_a_cwd_dot_mcp_json_is_not() {
        // The temp dir stands in for the working directory; assertions filter
        // to temp-dir-derived servers so a real `~/.tact/mcp.json` on the
        // machine running the tests cannot make this flaky.
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();

        std::fs::create_dir_all(cwd.join(".tact")).unwrap();
        std::fs::write(
            cwd.join(".tact/mcp.json"),
            r#"{"mcpServers":{"native":{"command":"/bin/native"}}}"#,
        )
        .unwrap();
        // A Claude-style file at cwd is deliberately *not* a Tact source: the
        // project-level answer is `<workdir>/.tact/mcp.json`, and two
        // near-identically named files would be ambiguous.
        std::fs::write(
            cwd.join(".mcp.json"),
            r#"{"mcpServers":{"ignored":{"command":"/bin/claude"}}}"#,
        )
        .unwrap();

        let servers = collect_sourced_servers(cwd).unwrap();
        let native = servers
            .iter()
            .find(|s| s.name == "native")
            .expect("project .tact/mcp.json is read");
        assert!(
            native.source.ends_with(".tact/mcp.json"),
            "{}",
            native.source
        );
        assert!(
            !servers.iter().any(|s| s.name == "ignored"),
            "a cwd .mcp.json must not be read: {servers:?}",
        );
    }
}
