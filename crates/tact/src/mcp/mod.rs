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

mod edit;
mod remote;
pub use edit::*;
pub use remote::*;

/// How the client reaches one MCP server.
///
/// Tact supports local subprocess servers (`command`) and remote Streamable
/// HTTP servers (`url`, optionally with OAuth), resolved per entry.
#[derive(Debug, Clone)]
pub enum McpTransportConfig {
    /// Local subprocess over stdio.
    Stdio(McpServerConfig),
    /// Remote Streamable HTTP (optionally OAuth-authorized).
    Remote(McpRemoteConfig),
}

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

/// Whether `name` is safe to use as a single path component.
///
/// A server name is not only a map key: it becomes a segment of every agent
/// tool name (`mcp__<server>__<tool>`) and the file name of the OAuth
/// credential (`<name>.json`). A name containing a path separator or `..`
/// would let a config file — or a CLI argument such as `mcp logout <name>` —
/// reach outside the directory it belongs to.
#[must_use]
pub fn is_safe_server_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// Validates a server name supplied by a user (config key or CLI argument).
///
/// Whitespace is refused because the name is also an argument to
/// `/mcp auth <name>`, where it would split into two arguments. Underscores are
/// allowed: plugin-contributed servers are already named `plugin__id__name`.
pub fn validate_server_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("server name must not be empty");
    }
    if name.trim() != name {
        bail!("server name '{name}' must not have leading or trailing whitespace");
    }
    if name.chars().any(char::is_whitespace) {
        bail!("server name '{name}' must not contain whitespace");
    }
    if name.chars().any(char::is_control) {
        bail!("server name '{name}' must not contain control characters");
    }
    if !is_safe_server_name(name) {
        bail!("server name '{name}' must not contain a path separator");
    }
    Ok(())
}

/// How a configured server will be reached, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransportKind {
    /// Local subprocess: the command that will be spawned.
    Stdio { command: String },
    /// Remote Streamable HTTP endpoint.
    Remote { url: String, oauth: bool },
}

impl std::fmt::Display for McpTransportKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdio { command } => write!(formatter, "stdio  {command}"),
            Self::Remote { url, oauth } => {
                write!(formatter, "remote {url}")?;
                if *oauth {
                    write!(formatter, " (oauth)")?;
                }
                Ok(())
            }
        }
    }
}

/// A server that survived resolution, with the source it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredServer {
    pub name: String,
    pub transport: McpTransportKind,
    /// Where the declaration was read from (a file path or "installed plugin").
    pub source: String,
}

/// Describes how a resolved transport is reached, for diagnostics.
fn transport_kind(transport: &McpTransportConfig) -> McpTransportKind {
    match transport {
        McpTransportConfig::Stdio(config) => McpTransportKind::Stdio {
            command: config.command.clone(),
        },
        McpTransportConfig::Remote(remote) => McpTransportKind::Remote {
            url: remote.url.clone(),
            oauth: remote.auth.is_some(),
        },
    }
}

/// One configured server's status against a **live** connection set.
///
/// Unlike [`McpServerStatus`] (which comes from connecting), this is derived
/// from the clients an agent already holds, so classifying a server never
/// dials it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpLiveStatus {
    /// A live client exists; carries the tool count it reported.
    Connected { tools: usize },
    /// Remote OAuth server with no stored credential — actionable with
    /// `/mcp auth <name>`, not an error.
    NeedsAuthorization,
    /// Configured but absent from the live set: it was not connected at
    /// startup (a connection failure, or a contact that never happened).
    NotConnected,
}

/// One configured server together with its live status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerView {
    pub server: ConfiguredServer,
    pub status: McpLiveStatus,
}

/// Describes every configured server against the given live connections.
///
/// `connected` is the `(server, tool count)` list an agent's router reports.
/// **Read-only**: nothing is dialled, so this is cheap and safe to call while
/// a task is in flight — unlike [`load_mcp_router_with_report`], which would
/// duplicate (or, for stdio servers, contend with) the live connections.
///
/// Returns an error only when the configuration on disk cannot be resolved
/// (an unparseable `mcp.json`); a missing file is simply an empty listing.
pub fn describe_servers(connected: &[(String, usize)]) -> Result<Vec<McpServerView>> {
    Ok(describe_resolved(resolve_current()?, connected))
}

/// Pure core of [`describe_servers`], separated so classification is testable
/// without reading the working directory.
fn describe_resolved(
    resolved: ResolvedServers,
    connected: &[(String, usize)],
) -> Vec<McpServerView> {
    let mut views: Vec<McpServerView> = resolved
        .servers
        .iter()
        .map(|(name, transport, source)| {
            let status = if let Some((_, tools)) = connected.iter().find(|(n, _)| n == name) {
                McpLiveStatus::Connected { tools: *tools }
            } else if matches!(
                transport,
                McpTransportConfig::Remote(remote) if remote.needs_authorization(name)
            ) {
                McpLiveStatus::NeedsAuthorization
            } else {
                McpLiveStatus::NotConnected
            };
            McpServerView {
                server: ConfiguredServer {
                    name: name.clone(),
                    transport: transport_kind(transport),
                    source: source.clone(),
                },
                status,
            }
        })
        .collect();
    views.sort_by(|a, b| a.server.name.cmp(&b.server.name));
    views
}

/// What happened while resolving every configured MCP server.
///
/// A clean load leaves every field empty; callers render a notice only when at
/// least one is non-empty, so the common case stays quiet.
#[derive(Debug, Clone, Default)]
pub struct McpLoadReport {
    /// Every server that survived resolution, sorted by name.
    ///
    /// Unlike the fields below this is *not* a problem list — it describes the
    /// full configuration, so `tact-ui mcp list` can show servers that loaded
    /// cleanly alongside the ones that did not.
    pub configured: Vec<ConfiguredServer>,
    /// Server name and tool count for each successful connection.
    pub connected: Vec<(String, usize)>,
    /// Server name and error for each failed connection.
    pub failures: Vec<(String, String)>,
    /// Server name and the lower-precedence source it displaced.
    pub shadowed: Vec<(String, String)>,
    /// Server names skipped because they declare an unsupported/incomplete
    /// transport (currently a remote entry without a `url`, or a stdio entry
    /// without a `command`).
    pub skipped_remote: Vec<String>,
    /// Remote servers that declare OAuth but have no stored credential yet.
    ///
    /// These are not failures: startup proceeds and the user authorizes them
    /// later with `/mcp auth <name>`.
    pub pending_auth: Vec<String>,
}

impl McpLoadReport {
    /// True when nothing noteworthy happened (no failures, overrides, skipped
    /// servers, or pending authorizations). `connected` alone does not count
    /// as noteworthy.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.failures.is_empty()
            && self.shadowed.is_empty()
            && self.skipped_remote.is_empty()
            && self.pending_auth.is_empty()
    }

    /// One-line-per-fact summary lines for display. Empty when quiet.
    #[must_use]
    pub fn notice_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (server, error) in &self.failures {
            lines.push(format!("MCP server {server} failed to connect: {error}"));
        }
        for server in &self.pending_auth {
            lines.push(format!(
                "MCP server {server} needs authorization — run /mcp auth {server}"
            ));
        }
        for (server, source) in &self.shadowed {
            lines.push(format!("MCP server {server} overrides {source}"));
        }
        if !self.skipped_remote.is_empty() {
            lines.push(format!(
                "MCP servers skipped (unsupported transport): {}",
                self.skipped_remote.join(", ")
            ));
        }
        lines
    }
}

/// Connection state of one server, as observed by [`inspect_server`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerStatus {
    /// The server answered and its tools were fetched.
    Connected,
    /// Remote OAuth server with no usable credential — actionable, not an
    /// error.
    PendingAuthorization,
    /// The connection failed; carries the rendered error.
    Failed(String),
}

/// One server, described and connected in isolation.
///
/// Unlike [`McpLoadReport`], which describes the whole configuration, this is
/// the `mcp get <name>` view of a single server — including its tool list,
/// which the aggregate report does not carry.
#[derive(Debug, Clone)]
pub struct McpServerInspection {
    pub server: ConfiguredServer,
    pub status: McpServerStatus,
    /// Short tool names (as the server reports them, without the
    /// `mcp__<server>__` prefix). Empty unless [`McpServerStatus::Connected`].
    pub tools: Vec<String>,
}

/// What one connection attempt produced.
enum ConnectOutcome {
    Connected(McpClient),
    NeedsAuthorization,
    Failed(String),
}

/// Connects one server, classifying the outcome.
///
/// A remote server that is *known* to need OAuth is not contacted at all (the
/// credential file is enough to decide), and a 401 without a declared `auth` is
/// upgraded to the actionable pending state rather than reported as a failure.
/// Shared by the full-config load and the single-server `get` view so both
/// classify identically.
async fn connect_server(name: &str, config: McpTransportConfig) -> ConnectOutcome {
    if let McpTransportConfig::Remote(remote) = &config
        && remote.needs_authorization(name)
    {
        tracing::info!(
            mcp_server = %name,
            "remote MCP server needs OAuth authorization; run `mcp login`"
        );
        return ConnectOutcome::NeedsAuthorization;
    }
    match McpClient::try_new(name.to_string(), config).await {
        Ok(client) => ConnectOutcome::Connected(client),
        Err(err)
            if err
                .downcast_ref::<remote::AuthorizationRequired>()
                .is_some()
                || remote::is_auth_required_error(&err) =>
        {
            tracing::info!(
                mcp_server = %name,
                "remote MCP server needs (re)authorization; run `mcp login`"
            );
            ConnectOutcome::NeedsAuthorization
        }
        Err(err) => {
            tracing::warn!(mcp_server = %name, error = %err, "MCP server connection failed");
            ConnectOutcome::Failed(format!("{err:#}"))
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpProjectConfig>,
}

/// MCP server entry in `mcp.json` / a plugin `.mcp.json`.
///
/// Exactly one transport is expected:
///
/// - `command` — local subprocess over stdio;
/// - `url` (optionally with `type: "http" | "sse"`) — remote Streamable HTTP,
///   with optional static `headers` and OAuth (`auth`).
///
/// `command` wins when both are present, so a stdio entry is never
/// reinterpreted as remote.
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
    /// Extra request headers for remote servers (static auth goes here).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// OAuth declaration for remote servers.
    #[serde(default)]
    pub auth: Option<McpAuthConfig>,
}

impl McpProjectConfig {
    /// Whether this entry declares a remote (Streamable HTTP) transport.
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

    /// Converts an entry to its transport config.
    ///
    /// `command` wins over `url` when both are present; an entry with neither
    /// (or a remote entry without a `url`) is unsupported and returns `None`,
    /// which the resolver reports as skipped rather than silently dropping.
    #[must_use]
    pub fn to_transport(&self) -> Option<McpTransportConfig> {
        if let Some(command) = &self.command {
            return Some(McpTransportConfig::Stdio(McpServerConfig {
                command: command.clone(),
                args: self.args.clone(),
                env: self.env.clone(),
            }));
        }
        let url = self.url.clone()?;
        Some(McpTransportConfig::Remote(McpRemoteConfig {
            url,
            headers: self.headers.clone(),
            auth: self.auth.clone(),
        }))
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
pub fn installed_plugin_mcp_servers(home: &PluginHome) -> Result<Vec<(String, McpProjectConfig)>> {
    let store = PluginStore::new(home.clone());
    let mut servers = Vec::new();
    for root in store.installed_plugin_roots()? {
        collect_plugin_mcp_servers(&root, &mut servers)?;
    }
    Ok(servers)
}

fn collect_plugin_mcp_servers(
    root: &PluginRoot,
    servers: &mut Vec<(String, McpProjectConfig)>,
) -> Result<()> {
    let prefix = |name: &str| format!("plugin__{}__{}", root.plugin_id, name);

    let manifest_path = root.root.join(".codex-plugin").join("plugin.json");
    if manifest_path.is_file() {
        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        match serde_json::from_str::<PluginManifest>(&raw) {
            Ok(manifest) => {
                for (name, config) in manifest.mcp_servers {
                    servers.push((prefix(&name), config));
                }
            }
            Err(error) => {
                tracing::warn!(
                    "plugin {} manifest at {} is unparseable as MCP manifest: {error}",
                    root.plugin_id,
                    manifest_path.display()
                );
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
            if config.to_transport().is_none() {
                // Unsupported entries are reported by the resolver (it knows
                // the final server name), not dropped here.
                tracing::debug!(
                    "plugin {} MCP server {} has no usable transport",
                    root.plugin_id,
                    name
                );
            }
            servers.push((prefix(&name), config));
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
    pub async fn try_new(
        server_name: impl Into<String>,
        config: McpTransportConfig,
    ) -> Result<Self> {
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
        config: McpTransportConfig,
    ) -> Result<RunningService<RoleClient, ()>> {
        let config = match config {
            McpTransportConfig::Stdio(config) => config,
            McpTransportConfig::Remote(remote) => {
                return remote::serve_remote(server_name, &remote).await;
            }
        };
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
                config,
            });
        }
    }

    Ok(servers)
}

/// Outcome of layering every declared source into one connect list.
struct ResolvedServers {
    /// Servers to connect, in declaration order, with the source they came from.
    servers: Vec<(String, McpTransportConfig, String)>,
    /// Overridden server name and the source it displaced.
    shadowed: Vec<(String, String)>,
    /// Servers dropped for an unsupported or incomplete transport.
    skipped_remote: Vec<String>,
}

impl ResolvedServers {
    /// Describes each server for diagnostics (`tact-ui mcp list`).
    ///
    /// Sorted by name: `mcp.json` is parsed into a `HashMap`, so there is no
    /// meaningful declaration order to preserve and an unstable listing would
    /// make repeated runs needlessly hard to compare.
    fn configured(&self) -> Vec<ConfiguredServer> {
        let mut described: Vec<ConfiguredServer> = self
            .servers
            .iter()
            .map(|(name, transport, source)| ConfiguredServer {
                name: name.clone(),
                transport: transport_kind(transport),
                source: source.clone(),
            })
            .collect();
        described.sort_by(|a, b| a.name.cmp(&b.name));
        described
    }
}

/// Resolves declared servers into the final connect list.
///
/// Later declarations win by server name; every displaced declaration is
/// recorded so the override is visible rather than silent. An entry with no
/// usable transport (neither `command` nor `url`) is dropped with a report
/// entry, never a hard error.
fn resolve_servers(servers: Vec<SourcedServer>) -> ResolvedServers {
    let mut order: Vec<(String, McpTransportConfig, String)> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut shadowed: Vec<(String, String)> = Vec::new();
    let mut skipped_remote: Vec<String> = Vec::new();

    for SourcedServer {
        name,
        source,
        config,
    } in servers
    {
        let Some(transport) = config.to_transport() else {
            // No `command` and no `url`: report, do not abort.
            tracing::warn!(
                mcp_server = %name,
                source = %source,
                "MCP server has neither a command nor a url; skipped"
            );
            skipped_remote.push(name);
            continue;
        };
        match transport {
            McpTransportConfig::Stdio(_) => tracing::debug!(
                mcp_server = %name, source = %source, transport = "stdio",
                "resolved MCP server"
            ),
            McpTransportConfig::Remote(ref remote) => tracing::debug!(
                mcp_server = %name, source = %source, transport = "streamable-http",
                url = %remote.url,
                oauth = remote.auth.is_some(),
                "resolved MCP server"
            ),
        }
        match index_of.get(&name) {
            Some(&existing) => {
                shadowed.push((name.clone(), order[existing].2.clone()));
                order[existing] = (name, transport, source);
            }
            None => {
                index_of.insert(name.clone(), order.len());
                order.push((name, transport, source));
            }
        }
    }

    // A name skipped in one scope can still be declared usefully by another
    // (project wins over user). It is only "skipped" when nothing usable was
    // declared for it anywhere, otherwise the report would list — and count —
    // the same server twice.
    skipped_remote.retain(|name| !index_of.contains_key(name));
    skipped_remote.sort();
    skipped_remote.dedup();
    ResolvedServers {
        servers: order,
        shadowed,
        skipped_remote,
    }
}

/// Loads every configured MCP server and reports what happened.
///
/// Connection failures are collected, never propagated: one broken server must
/// not prevent the agent from starting. A remote server that declares OAuth
/// but has no stored credential is reported as *pending authorization* and not
/// connected, so startup never blocks on a browser round-trip.
///
/// Returns a boxed future: the transport futures built here are deeply nested
/// generics, and boxing keeps the `Send` bound resolvable at this boundary
/// instead of overflowing the trait solver at every caller's `tokio::spawn`.
pub fn load_mcp_router_with_report() -> BoxFuture<'static, Result<(MCPToolRouter, McpLoadReport)>> {
    load_mcp_router_with_report_inner().boxed()
}

async fn load_mcp_router_with_report_inner() -> Result<(MCPToolRouter, McpLoadReport)> {
    let cwd = std::env::current_dir()?;
    let resolved = resolve_servers(collect_sourced_servers(&cwd)?);

    let mut report = McpLoadReport {
        configured: resolved.configured(),
        shadowed: resolved.shadowed,
        skipped_remote: resolved.skipped_remote,
        ..McpLoadReport::default()
    };

    let mut router = MCPToolRouter::new();
    let mut connections = FuturesUnordered::new();
    for (server_name, config, _source) in resolved.servers {
        connections.push(async move {
            let outcome = connect_server(&server_name, config).await;
            (server_name, outcome)
        });
    }
    while let Some((server_name, outcome)) = connections.next().await {
        match outcome {
            ConnectOutcome::Connected(client) => {
                let tools = client.list_tools().len();
                tracing::debug!(mcp_server = %server_name, tools, "MCP server connected");
                report.connected.push((server_name.clone(), tools));
                router.register_client(client);
            }
            ConnectOutcome::NeedsAuthorization => report.pending_auth.push(server_name),
            ConnectOutcome::Failed(error) => report.failures.push((server_name, error)),
        }
    }

    report.connected.sort_by(|a, b| a.0.cmp(&b.0));
    report.failures.sort_by(|a, b| a.0.cmp(&b.0));
    report.shadowed.sort_by(|a, b| a.0.cmp(&b.0));
    report.pending_auth.sort();
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

/// Resolves every declared source against the current working directory.
fn resolve_current() -> Result<ResolvedServers> {
    let cwd = std::env::current_dir()?;
    Ok(resolve_servers(collect_sourced_servers(&cwd)?))
}

/// Looks up one resolved server exactly as the loader would see it.
///
/// Returns `Ok(None)` for a name that is not declared anywhere, which lets a
/// CLI distinguish a typo from a connection failure — the same distinction
/// `mcp list` makes. The returned [`ConfiguredServer::source`] says which file
/// (or plugin) the declaration came from, so `mcp remove` can explain where a
/// server actually lives before failing.
pub fn resolved_server_for(
    server_name: &str,
) -> Result<Option<(ConfiguredServer, McpTransportConfig)>> {
    let resolved = resolve_current()?;
    let described = resolved
        .configured()
        .into_iter()
        .find(|server| server.name == server_name);
    let transport = resolved
        .servers
        .into_iter()
        .find(|(name, _, _)| name == server_name)
        .map(|(_, transport, _)| transport);
    Ok(match (described, transport) {
        (Some(server), Some(transport)) => Some((server, transport)),
        _ => None,
    })
}

/// Connects one server and reports its state, without touching the others.
///
/// `mcp get <name>` uses this so inspecting a single server never spawns or
/// dials the rest of the configuration. Returns `Ok(None)` for an unknown name.
pub async fn inspect_server(server_name: &str) -> Result<Option<McpServerInspection>> {
    let Some((server, transport)) = resolved_server_for(server_name)? else {
        return Ok(None);
    };
    let (status, tools) = match connect_server(server_name, transport).await {
        ConnectOutcome::Connected(client) => {
            let tools = client
                .list_tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            (McpServerStatus::Connected, tools)
        }
        ConnectOutcome::NeedsAuthorization => (McpServerStatus::PendingAuthorization, Vec::new()),
        ConnectOutcome::Failed(error) => (McpServerStatus::Failed(error), Vec::new()),
    };
    Ok(Some(McpServerInspection {
        server,
        status,
        tools,
    }))
}

/// Looks up the resolved remote config for a server by its final name.
///
/// Uses the same sources and precedence as [`load_mcp_router_with_report`], so
/// `/mcp auth <name>` can only authorize a server the loader would also try to
/// connect. Returns `Ok(None)` for an unknown name or a non-remote server.
pub fn remote_config_for(server_name: &str) -> Result<Option<McpRemoteConfig>> {
    Ok(resolve_current()?
        .servers
        .into_iter()
        .find_map(|(name, config, _source)| match config {
            McpTransportConfig::Remote(remote) if name == server_name => Some(remote),
            _ => None,
        }))
}

/// Runs the interactive OAuth flow for a configured remote server.
///
/// `notify` receives human-facing progress lines (above all the authorization
/// URL the user must open). Distinct from [`remote::authorize_remote_server`],
/// this resolves the server's config first and fails clearly for unknown or
/// non-remote names.
pub async fn authorize_server(
    server_name: &str,
    notify: &mut (dyn FnMut(&str) + Send),
) -> Result<()> {
    let Some(config) = remote_config_for(server_name)? else {
        bail!("no remote MCP server named {server_name} is configured");
    };
    remote::authorize_remote_server(server_name, &config, notify).await
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
    use std::{
        borrow::Cow,
        collections::{BTreeMap, HashMap},
        sync::Arc,
    };

    use rmcp::{
        ErrorData as McpError, ServerHandler, ServiceExt,
        model::{
            CallToolResult, Content, JsonObject, ListToolsResult, ServerInfo, Tool as McpTool,
        },
        service::{RequestContext, RoleServer},
    };
    use serde_json::json;

    use super::{
        MCPToolRouter, McpAuthConfig, McpClient, McpConfigFile, McpLiveStatus, McpLoadReport,
        McpProjectConfig, McpServerConfig, McpToolName, McpTransportConfig, MockMcpService,
        PluginManifest, RealMcpService, SourcedServer, collect_sourced_servers, describe_resolved,
        drain_mcp_stderr, installed_plugin_mcp_servers, resolve_servers,
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
        assert_eq!(
            manifest.mcp_servers["echo"].command.as_deref(),
            Some(expected.command.as_str())
        );
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
        // Remote plugin servers are real servers now, not dropped entries.
        assert!(
            names.contains(&"plugin__demo__remote"),
            "http server must be kept"
        );
        let (_, from_manifest) = servers
            .iter()
            .find(|(name, _)| name == "plugin__demo__fromManifest")
            .unwrap();
        assert_eq!(from_manifest.command.as_deref(), Some("cat"));
        let (_, from_dot_mcp) = servers
            .iter()
            .find(|(name, _)| name == "plugin__demo__fromDotMcp")
            .unwrap();
        assert_eq!(from_dot_mcp.command.as_deref(), Some("node"));
        assert_eq!(from_dot_mcp.args, vec!["srv.js"]);
        let (_, remote) = servers
            .iter()
            .find(|(name, _)| name == "plugin__demo__remote")
            .unwrap();
        assert_eq!(remote.url.as_deref(), Some("https://mcp.example.com/api"));
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
                headers: Default::default(),
                auth: None,
            },
        }
    }

    /// A name skipped in one scope can still be declared by another; it must be
    /// reported once (as configured), not also as skipped.
    #[test]
    fn a_name_configured_in_another_scope_is_not_also_reported_as_skipped() {
        let mut broken = sourced("shared", "~/.tact/mcp.json", "/bin/unused");
        broken.config.command = None;
        let working = sourced("shared", "./.tact/mcp.json", "/bin/project");

        let resolved = resolve_servers(vec![broken, working]);

        assert_eq!(resolved.servers.len(), 1);
        assert!(
            resolved.skipped_remote.is_empty(),
            "configured name was also listed as skipped: {:?}",
            resolved.skipped_remote
        );
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
        assert!(matches!(
            &resolved.servers[0].1,
            McpTransportConfig::Stdio(config) if config.command == "/bin/project"
        ));
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

        let names: Vec<&str> = resolved
            .servers
            .iter()
            .map(|(n, _, _)| n.as_str())
            .collect();
        assert_eq!(names, vec!["user-only", "project-only"]);
        assert!(resolved.shadowed.is_empty());
    }

    #[test]
    fn commandless_entries_are_skipped_not_fatal_while_remote_entries_connect() {
        let remote = SourcedServer {
            name: "hosted".to_owned(),
            source: "~/.tact/mcp.json".to_owned(),
            config: McpProjectConfig {
                server_type: Some("http".to_owned()),
                command: None,
                args: Vec::new(),
                env: Default::default(),
                url: Some("https://example.invalid/mcp".to_owned()),
                headers: Default::default(),
                auth: None,
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
                headers: Default::default(),
                auth: None,
            },
        };

        let resolved = resolve_servers(vec![remote, commandless]);

        assert_eq!(
            resolved
                .servers
                .iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["hosted"]
        );
        assert!(matches!(
            &resolved.servers[0].1,
            McpTransportConfig::Remote(remote) if remote.url == "https://example.invalid/mcp"
        ));
        assert_eq!(resolved.skipped_remote, vec!["typo".to_owned()]);
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

    /// Sorted by name: `mcp.json` is parsed into a `HashMap`, so the listing
    /// must not inherit its random iteration order.
    #[test]
    fn configured_servers_are_sorted_by_name() {
        let mut resolved = resolve_servers(vec![
            sourced("zeta", "~/.tact/mcp.json", "/bin/z"),
            sourced("alpha", "~/.tact/mcp.json", "/bin/a"),
            sourced("mid", "~/.tact/mcp.json", "/bin/m"),
        ]);
        resolved.servers.reverse();

        let names: Vec<String> = resolved
            .configured()
            .into_iter()
            .map(|server| server.name)
            .collect();
        assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn load_report_renders_overrides_and_skipped_servers() {
        let report = McpLoadReport {
            shadowed: vec![("shared".to_owned(), "~/.tact/mcp.json".to_owned())],
            skipped_remote: vec!["hosted".to_owned()],
            ..McpLoadReport::default()
        };

        let lines = report.notice_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("overrides"), "{lines:?}");
        assert!(lines[1].contains("hosted"), "{lines:?}");
    }

    #[test]
    fn load_report_renders_pending_authorization() {
        let report = McpLoadReport {
            pending_auth: vec!["hosted".to_owned()],
            ..McpLoadReport::default()
        };

        assert!(!report.is_quiet());
        let lines = report.notice_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("needs authorization"), "{lines:?}");
        assert!(lines[0].contains("/mcp auth hosted"), "{lines:?}");
    }

    #[test]
    fn remote_entry_with_oauth_needs_authorization_without_credentials() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{
                "url": "https://mcp.example.com/mcp",
                "auth": { "type": "oauth" }
            }"#,
        )
        .unwrap();
        let McpTransportConfig::Remote(remote) = config.to_transport().expect("remote") else {
            panic!("expected remote transport");
        };
        // No `~/.tact/mcp/oauth/<name>.json` exists for this random name.
        assert!(remote.needs_authorization("tact-mcp-oauth-test-missing"));
    }

    #[test]
    fn command_wins_over_url_when_both_are_present() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{
                "command": "node",
                "url": "https://mcp.example.com/mcp"
            }"#,
        )
        .unwrap();

        assert!(matches!(
            config.to_transport(),
            Some(McpTransportConfig::Stdio(stdio)) if stdio.command == "node"
        ));
    }

    #[test]
    fn entry_without_command_or_url_has_no_transport() {
        let config: McpProjectConfig = serde_json::from_str("{}").unwrap();
        assert!(config.to_transport().is_none());
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

    fn sourced_config(name: &str, config: McpProjectConfig) -> SourcedServer {
        SourcedServer {
            name: name.to_string(),
            source: "/tmp/mcp.json".to_string(),
            config,
        }
    }

    fn stdio_config(command: &str) -> McpProjectConfig {
        McpProjectConfig {
            server_type: None,
            command: Some(command.to_string()),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            auth: None,
        }
    }

    fn oauth_remote_config(url: &str) -> McpProjectConfig {
        McpProjectConfig {
            server_type: Some("http".to_string()),
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(url.to_string()),
            headers: HashMap::new(),
            auth: Some(McpAuthConfig::Oauth {
                client_id: None,
                client_name: None,
                scopes: Vec::new(),
                callback_port: None,
            }),
        }
    }

    #[test]
    fn describe_resolved_classifies_against_the_live_connection_set() {
        let resolved = resolve_servers(vec![
            sourced_config("ok", stdio_config("/bin/ok")),
            // A random name so the machine running the tests has no stored
            // credential at ~/.tact/mcp/oauth/<name>.json.
            sourced_config(
                "tact-mcp-live-test-missing",
                oauth_remote_config("https://example.invalid/mcp"),
            ),
            sourced_config("broken", stdio_config("/bin/broken")),
        ]);

        let views = describe_resolved(resolved, &[("ok".to_string(), 3), ("extra".to_string(), 1)]);

        let status = |name: &str| {
            views
                .iter()
                .find(|v| v.server.name == name)
                .unwrap_or_else(|| panic!("{name} missing from {views:?}"))
                .status
                .clone()
        };
        assert_eq!(status("ok"), McpLiveStatus::Connected { tools: 3 });
        assert_eq!(
            status("tact-mcp-live-test-missing"),
            McpLiveStatus::NeedsAuthorization
        );
        assert_eq!(status("broken"), McpLiveStatus::NotConnected);
        // A live client wins over the credential heuristic: a server that is
        // actually connected can never be reported as needing authorization.
        assert_eq!(views.len(), 3, "live-only servers are not listed");
        // Sorted by name, matching the loader's `configured()` order.
        let names: Vec<&str> = views.iter().map(|v| v.server.name.as_str()).collect();
        assert_eq!(names, ["broken", "ok", "tact-mcp-live-test-missing"]);
    }

    #[test]
    fn describe_resolved_lists_a_connected_oauth_server_as_connected() {
        // Precedence guard: the connected check must run before the
        // needs-authorization heuristic, or a working server would be shown
        // as pending once its credential file is the deciding factor.
        let resolved = resolve_servers(vec![sourced_config(
            "tact-mcp-live-test-missing",
            oauth_remote_config("https://example.invalid/mcp"),
        )]);

        let views = describe_resolved(resolved, &[("tact-mcp-live-test-missing".to_string(), 7)]);

        assert_eq!(
            views[0].status,
            McpLiveStatus::Connected { tools: 7 },
            "{views:?}"
        );
    }
}
