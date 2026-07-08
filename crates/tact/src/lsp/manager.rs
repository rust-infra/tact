use std::collections::HashMap;
use std::path::Path;

use super::client::LspClient;
use super::config::LspServerConfig;
use super::diagnostic::{LspDiagnostic, format_diagnostics as format_diagnostic_lines};
use super::uri::path_to_uri;

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

impl LspManager {
    /// Format a slice of diagnostics into a human-readable multi-line string.
    pub fn format_diagnostics(diagnostics: &[LspDiagnostic]) -> String {
        format_diagnostic_lines(diagnostics)
    }
}
