//! Model Context Protocol (MCP) integration.
//!
//! MCP is a protocol that allows external tools (written in any language)
//! to expose capabilities to the agent via a JSON-RPC transport.
//!
//! ## Architecture
//!
//! - [`McpConfigFile`] reads Tact's native `.mcp.json` (project
//!   `<workdir>/.tact/.mcp.json`, user `~/.tact/.mcp.json`) and is the only way
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
//! 1. `<workdir>/.mcp.json` (Claude Code project file)
//! 2. `~/.tact/.mcp.json` (user)
//! 3. `<workdir>/.tact/.mcp.json` (project)
//! 4. installed plugins (`plugin__<plugin>__<server>`)
//!
//! Tact's own files come first in that list because they are the ones the user
//! is told to write: a repository's `.mcp.json` is read so a shared project
//! configuration works out of the box, but it is the *lowest*-precedence
//! source, so it can never silently take over a server the user declared.
//! Tact still reads **no** cwd-level Codex manifest — that file (`config.toml`)
//! lives in `CODEX_HOME`, not in a project — while a plugin's own `.mcp.json`
//! is part of its bundle. A plugin is a distributable package, not a config
//! convention: its servers keep manifest-prefixed names and are never how a
//! user is told to configure MCP directly.
//!
//! An entry may be switched off with `"enabled": false` (the Codex convention,
//! used by OpenAI's bundled `unified-computer-use`). Such a declaration is
//! resolved — it can shadow, and be shadowed by, an enabled one — but is never
//! connected, and `mcp list` shows it as disabled.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Ceiling on the MCP `initialize` handshake.
///
/// Deliberately generous: stdio servers are commonly launched through
/// `npx -y …`, which may download a package on a cold cache. The purpose is to
/// bound a *hung* server — startup awaits every configured server, so one that
/// never answers would otherwise wedge the whole session.
const MCP_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Ceiling on `tools/list` for one server.
const MCP_LIST_TOOLS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Ceiling on one `resources/list` or `resources/read`.
///
/// The same order of magnitude as `tools/list`: a resource listing is served
/// from the server's own index, and reading one is a fetch, not a computation.
const MCP_RESOURCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Ceiling on a single `tools/call`.
///
/// Generous on purpose: some MCP tools are legitimately long-running, so this
/// only stops an unbounded hang from wedging the agent loop.
const MCP_CALL_TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Ceiling on one server's `InitializeResult.instructions`.
///
/// The handshake is the one place a server can hand over arbitrary prose that
/// reaches the *trusted* half of the system prompt, so the size is bounded per
/// server rather than trusted. Truncation is marked, never silent — a server
/// whose guidance matters will notice it is being cut.
const MCP_INSTRUCTIONS_MAX_CHARS: usize = 16_384;

use anyhow::{Context, Result, bail};
use futures_util::{
    StreamExt,
    future::{BoxFuture, FutureExt},
    stream::FuturesUnordered,
};
use rmcp::{
    RoleClient, ServiceExt,
    handler::client::ClientHandler,
    model::{
        CallToolRequestParams, CallToolResult, RawContent, RawResource, ReadResourceResult,
        Resource, ResourceContents, ResourceTemplate, Tool as McpTool,
    },
    service::{NotificationContext, RunningService, ServiceError},
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::{
    ToolSpec,
    consts::{PluginDirs, PluginHome, TactPath},
    permission::{CapabilityRisk, normalize_mcp_capability},
    plugin::{PluginRoot, PluginStore},
    tool::copy_tool_spec,
};

mod edit;
mod remote;
mod resource;
pub use edit::*;
pub use remote::*;
pub use resource::*;

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

/// One Codex `env_vars` entry.
///
/// Codex accepts both a bare name (`"env_vars": ["TOKEN"]`) and the explicit
/// `{ "name": …, "source": "local" | "remote" }` object; the bare form means
/// `local`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum McpEnvVar {
    /// The shorthand form.
    Name(String),
    /// The explicit form.
    Explicit {
        name: String,
        #[serde(default)]
        source: Option<String>,
    },
}

impl McpEnvVar {
    /// The variable this entry names.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) | Self::Explicit { name, .. } => name,
        }
    }

    /// Where the value comes from. `None` means Codex's default, `local`.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        match self {
            Self::Name(_) => None,
            Self::Explicit { source, .. } => source.as_deref(),
        }
    }
}

/// Resolves `env_vars` against Tact's environment for one stdio server.
///
/// `explicit` is the entry's literal `env` map and wins any name collision: a
/// user who wrote a value down meant it.
///
/// Failures are returned rather than skipped. A server that expects
/// `GITHUB_TOKEN` and starts without it fails later, somewhere unrelated, with
/// a message about authentication; failing at spawn names the variable. Codex
/// makes the same choice (`env var \`X\` is not set`).
fn resolve_env_vars(
    server_name: &str,
    explicit: &HashMap<String, String>,
    env_vars: &[McpEnvVar],
) -> Result<HashMap<String, String>> {
    let mut resolved = HashMap::new();
    for entry in env_vars {
        let name = entry.name();
        if name.is_empty() {
            bail!("MCP server {server_name} declares an env_vars entry with an empty name");
        }
        if explicit.contains_key(name) {
            tracing::debug!(
                mcp_server = %server_name,
                variable = %name,
                "env_vars entry is overridden by the literal env map; ignoring it"
            );
            continue;
        }
        match entry.source() {
            None | Some("local") => {}
            // Codex's `remote` source asks a remote stdio executor for the
            // value. Tact has no remote-stdio executor, so the honest outcome is
            // a named unsupported-feature error, not a silently missing value.
            Some("remote") => bail!(
                "MCP server {server_name}: env_vars source `remote` needs a remote stdio                  executor, which Tact does not implement"
            ),
            Some(other) => bail!(
                "MCP server {server_name}: unsupported env_vars source `{other}`;                  expected `local` or `remote`"
            ),
        }
        let value = std::env::var(name).map_err(|_| {
            anyhow::anyhow!("MCP server {server_name}: env var `{name}` is not set")
        })?;
        resolved.insert(name.to_string(), value);
    }
    Ok(resolved)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Codex `env_vars`: names copied out of Tact's own environment.
    ///
    /// The child gets these in addition to [`Self::env`]; an explicit `env`
    /// entry with the same name wins, so a literal value is never overridden by
    /// a pass-through. Resolution happens at spawn time (see
    /// [`resolve_env_vars`]) because an unset variable must fail *that server*
    /// with a reason, not start it with a missing credential.
    #[serde(default)]
    pub env_vars: Vec<McpEnvVar>,
    /// Working directory for the subprocess.
    ///
    /// `None` inherits tact's own directory, which is what a user-level
    /// `.mcp.json` entry has always done. A plugin-supplied entry always sets
    /// it: Agent Plugins §7.2.1 defaults it to the plugin root.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// Tact's native MCP configuration file (`.mcp.json`), using the same
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
    /// Declared with `enabled: false`: described, never connected.
    pub disabled: bool,
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
    /// Declared with `enabled: false`: deliberately never connected.
    Disabled,
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
/// (an unparseable `.mcp.json`); a missing file is simply an empty listing.
pub fn describe_servers(connected: &[(String, usize)]) -> Result<Vec<McpServerView>> {
    Ok(describe_resolved(resolve_current()?, connected))
}

/// Pure core of [`describe_servers`], separated so classification is testable
/// without reading the working directory.
fn describe_resolved(
    resolved: ResolvedServers,
    connected: &[(String, usize)],
) -> Vec<McpServerView> {
    let active = resolved
        .servers
        .iter()
        .map(|(name, transport, source)| (name, transport, source, false));
    let switched_off = resolved
        .disabled
        .iter()
        .map(|(name, transport, source)| (name, transport, source, true));
    let mut views: Vec<McpServerView> = active
        .chain(switched_off)
        .map(|(name, transport, source, disabled)| {
            let status = if disabled {
                McpLiveStatus::Disabled
            } else if let Some((_, tools)) = connected.iter().find(|(n, _)| n == name) {
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
                    disabled,
                },
                status,
            }
        })
        .collect();
    views.sort_by(|a, b| a.server.name.cmp(&b.server.name));
    views
}

/// An entry that declares keys Tact does not model.
///
/// Two things land here: a Codex per-entry field Tact still does not implement
/// (`omit_tools_from`), and a field Tact does implement but not *here*
/// (`env_vars` on a remote entry, which has no child process to receive it).
/// Parsing either without honouring it is only honest if the user can see
/// which ones were ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmodelledKeys {
    /// The server name as it appears in the file (before any plugin prefix).
    pub server: String,
    /// The file the entry was read from.
    pub source: String,
    /// The keys, sorted, exactly as they were written.
    pub keys: Vec<String>,
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
    /// Entries that declare keys Tact does not model.
    ///
    /// Deliberately *not* part of [`Self::is_quiet`]: a plugin bundle that
    /// carries Codex-only fields is normal, so it must not turn every startup
    /// into a notice — but `mcp list` names them, because a configuration that
    /// is silently ignored is worse than one that is visibly unimplemented.
    pub unmodelled: Vec<UnmodelledKeys>,
    /// Server name and the tool names this entry's `enabled_tools` /
    /// `disabled_tools` hide from the agent.
    ///
    /// Like [`Self::unmodelled`], deliberately *not* part of
    /// [`Self::is_quiet`]: filtering is a deliberate configuration, not a
    /// problem — but hiding a tool must be visible somewhere, so `mcp list` and
    /// `mcp get` report it.
    pub filtered: Vec<(String, Vec<String>)>,
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
    /// Declared with `enabled: false`: described without being dialled.
    Disabled,
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
    /// Names the server exposes but this entry's `enabled_tools` /
    /// `disabled_tools` keep away from the agent.
    ///
    /// Reported so a filtered server cannot be mistaken for one that simply
    /// lacks those tools.
    pub filtered: Vec<String>,
    /// Length of the server's `InitializeResult.instructions`, in characters.
    ///
    /// `None` when the server sent none. Reported so a server whose guidance is
    /// silently dropped is distinguishable from one that never sent any —
    /// the same reason `filtered` exists.
    pub instructions_chars: Option<usize>,
    /// How many resources `resources/list` returned.
    ///
    /// `None` means the server did not answer the request (no resource support,
    /// or an error): a server that publishes no resources and a server that
    /// cannot answer at all are different, and `list_mcp_resources` behaves
    /// differently for each.
    pub resources: Option<usize>,
    /// How many templates `resources/templates/list` returned.
    ///
    /// `None` means the server did not answer, which is a different fact from
    /// publishing none — and the one that matters, because a template-addressed
    /// server is exactly the case `resources/list` cannot describe.
    pub resource_templates: Option<usize>,
    /// Exposed tools the server marked `readOnlyHint: true`, sorted.
    ///
    /// Evidence for the human writing `tools.<name>.risk`, not an input to the
    /// risk itself — reported so a server's claim is visible instead of being
    /// silently dropped along with the rest of `Tool::annotations`.
    pub declared_read_only: Vec<String>,
    /// Tools whose entry declares a risk, with the tier it declares, sorted by
    /// name.
    ///
    /// Only *declared* tiers appear. A tool absent from this list keeps Tact's
    /// default (`high`), so an entry that declares a risk is never
    /// indistinguishable from one that silently kept the default — the same
    /// reason `filtered` and `declared_read_only` exist.
    pub declared_risks: Vec<(String, CapabilityRisk)>,
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
async fn connect_server(
    name: &str,
    config: McpTransportConfig,
    policy: McpServerPolicy,
) -> ConnectOutcome {
    if let McpTransportConfig::Remote(remote) = &config
        && remote.needs_authorization(name)
    {
        tracing::info!(
            mcp_server = %name,
            "remote MCP server needs OAuth authorization; run `mcp login`"
        );
        return ConnectOutcome::NeedsAuthorization;
    }
    match McpClient::try_new_with_policy(name.to_string(), config, policy).await {
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
    /// Left untyped on purpose.
    ///
    /// Codex bundles write either the server map inline or a path to it
    /// (`"mcpServers": "./.mcp.json"`). Typing this as a map made the whole
    /// manifest unparseable in the second case, which downgraded a valid bundle
    /// to "no MCP servers at all". Interpretation happens in
    /// [`plugin_manifest_mcp_servers`], where a bad shape costs one field
    /// instead of the manifest.
    #[serde(default)]
    pub mcp_servers: Option<Value>,
}

/// Codex's `tools.<name>` override for one MCP tool.
///
/// Keys are snake_case exactly as Codex writes them, so this struct declares
/// its own convention instead of inheriting `McpProjectConfig`'s `camelCase`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct McpToolConfig {
    /// Codex `approval_mode`: `auto` | `prompt` | `approve`.
    ///
    /// Left as a string so an unknown value can be reported and ignored rather
    /// than failing the whole entry.
    #[serde(default)]
    pub approval_mode: Option<String>,
    /// Codex `output_token_limit`: result budget for this tool.
    #[serde(default)]
    pub output_token_limit: Option<usize>,
    /// **Tact's own** `risk`: `read` | `write` | `high`.
    ///
    /// Not a Codex field — Codex has no per-tool risk axis, which is why every
    /// MCP tool used to resolve to `CapabilityRisk::High` and could only be
    /// made usable unattended through `approval_mode: "auto"`.
    ///
    /// Kept as a string for the same reason as `approval_mode`: an unknown
    /// value is warned about and ignored, never fatal. Modelled rather than
    /// left to serde's unknown-field handling, so a declared risk cannot be
    /// dropped in silence.
    #[serde(default)]
    pub risk: Option<String>,
}

/// Tact's per-tool risk declaration for an MCP tool.
///
/// The tiers are not steps on one knob — see [`Self::as_str`] and the chapter
/// table. `Write` is the tier to reach for when a tool must be usable
/// unattended; `Read` is the only one that bypasses plan mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    /// Read-only: allowed in every mode, including plan mode.
    Read,
    /// Same risk as a native writing tool: blocked in plan mode, asks once.
    Write,
    /// The default when nothing is declared: blocked in plan mode, asks, and
    /// denied outright in a non-interactive run.
    High,
}

impl ToolRisk {
    /// Parses a configured value. `None` for anything unrecognised.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::High => "high",
        }
    }

    #[must_use]
    pub fn to_capability(self) -> CapabilityRisk {
        match self {
            Self::Read => CapabilityRisk::Read,
            Self::Write => CapabilityRisk::Write,
            Self::High => CapabilityRisk::High,
        }
    }
}

/// Codex's MCP approval modes.
///
/// Only [`ApprovalMode::Auto`] changes Tact's behaviour; it is an
/// auto-**approve** on the *prompt* axis, never a re-classification. The
/// risk a tool is reported and gated at comes from [`ToolRisk`] — the entry's
/// `tools.<name>.risk` or `default_tool_risk`, and `CapabilityRisk::High` when
/// the entry declares neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Run without asking.
    Auto,
    /// Ask every time — Tact's unchanged default.
    Prompt,
    /// Ask every time. Kept as its own value so a Codex config round-trips
    /// instead of being rewritten.
    Approve,
}

impl ApprovalMode {
    /// Parses a configured value. `None` for anything unrecognised, which the
    /// caller reports and treats as absent.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "auto" => Some(Self::Auto),
            "prompt" => Some(Self::Prompt),
            "approve" => Some(Self::Approve),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Prompt => "prompt",
            Self::Approve => "approve",
        }
    }
}

/// Resolved per-tool policy inside one server's [`McpServerPolicy`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct McpToolPolicy {
    approval_mode: Option<ApprovalMode>,
    output_token_limit: Option<usize>,
    /// Already resolved to a capability, so the resolver cannot hand out a
    /// tier that has no meaning at the permission layer.
    risk: Option<CapabilityRisk>,
}

/// What one `.mcp.json` entry declares about the server's tools.
///
/// Empty by default: a plain `{"command": …, "args": […]}` entry exposes every
/// tool, uses the global handshake timeout, keeps every tool asking for
/// approval, and leaves every tool at `CapabilityRisk::High`.
#[derive(Debug, Clone, Default)]
pub struct McpServerPolicy {
    enabled_tools: Option<Vec<String>>,
    disabled_tools: Option<Vec<String>>,
    startup_timeout: Option<std::time::Duration>,
    tool_timeout: Option<std::time::Duration>,
    default_approval_mode: Option<ApprovalMode>,
    /// The entry's `default_tool_risk`, already resolved.
    default_risk: Option<CapabilityRisk>,
    tool_overrides: HashMap<String, McpToolPolicy>,
}

impl McpServerPolicy {
    /// Reads the Codex per-entry fields off a parsed configuration.
    #[must_use]
    fn from_config(config: &McpProjectConfig) -> Self {
        // Codex documents both spellings; seconds wins when both are present so
        // one entry can never mean two different budgets.
        let startup_timeout = match (config.startup_timeout_sec, config.startup_timeout_ms) {
            (Some(secs), _) => Some(std::time::Duration::from_secs(secs)),
            (None, Some(ms)) => Some(std::time::Duration::from_millis(ms)),
            (None, None) => None,
        };

        let tool_overrides = config
            .tools
            .as_ref()
            .map(|tools| {
                tools
                    .iter()
                    .map(|(name, raw)| {
                        (
                            name.clone(),
                            McpToolPolicy {
                                approval_mode: parse_approval_mode(
                                    raw.approval_mode.as_deref(),
                                    &format!("tools.{name}.approval_mode"),
                                ),
                                output_token_limit: raw.output_token_limit,
                                risk: parse_tool_risk(
                                    raw.risk.as_deref(),
                                    &format!("tools.{name}.risk"),
                                ),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            enabled_tools: config.enabled_tools.clone(),
            disabled_tools: config.disabled_tools.clone(),
            startup_timeout,
            tool_timeout: config.tool_timeout_sec.map(std::time::Duration::from_secs),
            default_approval_mode: parse_approval_mode(
                config.default_tools_approval_mode.as_deref(),
                "default_tools_approval_mode",
            ),
            default_risk: parse_tool_risk(config.default_tool_risk.as_deref(), "default_tool_risk"),
            tool_overrides,
        }
    }

    /// Whether this server exposes `tool` to the agent.
    ///
    /// `disabled_tools` is applied after `enabled_tools`, which is Codex's
    /// documented order: naming a tool in both lists hides it.
    #[must_use]
    pub fn exposes(&self, tool: &str) -> bool {
        if let Some(enabled) = &self.enabled_tools
            && !enabled.iter().any(|listed| listed == tool)
        {
            return false;
        }
        if let Some(disabled) = &self.disabled_tools
            && disabled.iter().any(|listed| listed == tool)
        {
            return false;
        }
        true
    }

    /// The handshake budget this entry asks for, if any.
    #[must_use]
    pub fn startup_timeout(&self) -> Option<std::time::Duration> {
        self.startup_timeout
    }

    /// The single-call budget this entry asks for, if any.
    #[must_use]
    pub fn tool_timeout(&self) -> Option<std::time::Duration> {
        self.tool_timeout
    }

    /// Whether the entry declares `approval_mode: "auto"` for `tool`.
    ///
    /// A per-tool override wins over the server default.
    #[must_use]
    pub fn is_auto_approved(&self, tool: &str) -> bool {
        let mode = self
            .tool_overrides
            .get(tool)
            .and_then(|policy| policy.approval_mode)
            .or(self.default_approval_mode);
        mode == Some(ApprovalMode::Auto)
    }

    /// The risk this entry declares for `tool`, if it declares one at all.
    ///
    /// A per-tool override wins over the entry's `default_tool_risk`. `None`
    /// means the entry is silent, which the caller resolves to Tact's default
    /// (`CapabilityRisk::High`) rather than to a guess.
    #[must_use]
    pub fn risk_for(&self, tool: &str) -> Option<CapabilityRisk> {
        self.tool_overrides
            .get(tool)
            .and_then(|policy| policy.risk)
            .or(self.default_risk)
    }

    /// The per-tool result budget this entry declares, if any.
    #[must_use]
    pub fn output_token_limit(&self, tool: &str) -> Option<usize> {
        self.tool_overrides
            .get(tool)
            .and_then(|policy| policy.output_token_limit)
    }
}

/// Parses an `approval_mode`, reporting an unknown value instead of ignoring it.
fn parse_approval_mode(value: Option<&str>, field: &str) -> Option<ApprovalMode> {
    let value = value?;
    match ApprovalMode::parse(value) {
        Some(mode) => Some(mode),
        None => {
            tracing::warn!(
                field,
                value,
                "unknown MCP approval_mode (expected auto|prompt|approve); ignoring it"
            );
            None
        }
    }
}

/// Parses a configured `risk`. An unknown value is reported and ignored, so a
/// typo cannot fail the whole entry — the same rule `approval_mode` follows.
fn parse_tool_risk(value: Option<&str>, field: &str) -> Option<CapabilityRisk> {
    let value = value?;
    match ToolRisk::parse(value) {
        Some(risk) => Some(risk.to_capability()),
        None => {
            tracing::warn!(
                field,
                value,
                "unknown MCP tool risk (expected read|write|high); ignoring it"
            );
            None
        }
    }
}

/// MCP server entry in `.mcp.json` / a plugin `.mcp.json`.
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
    /// Codex `env_vars`: names to copy out of Tact's environment.
    ///
    /// Applies to stdio entries. A remote entry that declares any is reported as
    /// unmodelled, because there is no child process to put them in.
    #[serde(rename = "env_vars", default)]
    pub env_vars: Vec<McpEnvVar>,
    /// Working directory for a stdio server (Agent Plugins §7.2.1).
    ///
    /// Modelled rather than left in [`Self::extra`] because a plugin entry may
    /// set it, and `mcp list` should not report a field tact honours as
    /// unmodelled. Entries from user and project configuration keep whatever
    /// they wrote; only plugin-supplied entries are resolved and contained.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Extra request headers for remote servers (static auth goes here).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// OAuth declaration for remote servers.
    #[serde(default)]
    pub auth: Option<McpAuthConfig>,
    /// Whether this declaration is active; absent means active.
    ///
    /// Codex plugin bundles ship servers switched off with `"enabled": false`
    /// (OpenAI's own `unified-computer-use` does). A disabled declaration still
    /// takes part in name resolution — it can shadow an enabled declaration
    /// from a lower-precedence source — but is never connected.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Codex `enabled_tools`: allow list of tool names this server exposes.
    ///
    /// Absent means every tool the server lists. Written snake_case because
    /// that is the spelling Codex uses for these fields.
    #[serde(rename = "enabled_tools", default)]
    pub enabled_tools: Option<Vec<String>>,
    /// Codex `disabled_tools`: deny list, applied **after** `enabled_tools`.
    #[serde(rename = "disabled_tools", default)]
    pub disabled_tools: Option<Vec<String>>,
    /// Codex `startup_timeout_sec`: handshake budget for this server.
    ///
    /// Overrides the global default for slow launchers (a cold `uvx` measured
    /// ~100s). Absent keeps Tact's own default rather than Codex's 10s: an
    /// entry that worked before must not start timing out.
    #[serde(rename = "startup_timeout_sec", default)]
    pub startup_timeout_sec: Option<u64>,
    /// Codex `startup_timeout_ms`: millisecond alias for `startup_timeout_sec`.
    #[serde(rename = "startup_timeout_ms", default)]
    pub startup_timeout_ms: Option<u64>,
    /// Codex `tool_timeout_sec`: budget for one `tools/call` on this server.
    ///
    /// Overrides Tact's global default for this server only, which is what the
    /// field means in Codex. Absent keeps Tact's own ceiling.
    #[serde(rename = "tool_timeout_sec", default)]
    pub tool_timeout_sec: Option<u64>,
    /// Codex `default_tools_approval_mode`: server-wide approval default.
    #[serde(rename = "default_tools_approval_mode", default)]
    pub default_tools_approval_mode: Option<String>,
    /// **Tact's own** `default_tool_risk`: risk for this server's tools that
    /// name no `risk` of their own.
    ///
    /// Not a Codex field. Absent keeps Tact's historical default
    /// (`CapabilityRisk::High`), so an entry that says nothing about risk
    /// behaves exactly as it did before this field existed.
    #[serde(rename = "default_tool_risk", default)]
    pub default_tool_risk: Option<String>,
    /// Codex `tools`: per-tool `approval_mode` / `output_token_limit`.
    #[serde(rename = "tools", default)]
    pub tools: Option<HashMap<String, McpToolConfig>>,
    /// Keys this struct does not model.
    ///
    /// Kept so the resolver can report them instead of dropping them silently:
    /// the remaining Codex per-entry field (`omit_tools_from`) lands here, and a
    /// configuration someone wrote must not disappear without a word.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// `enabled` defaults to on, so a plain `{"command": …}` entry stays active.
fn enabled_by_default() -> bool {
    true
}

impl Default for McpProjectConfig {
    /// Empty, but **enabled**: an entry that says nothing is an active entry,
    /// and `enabled_by_default` agrees.
    fn default() -> Self {
        Self {
            server_type: None,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            env_vars: Vec::new(),
            cwd: None,
            url: None,
            headers: HashMap::new(),
            auth: None,
            enabled: true,
            enabled_tools: None,
            disabled_tools: None,
            startup_timeout_sec: None,
            startup_timeout_ms: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            default_tool_risk: None,
            tools: None,
            extra: HashMap::new(),
        }
    }
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
            env_vars: self.env_vars.clone(),
            cwd: self.cwd.as_deref().map(PathBuf::from),
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
                env_vars: self.env_vars.clone(),
                cwd: self.cwd.as_deref().map(PathBuf::from),
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
/// read here — but never at the working directory, where `.tact/.mcp.json` is
/// the single answer.
pub fn installed_plugin_mcp_servers(home: &PluginHome) -> Result<Vec<(String, McpProjectConfig)>> {
    let store = PluginStore::new(home.clone());
    let mut servers = Vec::new();
    for root in store.installed_plugin_roots()? {
        let dirs = PluginDirs {
            data: home.plugin_data_dir(&root.marketplace, &root.plugin_id),
            root: root.root.clone(),
        };
        collect_plugin_mcp_servers(&root, &dirs, &mut servers)?;
    }
    Ok(servers)
}

fn collect_plugin_mcp_servers(
    root: &PluginRoot,
    dirs: &PluginDirs,
    servers: &mut Vec<(String, McpProjectConfig)>,
) -> Result<()> {
    let prefix = |name: &str| format!("plugin__{}__{}", root.plugin_id, name);
    let accept =
        |servers: &mut Vec<(String, McpProjectConfig)>, name: String, config: McpProjectConfig| {
            match prepare_plugin_entry(&config, dirs) {
                Some(prepared) => {
                    if let Err(error) = ensure_plugin_data_dir(&prepared, dirs) {
                        tracing::warn!(
                            "plugin {}: cannot create data directory {}: {error}",
                            root.plugin_id,
                            dirs.data.display()
                        );
                    }
                    servers.push((prefix(&name), prepared));
                }
                // Agent Plugins §7.2.2: a bad entry is invalid on its own,
                // never the whole plugin.
                None => tracing::warn!(
                    "plugin {} MCP server {name} is not a valid Agent Plugins entry; skipping",
                    root.plugin_id
                ),
            }
        };

    let manifest_path = root.root.join(".codex-plugin").join("plugin.json");
    if manifest_path.is_file() {
        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        match serde_json::from_str::<PluginManifest>(&raw) {
            Ok(manifest) => {
                match plugin_manifest_mcp_servers(&root.root, manifest.mcp_servers.as_ref()) {
                    Ok(configs) => {
                        for (name, config) in configs {
                            accept(servers, name, config);
                        }
                    }
                    Err(error) => tracing::warn!(
                        "plugin {} manifest MCP declaration is unusable: {error}",
                        root.plugin_id
                    ),
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

    // A plugin-root MCP file is part of the plugin *bundle* format, not a
    // project config convention. Tact deliberately does not read one at the
    // working directory — that role belongs to `<workdir>/.tact/.mcp.json`.
    //
    // `.mcp.json` is the Codex bundle name; `mcp.json` is the Agent Plugins
    // §7.2.1 core path. The Codex name wins when a bundle ships both.
    for file_name in [".mcp.json", "mcp.json"] {
        let mcp_path = root.root.join(file_name);
        if !mcp_path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&mcp_path)
            .with_context(|| format!("failed to read {}", mcp_path.display()))?;
        let configs = parse_plugin_mcp_document(&raw)?;
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
            accept(servers, name, config);
        }
    }

    Ok(())
}

/// The placeholder names Agent Plugins defines — v1 has exactly these two.
const PLUGIN_ROOT_VAR: &str = "${PLUGIN_ROOT}";
const PLUGIN_DATA_VAR: &str = "${PLUGIN_DATA}";

/// Expands both placeholders in a single non-recursive pass (§9.2).
///
/// Single-pass matters: text introduced by a replacement must never be
/// rescanned, so a plugin root that happens to contain `${PLUGIN_DATA}` stays
/// exactly as written. An unrecognized `${…}` is left literal.
fn expand_placeholders(value: &str, dirs: &PluginDirs) -> String {
    if !value.contains("${") {
        return value.to_owned();
    }
    let root = dirs.root.to_string_lossy();
    let data = dirs.data.to_string_lossy();
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(after) = tail.strip_prefix(PLUGIN_ROOT_VAR) {
            out.push_str(&root);
            rest = after;
        } else if let Some(after) = tail.strip_prefix(PLUGIN_DATA_VAR) {
            out.push_str(&data);
            rest = after;
        } else {
            out.push_str("${");
            rest = &tail[2..];
        }
    }
    out.push_str(rest);
    out
}

/// Folds `.` and `..` without touching the filesystem.
///
/// A plugin-supplied `cwd` may name a directory that does not exist yet, so
/// `canonicalize` is unavailable; lexical folding is still enough to reject an
/// escape such as `${PLUGIN_DATA}/../../etc`.
fn fold_path(path: &Path) -> PathBuf {
    let mut folded = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other.as_os_str()),
        }
    }
    folded
}

/// Resolves a plugin entry's `cwd` (§7.2.1).
///
/// Returns `None` when the value is not one of the three allowed forms, or when
/// it escapes the root it names — either way the entry is invalid.
fn resolve_plugin_cwd(raw: &str, dirs: &PluginDirs) -> Option<PathBuf> {
    let raw = raw.trim();
    let at_data = raw == PLUGIN_DATA_VAR || raw.starts_with(&format!("{PLUGIN_DATA_VAR}/"));
    let at_root = raw == PLUGIN_ROOT_VAR || raw.starts_with(&format!("{PLUGIN_ROOT_VAR}/"));
    let (base, candidate) = if at_data {
        (
            dirs.data.clone(),
            PathBuf::from(expand_placeholders(raw, dirs)),
        )
    } else if at_root {
        (
            dirs.root.clone(),
            PathBuf::from(expand_placeholders(raw, dirs)),
        )
    } else {
        let relative = raw.strip_prefix("./")?;
        (dirs.root.clone(), dirs.root.join(relative))
    };
    let folded = fold_path(&candidate);
    folded.starts_with(fold_path(&base)).then_some(folded)
}

/// Resolves a plugin entry's `command` (§7.2.1).
///
/// `command` takes **no** placeholder expansion (§9.2). A `./` path resolves
/// against the plugin root and must stay inside it; anything else is left to
/// the platform's executable search, which is also what a user-level `.mcp.json`
/// entry relies on.
fn resolve_plugin_command(command: &str, dirs: &PluginDirs) -> Option<String> {
    let Some(relative) = command.strip_prefix("./") else {
        return Some(command.to_owned());
    };
    let folded = fold_path(&dirs.root.join(relative));
    folded
        .starts_with(&dirs.root)
        .then(|| folded.to_string_lossy().into_owned())
}

/// Interprets `plugin.json`'s `mcpServers` field.
///
/// Codex bundles either inline the map or point at a file with
/// `"mcpServers": "./.mcp.json"`. Agent Plugins 1.0.0 itself declares MCP only
/// in the root `mcp.json` and forbids this field, but the Codex compatibility
/// layout is what installed bundles actually ship, so both shapes are read.
fn plugin_manifest_mcp_servers(
    root: &Path,
    declared: Option<&Value>,
) -> Result<HashMap<String, McpProjectConfig>> {
    match declared {
        None | Some(Value::Null) => Ok(HashMap::new()),
        Some(Value::Object(map)) => Ok(serde_json::from_value(Value::Object(map.clone()))?),
        Some(Value::String(path)) => {
            let resolved = fold_path(&root.join(path));
            if !resolved.starts_with(root) {
                bail!("mcpServers path {path} escapes the plugin root");
            }
            let raw = fs::read_to_string(&resolved)
                .with_context(|| format!("failed to read {}", resolved.display()))?;
            parse_plugin_mcp_document(&raw)
        }
        Some(other) => bail!("mcpServers must be an object or a path, found {other}"),
    }
}

/// Applies the Agent Plugins rules a plugin-supplied MCP entry must satisfy,
/// returning `None` when the entry is invalid on its own (§7.2.2).
///
/// - `env` may not declare the two reserved names (§9.2)
/// - `args` values, `env` values and `cwd` get placeholder expansion (§9.2)
/// - a `cwd` must be one of the three allowed forms and stay contained (§7.2.1)
/// - the client supplies `PLUGIN_ROOT`/`PLUGIN_DATA` itself, last (§9.1)
fn prepare_plugin_entry(config: &McpProjectConfig, dirs: &PluginDirs) -> Option<McpProjectConfig> {
    // A remote entry launches nothing and §9.2 scopes expansion to stdio
    // configuration, so it passes through untouched.
    if config.command.is_none() {
        return Some(config.clone());
    }
    if config
        .env
        .keys()
        .any(|key| key == "PLUGIN_ROOT" || key == "PLUGIN_DATA")
    {
        return None;
    }
    let mut prepared = config.clone();
    // A `./` command that escapes the plugin root invalidates the entry — the
    // `?` is the whole point: without it the entry would survive with no
    // command and be reported as merely unsupported.
    prepared.command = Some(resolve_plugin_command(config.command.as_deref()?, dirs)?);
    prepared.args = config
        .args
        .iter()
        .map(|arg| expand_placeholders(arg, dirs))
        .collect();
    prepared.env = config
        .env
        .iter()
        .map(|(key, value)| (key.clone(), expand_placeholders(value, dirs)))
        .collect();
    // §7.2.1: an absent `cwd` means the plugin root.
    prepared.cwd = match config.cwd.as_deref() {
        Some(raw) => Some(
            resolve_plugin_cwd(raw, dirs)?
                .to_string_lossy()
                .into_owned(),
        ),
        None => Some(dirs.root.to_string_lossy().into_owned()),
    };
    // §9.1: the client sets these after the configured env, replacing same-name
    // entries — which the reserved-name rejection above guarantees cannot be
    // present.
    prepared.env.insert(
        "PLUGIN_ROOT".to_owned(),
        dirs.root.to_string_lossy().into_owned(),
    );
    prepared.env.insert(
        "PLUGIN_DATA".to_owned(),
        dirs.data.to_string_lossy().into_owned(),
    );
    Some(prepared)
}

/// Creates the plugin's data directory the first time a stdio entry needs it.
///
/// Agent Plugins §9.1 requires the directory to exist before the subprocess
/// starts. It is deliberately not created for a plugin that only declares
/// remote servers, which launch nothing.
fn ensure_plugin_data_dir(config: &McpProjectConfig, dirs: &PluginDirs) -> std::io::Result<()> {
    if config.command.is_none() {
        return Ok(());
    }
    fs::create_dir_all(&dirs.data)
}

/// Parse a plugin bundle's `.mcp.json`.
///
/// Accepts both the Codex `{"mcpServers": {...}}` wrapper (the shape every MCP
/// client and the reference plugins ship) and a flat `name -> config` map. The
/// wrapper wins when present, so one file cannot be interpreted two ways.
fn parse_plugin_mcp_document(raw: &str) -> Result<HashMap<String, McpProjectConfig>> {
    let document: serde_json::Value = serde_json::from_str(raw)?;
    let servers = document.get("mcpServers").unwrap_or(&document);
    Ok(serde_json::from_value(servers.clone())?)
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

    /// `resources/list`, paginated to the end by the implementation.
    ///
    /// Required rather than defaulted: a service that cannot answer must say so
    /// (the mock returns an empty list, and `read_resource` a not-found error),
    /// because a silently empty resource list is indistinguishable from a server
    /// that publishes none.
    fn list_resources(&self) -> BoxFuture<'_, Result<Vec<Resource>, ServiceError>>;

    /// `resources/read` for one URI.
    fn read_resource(&self, uri: String)
    -> BoxFuture<'_, Result<ReadResourceResult, ServiceError>>;

    /// `resources/templates/list`, paginated to the end by the implementation.
    ///
    /// Required for the same reason [`Self::list_resources`] is: a service that
    /// cannot ask must say so, or "this transport cannot answer" becomes
    /// indistinguishable from "the server publishes no templates".
    fn list_resource_templates(
        &self,
    ) -> BoxFuture<'_, Result<Vec<ResourceTemplate>, ServiceError>>;

    /// The `instructions` string the server returned during `initialize`.
    ///
    /// This is the MCP spec's channel for "what this server is and how to use
    /// it", and the only thing a newly connected model gets *for free* — the
    /// resources and tool descriptions behind it need a deliberate fetch. A
    /// default of `None` keeps every test double compiling; only the real
    /// client answers.
    fn instructions(&self) -> Option<String> {
        None
    }

    /// Whether the server signalled `notifications/tools/list_changed` since the
    /// last check, clearing the flag.
    ///
    /// Defaulted to `false` rather than required: a test double, or a transport
    /// that cannot carry notifications, must never look like a server that keeps
    /// changing its mind — every request would re-list.
    fn take_tools_changed(&self) -> bool {
        false
    }
}

/// Records that a server said its tool list changed.
///
/// The connection's `ClientHandler` is this type, so rmcp delivers
/// `notifications/tools/list_changed` here instead of dropping it on the unit
/// handler Tact used before. The handler deliberately records *only* that
/// something changed, and never re-lists: a refresh needs `&mut McpClient`, and
/// running one inside the service's own notification task would contend with
/// the transport driving it. The flag means "ask again", not what the answer is.
#[derive(Debug, Clone, Default)]
struct ToolListChangedSignal {
    changed: Arc<AtomicBool>,
}

impl ToolListChangedSignal {
    /// Whether a notification arrived since the last check, clearing the flag.
    fn take(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

impl ClientHandler for ToolListChangedSignal {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.changed.store(true, Ordering::Release);
    }
}

struct RealMcpService {
    service: tokio::sync::RwLock<Option<RunningService<RoleClient, ToolListChangedSignal>>>,
    /// Snapshotted at construction: `peer_info()` is only readable while the
    /// service is alive, and `McpService::instructions` is synchronous.
    instructions: Option<String>,
    /// Shared with the connection's handler, so a notification the server sends
    /// is visible to [`McpService::take_tools_changed`].
    tools_changed: ToolListChangedSignal,
}

impl RealMcpService {
    fn new(
        service: RunningService<RoleClient, ToolListChangedSignal>,
        tools_changed: ToolListChangedSignal,
    ) -> Self {
        // Snapshotted at construction: `peer_info()` is only readable while the
        // service is alive, and `McpService::instructions` is synchronous.
        let instructions = service
            .peer_info()
            .and_then(|info| info.instructions.clone());
        Self {
            service: tokio::sync::RwLock::new(Some(service)),
            instructions,
            tools_changed,
        }
    }
}

impl McpService for RealMcpService {
    fn list_all_tools(&self) -> BoxFuture<'_, Result<Vec<McpTool>, ServiceError>> {
        async move {
            let guard = self.service.read().await;
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
            let guard = self.service.read().await;
            match guard.as_ref() {
                Some(service) => service.call_tool(params).await,
                None => Err(ServiceError::TransportClosed),
            }
        }
        .boxed()
    }

    fn cancel(&self) -> BoxFuture<'_, ()> {
        async move {
            let mut guard = self.service.write().await;
            if let Some(service) = guard.take() {
                let _ = service.cancel().await;
            }
        }
        .boxed()
    }

    fn instructions(&self) -> Option<String> {
        self.instructions.clone()
    }

    fn take_tools_changed(&self) -> bool {
        self.tools_changed.take()
    }

    fn list_resources(&self) -> BoxFuture<'_, Result<Vec<Resource>, ServiceError>> {
        async move {
            let guard = self.service.read().await;
            match guard.as_ref() {
                Some(service) => service.list_all_resources().await,
                None => Err(ServiceError::TransportClosed),
            }
        }
        .boxed()
    }

    fn read_resource(
        &self,
        uri: String,
    ) -> BoxFuture<'_, Result<ReadResourceResult, ServiceError>> {
        async move {
            let guard = self.service.read().await;
            match guard.as_ref() {
                Some(service) => service.read_resource(read_params(&uri)).await,
                None => Err(ServiceError::TransportClosed),
            }
        }
        .boxed()
    }

    fn list_resource_templates(
        &self,
    ) -> BoxFuture<'_, Result<Vec<ResourceTemplate>, ServiceError>> {
        async move {
            let guard = self.service.read().await;
            match guard.as_ref() {
                Some(service) => service.list_all_resource_templates().await,
                None => Err(ServiceError::TransportClosed),
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

/// Bounds one server's instructions, marking the cut.
///
/// The truncation marker is part of the injected text on purpose: a silent cut
/// would leave the model with guidance that stops mid-sentence and no way to
/// know it is incomplete.
fn cap_instructions(text: &str) -> String {
    if text.chars().count() <= MCP_INSTRUCTIONS_MAX_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MCP_INSTRUCTIONS_MAX_CHARS).collect();
    format!("{kept}\n… (truncated at {MCP_INSTRUCTIONS_MAX_CHARS} characters)")
}

pub struct McpClient {
    pub server_name: String,
    service: Arc<dyn McpService>,
    tools: Vec<McpTool>,
    tool_specs: Vec<ToolSpec>,
    /// Tool names the entry's `enabled_tools` / `disabled_tools` hide.
    ///
    /// Kept so the filtering can be *reported*: a configuration that removes a
    /// tool must not look identical to a server that never had it.
    hidden: Vec<String>,
    /// The entry's per-tool policy (approval mode, output budget, risk).
    ///
    /// Boxed so `ConnectOutcome::Connected` stays small: the enum is built
    /// once per server, and an unboxed policy doubled the variant's size
    /// (`clippy::large_enum_variant`).
    policy: Box<McpServerPolicy>,
    /// Exposed tools the server itself marked `readOnlyHint: true`, sorted.
    ///
    /// Evidence, never authority: it is displayed by `mcp get` so a human can
    /// decide whether to declare `tools.<name>.risk`, and it deliberately does
    /// **not** reach [`McpServerPolicy::risk_for`]. A server that lies is the
    /// case a permission system exists to survive, and even an honest
    /// read-only tool can be an egress path when it also sets
    /// `openWorldHint` — Tact has no data-flow axis to express that.
    declared_read_only: Vec<String>,
    /// The server's `InitializeResult.instructions`, normalized and capped.
    ///
    /// `None` when the server sent none (or only whitespace). Capped at
    /// [`MCP_INSTRUCTIONS_MAX_CHARS`] here, once, so every consumer — the
    /// prompt block and `mcp get` — reports the same length.
    instructions: Option<String>,
}

impl McpClient {
    pub async fn try_new(
        server_name: impl Into<String>,
        config: McpTransportConfig,
    ) -> Result<Self> {
        Self::try_new_with_policy(server_name, config, McpServerPolicy::default()).await
    }

    /// Connects a server together with the policy its entry declared.
    pub async fn try_new_with_policy(
        server_name: impl Into<String>,
        config: McpTransportConfig,
        policy: McpServerPolicy,
    ) -> Result<Self> {
        let server_name = server_name.into();
        let (running, tools_changed) =
            Self::connect(&server_name, config, policy.startup_timeout()).await?;
        let service: Arc<dyn McpService> = Arc::new(RealMcpService::new(running, tools_changed));
        match Self::fetch_tools(&server_name, service.as_ref()).await {
            Ok(tools) => Ok(Self::assemble(server_name, tools, service, policy)),
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
        Self::with_service_and_policy(server_name, tools, service, McpServerPolicy::default())
    }

    /// [`Self::with_service`] for a server whose entry declares a tool policy.
    pub fn with_service_and_policy(
        server_name: impl Into<String>,
        tools: Vec<McpTool>,
        service: Arc<dyn McpService>,
        policy: McpServerPolicy,
    ) -> Self {
        Self::assemble(server_name.into(), tools, service, policy)
    }

    /// Applies the entry's tool filter and keeps the hidden names for reporting.
    fn assemble(
        server_name: String,
        tools: Vec<McpTool>,
        service: Arc<dyn McpService>,
        policy: McpServerPolicy,
    ) -> Self {
        let ExposedTools {
            tools,
            hidden,
            tool_specs,
            declared_read_only,
        } = derive_exposed(&server_name, tools, &policy);
        // Read the server's prose before the service is moved into the client.
        // Normalizing here (not in the transport) keeps the mock and the real
        // client on one path: a whitespace-only payload means "sent nothing".
        let instructions = service
            .instructions()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .map(|text| cap_instructions(&text));
        Self {
            server_name,
            service,
            tools,
            tool_specs,
            hidden,
            policy: Box::new(policy),
            declared_read_only,
            instructions,
        }
    }

    /// Re-lists this server's tools when it said they changed.
    ///
    /// `Ok(None)` means the server was quiet and nothing was re-listed: the
    /// notification is only a hint that the answer moved, so a server that
    /// never sends one costs exactly nothing. A re-list failure leaves the
    /// previous list in place and is reported by the caller.
    pub async fn refresh_tools_if_stale(&mut self) -> Result<Option<ToolListRefresh>> {
        if !self.service.take_tools_changed() {
            return Ok(None);
        }
        let tools = Self::fetch_tools(&self.server_name, self.service.as_ref()).await?;
        let before: BTreeSet<String> = self.tools.iter().map(|t| t.name.to_string()).collect();
        let before_hidden: BTreeSet<String> = self.hidden.iter().cloned().collect();
        let ExposedTools {
            tools,
            hidden,
            tool_specs,
            declared_read_only,
        } = derive_exposed(&self.server_name, tools, &self.policy);
        let after: BTreeSet<String> = tools.iter().map(|t| t.name.to_string()).collect();
        let after_hidden: BTreeSet<String> = hidden.iter().cloned().collect();
        let refresh = ToolListRefresh {
            added: after.difference(&before).cloned().collect(),
            removed: before.difference(&after).cloned().collect(),
            newly_hidden: after_hidden.difference(&before_hidden).cloned().collect(),
        };
        self.tools = tools;
        self.tool_specs = tool_specs;
        self.hidden = hidden;
        self.declared_read_only = declared_read_only;
        Ok(Some(refresh))
    }

    /// The server's `initialize` instructions, capped and trimmed.
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    pub fn list_tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Tool names hidden by this entry's `enabled_tools` / `disabled_tools`.
    pub fn hidden_tools(&self) -> &[String] {
        &self.hidden
    }

    /// The server's own `readOnlyHint: true` declarations, sorted.
    ///
    /// Display-only evidence — see the field's own note. Never consulted by
    /// [`McpServerPolicy::risk_for`].
    pub fn declared_read_only(&self) -> &[String] {
        &self.declared_read_only
    }

    /// This server's resolved tool policy.
    pub fn policy(&self) -> &McpServerPolicy {
        self.policy.as_ref()
    }

    async fn connect(
        server_name: &str,
        config: McpTransportConfig,
        timeout: Option<std::time::Duration>,
    ) -> Result<(RunningService<RoleClient, ToolListChangedSignal>, ToolListChangedSignal)> {
        let config = match config {
            McpTransportConfig::Stdio(config) => config,
            McpTransportConfig::Remote(remote) => {
                let signal = ToolListChangedSignal::default();
                let service = remote::serve_remote(server_name, &remote, signal.clone()).await?;
                return Ok((service, signal));
            }
        };
        let command = config.command;
        let args = config.args;
        let env = config.env;
        let env_vars = config.env_vars;
        let cwd = config.cwd;
        // Resolved before the spawn: an unset variable fails this server with a
        // message naming it, rather than starting a child that will fail later
        // somewhere unrelated.
        let inherited = resolve_env_vars(server_name, &env, &env_vars)?;
        let mut child_env = env;
        child_env.extend(inherited);
        // Capture and drain the server's stderr instead of inheriting it to
        // the terminal. Many stdio MCP servers log incidental progress (e.g.
        // index/recovery "Reconstruction complete") there; forwarding those
        // lines through tracing would still pollute the TUI when verbose logs
        // are enabled.
        let (transport, stderr) =
            TokioChildProcess::builder(Command::new(&command).configure(move |cmd| {
                cmd.args(&args).envs(&child_env);
                if let Some(cwd) = cwd.as_deref() {
                    cmd.current_dir(cwd);
                }
            }))
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn MCP server {server_name}"))?;

        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                drain_mcp_stderr(BufReader::new(stderr)).await;
            });
        }

        // The connection's handler records `notifications/tools/list_changed`
        // into the signal we keep, so a server that grows or drops a tool after
        // the handshake is not frozen at whatever it advertised then.
        let signal = ToolListChangedSignal::default();
        // Codex names this budget `startup_timeout_sec`; an entry that declares
        // it overrides the global default so a slow launcher (a cold `uvx`) is a
        // configuration problem, not a permanently failed server.
        let budget = timeout.unwrap_or(MCP_INIT_TIMEOUT);
        let service = tokio::time::timeout(budget, signal.clone().serve(transport))
            .await
            .with_context(|| {
                format!(
                    "MCP server {server_name} did not complete the handshake within {}s",
                    budget.as_secs()
                )
            })?
            .with_context(|| format!("failed to initialize MCP client for server {server_name}"))?;
        Ok((service, signal))
    }

    async fn fetch_tools(server_name: &str, service: &dyn McpService) -> Result<Vec<McpTool>> {
        tokio::time::timeout(MCP_LIST_TOOLS_TIMEOUT, service.list_all_tools())
            .await
            .with_context(|| {
                format!(
                    "MCP server {server_name} did not list its tools within {}s",
                    MCP_LIST_TOOLS_TIMEOUT.as_secs()
                )
            })?
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

        // The entry's `tool_timeout_sec` overrides the global ceiling for this
        // server; the error names whichever budget was actually applied.
        let budget = self.policy.tool_timeout().unwrap_or(MCP_CALL_TOOL_TIMEOUT);
        let result = tokio::time::timeout(
            budget,
            self.service.call_tool(CallToolRequestParams {
                meta: None,
                name: tool_name.to_string().into(),
                arguments,
                task: None,
            }),
        )
        .await
        .with_context(|| {
            format!(
                "MCP tool {tool_name} did not return within {}s",
                budget.as_secs()
            )
        })?
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
    /// Behind a lock so a test can grow the list the way a real server does
    /// after `notifications/tools/list_changed` — a fixed vec could only ever
    /// prove that a re-list was issued, not that it was used.
    tools: std::sync::Mutex<Vec<McpTool>>,
    handler: McpToolHandler,
    calls: std::sync::Mutex<Vec<(String, Value)>>,
    instructions: Option<String>,
    resources: Vec<Resource>,
    resource_templates: Vec<ResourceTemplate>,
    resource_text: HashMap<String, String>,
    /// Set by [`Self::announce_tools_changed`], read by
    /// [`McpService::take_tools_changed`].
    tools_changed: Arc<AtomicBool>,
    /// When set, `tools/list` fails — the shape a re-list takes when the
    /// transport went away between the notification and the request.
    list_tools_fails: bool,
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
            tools: std::sync::Mutex::new(tools),
            handler: Arc::new(handler),
            calls: std::sync::Mutex::new(Vec::new()),
            instructions: None,
            resources: Vec::new(),
            resource_templates: Vec::new(),
            resource_text: HashMap::new(),
            tools_changed: Arc::new(AtomicBool::new(false)),
            list_tools_fails: false,
        }
    }

    /// The tools this double currently advertises.
    fn tools(&self) -> Vec<McpTool> {
        self.tools
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replaces the advertised tools, as a server would when its list moves.
    pub fn set_tools(&self, tools: Vec<McpTool>) {
        *self.tools.lock().unwrap_or_else(|e| e.into_inner()) = tools;
    }

    /// Blows the `notifications/tools/list_changed` whistle.
    pub fn announce_tools_changed(&self) {
        self.tools_changed.store(true, Ordering::Release);
    }

    /// Makes `tools/list` fail, so a re-list can be tested against a server that
    /// announced a change and then went away.
    #[must_use]
    pub fn failing_to_list(mut self) -> Self {
        self.list_tools_fails = true;
        self
    }

    /// Publishes a readable text resource under `uri`.
    #[must_use]
    pub fn with_text_resource(mut self, uri: &str, name: &str, text: &str) -> Self {
        self.resources
            .push(Resource::new(RawResource::new(uri, name), None));
        self.resource_text.insert(uri.to_string(), text.to_string());
        self
    }

    /// Publishes a URI *template* this server can serve.
    ///
    /// A template-only server is the case `resources/list` cannot describe: it
    /// answers with nothing, and the model needs the placeholder vocabulary
    /// before it can read anything.
    #[must_use]
    pub fn with_resource_template(mut self, uri_template: &str, name: &str) -> Self {
        self.resource_templates.push(rmcp::model::ResourceTemplate::new(
            rmcp::model::RawResourceTemplate {
                uri_template: uri_template.to_string(),
                name: name.to_string(),
                title: None,
                description: None,
                mime_type: None,
                icons: None,
            },
            None,
        ));
        self
    }

    /// Adds an `InitializeResult.instructions` payload for this server.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Return every `(tool_name, arguments)` pair received so far.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl McpService for MockMcpService {
    fn list_all_tools(&self) -> BoxFuture<'_, Result<Vec<McpTool>, ServiceError>> {
        if self.list_tools_fails {
            return std::future::ready(Err(ServiceError::TransportClosed)).boxed();
        }
        let tools = self.tools();
        std::future::ready(Ok(tools)).boxed()
    }

    fn take_tools_changed(&self) -> bool {
        self.tools_changed.swap(false, Ordering::AcqRel)
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

    fn instructions(&self) -> Option<String> {
        self.instructions.clone()
    }

    fn list_resources(&self) -> BoxFuture<'_, Result<Vec<Resource>, ServiceError>> {
        let resources = self.resources.clone();
        std::future::ready(Ok(resources)).boxed()
    }

    fn list_resource_templates(
        &self,
    ) -> BoxFuture<'_, Result<Vec<ResourceTemplate>, ServiceError>> {
        let templates = self.resource_templates.clone();
        std::future::ready(Ok(templates)).boxed()
    }

    fn read_resource(
        &self,
        uri: String,
    ) -> BoxFuture<'_, Result<ReadResourceResult, ServiceError>> {
        // A missing URI is a real not-found error, not an empty result: the two
        // must not be confusable in a test.
        let result = match self.resource_text.get(&uri) {
            Some(text) => Ok(ReadResourceResult {
                contents: vec![rmcp::model::ResourceContents::text(
                    text.clone(),
                    uri.clone(),
                )],
            }),
            None => Err(ServiceError::McpError(
                rmcp::model::ErrorData::resource_not_found(
                    format!("no such resource: {uri}"),
                    None,
                ),
            )),
        };
        std::future::ready(result).boxed()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolName {
    pub server: String,
    pub tool: String,
}

/// Builds the `mcp__<server>__<tool>` name the agent calls.
///
/// One place, because routing keys on this exact spelling: a second literal
/// (the hook path needs the name too) would eventually differ from this one,
/// and the failure would look like "the tool does not exist".
#[must_use]
pub fn mcp_tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

impl McpToolName {
    /// The namespaced name, the inverse of `TryFrom<&str>`.
    #[must_use]
    pub fn full_name(&self) -> String {
        mcp_tool_name(&self.server, &self.tool)
    }
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

    /// Whether the server's entry declares `approval_mode: "auto"` for a tool.
    ///
    /// False for an unknown server or tool, so an unresolvable name keeps the
    /// default ask behaviour instead of becoming auto-approved by accident.
    #[must_use]
    pub fn is_auto_approved(&self, server: &str, tool: &str) -> bool {
        self.clients
            .get(server)
            .is_some_and(|client| client.policy().is_auto_approved(tool))
    }

    /// The risk to report for one of this server's tools.
    ///
    /// `self.clients.get(server).and_then(risk_for)` when the entry declares a
    /// tier, and [`normalize_mcp_capability`] otherwise — so "the entry is
    /// silent" and "the entry said high" end up the same, while only an
    /// explicit declaration can lower it. An unknown server or tool keeps the
    /// default rather than becoming approved by accident.
    #[must_use]
    pub fn risk_for(&self, server: &str, tool: &str) -> CapabilityRisk {
        self.clients
            .get(server)
            .and_then(|client| client.policy().risk_for(tool))
            .unwrap_or_else(|| normalize_mcp_capability(server, tool))
    }

    /// Re-lists every server that announced `notifications/tools/list_changed`.
    ///
    /// Quiet servers are untouched — no request is sent — so the cost of this
    /// pass is zero until a server actually says something. A server that
    /// announced a change and then failed to answer keeps its previous list,
    /// and is reported rather than silently emptied.
    pub async fn refresh_changed(&mut self) -> ToolListReport {
        let mut report = ToolListReport::default();
        for (server, client) in self.clients.iter_mut() {
            match client.refresh_tools_if_stale().await {
                Ok(None) => {}
                Ok(Some(refresh)) if refresh.is_empty() => {}
                Ok(Some(refresh)) => report.changed.push((server.clone(), refresh)),
                Err(err) => report.failed.push((server.clone(), format!("{err:#}"))),
            }
        }
        report.changed.sort_by(|a, b| a.0.cmp(&b.0));
        report.failed.sort();
        report
    }

    /// The per-tool result budget this server's entry declares, by full tool
    /// name (`mcp__<server>__<tool>`).
    #[must_use]
    pub fn output_token_limit(&self, full_name: &str) -> Option<usize> {
        let parsed = McpToolName::try_from(full_name).ok()?;
        self.clients
            .get(&parsed.server)
            .and_then(|client| client.policy().output_token_limit(&parsed.tool))
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

    /// The `InitializeResult.instructions` of every connected server, as one
    /// markdown body for the system prompt — empty when no server sent any.
    ///
    /// Servers are emitted in name order so the block is deterministic (the
    /// system prompt sits before the KV-cache boundary; an unstable body would
    /// invalidate the cached prefix on every render).
    ///
    /// A server whose tools are all filtered out is skipped: its guidance is
    /// about tools the agent cannot call, and `mcp list` already reports the
    /// filter. A server that sends no tools at all is skipped for the same
    /// reason.
    pub fn instructions_block(&self) -> String {
        let mut servers: Vec<(&str, &str)> = self
            .clients
            .values()
            .filter(|client| !client.tools.is_empty())
            .filter_map(|client| {
                client
                    .instructions()
                    .map(|text| (client.server_name.as_str(), text))
            })
            .collect();
        servers.sort_by(|a, b| a.0.cmp(b.0));

        let mut sections = Vec::with_capacity(servers.len());
        for (server, text) in servers {
            sections.push(format!("## {server}\n\n{text}"));
        }
        sections.join("\n\n")
    }

    pub async fn disconnect_all(&mut self) {
        for (_, client) in self.clients.drain() {
            client.shutdown().await;
        }
    }
}

/// Applies the entry's tool filter and derives everything the client keeps.
///
/// One place, because `assemble` (at connect) and `refresh_tools_if_stale`
/// (after `notifications/tools/list_changed`) must not disagree about which
/// tools a server exposes — a disagreement would be invisible, and `hidden` /
/// `declared_read_only` exist precisely to make that decision visible.
struct ExposedTools {
    tools: Vec<McpTool>,
    hidden: Vec<String>,
    tool_specs: Vec<ToolSpec>,
    declared_read_only: Vec<String>,
}

fn derive_exposed(
    server_name: &str,
    tools: Vec<McpTool>,
    policy: &McpServerPolicy,
) -> ExposedTools {
    let mut exposed = Vec::new();
    let mut hidden = Vec::new();
    for tool in tools {
        if policy.exposes(&tool.name) {
            exposed.push(tool);
        } else {
            hidden.push(tool.name.to_string());
        }
    }
    hidden.sort();
    // Read the server's own read-only declarations off the tools we are about
    // to keep. Only *exposed* tools matter: a tool the entry filtered away is
    // not something the human can be asked to declare a risk for.
    let mut declared_read_only: Vec<String> = exposed
        .iter()
        .filter(|tool| {
            tool.annotations
                .as_ref()
                .and_then(|a| a.read_only_hint)
                .unwrap_or(false)
        })
        .map(|tool| tool.name.to_string())
        .collect();
    declared_read_only.sort();
    declared_read_only.dedup();
    let tool_specs = build_tool_specs(server_name, &exposed);
    ExposedTools {
        tools: exposed,
        hidden,
        tool_specs,
        declared_read_only,
    }
}

/// What one server's tool list did when it was re-read.
///
/// Names rather than counts: "21 → 21" is not a debuggable fact, and a server
/// that swaps one tool for another would otherwise look unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolListRefresh {
    /// Tools the agent can now call and could not before.
    pub added: Vec<String>,
    /// Tools the agent can no longer call.
    pub removed: Vec<String>,
    /// Tools that became hidden by this entry's `enabled_tools` /
    /// `disabled_tools` — not callable, but not the server's doing either.
    pub newly_hidden: Vec<String>,
}

impl ToolListRefresh {
    /// Whether the re-read changed anything the log is worth showing.
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.newly_hidden.is_empty()
    }

    /// The phrase a report line carries.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("added {}", self.added.join(", ")));
        }
        if !self.removed.is_empty() {
            parts.push(format!("removed {}", self.removed.join(", ")));
        }
        if !self.newly_hidden.is_empty() {
            parts.push(format!("now hidden {}", self.newly_hidden.join(", ")));
        }
        if parts.is_empty() {
            parts.push("no visible change".to_string());
        }
        parts.join("; ")
    }
}

/// The outcome of one refresh pass over every connected server.
#[derive(Debug, Clone, Default)]
pub struct ToolListReport {
    /// Servers whose tool list was re-read, with what moved.
    pub changed: Vec<(String, ToolListRefresh)>,
    /// Servers that announced a change and could not be re-listed. Their
    /// previous list is kept: a transient error must not leave a working
    /// server looking like a server with no tools.
    pub failed: Vec<(String, String)>,
}

impl ToolListReport {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.failed.is_empty()
    }
}

fn build_tool_specs(server_name: &str, tools: &[McpTool]) -> Vec<ToolSpec> {
    tools
        .iter()
        .map(|tool| ToolSpec {
            name: mcp_tool_name(server_name, &tool.name),
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
/// A compatibility source is read *before* the native files so it can never
/// silently outrank a declaration the user wrote in `.tact/.mcp.json`.
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

    // 1. The Claude Code project file at the working directory. It is read
    //    first — lowest precedence — so a repository can never silently
    //    outrank the user's own declarations.
    //
    //    Unlike the native files below, this one belongs to the project rather
    //    than to the user, so an unreadable file is skipped with a warning
    //    instead of aborting the load: otherwise cloning a repository with a
    //    broken `.mcp.json` would stop Tact from starting in it at all.
    let foreign = cwd.join(".mcp.json");
    match McpConfigFile::read(&foreign) {
        Ok(Some(file)) => {
            let source = foreign.display().to_string();
            for (name, config) in file.mcp_servers {
                servers.push(SourcedServer {
                    name,
                    source: source.clone(),
                    config,
                });
            }
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(
            path = %foreign.display(),
            "ignoring an unreadable .mcp.json in the working directory: {error:#}"
        ),
    }

    // 2. Native user config, then 3. native project config.
    if let Some(path) = TactPath::home_mcp_config_path() {
        push_file(&path, &mut servers)?;
    }
    push_file(&TactPath::new(cwd).mcp_config_path(), &mut servers)?;

    // 4. Installed plugins. A plugin is the only remaining multi-file bundle
    // source; there is no cwd-level `.codex-plugin/plugin.json` read: an
    // installed plugin is a package the user opted into, and a plugin's own
    // `.mcp.json` is read from its bundle, never from the working directory.
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
    /// Declarations switched off with `enabled: false`, with the source that
    /// won them.
    ///
    /// Separate from [`Self::servers`] because that list is the connect list,
    /// but still *resolved*: they take part in shadowing and are described by
    /// `mcp list`. A name switched off in one source and on in another reports
    /// the winner instead of appearing twice.
    disabled: Vec<(String, McpTransportConfig, String)>,
    /// Overridden server name and the source it displaced.
    shadowed: Vec<(String, String)>,
    /// Servers dropped for an unsupported or incomplete transport.
    skipped_remote: Vec<String>,
    /// Entries that declare keys Tact does not model, in declaration order.
    unmodelled: Vec<UnmodelledKeys>,
    /// Winning per-entry tool policy by server name.
    ///
    /// Kept beside the connect list rather than inside it so the
    /// `(name, transport, source)` tuples every caller already matches on stay
    /// unchanged.
    policies: HashMap<String, McpServerPolicy>,
}

impl ResolvedServers {
    /// The winning declaration's tool policy for `server_name`.
    ///
    /// An unknown name — or an entry that declared nothing — gets the empty
    /// policy, so callers never branch on `Option`.
    fn policy_for(&self, server_name: &str) -> McpServerPolicy {
        self.policies.get(server_name).cloned().unwrap_or_default()
    }

    /// Describes each server for diagnostics (`tact-ui mcp list`).
    ///
    /// Sorted by name: `.mcp.json` is parsed into a `HashMap`, so there is no
    /// meaningful declaration order to preserve and an unstable listing would
    /// make repeated runs needlessly hard to compare.
    fn configured(&self) -> Vec<ConfiguredServer> {
        let active = self
            .servers
            .iter()
            .map(|(name, transport, source)| (name, transport, source, false));
        let switched_off = self
            .disabled
            .iter()
            .map(|(name, transport, source)| (name, transport, source, true));
        let mut described: Vec<ConfiguredServer> = active
            .chain(switched_off)
            .map(|(name, transport, source, disabled)| ConfiguredServer {
                name: name.clone(),
                transport: transport_kind(transport),
                source: source.clone(),
                disabled,
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
    let mut order: Vec<Resolution> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut shadowed: Vec<(String, String)> = Vec::new();
    let mut skipped_remote: Vec<String> = Vec::new();
    let mut unmodelled: Vec<UnmodelledKeys> = Vec::new();

    for SourcedServer {
        name,
        source,
        config,
    } in servers
    {
        if let Some(entry) = unmodelled_keys(&name, &source, &config) {
            unmodelled.push(entry);
        }

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
        if config.enabled {
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
        } else {
            tracing::debug!(
                mcp_server = %name, source = %source,
                "MCP server is switched off by its own `enabled: false`"
            );
        }
        let resolution = Resolution {
            name: name.clone(),
            disabled: !config.enabled,
            transport,
            source,
            policy: McpServerPolicy::from_config(&config),
        };
        match index_of.get(&name) {
            Some(&existing) => {
                shadowed.push((name, order[existing].source.clone()));
                order[existing] = resolution;
            }
            None => {
                index_of.insert(name, order.len());
                order.push(resolution);
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

    let mut servers = Vec::new();
    let mut disabled = Vec::new();
    let mut policies: HashMap<String, McpServerPolicy> = HashMap::new();
    for resolution in order {
        policies.insert(resolution.name.clone(), resolution.policy);
        let entry = (resolution.name, resolution.transport, resolution.source);
        if resolution.disabled {
            disabled.push(entry);
        } else {
            servers.push(entry);
        }
    }

    ResolvedServers {
        servers,
        disabled,
        policies,
        shadowed,
        skipped_remote,
        unmodelled,
    }
}

/// One name's winning declaration, before the connect list and the disabled
/// list are split apart.
struct Resolution {
    name: String,
    transport: McpTransportConfig,
    source: String,
    /// Declared with `enabled: false`: listed, never connected.
    disabled: bool,
    /// The Codex per-entry tool policy of the winning declaration.
    policy: McpServerPolicy,
}

/// Describes the entry keys Tact does not model, or `None` when there are none.
///
/// Silently ignoring configuration is the failure this exists to prevent: a
/// Codex entry can declare `omit_tools_from`, or `env_vars` on a server that has
/// no child process, and Tact would otherwise look as if it honored them. The keys are both logged and
/// returned so `mcp list` can name them — the log subscriber is only installed
/// when `RUST_LOG` (or `tokio_console`) asks for it, so a warning alone would be
/// invisible to a default run.
fn unmodelled_keys(name: &str, source: &str, config: &McpProjectConfig) -> Option<UnmodelledKeys> {
    let mut keys: Vec<String> = config.extra.keys().cloned().collect();
    // `env_vars` is modelled, but only for stdio: a remote entry has no child
    // process to put the variables in, so declaring them there is the same kind
    // of silence `extra` exists to prevent.
    if !config.env_vars.is_empty() && config.is_remote() {
        keys.push("env_vars".to_string());
    }
    if keys.is_empty() {
        return None;
    }
    keys.sort_unstable();
    tracing::warn!(
        mcp_server = %name,
        source = %source,
        keys = %keys.join(", "),
        "MCP entry declares keys Tact does not model; ignoring them"
    );
    Some(UnmodelledKeys {
        server: name.to_owned(),
        source: source.to_owned(),
        keys,
    })
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
        unmodelled: resolved.unmodelled,
        ..McpLoadReport::default()
    };

    let policies = resolved.policies.clone();
    let mut router = MCPToolRouter::new();
    let mut connections = FuturesUnordered::new();
    for (server_name, config, _source) in resolved.servers {
        let policy = policies.get(&server_name).cloned().unwrap_or_default();
        connections.push(async move {
            let outcome = connect_server(&server_name, config, policy).await;
            (server_name, outcome)
        });
    }
    while let Some((server_name, outcome)) = connections.next().await {
        match outcome {
            ConnectOutcome::Connected(client) => {
                let tools = client.list_tools().len();
                tracing::debug!(mcp_server = %server_name, tools, "MCP server connected");
                report.connected.push((server_name.clone(), tools));
                if !client.hidden_tools().is_empty() {
                    report
                        .filtered
                        .push((server_name.clone(), client.hidden_tools().to_vec()));
                }
                router.register_client(client);
            }
            ConnectOutcome::NeedsAuthorization => report.pending_auth.push(server_name),
            ConnectOutcome::Failed(error) => report.failures.push((server_name, error)),
        }
    }

    report.connected.sort_by(|a, b| a.0.cmp(&b.0));
    report.failures.sort_by(|a, b| a.0.cmp(&b.0));
    report.shadowed.sort_by(|a, b| a.0.cmp(&b.0));
    report.filtered.sort_by(|a, b| a.0.cmp(&b.0));
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
) -> Result<Option<(ConfiguredServer, McpTransportConfig, McpServerPolicy)>> {
    let resolved = resolve_current()?;
    let described = resolved
        .configured()
        .into_iter()
        .find(|server| server.name == server_name);
    let transport = resolved
        .servers
        .iter()
        .chain(resolved.disabled.iter())
        .find(|(name, _, _)| name == server_name)
        .map(|(_, transport, _)| transport.clone());
    let policy = resolved.policy_for(server_name);
    Ok(match (described, transport) {
        (Some(server), Some(transport)) => Some((server, transport, policy)),
        _ => None,
    })
}

/// The per-status facts [`inspect_server`] fills.
///
/// A named struct rather than a tuple: the three outcomes differ only in which
/// facts they leave empty, and counting tuple positions made that unreadable
/// once `declared_read_only` and `declared_risks` joined.
struct InspectionFacts {
    status: McpServerStatus,
    tools: Vec<String>,
    filtered: Vec<String>,
    instructions_chars: Option<usize>,
    resources: Option<usize>,
    resource_templates: Option<usize>,
    declared_read_only: Vec<String>,
    declared_risks: Vec<(String, CapabilityRisk)>,
}

impl InspectionFacts {
    /// Nothing but the status: what a server that never connected can report.
    fn empty(status: McpServerStatus) -> Self {
        Self {
            status,
            tools: Vec::new(),
            filtered: Vec::new(),
            instructions_chars: None,
            resources: None,
            resource_templates: None,
            declared_read_only: Vec::new(),
            declared_risks: Vec::new(),
        }
    }

    fn into_inspection(self, server: ConfiguredServer) -> McpServerInspection {
        McpServerInspection {
            server,
            status: self.status,
            tools: self.tools,
            filtered: self.filtered,
            instructions_chars: self.instructions_chars,
            resources: self.resources,
            resource_templates: self.resource_templates,
            declared_read_only: self.declared_read_only,
            declared_risks: self.declared_risks,
        }
    }
}

/// Connects one server and reports its state, without touching the others.
///
/// `mcp get <name>` uses this so inspecting a single server never spawns or
/// dials the rest of the configuration. Returns `Ok(None)` for an unknown name.
pub async fn inspect_server(server_name: &str) -> Result<Option<McpServerInspection>> {
    let Some((server, transport, policy)) = resolved_server_for(server_name)? else {
        return Ok(None);
    };
    if server.disabled {
        return Ok(Some(
            InspectionFacts::empty(McpServerStatus::Disabled).into_inspection(server),
        ));
    }
    let facts = match connect_server(server_name, transport, policy).await {
        ConnectOutcome::Connected(client) => {
            let tools = client
                .list_tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            let filtered = client.hidden_tools().to_vec();
            let chars = client.instructions().map(|text| text.chars().count());
            // A server without resource support is expected, not an error,
            // so a failed listing is reported as "did not answer" rather
            // than failing the inspection.
            let resources = client.list_resources().await.ok().map(|list| list.len());
            // Same reasoning for templates: a server that does not answer and a
            // server that publishes none must not look alike.
            let resource_templates = client
                .list_resource_templates()
                .await
                .ok()
                .map(|list| list.len());
            // Only *declared* tiers, so "the entry is silent" and "the entry
            // said high" stay distinguishable in the printed view.
            let mut declared_risks: Vec<(String, CapabilityRisk)> = client
                .list_tools()
                .iter()
                .filter_map(|tool| {
                    client
                        .policy()
                        .risk_for(&tool.name)
                        .map(|risk| (tool.name.to_string(), risk))
                })
                .collect();
            declared_risks.sort_by(|a, b| a.0.cmp(&b.0));
            InspectionFacts {
                status: McpServerStatus::Connected,
                tools,
                filtered,
                instructions_chars: chars,
                resources,
                resource_templates,
                declared_read_only: client.declared_read_only().to_vec(),
                declared_risks,
            }
        }
        ConnectOutcome::NeedsAuthorization => {
            InspectionFacts::empty(McpServerStatus::PendingAuthorization)
        }
        ConnectOutcome::Failed(error) => InspectionFacts::empty(McpServerStatus::Failed(error)),
    };
    Ok(Some(facts.into_inspection(server)))
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
        path::Path,
        sync::Arc,
    };

    use rmcp::{
        ErrorData as McpError, ServerHandler, ServiceExt,
        model::{
            CallToolResult, Content, JsonObject, ListToolsResult, ServerInfo, Tool as McpTool,
            ToolAnnotations,
        },
        service::{RequestContext, RoleServer},
    };
    use serde_json::json;

    use super::{
        ApprovalMode, MCP_INSTRUCTIONS_MAX_CHARS, MCPToolRouter, McpAuthConfig, McpClient,
        McpConfigFile, McpEnvVar, McpLiveStatus, McpLoadReport, McpProjectConfig, McpServerConfig,
        McpServerPolicy, McpToolConfig, McpToolName, McpTransportConfig, MockMcpService,
        PluginDirs, PluginManifest, PluginRoot, RealMcpService, SourcedServer, ToolListChangedSignal,
        ToolListRefresh, ToolRisk, UnmodelledKeys, cap_instructions, collect_plugin_mcp_servers,
        collect_sourced_servers, describe_resolved, drain_mcp_stderr,
        installed_plugin_mcp_servers, plugin_manifest_mcp_servers, prepare_plugin_entry,
        resolve_env_vars, resolve_servers, unmodelled_keys,
    };

    use crate::{
        consts::{PluginHome, TactPath},
        permission::CapabilityRisk,
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
            env_vars: Vec::new(),
            cwd: None,
        };

        assert_eq!(manifest.name, "demo");
        assert_eq!(manifest.version.as_deref(), Some("1.0.0"));
        let servers =
            plugin_manifest_mcp_servers(Path::new("/plugins/demo"), manifest.mcp_servers.as_ref())
                .expect("an inline map is a valid declaration");
        let echo = servers.get("echo").expect("echo is declared");
        assert_eq!(echo.command.as_deref(), Some(expected.command.as_str()));
        assert_eq!(echo.args, expected.args);
        assert_eq!(echo.env, expected.env);
    }

    /// The `(root, data)` pair for a throwaway plugin root.
    fn plugin_dirs(root: &Path) -> PluginDirs {
        PluginDirs {
            root: root.to_path_buf(),
            data: root.join("data"),
        }
    }

    #[test]
    fn plugin_placeholders_expand_in_args_env_and_cwd() {
        let dirs = plugin_dirs(Path::new("/plugins/demo"));
        let config = McpProjectConfig {
            command: Some("./bin/server".to_owned()),
            args: vec!["--data".to_owned(), "${PLUGIN_DATA}/db".to_owned()],
            env: HashMap::from([("CONFIG".to_owned(), "${PLUGIN_ROOT}/config.json".to_owned())]),
            cwd: Some("${PLUGIN_ROOT}/sub".to_owned()),
            ..McpProjectConfig::default()
        };

        let prepared = prepare_plugin_entry(&config, &dirs).expect("a valid entry");

        assert_eq!(
            prepared.command.as_deref(),
            Some("/plugins/demo/bin/server")
        );
        assert_eq!(prepared.args[1], "/plugins/demo/data/db");
        assert_eq!(prepared.env["CONFIG"], "/plugins/demo/config.json");
        assert_eq!(prepared.cwd.as_deref(), Some("/plugins/demo/sub"));
        // §9.1: the client supplies the two reserved variables itself.
        assert_eq!(prepared.env["PLUGIN_ROOT"], "/plugins/demo");
        assert_eq!(prepared.env["PLUGIN_DATA"], "/plugins/demo/data");
    }

    #[test]
    fn an_absent_cwd_defaults_to_the_plugin_root() {
        let dirs = plugin_dirs(Path::new("/plugins/demo"));
        let config = McpProjectConfig {
            command: Some("node".to_owned()),
            ..McpProjectConfig::default()
        };

        let prepared = prepare_plugin_entry(&config, &dirs).expect("a valid entry");

        assert_eq!(prepared.cwd.as_deref(), Some("/plugins/demo"));
    }

    #[test]
    fn a_cwd_outside_the_three_allowed_forms_is_rejected() {
        let dirs = plugin_dirs(Path::new("/plugins/demo"));
        for raw in [
            "data",
            "../elsewhere",
            "${PLUGIN_ROOT}/../elsewhere",
            "${PLUGIN_DATA}/../../etc",
            "~/somewhere",
        ] {
            let config = McpProjectConfig {
                command: Some("run".to_owned()),
                cwd: Some(raw.to_owned()),
                ..McpProjectConfig::default()
            };
            assert!(
                prepare_plugin_entry(&config, &dirs).is_none(),
                "cwd {raw:?} must invalidate the entry"
            );
        }
    }

    #[test]
    fn a_relative_cwd_stays_inside_the_plugin_root() {
        let dirs = plugin_dirs(Path::new("/plugins/demo"));
        let config = McpProjectConfig {
            command: Some("run".to_owned()),
            cwd: Some("./nested/dir".to_owned()),
            ..McpProjectConfig::default()
        };

        let prepared = prepare_plugin_entry(&config, &dirs).expect("a valid entry");

        assert_eq!(prepared.cwd.as_deref(), Some("/plugins/demo/nested/dir"));
    }

    #[test]
    fn a_command_escaping_the_plugin_root_is_rejected() {
        let dirs = plugin_dirs(Path::new("/plugins/demo"));
        let escaping = McpProjectConfig {
            command: Some("./../bin/server".to_owned()),
            ..McpProjectConfig::default()
        };
        assert!(prepare_plugin_entry(&escaping, &dirs).is_none());

        // A bare name is the platform's executable search, never tact's business.
        let bare = McpProjectConfig {
            command: Some("node".to_owned()),
            ..McpProjectConfig::default()
        };
        assert_eq!(
            prepare_plugin_entry(&bare, &dirs)
                .expect("a valid entry")
                .command
                .as_deref(),
            Some("node")
        );
    }

    #[test]
    fn a_reserved_env_name_invalidates_the_entry() {
        let dirs = plugin_dirs(Path::new("/plugins/demo"));
        for name in ["PLUGIN_ROOT", "PLUGIN_DATA"] {
            let config = McpProjectConfig {
                command: Some("run".to_owned()),
                env: HashMap::from([(name.to_owned(), "not yours".to_owned())]),
                ..McpProjectConfig::default()
            };
            assert!(
                prepare_plugin_entry(&config, &dirs).is_none(),
                "{name} is reserved for the client"
            );
        }
    }

    #[test]
    fn an_unknown_placeholder_stays_literal() {
        let dirs = plugin_dirs(Path::new("/plugins/demo"));
        let config = McpProjectConfig {
            command: Some("run".to_owned()),
            args: vec!["${OTHER}/x".to_owned()],
            ..McpProjectConfig::default()
        };

        let prepared = prepare_plugin_entry(&config, &dirs).expect("a valid entry");

        assert_eq!(prepared.args[0], "${OTHER}/x");
    }

    #[test]
    fn a_replacement_is_never_rescanned() {
        // A root whose own path contains the data placeholder: a second pass
        // would expand inside the text the first pass produced.
        let tricky = Path::new("/plugins/${PLUGIN_DATA}/loop");
        let dirs = plugin_dirs(tricky);
        let config = McpProjectConfig {
            command: Some("run".to_owned()),
            args: vec!["${PLUGIN_ROOT}/x".to_owned()],
            ..McpProjectConfig::default()
        };

        let prepared = prepare_plugin_entry(&config, &dirs).expect("a valid entry");

        assert_eq!(prepared.args[0], "/plugins/${PLUGIN_DATA}/loop/x");
    }

    #[test]
    fn a_manifest_mcp_servers_path_is_read_from_the_plugin_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"echo":{"command":"node"}}}"#,
        )
        .unwrap();
        let manifest: PluginManifest =
            serde_json::from_str(r#"{"name":"demo","mcpServers":"./.mcp.json"}"#).unwrap();

        let servers =
            plugin_manifest_mcp_servers(dir.path(), manifest.mcp_servers.as_ref()).unwrap();

        assert_eq!(
            servers
                .get("echo")
                .expect("echo is declared")
                .command
                .as_deref(),
            Some("node")
        );
    }

    #[test]
    fn a_manifest_mcp_servers_path_may_not_escape_the_plugin_root() {
        let dir = tempfile::tempdir().unwrap();
        let manifest: PluginManifest =
            serde_json::from_str(r#"{"name":"demo","mcpServers":"../outside.json"}"#).unwrap();

        assert!(plugin_manifest_mcp_servers(dir.path(), manifest.mcp_servers.as_ref()).is_err());
    }

    #[test]
    fn a_plugin_root_mcp_file_is_read_under_both_names() {
        for file_name in [".mcp.json", "mcp.json"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join(file_name),
                r#"{"mcpServers":{"echo":{"command":"node","args":["${PLUGIN_DATA}/x"]}}}"#,
            )
            .unwrap();
            let root = PluginRoot {
                plugin_id: "demo".to_owned(),
                marketplace: "mk".to_owned(),
                root: dir.path().to_path_buf(),
            };
            let dirs = plugin_dirs(dir.path());
            let mut servers = Vec::new();

            collect_plugin_mcp_servers(&root, &dirs, &mut servers).unwrap();

            assert_eq!(servers.len(), 1, "{file_name}");
            assert_eq!(servers[0].0, "plugin__demo__echo");
            assert_eq!(
                servers[0].1.args[0],
                format!("{}/data/x", dir.path().display())
            );
            // §9.1: a stdio entry's data directory exists before the spawn.
            assert!(dirs.data.is_dir(), "{file_name}");
        }
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
    fn plugin_mcp_document_accepts_codex_wrapper_and_flat_map() {
        let wrapped = r#"{"mcpServers":{"srv":{"command":"cat"}}}"#;
        let flat = r#"{"srv":{"command":"cat"}}"#;
        for raw in [wrapped, flat] {
            let configs = super::parse_plugin_mcp_document(raw).unwrap();
            assert_eq!(configs["srv"].command.as_deref(), Some("cat"), "raw: {raw}");
        }
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

    /// A client whose service carries `instructions`, for the prompt-block tests.
    fn client_with_instructions(
        server: &str,
        tools: Vec<McpTool>,
        instructions: &str,
    ) -> McpClient {
        let service = MockMcpService::new(tools, |_| {
            Ok(CallToolResult::success(vec![Content::text("ok")]))
        })
        .with_instructions(instructions);
        let tools = service.tools();
        McpClient::with_service(server, tools, Arc::new(service))
    }

    #[test]
    fn instructions_are_captured_from_the_service_and_trimmed() {
        let client = client_with_instructions("demo", vec![echo_tool()], "  Use me well.\n");
        assert_eq!(client.instructions(), Some("Use me well."));
    }

    #[test]
    fn whitespace_only_instructions_are_absent_not_empty() {
        // A server that sends `""` is indistinguishable from one that sends
        // nothing; a fenced empty section would only cost tokens.
        let client = client_with_instructions("demo", vec![echo_tool()], "   \n\t ");
        assert_eq!(client.instructions(), None);
        assert_eq!(MCPToolRouter::new().instructions_block(), "");
    }

    #[test]
    fn the_instructions_block_is_server_sorted_and_headed() {
        let mut router = MCPToolRouter::new();
        router.register_client(client_with_instructions(
            "zeta",
            vec![echo_tool()],
            "Zeta guidance.",
        ));
        router.register_client(client_with_instructions(
            "alpha",
            vec![echo_tool()],
            "Alpha guidance.",
        ));

        assert_eq!(
            router.instructions_block(),
            "## alpha\n\nAlpha guidance.\n\n## zeta\n\nZeta guidance."
        );
    }

    #[test]
    fn a_server_with_every_tool_filtered_contributes_no_instructions() {
        // The guidance describes tools the agent cannot call, and `mcp list`
        // already reports the filter.
        let service = MockMcpService::new(vec![echo_tool()], |_| {
            Ok(CallToolResult::success(Vec::new()))
        })
        .with_instructions("You can echo things.");
        let config: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","enabled_tools":["other"]}"#).unwrap();
        let client = McpClient::with_service_and_policy(
            "demo",
            vec![echo_tool()],
            Arc::new(service),
            McpServerPolicy::from_config(&config),
        );
        assert_eq!(client.instructions(), Some("You can echo things."));

        let mut router = MCPToolRouter::new();
        router.register_client(client);
        assert_eq!(router.instructions_block(), "");
    }

    #[test]
    fn oversized_instructions_are_capped_with_a_marker() {
        let long = "x".repeat(MCP_INSTRUCTIONS_MAX_CHARS + 10);
        let capped = cap_instructions(&long);

        assert!(capped.starts_with(&"x".repeat(64)));
        assert!(capped.ends_with(&format!(
            "… (truncated at {MCP_INSTRUCTIONS_MAX_CHARS} characters)"
        )));
        // The kept prefix is exactly the ceiling — not one character more.
        assert_eq!(
            capped
                .lines()
                .next()
                .unwrap()
                .chars()
                .filter(|c| *c == 'x')
                .count(),
            MCP_INSTRUCTIONS_MAX_CHARS
        );
    }

    #[test]
    fn instructions_at_the_cap_are_left_alone() {
        let exact = "y".repeat(MCP_INSTRUCTIONS_MAX_CHARS);
        assert_eq!(cap_instructions(&exact), exact);
        assert!(!cap_instructions(&exact).contains("truncated"));
    }

    #[test]
    fn capping_counts_characters_not_bytes() {
        // A multi-byte payload must not be sliced mid-codepoint.
        let cjk = "記".repeat(MCP_INSTRUCTIONS_MAX_CHARS + 5);
        let capped = cap_instructions(&cjk);
        assert!(capped.ends_with("characters)"));
        assert_eq!(
            capped.lines().next().unwrap().chars().count(),
            MCP_INSTRUCTIONS_MAX_CHARS
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
        /// Mutable, because `notifications/tools/list_changed` is only
        /// meaningful if the list can move after the handshake.
        tools: std::sync::Mutex<Vec<McpTool>>,
        /// What a `reveal` call appends. The test chooses, so one fixture covers
        /// a plain newcomer, a server-declared read-only one, and one this
        /// entry's filter hides.
        revealed: Vec<McpTool>,
        /// Counts `tools/list` requests, so a test can prove a quiet server is
        /// not polled.
        list_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl EchoServer {
        fn new(tools: Vec<McpTool>) -> Self {
            Self {
                tools: std::sync::Mutex::new(tools),
                revealed: Vec::new(),
                list_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }

        /// The shared `tools/list` counter, taken before the server is moved
        /// into its task.
        fn list_calls(&self) -> Arc<std::sync::atomic::AtomicUsize> {
            self.list_calls.clone()
        }

        /// Reveals `tools` when a client calls `reveal`.
        fn revealing(mut self, tools: Vec<McpTool>) -> Self {
            self.revealed = tools;
            self
        }

        fn tools(&self) -> Vec<McpTool> {
            self.tools
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl ServerHandler for EchoServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::default()
        }

        fn get_tool(&self, name: &str) -> Option<McpTool> {
            self.tools().iter().find(|t| t.name == name).cloned()
        }

        fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_
        {
            self.list_calls
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            std::future::ready(Ok(ListToolsResult::with_all_items(self.tools())))
        }

        async fn call_tool(
            &self,
            request: rmcp::model::CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, McpError> {
            // `reveal` is the fixture's stand-in for a server that finishes an
            // auth or indexing pass and grows its tool list. Announcing it is
            // the point: the client must learn the list moved without being
            // told to look again.
            if request.name.as_ref() == "reveal" {
                self.tools
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend(self.revealed.iter().cloned());
                return match context.peer.notify_tool_list_changed().await {
                    Ok(()) => Ok(CallToolResult::success(vec![Content::text("revealed")])),
                    Err(err) => Err(McpError::internal_error(
                        format!("fixture failed to notify: {err}"),
                        None,
                    )),
                };
            }
            let text = request
                .arguments
                .as_ref()
                .and_then(|args| args.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }

        fn list_resources(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<
            Output = Result<rmcp::model::ListResourcesResult, McpError>,
        > + Send
        + '_ {
            std::future::ready(Ok(rmcp::model::ListResourcesResult {
                resources: vec![rmcp::model::Resource::new(
                    rmcp::model::RawResource::new("memory://guide", "Guide"),
                    None,
                )],
                next_cursor: None,
                meta: None,
            }))
        }

        fn read_resource(
            &self,
            request: rmcp::model::ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> impl std::future::Future<Output = Result<rmcp::model::ReadResourceResult, McpError>>
        + Send
        + '_ {
            std::future::ready(Ok(rmcp::model::ReadResourceResult {
                contents: vec![rmcp::model::ResourceContents::text(
                    format!("content of {}", request.uri),
                    request.uri,
                )],
            }))
        }
    }

    #[tokio::test]
    async fn mcp_client_reads_resources_from_a_real_in_process_server() {
        // The mock proves the routing; this proves the rmcp call shapes are
        // right, which only a real server can.
        let server = EchoServer::new(Vec::new());
        let (client_stream, server_stream) = tokio::io::duplex(64);
        let _server_handle = tokio::spawn(async move {
            let running = server.serve(server_stream).await.unwrap();
            while !running.is_transport_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });

        let signal = ToolListChangedSignal::default();
        let running = signal.clone().serve(client_stream).await.unwrap();
        let client = McpClient::with_service(
            "fixture",
            Vec::new(),
            Arc::new(RealMcpService::new(running, signal)),
        );

        let resources = client.list_resources().await.unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].uri, "memory://guide");

        let contents = client.read_resource("memory://guide").await.unwrap();
        assert_eq!(contents.contents.len(), 1);
    }

    #[tokio::test]
    async fn mcp_client_talks_to_real_in_process_server() {
        let tool = echo_tool();
        let server = EchoServer::new(vec![tool.clone()]);
        let (client_stream, server_stream) = tokio::io::duplex(64);

        let _server_handle = tokio::spawn(async move {
            let running = server.serve(server_stream).await.unwrap();
            // Keep the server alive until the client closes the transport.
            while !running.is_transport_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });

        let signal = ToolListChangedSignal::default();
        let running = signal.clone().serve(client_stream).await.unwrap();
        let client = McpClient::with_service(
            "fixture",
            vec![tool],
            Arc::new(RealMcpService::new(running, signal)),
        );

        let output = client
            .call_tool("echo", json!({"text": "hello"}))
            .await
            .unwrap();
        assert_eq!(output, "hello");
    }

    // ── Native `.mcp.json` sources ───────────────────────────────────────

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
                ..McpProjectConfig::default()
            },
        }
    }

    /// A name skipped in one scope can still be declared by another; it must be
    /// reported once (as configured), not also as skipped.
    #[test]
    fn a_name_configured_in_another_scope_is_not_also_reported_as_skipped() {
        let mut broken = sourced("shared", "~/.tact/.mcp.json", "/bin/unused");
        broken.config.command = None;
        let working = sourced("shared", "./.tact/.mcp.json", "/bin/project");

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
        let path = dir.path().join(".mcp.json");
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
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "{ not json").unwrap();

        let message = format!("{:#}", McpConfigFile::read(&path).unwrap_err());
        assert!(message.contains(".mcp.json"), "{message}");
    }

    #[test]
    fn later_source_overrides_earlier_by_server_name() {
        let resolved = resolve_servers(vec![
            sourced("shared", "~/.tact/.mcp.json", "/bin/user"),
            sourced("shared", "./.tact/.mcp.json", "/bin/project"),
        ]);

        assert_eq!(resolved.servers.len(), 1);
        assert!(matches!(
            &resolved.servers[0].1,
            McpTransportConfig::Stdio(config) if config.command == "/bin/project"
        ));
        assert_eq!(
            resolved.shadowed,
            vec![("shared".to_owned(), "~/.tact/.mcp.json".to_owned())]
        );
        assert!(resolved.skipped_remote.is_empty());
    }

    #[test]
    fn non_conflicting_sources_merge() {
        let resolved = resolve_servers(vec![
            sourced("user-only", "~/.tact/.mcp.json", "/bin/user"),
            sourced("project-only", "./.tact/.mcp.json", "/bin/project"),
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
            source: "~/.tact/.mcp.json".to_owned(),
            config: McpProjectConfig {
                server_type: Some("http".to_owned()),
                command: None,
                args: Vec::new(),
                env: Default::default(),
                url: Some("https://example.invalid/mcp".to_owned()),
                headers: Default::default(),
                auth: None,
                ..McpProjectConfig::default()
            },
        };
        let commandless = SourcedServer {
            name: "typo".to_owned(),
            source: "~/.tact/.mcp.json".to_owned(),
            config: McpProjectConfig {
                server_type: None,
                command: None,
                args: Vec::new(),
                env: Default::default(),
                url: None,
                headers: Default::default(),
                auth: None,
                ..McpProjectConfig::default()
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
        // The whole point of `.mcp.json`: a plain key, so the agent-side tool
        // name is exactly `mcp__<key>__<tool>`.
        let resolved = resolve_servers(vec![sourced(
            "basic-memory",
            "~/.tact/.mcp.json",
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

    /// Sorted by name: `.mcp.json` is parsed into a `HashMap`, so the listing
    /// must not inherit its random iteration order.
    #[test]
    fn configured_servers_are_sorted_by_name() {
        let mut resolved = resolve_servers(vec![
            sourced("zeta", "~/.tact/.mcp.json", "/bin/z"),
            sourced("alpha", "~/.tact/.mcp.json", "/bin/a"),
            sourced("mid", "~/.tact/.mcp.json", "/bin/m"),
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
            shadowed: vec![("shared".to_owned(), "~/.tact/.mcp.json".to_owned())],
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
    fn a_cwd_dot_mcp_json_is_read_and_never_outranks_a_native_file() {
        // The temp dir stands in for the working directory; assertions look at
        // these two names only, so a real `~/.tact/.mcp.json` on the machine
        // running the tests cannot make this flaky.
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();

        // A Claude Code project file at cwd, sharing one name with the native
        // project file and contributing one of its own.
        std::fs::write(
            cwd.join(".mcp.json"),
            r#"{"mcpServers":{
                 "claude-town":{"command":"/bin/claude"},
                 "claude-shared-name":{"command":"/bin/claude"}
               }}"#,
        )
        .unwrap();
        std::fs::create_dir_all(cwd.join(".tact")).unwrap();
        std::fs::write(
            cwd.join(".tact/.mcp.json"),
            r#"{"mcpServers":{"claude-shared-name":{"command":"/bin/native"}}}"#,
        )
        .unwrap();

        let resolved = resolve_servers(collect_sourced_servers(cwd).unwrap());

        let shared = resolved
            .servers
            .iter()
            .find(|(name, _, _)| name == "claude-shared-name")
            .expect("the shared name resolves");
        assert!(
            matches!(&shared.1, McpTransportConfig::Stdio(c) if c.command == "/bin/native"),
            "the native project file must win over a cwd .mcp.json",
        );
        assert_eq!(
            resolved.shadowed,
            vec![(
                "claude-shared-name".to_owned(),
                cwd.join(".mcp.json").display().to_string()
            )],
            "the displaced declaration must be reported, not silently dropped",
        );
        assert!(
            resolved
                .servers
                .iter()
                .any(|(name, _, _)| name == "claude-town"),
            "a name only the cwd .mcp.json declares must still be usable",
        );
    }

    #[test]
    fn the_native_config_file_is_dot_mcp_json_in_the_tact_dir() {
        let dir = tempfile::tempdir().unwrap();
        let project = TactPath::new(dir.path()).mcp_config_path();

        assert!(project.ends_with(".tact/.mcp.json"), "{project:?}");
    }

    #[test]
    fn an_unreadable_cwd_dot_mcp_json_is_skipped_not_fatal() {
        // The file belongs to the project, not to the user: a repository must
        // not be able to stop Tact from starting in its directory.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".mcp.json"), "{ this is not json").unwrap();

        let servers = collect_sourced_servers(dir.path()).expect("must not be fatal");
        assert!(
            !servers
                .iter()
                .any(|s| s.source == dir.path().join(".mcp.json").display().to_string()),
            "{servers:?}",
        );
    }

    #[test]
    fn disabled_entries_use_the_default_only_when_absent() {
        let shown: McpProjectConfig = serde_json::from_str(r#"{"command":"node"}"#).unwrap();
        assert!(shown.enabled, "an entry that says nothing is active");
        assert!(shown.extra.is_empty(), "{:?}", shown.extra);

        let off: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","enabled":false}"#).unwrap();
        assert!(!off.enabled);
    }

    /// Codex per-entry fields Tact has no equivalent for must not vanish
    /// without a word — `unmodelled_keys` names them.
    #[test]
    fn codex_only_entry_keys_are_captured_instead_of_dropped() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{
                "command": "node",
                "args": ["scripts/launch.mjs"],
                "enabled": false,
                "enabled_tools": ["js", "js_reset"],
                "omit_tools_from": ["code_mode", "deferred"],
                "startup_timeout_sec": 120,
                "tools": { "js": { "output_token_limit": 25000 } }
            }"#,
        )
        .unwrap();

        assert!(!config.enabled);
        // The modelled fields are no longer "unmodelled": `enabled_tools`,
        // `startup_timeout_sec` and `tools` are honoured, so only the fields
        // Tact genuinely has no equivalent for remain.
        let mut keys: Vec<&str> = config.extra.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["omit_tools_from"]);
        // `enabled` is modelled, so it must *not* also land in `extra`.
        assert!(!config.extra.contains_key("enabled"));
        assert_eq!(
            config.enabled_tools.as_deref(),
            Some(["js".to_owned(), "js_reset".to_owned()].as_slice())
        );
        assert_eq!(config.startup_timeout_sec, Some(120));
        assert_eq!(
            config.tools.as_ref().and_then(|tools| tools.get("js")),
            Some(&McpToolConfig {
                approval_mode: None,
                output_token_limit: Some(25_000),
                risk: None,
            })
        );
    }

    #[test]
    fn codex_only_keys_are_reported_with_their_source() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{"command":"node","omit_tools_from":["code_mode"],"tool_timeout_sec":30}"#,
        )
        .unwrap();

        let resolved = resolve_servers(vec![sourced_config("codexish", config)]);

        assert_eq!(resolved.unmodelled.len(), 1, "{:?}", resolved.unmodelled);
        assert_eq!(resolved.unmodelled[0].server, "codexish");
        assert_eq!(resolved.unmodelled[0].source, "/tmp/.mcp.json");
        // `tool_timeout_sec` is honoured now, so only the field with no
        // counterpart here is left to report.
        assert_eq!(resolved.unmodelled[0].keys, vec!["omit_tools_from"]);
        assert_eq!(
            resolved.policies["codexish"].tool_timeout(),
            Some(std::time::Duration::from_secs(30))
        );
        // The entry still connects: an unimplemented *option* is not a reason
        // to drop a server that declares a working transport.
        assert_eq!(resolved.servers.len(), 1);
    }

    // ── Per-entry tool policy ────────────────────────────────────────────

    #[test]
    fn enabled_tools_limits_exposure_and_disabled_tools_wins() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{
                "command": "node",
                "enabled_tools": ["keep", "also_hidden"],
                "disabled_tools": ["also_hidden"]
            }"#,
        )
        .unwrap();
        let policy = McpServerPolicy::from_config(&config);

        assert!(policy.exposes("keep"));
        assert!(!policy.exposes("other"));
        // Codex applies the deny list *after* the allow list, so naming a tool
        // in both hides it.
        assert!(!policy.exposes("also_hidden"));
    }

    #[test]
    fn an_entry_without_tool_policy_exposes_everything() {
        let config: McpProjectConfig = serde_json::from_str(r#"{"command":"node"}"#).unwrap();
        let policy = McpServerPolicy::from_config(&config);

        assert!(policy.exposes("anything"));
        assert!(policy.startup_timeout().is_none());
        assert!(policy.tool_timeout().is_none());
        assert!(!policy.is_auto_approved("anything"));
        assert!(policy.output_token_limit("anything").is_none());
    }

    /// A variable name nothing can have set, so the "unset" tests cannot pass by
    /// accident on a machine that happens to define it.
    fn absent_var() -> String {
        format!("TACT_TEST_ABSENT_{}", std::process::id())
    }

    #[test]
    fn env_vars_accept_the_shorthand_and_the_explicit_form() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{
                "command": "node",
                "env_vars": ["SHORTHAND", {"name": "LOCAL", "source": "local"}, {"name": "PLAIN"}]
            }"#,
        )
        .unwrap();

        let names: Vec<&str> = config.env_vars.iter().map(|v| v.name()).collect();
        assert_eq!(names, ["SHORTHAND", "LOCAL", "PLAIN"]);
        assert_eq!(config.env_vars[0].source(), None, "shorthand means local");
        assert_eq!(config.env_vars[1].source(), Some("local"));
        assert_eq!(config.env_vars[2].source(), None, "source is optional");
    }

    #[test]
    fn env_vars_are_resolved_from_the_process_environment() {
        let inherited =
            resolve_env_vars("demo", &HashMap::new(), &[McpEnvVar::Name("PATH".into())])
                .expect("PATH is always set");
        assert_eq!(
            inherited.get("PATH"),
            std::env::var("PATH").ok().as_ref(),
            "the value must be the process's own"
        );
    }

    #[test]
    fn a_literal_env_entry_wins_over_env_vars() {
        // The name is deliberately unset: if the literal map did not win, this
        // would fail instead of resolving, which is exactly the bug being
        // pinned — a pass-through must never override what the user wrote down.
        let explicit = HashMap::from([(absent_var(), "literal".to_string())]);
        let resolved = resolve_env_vars("demo", &explicit, &[McpEnvVar::Name(absent_var())])
            .expect("the literal entry wins, so the unset variable is never read");
        assert!(resolved.is_empty(), "nothing needed inheriting");
    }

    #[test]
    fn an_unset_env_var_fails_the_server_by_name() {
        let error = resolve_env_vars("demo", &HashMap::new(), &[McpEnvVar::Name(absent_var())])
            .expect_err("an unset variable must fail rather than start a broken child");
        let message = error.to_string();
        assert!(message.contains(&absent_var()), "{message}");
        assert!(message.contains("is not set"), "{message}");
    }

    #[test]
    fn a_remote_env_vars_source_names_the_missing_executor() {
        let entry = McpEnvVar::Explicit {
            name: "TOKEN".to_string(),
            source: Some("remote".to_string()),
        };
        let error = resolve_env_vars("demo", &HashMap::new(), &[entry])
            .expect_err("remote stdio is not implemented");
        assert!(error.to_string().contains("remote stdio"), "{error}");
    }

    #[test]
    fn an_unknown_env_vars_source_names_the_allowed_set() {
        let entry = McpEnvVar::Explicit {
            name: "TOKEN".to_string(),
            source: Some("keychain".to_string()),
        };
        let error = resolve_env_vars("demo", &HashMap::new(), &[entry])
            .expect_err("an unknown source must be reported, never ignored");
        let message = error.to_string();
        assert!(message.contains("keychain"), "{message}");
        assert!(message.contains("`local` or `remote`"), "{message}");
    }

    #[test]
    fn env_vars_on_a_remote_entry_are_reported_as_unmodelled() {
        // A remote entry has no child process, so claiming to honour its
        // `env_vars` would be exactly the silence `unmodelled_keys` prevents.
        let config: McpProjectConfig =
            serde_json::from_str(r#"{"url":"https://example.test/mcp","env_vars":["TOKEN"]}"#)
                .unwrap();
        let reported = unmodelled_keys("hosted", "test", &config).expect("must be reported");
        assert_eq!(reported.keys, ["env_vars"]);

        // …and a stdio entry that uses it is not reported at all.
        let stdio: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","env_vars":["TOKEN"]}"#).unwrap();
        assert!(unmodelled_keys("local", "test", &stdio).is_none());
    }

    #[tokio::test]
    async fn env_vars_are_resolved_before_the_child_is_spawned() {
        // The command does not exist: if resolution ran after the spawn, the
        // error would be about spawning. Naming the variable proves the order.
        let config = McpServerConfig {
            command: "/nonexistent-tact-test-binary".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            env_vars: vec![McpEnvVar::Name(absent_var())],
            cwd: None,
        };
        let error = McpClient::connect("demo", McpTransportConfig::Stdio(config), None)
            .await
            .expect_err("an unset env var must fail the connect");
        let message = format!("{error:#}");
        assert!(message.contains(&absent_var()), "{message}");
        assert!(!message.contains("failed to spawn"), "{message}");
    }

    #[test]
    fn tool_timeout_sec_overrides_the_global_call_ceiling() {
        let config: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","tool_timeout_sec":5}"#).unwrap();
        assert_eq!(
            McpServerPolicy::from_config(&config).tool_timeout(),
            Some(std::time::Duration::from_secs(5))
        );

        // Absent keeps Tact's own ceiling rather than Codex's.
        let plain: McpProjectConfig = serde_json::from_str(r#"{"command":"node"}"#).unwrap();
        assert!(
            McpServerPolicy::from_config(&plain)
                .tool_timeout()
                .is_none()
        );
    }

    #[test]
    fn tool_timeout_sec_is_no_longer_reported_as_unmodelled() {
        let config: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","tool_timeout_sec":5}"#).unwrap();
        assert!(
            unmodelled_keys("demo", "test", &config).is_none(),
            "Tact honours this field now, so reporting it would be a lie"
        );
    }

    #[test]
    fn startup_timeout_accepts_seconds_and_the_millisecond_alias() {
        let secs: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","startup_timeout_sec":120}"#).unwrap();
        assert_eq!(
            McpServerPolicy::from_config(&secs).startup_timeout(),
            Some(std::time::Duration::from_secs(120))
        );

        let ms: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","startup_timeout_ms":1500}"#).unwrap();
        assert_eq!(
            McpServerPolicy::from_config(&ms).startup_timeout(),
            Some(std::time::Duration::from_millis(1500))
        );

        // Both present: seconds is the documented unit and wins, so one entry
        // can never mean two budgets.
        let both: McpProjectConfig = serde_json::from_str(
            r#"{"command":"node","startup_timeout_sec":5,"startup_timeout_ms":9000}"#,
        )
        .unwrap();
        assert_eq!(
            McpServerPolicy::from_config(&both).startup_timeout(),
            Some(std::time::Duration::from_secs(5))
        );
    }

    #[test]
    fn a_tool_risk_parses_its_three_tiers_and_nothing_else() {
        assert_eq!(ToolRisk::parse(" read "), Some(ToolRisk::Read));
        assert_eq!(ToolRisk::parse("write"), Some(ToolRisk::Write));
        assert_eq!(ToolRisk::parse("high"), Some(ToolRisk::High));
        // The empty string is not a tier, so an empty declaration cannot be
        // mistaken for one.
        assert_eq!(ToolRisk::parse(""), None);
        assert_eq!(ToolRisk::parse("medium"), None);

        // Each tier maps onto the capability the permission layer acts on.
        assert_eq!(ToolRisk::Read.to_capability(), CapabilityRisk::Read);
        assert_eq!(ToolRisk::Write.to_capability(), CapabilityRisk::Write);
        assert_eq!(ToolRisk::High.to_capability(), CapabilityRisk::High);
        // `as_str` is the spelling the docs and the config use.
        assert_eq!(ToolRisk::Read.as_str(), "read");
        assert_eq!(ToolRisk::High.as_str(), "high");
    }

    #[test]
    fn a_declared_tool_risk_overrides_the_entry_default() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{
                "command": "node",
                "default_tool_risk": "write",
                "tools": {
                    "search_notes": { "risk": "read" },
                    "delete_project": { "risk": "high" }
                }
            }"#,
        )
        .unwrap();
        let policy = McpServerPolicy::from_config(&config);

        // The entry default covers tools that say nothing of their own.
        assert_eq!(policy.risk_for("read_thing"), Some(CapabilityRisk::Write));
        // A per-tool tier wins over the default, in both directions: one
        // lowers below it, one raises back to High.
        assert_eq!(policy.risk_for("search_notes"), Some(CapabilityRisk::Read));
        assert_eq!(
            policy.risk_for("delete_project"),
            Some(CapabilityRisk::High)
        );
        // A tool with an unrelated override still inherits the default.
        let with_unrelated: McpProjectConfig = serde_json::from_str(
            r#"{"command":"node","default_tool_risk":"write",
                "tools":{"documented":{"output_token_limit":2500}}}"#,
        )
        .unwrap();
        let policy = McpServerPolicy::from_config(&with_unrelated);
        assert_eq!(policy.risk_for("documented"), Some(CapabilityRisk::Write));
    }

    #[test]
    fn an_entry_that_declares_no_risk_declares_none_at_all() {
        // `None` is what keeps a silent entry on Tact's default instead of
        // letting an absent field read as a tier.
        let config: McpProjectConfig = serde_json::from_str(r#"{"command":"node"}"#).unwrap();
        let policy = McpServerPolicy::from_config(&config);
        assert_eq!(policy.risk_for("anything"), None);
    }

    #[test]
    fn an_unknown_tool_risk_is_ignored_rather_than_guessed() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{"command":"node","default_tool_risk":"harmless",
                "tools":{"write_thing":{"risk":"kinda-safe"}}}"#,
        )
        .unwrap();
        let policy = McpServerPolicy::from_config(&config);

        // Neither the bad default nor the bad override becomes a tier.
        assert_eq!(policy.risk_for("write_thing"), None);
        assert_eq!(policy.risk_for("read_thing"), None);
    }

    #[test]
    fn risk_keys_are_modelled_and_never_reported_as_unmodelled() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{"command":"node","default_tool_risk":"write",
                "tools":{"search_notes":{"risk":"read"}},
                "omit_tools_from":["code_mode"]}"#,
        )
        .unwrap();

        // Only the field Tact genuinely does not model is reported.
        assert!(unmodelled_keys("demo", "test", &config).is_some_and(|u| u.keys == ["omit_tools_from"]));
        assert!(!config.extra.contains_key("default_tool_risk"));
        assert_eq!(config.default_tool_risk.as_deref(), Some("write"));
        assert_eq!(
            config
                .tools
                .as_ref()
                .and_then(|tools| tools.get("search_notes"))
                .and_then(|tool| tool.risk.as_deref()),
            Some("read")
        );
    }

    #[test]
    fn a_per_tool_approval_mode_overrides_the_server_default() {
        let config: McpProjectConfig = serde_json::from_str(
            r#"{
                "command": "node",
                "default_tools_approval_mode": "auto",
                "tools": {
                    "write_thing": { "approval_mode": "prompt" },
                    "documented": { "output_token_limit": 2500 }
                }
            }"#,
        )
        .unwrap();
        let policy = McpServerPolicy::from_config(&config);

        // Server default applies where nothing overrides it.
        assert!(policy.is_auto_approved("read_thing"));
        // An explicit per-tool mode wins over the default.
        assert!(!policy.is_auto_approved("write_thing"));
        // A tool with an unrelated override still inherits the default.
        assert!(policy.is_auto_approved("documented"));
        assert_eq!(policy.output_token_limit("documented"), Some(2500));
        assert_eq!(policy.output_token_limit("read_thing"), None);
    }

    #[test]
    fn an_unknown_approval_mode_is_ignored_rather_than_guessed() {
        let config: McpProjectConfig =
            serde_json::from_str(r#"{"command":"node","default_tools_approval_mode":"yolo"}"#)
                .unwrap();
        let policy = McpServerPolicy::from_config(&config);

        // Not auto-approved: an unreadable value must fail towards asking.
        assert!(!policy.is_auto_approved("anything"));
        assert_eq!(ApprovalMode::parse(" yolo "), None);
        assert_eq!(ApprovalMode::parse(" auto "), Some(ApprovalMode::Auto));
    }

    #[test]
    fn filtering_hides_tools_from_the_agent_and_reports_them() {
        let policy = McpServerPolicy::from_config(
            &serde_json::from_str(
                r#"{"command":"node","enabled_tools":["visible"],"disabled_tools":["nope"]}"#,
            )
            .unwrap(),
        );
        let client = McpClient::with_service_and_policy(
            "demo",
            vec![
                named_tool("visible"),
                named_tool("hidden"),
                named_tool("unlisted"),
            ],
            Arc::new(MockMcpService::new(Vec::new(), |_| {
                Ok(CallToolResult::success(Vec::new()))
            })),
            policy,
        );

        assert_eq!(client.list_tools().len(), 1);
        assert_eq!(client.hidden_tools(), ["hidden", "unlisted"]);
        assert_eq!(client.agent_tools().len(), 1);
        assert_eq!(client.agent_tools()[0].name, "mcp__demo__visible");
        assert_eq!(client.tool_count(), 1);
    }

    /// A tool that declares itself read-only, as a real server would.
    fn read_only_tool(name: &'static str) -> McpTool {
        McpTool {
            annotations: Some(ToolAnnotations {
                read_only_hint: Some(true),
                ..ToolAnnotations::default()
            }),
            ..named_tool(name)
        }
    }

    #[test]
    fn the_router_resolves_a_declared_tier_and_defaults_the_rest_to_high() {
        // End of the wiring: JSON entry → resolved policy → what the permission
        // layer is handed. `agent::tool_dispatch` delegates to this in one line.
        let policy = McpServerPolicy::from_config(
            &serde_json::from_str(
                r#"{"command":"node","tools":{"search_notes":{"risk":"read"}}}"#,
            )
            .unwrap(),
        );
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service_and_policy(
            "bm",
            vec![named_tool("search_notes"), named_tool("delete_project")],
            Arc::new(MockMcpService::new(Vec::new(), |_| {
                Ok(CallToolResult::success(Vec::new()))
            })),
            policy,
        ));

        assert_eq!(
            router.risk_for("bm", "search_notes"),
            CapabilityRisk::Read
        );
        // The tool the entry said nothing about keeps the default.
        assert_eq!(
            router.risk_for("bm", "delete_project"),
            CapabilityRisk::High
        );
        // An unknown server is not a reason to relax anything.
        assert_eq!(router.risk_for("ghost", "search_notes"), CapabilityRisk::High);
    }

    #[test]
    fn a_server_declared_read_only_tool_is_reported_but_never_downgrades_the_risk() {
        // rmcp's own docs are explicit: "Clients should never make tool use
        // decisions based on ToolAnnotations received from untrusted servers."
        // So the declaration is surfaced for a human to act on, and the risk
        // stays whatever the entry declared — here, the default.
        let client = McpClient::with_service(
            "demo",
            vec![read_only_tool("search"), named_tool("write")],
            Arc::new(MockMcpService::new(Vec::new(), |_| {
                Ok(CallToolResult::success(Vec::new()))
            })),
        );

        assert_eq!(client.declared_read_only(), ["search"]);
        assert_eq!(client.policy().risk_for("search"), None);
    }

    #[test]
    fn an_exposed_only_declaration_is_what_gets_reported() {
        // A tool the entry filtered away is not something a human can be asked
        // to declare a risk for, so it must not appear as evidence.
        let policy = McpServerPolicy::from_config(
            &serde_json::from_str(r#"{"command":"node","disabled_tools":["search"]}"#).unwrap(),
        );
        let client = McpClient::with_service_and_policy(
            "demo",
            vec![read_only_tool("search"), read_only_tool("recall")],
            Arc::new(MockMcpService::new(Vec::new(), |_| {
                Ok(CallToolResult::success(Vec::new()))
            })),
            policy,
        );

        assert_eq!(client.declared_read_only(), ["recall"]);
    }

    #[test]
    fn a_read_only_hint_of_false_is_not_a_declaration() {
        let tool = McpTool {
            annotations: Some(ToolAnnotations {
                read_only_hint: Some(false),
                ..ToolAnnotations::default()
            }),
            ..named_tool("write")
        };
        let client = McpClient::with_service(
            "demo",
            vec![tool],
            Arc::new(MockMcpService::new(Vec::new(), |_| {
                Ok(CallToolResult::success(Vec::new()))
            })),
        );

        assert!(client.declared_read_only().is_empty());
    }

    /// Polls `refresh_tools_if_stale` until it reports a refresh, with a
    /// deadline.
    ///
    /// The notification travels on the service's own task, so "the server said
    /// so" and "we noticed" are separated by a real scheduling hop. Asserting
    /// once would be a race; waiting forever would hang the suite, which AGENTS.md
    /// forbids.
    async fn refresh_until_changed(client: &mut McpClient) -> ToolListRefresh {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(refresh) = client.refresh_tools_if_stale().await.unwrap() {
                return refresh;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no tools/list_changed notification arrived within the deadline"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Connects a real in-process server and hands back the client, plus the
    /// server's shared `tools/list` counter.
    async fn connect_with_policy(
        server: EchoServer,
        policy: McpServerPolicy,
    ) -> (McpClient, Arc<std::sync::atomic::AtomicUsize>) {
        let list_calls = server.list_calls();
        let (client_stream, server_stream) = tokio::io::duplex(64);
        tokio::spawn(async move {
            let running = server.serve(server_stream).await.unwrap();
            // Keep the server alive until the client closes the transport.
            while !running.is_transport_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });
        let signal = ToolListChangedSignal::default();
        let running = signal.clone().serve(client_stream).await.unwrap();
        let client = McpClient::with_service_and_policy(
            "fixture",
            vec![echo_tool()],
            Arc::new(RealMcpService::new(running, signal)),
            policy,
        );
        (client, list_calls)
    }

    /// [`connect_with_policy`] with the default (unfiltered) policy.
    async fn connect(server: EchoServer) -> (McpClient, Arc<std::sync::atomic::AtomicUsize>) {
        connect_with_policy(server, McpServerPolicy::default()).await
    }

    #[tokio::test]
    async fn a_list_changed_notification_makes_the_new_tool_callable() {
        let server = EchoServer::new(vec![echo_tool()]).revealing(vec![named_tool("recall")]);
        let (mut client, _list_calls) = connect(server).await;

        // What the handshake advertised.
        assert_eq!(client.list_tools().len(), 1);

        // A call that grows the list and announces it. The announcement is the
        // whole difference from a server that changed silently.
        client.call_tool("reveal", json!({})).await.unwrap();

        let refresh = refresh_until_changed(&mut client).await;
        assert_eq!(refresh.added, ["recall"]);
        assert!(refresh.removed.is_empty());
        assert_eq!(client.list_tools().len(), 2);
        // The derived specs move with it: what the model is offered is what the
        // server now has.
        assert!(
            client
                .agent_tools()
                .iter()
                .any(|spec| spec.name == "mcp__fixture__recall"),
            "{:?}",
            client.agent_tools().iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn a_quiet_server_is_never_polled() {
        // The notification is a hint, not a schedule: a server that never sends
        // one must cost exactly nothing, so a refresh must not turn into a
        // `tools/list` on every request.
        let server = EchoServer::new(vec![echo_tool()]);
        let (mut client, list_calls) = connect(server).await;
        let after_connect = list_calls.load(std::sync::atomic::Ordering::Acquire);

        assert!(client.refresh_tools_if_stale().await.unwrap().is_none());
        assert!(client.refresh_tools_if_stale().await.unwrap().is_none());

        assert_eq!(
            list_calls.load(std::sync::atomic::Ordering::Acquire),
            after_connect,
            "a quiet server must not be re-listed"
        );
    }

    #[tokio::test]
    async fn a_refresh_re_derives_the_filter_and_the_read_only_claims() {
        // The newcomer set is chosen to hit every derivation at once: one tool
        // the entry hides, and one the server declares read-only.
        let server = EchoServer::new(vec![echo_tool()])
            .revealing(vec![read_only_tool("recall"), named_tool("secret")]);
        let policy = McpServerPolicy::from_config(
            &serde_json::from_str(r#"{"command":"node","disabled_tools":["secret"]}"#).unwrap(),
        );
        let (mut client, _list_calls) = connect_with_policy(server, policy).await;

        client.call_tool("reveal", json!({})).await.unwrap();
        let refresh = refresh_until_changed(&mut client).await;

        // Only the newcomer the policy exposes counts as added…
        assert_eq!(refresh.added, ["recall"]);
        // …the hidden one is reported rather than silently dropped…
        assert_eq!(refresh.newly_hidden, ["secret"]);
        assert_eq!(client.hidden_tools(), ["secret"]);
        // …and the read-only claim is re-derived from the new list, not the old.
        assert_eq!(client.declared_read_only(), ["recall"]);
    }

    #[tokio::test]
    async fn the_router_reports_what_moved_and_keeps_a_list_it_could_not_re_read() {
        let moving = Arc::new(MockMcpService::new(vec![named_tool("one")], |_| {
            Ok(CallToolResult::success(Vec::new()))
        }));
        let broken = Arc::new(
            MockMcpService::new(vec![named_tool("kept")], |_| {
                Ok(CallToolResult::success(Vec::new()))
            })
            .failing_to_list(),
        );
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service(
            "moving",
            vec![named_tool("one")],
            moving.clone(),
        ));
        router.register_client(McpClient::with_service(
            "broken",
            vec![named_tool("kept")],
            broken.clone(),
        ));

        // Nothing announced: the pass costs nothing and reports nothing.
        assert!(router.refresh_changed().await.is_empty());

        moving.set_tools(vec![named_tool("one"), named_tool("two")]);
        moving.announce_tools_changed();
        broken.announce_tools_changed();

        let report = router.refresh_changed().await;
        assert_eq!(report.changed.len(), 1, "{report:?}");
        assert_eq!(report.changed[0].0, "moving");
        assert_eq!(report.changed[0].1.added, ["two"]);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].0, "broken");
        // A server that announced a change and could not answer keeps what it
        // had: a transient failure must not leave it looking empty.
        assert_eq!(router.server_summaries().len(), 2);
        assert!(
            router
                .all_tools()
                .iter()
                .any(|spec| spec.name == "mcp__broken__kept"),
            "the unreachable server's tools must survive the failed re-list"
        );
    }

    /// A tool with only a name — enough for exposure filtering.
    fn named_tool(name: &'static str) -> McpTool {
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

    /// A plugin bundle carrying Codex-only fields is normal, so it must not
    /// turn every startup into a notice — it is `mcp list` that names them.
    #[test]
    fn unmodelled_keys_do_not_make_the_load_report_noisy() {
        let report = McpLoadReport {
            unmodelled: vec![UnmodelledKeys {
                server: "codexish".to_owned(),
                source: "/tmp/.mcp.json".to_owned(),
                keys: vec!["enabled_tools".to_owned()],
            }],
            ..McpLoadReport::default()
        };
        assert!(report.is_quiet(), "{:?}", report.notice_lines());
    }

    #[test]
    fn a_disabled_entry_is_listed_but_never_connected() {
        let mut off = stdio_config("/bin/off");
        off.enabled = false;

        let resolved = resolve_servers(vec![sourced_config("off", off)]);

        assert!(
            resolved.servers.is_empty(),
            "a disabled server must not be connected: {:?}",
            resolved.servers,
        );
        assert_eq!(resolved.disabled.len(), 1);
        let described = resolved.configured();
        assert_eq!(described.len(), 1, "it must still be visible to `mcp list`");
        assert!(described[0].disabled);
        assert_eq!(described[0].name, "off");
    }

    /// The winner of a name decides whether it connects, in both directions:
    /// disabling in a higher-precedence source switches an enabled lower one
    /// off, and enabling higher up switches a disabled lower one on.
    #[test]
    fn the_winning_declaration_decides_whether_a_name_connects() {
        let mut off = stdio_config("/bin/off");
        off.enabled = false;
        let mut enabled = stdio_config("/bin/on");
        enabled.enabled = true;

        let disabled_wins = resolve_servers(vec![
            sourced_config("svc", enabled.clone()),
            sourced_config("svc", off.clone()),
        ]);
        assert!(disabled_wins.servers.is_empty());
        assert_eq!(disabled_wins.disabled.len(), 1);

        let enabled_wins = resolve_servers(vec![
            sourced_config("svc", off),
            sourced_config("svc", enabled),
        ]);
        assert_eq!(enabled_wins.servers.len(), 1);
        assert!(enabled_wins.disabled.is_empty());
    }

    fn sourced_config(name: &str, config: McpProjectConfig) -> SourcedServer {
        SourcedServer {
            name: name.to_string(),
            source: "/tmp/.mcp.json".to_string(),
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
            ..McpProjectConfig::default()
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
            ..McpProjectConfig::default()
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
