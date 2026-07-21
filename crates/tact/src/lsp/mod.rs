//! Language Server Protocol client.
//!
//! Implements the client side of the LSP JSON-RPC protocol over the LSP
//! server's stdin/stdout.  Each [`LspClient`] manages one server process;
//! [`LspManager`] tracks a collection of clients keyed by server name.

mod client;
mod config;
mod diagnostic;
mod manager;
mod protocol;
mod symbols;
mod uri;

use std::sync::{Arc, LazyLock};

pub use client::LspClient;
pub use config::LspServerConfig;
pub use diagnostic::{DiagnosticSeverity, LspDiagnostic, format_diagnostics};
pub use manager::LspManager;

static GLOBAL_LSP_MANAGER: LazyLock<Arc<tokio::sync::Mutex<LspManager>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(LspManager::new())));

/// Access the global [`LspManager`] instance.
pub fn global_lsp_manager() -> Arc<tokio::sync::Mutex<LspManager>> {
    GLOBAL_LSP_MANAGER.clone()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use super::{
        DiagnosticSeverity, LspDiagnostic, LspManager, LspServerConfig,
        diagnostic::parse_diagnostic,
        global_lsp_manager,
        uri::{path_to_uri, uri_to_path},
    };

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
