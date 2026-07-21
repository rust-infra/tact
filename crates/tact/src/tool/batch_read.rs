//! BatchRead tool: read multiple files in parallel.
//!
//! All paths are validated before any read begins. Files are read
//! concurrently via `tokio::spawn` + `join_all`.

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;

use crate::tool::{ToolContext, safe_path};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FileRead {
    #[schemars(description = "Path to the file to read, relative to the current workspace.")]
    pub path: String,
    #[schemars(
        description = "Optional maximum number of lines to return from the start of the file."
    )]
    pub limit: Option<u64>,
    #[schemars(
        description = "Optional 1-based line number to start reading from. Use with limit to read specific sections."
    )]
    pub offset: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BatchReadInput {
    #[schemars(description = "List of files to read in parallel.")]
    pub files: Vec<FileRead>,
    #[schemars(
        description = "Optional human-readable description of what this batch read is for."
    )]
    #[serde(default)]
    #[allow(dead_code)]
    pub description: Option<String>,
}

#[tool(
    name = "batch_read",
    description = "Read multiple files in parallel. All paths are validated before any \
                    read begins. Returns file contents separated by headers."
)]
pub async fn batch_read(ctx: ToolContext, input: BatchReadInput) -> Result<String> {
    if input.files.is_empty() {
        return Err(anyhow::anyhow!("files array must not be empty"));
    }

    // Phase 1: validate all paths before reading
    let mut prepare: Vec<(String, Option<u64>, Option<u64>)> =
        Vec::with_capacity(input.files.len());
    let mut errors: Vec<String> = Vec::new();

    for (i, f) in input.files.iter().enumerate() {
        match safe_path(&ctx.work_dir, &f.path) {
            Ok(p) => prepare.push((p.display().to_string(), f.limit, f.offset)),
            Err(e) => {
                errors.push(format!("File {}: invalid path {}: {}", i, f.path, e));
            }
        }
    }

    if !errors.is_empty() {
        return Err(anyhow::anyhow!(
            "BatchRead aborted — {} validation error(s):\n{}",
            errors.len(),
            errors.join("\n")
        ));
    }

    // Phase 2: read all files in parallel
    let total = prepare.len();
    let handles: Vec<_> = prepare
        .into_iter()
        .map(|(path_str, limit, offset)| {
            tokio::spawn(async move {
                let content = tokio::fs::read_to_string(&path_str).await;
                (path_str, limit, offset, content)
            })
        })
        .collect();

    let results = futures_util::future::join_all(handles).await;

    // Phase 3: assemble output with file headers
    let mut output = String::new();
    output.push_str(&format!(
        "BatchRead {} file{}:\n\n",
        total,
        if total != 1 { "s" } else { "" }
    ));

    for (i, result) in results.into_iter().enumerate() {
        let (path_str, limit, offset, content) =
            result.map_err(|e| anyhow::anyhow!("BatchRead task panicked: {e}"))?;

        output.push_str(&format!("── {} ──\n", path_str));

        match content {
            Err(e) => {
                output.push_str(&format!("Error reading file: {}\n", e));
            }
            Ok(text) => {
                let mut lines: Vec<&str> = text.lines().collect();

                let skip = offset
                    .map(|o| (o.saturating_sub(1) as usize).min(lines.len()))
                    .unwrap_or(0);
                if skip > 0 {
                    lines = lines.into_iter().skip(skip).collect();
                    output.push_str(&format!("... ({} lines skipped) ...\n", skip));
                }

                if let Some(lim) = limit
                    && (lim as usize) < lines.len()
                {
                    let remaining = lines.len() - lim as usize;
                    lines.truncate(lim as usize);
                    let chunk = lines.join("\n");
                    output.push_str(&chunk);
                    output.push_str(&format!("\n... ({} more lines)\n", remaining));
                } else {
                    output.push_str(&lines.join("\n"));
                    output.push('\n');
                }
            }
        }

        if i < total - 1 {
            output.push('\n');
        }
    }

    // Hard cap at 200 KB to avoid blowing up context
    Ok(output.chars().take(200_000).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context, write_workspace_file};

    #[tokio::test]
    async fn batch_read_rejects_invalid_path_before_reading() {
        let context = test_context("batch_read_rejects_invalid_path_before_reading");
        write_workspace_file(&context.work_dir, "ok.txt", "content");

        let error = run_tool(
            &context,
            BatchReadTool,
            "batch_read",
            serde_json::json!({
                "files": [
                    { "path": "ok.txt" },
                    { "path": "../../../etc/passwd" }
                ]
            }),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("BatchRead aborted"));
        assert!(error.to_string().contains("invalid path"));
    }

    #[tokio::test]
    async fn batch_read_applies_offset_and_limit() {
        let context = test_context("batch_read_applies_offset_and_limit");
        write_workspace_file(&context.work_dir, "lines.txt", "a\nb\nc\nd\n");

        let output = run_tool(
            &context,
            BatchReadTool,
            "batch_read",
            serde_json::json!({
                "files": [{ "path": "lines.txt", "offset": 2, "limit": 2 }]
            }),
        )
        .await
        .unwrap();

        assert!(output.contains("BatchRead 1 file"));
        assert!(output.contains("... (1 lines skipped) ..."));
        assert!(output.contains("b"));
        assert!(output.contains("... (1 more lines)"));
    }
}
