use std::sync::Arc;

use dashmap::DashMap;
use serde_json;

use super::uri::uri_to_path;

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

    pub(crate) fn from_lsp_int(n: u64) -> Self {
        match n {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            _ => Self::Hint,
        }
    }
}

pub(crate) fn handle_diagnostics(
    diagnostics: Arc<DashMap<String, Vec<LspDiagnostic>>>,
    params: Option<&serde_json::Value>,
    server_name: &str,
) {
    let uri = match params.and_then(|p| p.get("uri")).and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return,
    };

    let raw_diags = match params.and_then(|p| p.get("diagnostics")).and_then(|v| v.as_array()) {
        Some(d) => d,
        None => {
            diagnostics.insert(uri, Vec::new());
            return;
        },
    };

    // Convert the URI back to a file path for storage
    let file_path = uri_to_path(&uri);

    let parsed: Vec<LspDiagnostic> =
        raw_diags.iter().filter_map(|d| parse_diagnostic(d, &file_path, server_name)).collect();

    tracing::debug!("LSP server {}: {} diagnostics for {}", server_name, parsed.len(), file_path);

    diagnostics.insert(uri, parsed);
}

pub(crate) fn parse_diagnostic(d: &serde_json::Value, file_path: &str, server_name: &str) -> Option<LspDiagnostic> {
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

    let source =
        d.get("source").and_then(|v| v.as_str()).map(|s| s.to_string()).or_else(|| Some(server_name.to_string()));

    let code = d.get("code").map(|v| match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    });

    Some(LspDiagnostic { file: file_path.to_string(), line, column, severity, message, source, code })
}

/// Format diagnostics into a human-readable multi-line string.
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
                d.source.as_deref().map(|s| format!(" ({})", s)).unwrap_or_default(),
                d.code.as_deref().map(|c| format!(" [{}]", c)).unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
