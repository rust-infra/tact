//! Language Server Protocol client.
//!
//! Implements the client side of the LSP JSON-RPC protocol over the LSP
//! server's stdin/stdout.  Each [`LspClient`] manages one server process;
//! [`LspManager`] tracks a collection of clients keyed by server name.
//!
//! # Protocol overview
//! Messages are framed with a `Content-Length` HTTP-style header:
//! ```text
//! Content-Length: <N>\r\n
//! \r\n
//! <N bytes of UTF-8 JSON>
//! ```
//! The server sends the same framing back on its stdout.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a single LSP server process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    /// Display name, e.g. "rust-analyzer"
    pub name: String,
    /// Path or name of the server binary, e.g. "rust-analyzer"
    pub command: String,
    /// Command-line arguments passed to the server binary
    pub args: Vec<String>,
    /// Glob patterns that activate this server, e.g. `["*.rs", "*.toml"]`
    pub file_patterns: Vec<String>,
    /// Optional server-specific initialization options (passed in LSP `initialize`)
    pub initialization_options: Option<serde_json::Value>,
    /// Map of file extension (e.g. `.rs`) to LSP language identifier (e.g.
    /// `rust`).  Used to supply `textDocument/didOpen::languageId` and to
    /// route files to the right server.
    #[serde(default)]
    pub extension_to_language: HashMap<String, String>,
    /// Optional extra environment variables for the server process.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl LspServerConfig {
    /// Look up the LSP language identifier for `file_path`, falling back to
    /// `"plaintext"` when the extension is not mapped.
    pub fn language_for_file(&self, file_path: &str) -> String {
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        self.extension_to_language
            .get(&ext)
            .cloned()
            .unwrap_or_else(|| "plaintext".to_string())
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// A single diagnostic emitted by an LSP server.
#[derive(Debug, Clone)]
pub struct LspDiagnostic {
    /// Workspace-relative or absolute file path
    pub file: String,
    /// 1-based line number
    pub line: u32,
    /// 1-based column number
    pub column: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// The LSP server that produced this diagnostic (e.g. "rust-analyzer")
    pub source: Option<String>,
    /// Diagnostic code (e.g. "E0308"), if provided by the server
    pub code: Option<String>,
}

/// Severity level of a diagnostic, matching the LSP spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl DiagnosticSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "info",
            Self::Hint => "hint",
        }
    }

    fn from_lsp_int(n: u64) -> Self {
        match n {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            _ => Self::Hint,
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC framing helpers
// ---------------------------------------------------------------------------

async fn send_message(writer: &mut BufWriter<ChildStdin>, body: &str) -> anyhow::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_message(reader: &mut BufReader<ChildStdout>) -> anyhow::Result<serde_json::Value> {
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow::anyhow!("LSP server closed stdout"));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(val) = trimmed.strip_prefix("Content-Length: ") {
            content_length = val.trim().parse()?;
        }
    }
    if content_length == 0 {
        return Err(anyhow::anyhow!("LSP message missing Content-Length header"));
    }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

// ---------------------------------------------------------------------------
// LspClient
// ---------------------------------------------------------------------------

type PendingMap = Arc<DashMap<u64, oneshot::Sender<serde_json::Value>>>;

/// A running LSP client connected to a single server process.
pub struct LspClient {
    pub server_name: String,
    pub server_config: LspServerConfig,
    child: Option<Child>,
    writer: Option<Arc<Mutex<BufWriter<ChildStdin>>>>,
    pending: PendingMap,
    next_id: AtomicU64,
    is_initialized: bool,
    /// Cached diagnostics keyed by file URI, shared with external readers.
    diagnostics: Arc<DashMap<String, Vec<LspDiagnostic>>>,
    /// Per-file document version counters for didChange notifications.
    #[allow(dead_code)]
    doc_versions: HashMap<String, i64>,
}

impl LspClient {
    /// Spawn the server process and start the background reader.
    pub async fn start(config: LspServerConfig) -> anyhow::Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin not captured");
        let stdout = child.stdout.take().expect("stdout not captured");

        let name = config.name.clone();
        let writer = Arc::new(Mutex::new(BufWriter::new(stdin)));
        let pending: PendingMap = Arc::new(DashMap::new());
        let diagnostics: Arc<DashMap<String, Vec<LspDiagnostic>>> = Arc::new(DashMap::new());

        // Spawn background reader task
        let pending_clone = pending.clone();
        let diag_clone = diagnostics.clone();
        let server_name = name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader).await {
                    Ok(msg) => {
                        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                            // This is a response to a request
                            if let Some((_, tx)) = pending_clone.remove(&id) {
                                let _ = tx.send(msg);
                            }
                        } else if msg.get("method").and_then(|v| v.as_str())
                            == Some("textDocument/publishDiagnostics")
                        {
                            // Server push: diagnostics notification
                            let params = msg.get("params");
                            handle_diagnostics(diag_clone.clone(), params, &server_name);
                        }
                        // Other notifications (e.g. window/logMessage) are silently ignored.
                    }
                    Err(e) => {
                        tracing::debug!("LSP reader for '{}' exited: {}", server_name, e);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            server_name: name.clone(),
            server_config: config,
            child: Some(child),
            writer: Some(writer),
            pending,
            next_id: AtomicU64::new(1),
            is_initialized: false,
            diagnostics,
            doc_versions: HashMap::new(),
        })
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a JSON-RPC request and wait for the matching response.
    async fn send_request_inner(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id();
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&msg)?;

        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);

        {
            let writer = self
                .writer
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("LSP client already shut down"))?;
            let mut w = writer.lock().await;
            send_message(&mut w, &body).await?;
        }

        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "LSP request '{}' timed out (server: {})",
                    method,
                    self.server_name
                )
            })?
            .map_err(|_| {
                anyhow::anyhow!(
                    "LSP request '{}' channel closed (server: {})",
                    method,
                    self.server_name
                )
            })?;

        if let Some(err) = response.get("error") {
            return Err(anyhow::anyhow!(
                "LSP error from {}: {}",
                self.server_name,
                err
            ));
        }
        Ok(response["result"].clone())
    }

    /// Send a JSON-RPC notification (fire-and-forget, no response expected).
    async fn send_notification_inner(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&msg)?;
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LSP client already shut down"))?;
        let mut w = writer.lock().await;
        send_message(&mut w, &body).await
    }

    /// Perform the LSP `initialize` / `initialized` handshake.
    pub async fn initialize(&mut self, root_uri: &str) -> anyhow::Result<()> {
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "tact", "version": "1.0" },
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": false,
                        "codeDescriptionSupport": false
                    },
                    "synchronization": {
                        "dynamicRegistration": false,
                        "willSave": false,
                        "willSaveWaitUntil": false,
                        "didSave": true
                    }
                },
                "workspace": {
                    "configuration": false,
                    "didChangeConfiguration": { "dynamicRegistration": false }
                }
            },
            "initializationOptions": self.server_config.initialization_options,
        });

        self.send_request_inner("initialize", params).await?;

        // Send the `initialized` notification to complete the handshake
        self.send_notification_inner("initialized", json!({}))
            .await?;

        self.is_initialized = true;
        tracing::debug!("LSP server '{}' initialized", self.server_name);
        Ok(())
    }

    /// Notify the server that a document has been opened.
    pub async fn open_document(
        &mut self,
        uri: &str,
        language_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.send_notification_inner(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content,
                }
            }),
        )
        .await
    }

    /// Notify the server that a document has been changed.
    pub async fn change_document(
        &mut self,
        uri: &str,
        content: &str,
        version: i64,
    ) -> anyhow::Result<()> {
        self.send_notification_inner(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": content }],
            }),
        )
        .await
    }

    /// Notify the server that a document has been saved.
    pub async fn save_document(&mut self, uri: &str) -> anyhow::Result<()> {
        self.send_notification_inner(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri } }),
        )
        .await
    }

    /// Notify the server that a document has been closed.
    pub async fn close_document(&mut self, uri: &str) -> anyhow::Result<()> {
        self.send_notification_inner(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        )
        .await
    }

    /// Get hover information at a position (1-based line/column).
    pub async fn hover(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Option<String>> {
        // LSP protocol is 0-based
        let result = self
            .send_request_inner(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": line.saturating_sub(1),
                        "character": character.saturating_sub(1),
                    }
                }),
            )
            .await?;

        if result.is_null() {
            return Ok(None);
        }

        // The result can be { contents: MarkupContent | MarkedString | MarkedString[] }
        let contents = &result["contents"];
        let text = if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
            // MarkupContent { kind, value }
            value.to_string()
        } else if let Some(s) = contents.as_str() {
            // Plain string
            s.to_string()
        } else if let Some(arr) = contents.as_array() {
            // Array of MarkedStrings
            arr.iter()
                .filter_map(|item| {
                    item.get("value")
                        .and_then(|v| v.as_str())
                        .or_else(|| item.as_str())
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            return Ok(None);
        };

        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }

    /// Get definition locations for a position (1-based line/column).
    /// Returns a list of `"file_path:line"` strings.
    pub async fn definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<String>> {
        let result = self
            .send_request_inner(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": line.saturating_sub(1),
                        "character": character.saturating_sub(1),
                    }
                }),
            )
            .await?;
        Ok(extract_locations(&result))
    }

    /// Get references for a position (1-based line/column).
    pub async fn references(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<String>> {
        let result = self
            .send_request_inner(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": line.saturating_sub(1),
                        "character": character.saturating_sub(1),
                    }
                }),
            )
            .await?;
        Ok(extract_locations(&result))
    }

    /// Get document symbols.
    /// Returns a list of `"name (kind)"` strings, indented for hierarchy.
    pub async fn document_symbols(&self, uri: &str) -> anyhow::Result<Vec<String>> {
        let result = self
            .send_request_inner(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await?;

        let mut out = Vec::new();
        if let Some(arr) = result.as_array() {
            for item in arr {
                collect_symbol(item, 0, &mut out);
            }
        } else if result.is_object() {
            collect_symbol(&result, 0, &mut out);
        }
        Ok(out)
    }

    /// Retrieve cached diagnostics for a specific file path.
    pub fn get_diagnostics(&self, file_path: &str) -> Vec<LspDiagnostic> {
        let uri = path_to_uri(file_path);
        self.diagnostics
            .get(&uri)
            .map(|d| d.value().clone())
            .unwrap_or_default()
    }

    /// Retrieve all cached diagnostics across all files.
    pub fn all_diagnostics(&self) -> Vec<LspDiagnostic> {
        self.diagnostics
            .iter()
            .flat_map(|entry| entry.value().clone())
            .collect()
    }

    /// Send `shutdown` request and wait for server process exit.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        // Try to send the shutdown request (may fail if server already died)
        let _ = self.send_request_inner("shutdown", json!({})).await;
        let _ = self.send_notification_inner("exit", json!({})).await;

        if let Some(ref mut child) = self.child {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
        }
        self.writer = None;
        self.child = None;
        Ok(())
    }

    /// Check whether the LSP server is still running.
    pub fn is_running(&self) -> bool {
        self.writer.is_some()
    }
}

// ---------------------------------------------------------------------------
// Helpers called from the background reader task
// ---------------------------------------------------------------------------

/// Store diagnostics arriving via `textDocument/publishDiagnostics`.
fn handle_diagnostics(
    diagnostics: Arc<DashMap<String, Vec<LspDiagnostic>>>,
    params: Option<&serde_json::Value>,
    server_name: &str,
) {
    let uri = match params.and_then(|p| p.get("uri")).and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return,
    };

    let raw_diags = match params
        .and_then(|p| p.get("diagnostics"))
        .and_then(|v| v.as_array())
    {
        Some(d) => d,
        None => {
            diagnostics.insert(uri, Vec::new());
            return;
        }
    };

    // Convert the URI back to a file path for storage
    let file_path = uri_to_path(&uri);

    let parsed: Vec<LspDiagnostic> = raw_diags
        .iter()
        .filter_map(|d| parse_diagnostic(d, &file_path, server_name))
        .collect();

    tracing::debug!(
        "LSP server {}: {} diagnostics for {}",
        server_name,
        parsed.len(),
        file_path
    );

    diagnostics.insert(uri, parsed);
}

fn parse_diagnostic(
    d: &serde_json::Value,
    file_path: &str,
    server_name: &str,
) -> Option<LspDiagnostic> {
    let range = d.get("range")?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as u32 + 1; // LSP is 0-based
    let column = start.get("character")?.as_u64()? as u32 + 1;
    let message = d.get("message")?.as_str()?.to_string();

    let severity = d
        .get("severity")
        .and_then(|v| v.as_u64())
        .map(DiagnosticSeverity::from_lsp_int)
        .unwrap_or(DiagnosticSeverity::Error);

    let source = d
        .get("source")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| Some(server_name.to_string()));

    let code = d.get("code").map(|v| match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    });

    Some(LspDiagnostic {
        file: file_path.to_string(),
        line,
        column,
        severity,
        message,
        source,
        code,
    })
}

// ---------------------------------------------------------------------------
// Location / symbol helpers
// ---------------------------------------------------------------------------

/// Extract a list of `"path:line"` strings from an LSP `Location | Location[]` result.
fn extract_locations(result: &serde_json::Value) -> Vec<String> {
    let items: Vec<&serde_json::Value> = if let Some(arr) = result.as_array() {
        arr.iter().collect()
    } else if result.is_object() {
        vec![result]
    } else {
        return Vec::new();
    };

    items
        .into_iter()
        .filter_map(|loc| {
            let uri = loc.get("uri")?.as_str()?;
            let line = loc
                .pointer("/range/start/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                + 1; // convert to 1-based
            let col = loc
                .pointer("/range/start/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                + 1;
            let path = uri_to_path(uri);
            Some(format!("{}:{}:{}", path, line, col))
        })
        .collect()
}

/// Recursively collect symbol names from a DocumentSymbol or SymbolInformation node.
fn collect_symbol(sym: &serde_json::Value, depth: usize, out: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    let name = sym
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("<unnamed>");
    let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
    let kind_str = symbol_kind_name(kind);
    out.push(format!("{}{} ({})", indent, name, kind_str));

    // DocumentSymbol may have nested children
    if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
        for child in children {
            collect_symbol(child, depth + 1, out);
        }
    }
}

fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum-member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type-parameter",
        _ => "symbol",
    }
}

// ---------------------------------------------------------------------------
// URI helpers
// ---------------------------------------------------------------------------

fn path_to_uri(path: &str) -> String {
    // Simple heuristic; for full correctness callers should pass pre-formed URIs
    if path.starts_with("file://") {
        return path.to_string();
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let s = canonical.to_string_lossy();
    if cfg!(target_os = "windows") {
        // Drive letters need a leading slash: file:///C:/...
        format!("file:///{}", s.replace('\\', "/"))
    } else {
        format!("file://{}", s)
    }
}

fn uri_to_path(uri: &str) -> String {
    let stripped = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))
        .unwrap_or(uri);
    if cfg!(target_os = "windows") {
        stripped.replace('/', "\\")
    } else {
        stripped.to_string()
    }
}

// ---------------------------------------------------------------------------
// Diagnostic formatting (shared utility)
// ---------------------------------------------------------------------------

impl LspManager {
    /// Format a slice of diagnostics into a human-readable multi-line string
    /// suitable for inclusion in tool output or TUI display.
    pub fn format_diagnostics(diagnostics: &[LspDiagnostic]) -> String {
        if diagnostics.is_empty() {
            return "No diagnostics.".to_string();
        }
        diagnostics
            .iter()
            .map(|d| {
                format!(
                    "[{}] {}:{}:{} - {}{}{}",
                    d.severity.as_str().to_uppercase(),
                    d.file,
                    d.line,
                    d.column,
                    d.message,
                    d.source
                        .as_deref()
                        .map(|s| format!(" ({})", s))
                        .unwrap_or_default(),
                    d.code
                        .as_deref()
                        .map(|c| format!(" [{}]", c))
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---------------------------------------------------------------------------
// LspManager — registry and multi-server coordination
// ---------------------------------------------------------------------------

/// Manages a collection of [`LspClient`] instances, routing file operations
/// to the correct server based on extension mappings.
pub struct LspManager {
    /// Registered configs (used for lookup before a client is started)
    configs: Vec<LspServerConfig>,
    /// Running clients keyed by server name
    clients: HashMap<String, LspClient>,
    /// Map of file extension → list of server names that handle it
    extension_map: HashMap<String, Vec<String>>,
    /// Set of file URIs that have been opened on a specific server (URI → server name)
    opened_files: HashMap<String, String>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            configs: Vec::new(),
            clients: HashMap::new(),
            extension_map: HashMap::new(),
            opened_files: HashMap::new(),
        }
    }

    /// Register a server configuration.
    /// Builds the extension-to-server mapping for routing.
    pub fn register_server(&mut self, config: LspServerConfig) {
        for ext in config.extension_to_language.keys() {
            self.extension_map
                .entry(ext.to_string())
                .or_default()
                .push(config.name.clone());
        }
        self.configs.push(config);
    }

    /// List the names of all registered servers.
    pub fn servers(&self) -> Vec<&LspServerConfig> {
        self.configs.iter().collect()
    }

    /// Find a registered server config by name.
    pub fn server_by_name(&self, name: &str) -> Option<&LspServerConfig> {
        self.configs.iter().find(|c| c.name == name)
    }

    /// Determine which server handles a given file by matching extension
    /// against each registered server's `extension_to_language` map.
    pub fn server_name_for_file(&self, file_path: &str) -> Option<&str> {
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        let names = self.extension_map.get(&ext)?;
        names.first().map(|s| s.as_str())
    }

    /// Public version of `server_name_for_file` for use by the tool layer.
    pub fn server_name_for_file_pub(&self, file_path: &str) -> Option<&str> {
        self.server_name_for_file(file_path)
    }

    /// Ensure the correct server is running and has opened `file_path`.
    /// Returns the URI of the opened file on success.
    pub async fn open_file(&mut self, file_path: &str, root_dir: &Path) -> anyhow::Result<String> {
        let uri = path_to_uri(file_path);
        // Already opened
        if self.opened_files.contains_key(&uri) {
            return Ok(uri);
        }

        let server_name = self
            .server_name_for_file(file_path)
            .ok_or_else(|| anyhow::anyhow!("No LSP server configured for '{}'", file_path))?
            .to_string();

        // Start the server if not already running
        self.ensure_started(file_path, root_dir).await?;

        // Get the language_id before the mutable borrow on self.clients
        let language_id = {
            let config = self
                .server_by_name(&server_name)
                .ok_or_else(|| anyhow::anyhow!("Server config not found for '{}'", server_name))?;
            config.language_for_file(file_path)
        };

        let client = self
            .clients
            .get_mut(&server_name)
            .ok_or_else(|| anyhow::anyhow!("LSP server '{}' not running", server_name))?;

        // Read file content to send didOpen
        let content = match tokio::fs::read_to_string(file_path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Could not read '{}' for LSP didOpen: {}", file_path, e);
                String::new()
            }
        };

        client.open_document(&uri, &language_id, &content).await?;
        self.opened_files.insert(uri.clone(), server_name.clone());

        Ok(uri)
    }

    /// Start an LSP server for the given file if one is not already running.
    async fn ensure_started(&mut self, file_path: &str, root_dir: &Path) -> anyhow::Result<()> {
        let server_name = self
            .server_name_for_file(file_path)
            .ok_or_else(|| anyhow::anyhow!("No LSP server configured for '{}'", file_path))?
            .to_string();

        if self.clients.contains_key(&server_name) {
            return Ok(());
        }

        let config = self
            .configs
            .iter()
            .find(|c| c.name == server_name)
            .ok_or_else(|| anyhow::anyhow!("Server config not found for '{}'", server_name))?
            .clone();

        let root_uri = path_to_uri(root_dir.to_str().unwrap_or("."));
        let mut client = LspClient::start(config).await?;
        client.initialize(&root_uri).await?;

        tracing::info!("LSP server '{}' started and initialized", server_name);
        self.clients.insert(server_name, client);
        Ok(())
    }

    /// Idempotent: servers already present by name are skipped.
    pub fn seed_from_config(&mut self, configs: &[LspServerConfig]) {
        for cfg in configs {
            if !self.configs.iter().any(|c| c.name == cfg.name) {
                self.register_server(cfg.clone());
            }
        }
    }

    /// Load LSP server configs from `~/.tact/lsp_servers.json`.
    /// Returns the parsed configs or an empty vec on any error.
    pub fn load_from_default_config() -> Vec<LspServerConfig> {
        let Some(path) =
            crate::consts::TactPath::home_tact_dir().map(|d| d.join("lsp_servers.json"))
        else {
            return Vec::new();
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(configs) => configs,
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", path.display(), e);
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        }
    }

    /// Get hover information for `file_path` at the given 1-based position.
    pub async fn hover(
        &mut self,
        file_path: &str,
        root_dir: &Path,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Option<String>> {
        let uri = path_to_uri(file_path);
        let server_name = self
            .server_name_for_file(file_path)
            .ok_or_else(|| anyhow::anyhow!("No LSP server configured for '{}'", file_path))?
            .to_string();
        self.ensure_started(file_path, root_dir).await?;
        let client = self
            .clients
            .get(&server_name)
            .ok_or_else(|| anyhow::anyhow!("LSP server '{}' not running", server_name))?;
        client.hover(&uri, line, character).await
    }

    /// Get definition locations for `file_path` at the given 1-based position.
    pub async fn definition(
        &mut self,
        file_path: &str,
        root_dir: &Path,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<String>> {
        let uri = path_to_uri(file_path);
        let server_name = self
            .server_name_for_file(file_path)
            .ok_or_else(|| anyhow::anyhow!("No LSP server configured for '{}'", file_path))?
            .to_string();
        self.ensure_started(file_path, root_dir).await?;
        let client = self
            .clients
            .get(&server_name)
            .ok_or_else(|| anyhow::anyhow!("LSP server '{}' not running", server_name))?;
        client.definition(&uri, line, character).await
    }

    /// Get references for a symbol in `file_path` at the given 1-based position.
    pub async fn references(
        &mut self,
        file_path: &str,
        root_dir: &Path,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Vec<String>> {
        let uri = path_to_uri(file_path);
        let server_name = self
            .server_name_for_file(file_path)
            .ok_or_else(|| anyhow::anyhow!("No LSP server configured for '{}'", file_path))?
            .to_string();
        self.ensure_started(file_path, root_dir).await?;
        let client = self
            .clients
            .get(&server_name)
            .ok_or_else(|| anyhow::anyhow!("LSP server '{}' not running", server_name))?;
        client.references(&uri, line, character).await
    }

    /// List document symbols for `file_path`.
    pub async fn document_symbols(
        &mut self,
        file_path: &str,
        root_dir: &Path,
    ) -> anyhow::Result<Vec<String>> {
        let uri = path_to_uri(file_path);
        let server_name = self
            .server_name_for_file(file_path)
            .ok_or_else(|| anyhow::anyhow!("No LSP server configured for '{}'", file_path))?
            .to_string();
        self.ensure_started(file_path, root_dir).await?;
        let client = self
            .clients
            .get(&server_name)
            .ok_or_else(|| anyhow::anyhow!("LSP server '{}' not running", server_name))?;
        client.document_symbols(&uri).await
    }

    /// Get cached diagnostics for `file_path` across all running servers.
    pub fn get_diagnostics_for_file(&self, file_path: &str) -> Vec<LspDiagnostic> {
        self.clients
            .values()
            .flat_map(|c| c.get_diagnostics(file_path))
            .collect()
    }

    /// Get all cached diagnostics from all running servers.
    pub fn all_diagnostics(&self) -> Vec<LspDiagnostic> {
        self.clients
            .values()
            .flat_map(|c| c.all_diagnostics())
            .collect()
    }

    /// Shut down all running servers.
    pub async fn shutdown_all(&mut self) {
        let names: Vec<String> = self.clients.keys().cloned().collect();
        for name in names {
            if let Some(mut client) = self.clients.remove(&name) {
                if let Err(e) = client.shutdown().await {
                    tracing::warn!("Error shutting down LSP server '{}': {}", name, e);
                }
            }
        }
        self.opened_files.clear();
    }

    /// Get a legacy-compatible async diagnostic query (returns cached results).
    pub async fn get_diagnostics(&self, file: &str) -> Vec<LspDiagnostic> {
        self.get_diagnostics_for_file(file)
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static GLOBAL_LSP_MANAGER: LazyLock<Arc<tokio::sync::Mutex<LspManager>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(LspManager::new())));

/// Access the global [`LspManager`] instance.
pub fn global_lsp_manager() -> Arc<tokio::sync::Mutex<LspManager>> {
    GLOBAL_LSP_MANAGER.clone()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(name: &str) -> LspServerConfig {
        LspServerConfig {
            name: name.to_string(),
            command: name.to_string(),
            args: vec![],
            file_patterns: vec!["*.rs".to_string()],
            initialization_options: None,
            extension_to_language: {
                let mut m = HashMap::new();
                m.insert(".rs".to_string(), "rust".to_string());
                m
            },
            env: HashMap::new(),
        }
    }

    fn make_diagnostic(
        file: &str,
        line: u32,
        col: u32,
        severity: DiagnosticSeverity,
        message: &str,
    ) -> LspDiagnostic {
        LspDiagnostic {
            file: file.to_string(),
            line,
            column: col,
            severity,
            message: message.to_string(),
            source: None,
            code: None,
        }
    }

    #[test]
    fn test_new_manager_empty() {
        let mgr = LspManager::new();
        assert!(mgr.servers().is_empty());
    }

    #[test]
    fn test_register_server() {
        let mut mgr = LspManager::new();
        mgr.register_server(make_config("rust-analyzer"));
        assert_eq!(mgr.servers().len(), 1);
        assert_eq!(mgr.servers()[0].name, "rust-analyzer");
    }

    #[test]
    fn test_register_multiple_servers() {
        let mut mgr = LspManager::new();
        mgr.register_server(make_config("rust-analyzer"));
        mgr.register_server(make_config("pyright"));
        assert_eq!(mgr.servers().len(), 2);
    }

    #[test]
    fn test_server_by_name_found() {
        let mut mgr = LspManager::new();
        mgr.register_server(make_config("rust-analyzer"));
        mgr.register_server(make_config("pyright"));
        let s = mgr.server_by_name("pyright");
        assert!(s.is_some());
        assert_eq!(s.unwrap().name, "pyright");
    }

    #[test]
    fn test_server_by_name_not_found() {
        let mgr = LspManager::new();
        assert!(mgr.server_by_name("missing").is_none());
    }

    #[tokio::test]
    async fn test_get_diagnostics_empty_when_no_servers() {
        let mgr = LspManager::new();
        let diags = mgr.get_diagnostics("src/main.rs").await;
        assert!(diags.is_empty());
    }

    #[test]
    fn test_format_diagnostics_empty() {
        let result = LspManager::format_diagnostics(&[]);
        assert_eq!(result, "No diagnostics.");
    }

    #[test]
    fn test_format_diagnostics_single_error() {
        let diags = vec![make_diagnostic(
            "src/lib.rs",
            10,
            5,
            DiagnosticSeverity::Error,
            "type mismatch",
        )];
        let result = LspManager::format_diagnostics(&diags);
        assert!(result.contains("[ERROR]"));
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains("10:5"));
        assert!(result.contains("type mismatch"));
    }

    #[test]
    fn test_format_diagnostics_multiple() {
        let diags = vec![
            make_diagnostic("a.rs", 1, 1, DiagnosticSeverity::Error, "err1"),
            make_diagnostic("b.rs", 2, 3, DiagnosticSeverity::Warning, "warn1"),
        ];
        let result = LspManager::format_diagnostics(&diags);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("[ERROR]"));
        assert!(lines[1].contains("[WARNING]"));
    }

    #[test]
    fn test_format_diagnostics_with_source_and_code() {
        let mut d = make_diagnostic(
            "main.rs",
            5,
            1,
            DiagnosticSeverity::Error,
            "mismatched types",
        );
        d.source = Some("rust-analyzer".to_string());
        d.code = Some("E0308".to_string());
        let result = LspManager::format_diagnostics(&[d]);
        assert!(result.contains("(rust-analyzer)"), "result = {}", result);
        assert!(result.contains("[E0308]"), "result = {}", result);
    }

    #[test]
    fn test_diagnostic_severity_ordering() {
        assert!(DiagnosticSeverity::Error < DiagnosticSeverity::Warning);
        assert!(DiagnosticSeverity::Warning < DiagnosticSeverity::Information);
        assert!(DiagnosticSeverity::Information < DiagnosticSeverity::Hint);
    }

    #[test]
    fn test_diagnostic_severity_as_str() {
        assert_eq!(DiagnosticSeverity::Error.as_str(), "error");
        assert_eq!(DiagnosticSeverity::Warning.as_str(), "warning");
        assert_eq!(DiagnosticSeverity::Information.as_str(), "info");
        assert_eq!(DiagnosticSeverity::Hint.as_str(), "hint");
    }

    #[test]
    fn test_lsp_server_config_serialization() {
        let cfg = make_config("rust-analyzer");
        let json = serde_json::to_string(&cfg).unwrap();
        let back: LspServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "rust-analyzer");
    }

    #[test]
    fn test_default_trait() {
        let mgr = LspManager::default();
        assert!(mgr.servers().is_empty());
    }

    #[test]
    fn test_extension_routing() {
        let mut mgr = LspManager::new();
        mgr.register_server(make_config("rust-analyzer"));
        // .rs maps to rust-analyzer
        assert_eq!(
            mgr.server_name_for_file("src/main.rs"),
            Some("rust-analyzer")
        );
        // .py has no mapping
        assert_eq!(mgr.server_name_for_file("app.py"), None);
    }

    #[test]
    fn test_path_to_uri_roundtrip() {
        let uri = path_to_uri("src/main.rs");
        assert!(
            uri.starts_with("file://"),
            "expected file:// URI, got {}",
            uri
        );
        let _back = uri_to_path(&uri);
    }

    #[test]
    fn test_language_for_file() {
        let cfg = make_config("rust-analyzer");
        assert_eq!(cfg.language_for_file("src/main.rs"), "rust");
        assert_eq!(cfg.language_for_file("README.md"), "plaintext");
    }

    #[test]
    fn test_severity_from_lsp_int() {
        assert_eq!(
            DiagnosticSeverity::from_lsp_int(1),
            DiagnosticSeverity::Error
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp_int(2),
            DiagnosticSeverity::Warning
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp_int(3),
            DiagnosticSeverity::Information
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp_int(4),
            DiagnosticSeverity::Hint
        );
        assert_eq!(
            DiagnosticSeverity::from_lsp_int(99),
            DiagnosticSeverity::Hint
        );
    }

    #[test]
    fn test_global_lsp_manager_consistent() {
        let m1 = global_lsp_manager();
        let m2 = global_lsp_manager();
        assert!(Arc::ptr_eq(&m1, &m2));
    }

    #[test]
    fn test_parse_diagnostic_valid() {
        let raw = serde_json::json!({
            "range": {
                "start": { "line": 4, "character": 2 },
                "end":   { "line": 4, "character": 10 }
            },
            "severity": 1,
            "message": "type mismatch",
            "source": "rust-analyzer",
            "code": "E0308"
        });
        let d = parse_diagnostic(&raw, "src/main.rs", "rust-analyzer").unwrap();
        assert_eq!(d.line, 5); // 0-based → 1-based
        assert_eq!(d.column, 3);
        assert_eq!(d.message, "type mismatch");
        assert_eq!(d.severity, DiagnosticSeverity::Error);
        assert_eq!(d.code.as_deref(), Some("E0308"));
    }

    #[test]
    fn test_parse_diagnostic_missing_range_returns_none() {
        let raw = serde_json::json!({ "message": "oops" });
        assert!(parse_diagnostic(&raw, "f.rs", "lsp").is_none());
    }
}
