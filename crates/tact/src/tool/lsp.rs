// LspTool — code intelligence via Language Server Protocol.
//
// Supports hover, definition, references, document symbols, and diagnostics.
// Ported from claurst; LSP server configs are loaded from
// `~/.tact/lsp_servers.json`.

use crate::lsp::{self, LspManager};
use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LspInput {
    #[schemars(
        description = "The LSP action: hover, definition, references, symbols, or diagnostics."
    )]
    pub action: String,
    #[schemars(description = "Absolute or working-directory-relative path to the source file.")]
    pub file: String,
    #[schemars(description = "1-based line number (required for hover, definition, references).")]
    #[serde(default)]
    pub line: Option<u32>,
    #[schemars(
        description = "1-based column number (required for hover, definition, references)."
    )]
    #[serde(default)]
    pub column: Option<u32>,
}

#[tool(
    name = "lsp",
    description = "Query a language server for code intelligence. Supports hover documentation, \
                    go-to-definition, find-references, document symbols, and diagnostics. \
                    Language servers must be configured in ~/.tact/lsp_servers.json."
)]
pub async fn query_lsp(ctx: ToolContext, input: LspInput) -> Result<String> {
    let action = input.action;
    let file_raw = input.file;
    let line = input.line.unwrap_or(1);
    let column = input.column.unwrap_or(1);

    // Resolve to absolute path
    let file_path = if std::path::Path::new(&file_raw).is_absolute() {
        file_raw.clone()
    } else {
        ctx.work_dir.join(&file_raw).to_string_lossy().into_owned()
    };

    // Seed the global LSP manager from the default config file
    let lsp_manager_arc = lsp::global_lsp_manager();
    {
        let mut manager = lsp_manager_arc.lock().await;
        let configs = LspManager::load_from_default_config();
        manager.seed_from_config(&configs);
    }

    // Check that at least one server is registered for this file
    {
        let manager = lsp_manager_arc.lock().await;
        if manager.server_name_for_file_pub(&file_path).is_none() {
            return Ok(format!(
                "No LSP server configured for '{}'. \
                 Add a server entry to ~/.tact/lsp_servers.json to enable \
                 code intelligence for this file type. Example:\n\
                 [\n  {{\n    \"name\": \"rust-analyzer\",\n    \
                 \"command\": \"rust-analyzer\",\n    \"args\": [],\n    \
                 \"file_patterns\": [\"*.rs\"],\n    \
                 \"extension_to_language\": {{\".rs\": \"rust\"}}\n  }}\n]",
                file_path
            ));
        }
    }

    // Ensure the file is opened on its LSP server
    {
        let mut manager = lsp_manager_arc.lock().await;
        if let Err(e) = manager.open_file(&file_path, &ctx.work_dir).await {
            return Err(anyhow::anyhow!("Failed to open file in LSP: {}", e));
        }
    }

    // Dispatch action
    match action.as_str() {
        "hover" => {
            let result = {
                let mut manager = lsp_manager_arc.lock().await;
                manager.hover(&file_path, &ctx.work_dir, line, column).await
            };
            match result {
                Ok(Some(text)) => Ok(text),
                Ok(None) => Ok(format!(
                    "No hover information at {}:{}:{}",
                    file_path, line, column
                )),
                Err(e) => Err(anyhow::anyhow!("hover failed: {}", e)),
            }
        }

        "definition" => {
            let result = {
                let mut manager = lsp_manager_arc.lock().await;
                manager
                    .definition(&file_path, &ctx.work_dir, line, column)
                    .await
            };
            match result {
                Ok(locs) if locs.is_empty() => Ok(format!(
                    "No definition found at {}:{}:{}",
                    file_path, line, column
                )),
                Ok(locs) => Ok(locs.join("\n")),
                Err(e) => Err(anyhow::anyhow!("definition failed: {}", e)),
            }
        }

        "references" => {
            let result = {
                let mut manager = lsp_manager_arc.lock().await;
                manager
                    .references(&file_path, &ctx.work_dir, line, column)
                    .await
            };
            match result {
                Ok(locs) if locs.is_empty() => Ok(format!(
                    "No references found at {}:{}:{}",
                    file_path, line, column
                )),
                Ok(locs) => Ok(format!("{} reference(s):\n{}", locs.len(), locs.join("\n"))),
                Err(e) => Err(anyhow::anyhow!("references failed: {}", e)),
            }
        }

        "symbols" => {
            let result = {
                let mut manager = lsp_manager_arc.lock().await;
                manager.document_symbols(&file_path, &ctx.work_dir).await
            };
            match result {
                Ok(syms) if syms.is_empty() => Ok(format!("No symbols found in '{}'.", file_path)),
                Ok(syms) => Ok(syms.join("\n")),
                Err(e) => Err(anyhow::anyhow!("symbols failed: {}", e)),
            }
        }

        "diagnostics" => {
            // Give the server a short window to deliver diagnostics
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            let diagnostics = {
                let manager = lsp_manager_arc.lock().await;
                manager.get_diagnostics_for_file(&file_path)
            };

            if diagnostics.is_empty() {
                return Ok(format!("No diagnostics for '{}'.", file_path));
            }

            let output = LspManager::format_diagnostics(&diagnostics);
            Ok(output)
        }

        other => Err(anyhow::anyhow!(
            "Unknown action '{}'. Valid actions: hover, definition, references, symbols, diagnostics",
            other
        )),
    }
}
