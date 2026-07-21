use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};

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
