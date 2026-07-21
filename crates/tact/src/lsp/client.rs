use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use dashmap::DashMap;
use serde_json::json;
use tokio::{
    io::{BufReader, BufWriter},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, oneshot},
};

use super::{
    config::LspServerConfig,
    diagnostic::{LspDiagnostic, handle_diagnostics},
    protocol::{read_message, send_message},
    symbols::{collect_symbol, extract_locations},
    uri::path_to_uri,
};

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
