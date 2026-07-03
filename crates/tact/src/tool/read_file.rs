use crate::tool::{ToolContext, safe_path};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::fs;
use tool_refactor_macros::tool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileInput {
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

#[tool(name = "read_file", description = "Read file contents.")]
pub async fn read_file(ctx: ToolContext, input: ReadFileInput) -> Result<String> {
    let path = safe_path(&ctx.work_dir, &input.path)?;

    let content = fs::read_to_string(path)
        .await
        .map_err(|e| anyhow::anyhow!("Error: {}", e))?;

    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    let offset = input
        .offset
        .map(|o| o.saturating_sub(1) as usize)
        .unwrap_or(0);
    if offset > 0 {
        if offset >= lines.len() {
            return Ok(String::new());
        }
        lines = lines.into_iter().skip(offset).collect();
        lines.insert(0, format!("... ({} lines skipped) ...", offset));
    }

    if let Some(limit) = input.limit
        && (limit as usize) < lines.len()
    {
        let remaining = lines.len() - limit as usize;
        lines.truncate(limit as usize);
        lines.push(format!("... ({} more lines)", remaining));
    }

    let result = lines.join("\n");

    Ok(result.chars().take(50000).collect())
}

#[cfg(test)]
mod tests {
    use crate::tool::test_support::{run_tool, test_context, write_workspace_file};

    use super::*;

    #[tokio::test]
    async fn read_file_errors_when_file_missing() {
        let context = test_context("read_file_errors_when_file_missing");

        let error = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "missing.txt" }),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("No such file")
                || error.to_string().contains("Error:")
        );
    }

    #[tokio::test]
    async fn read_file_returns_empty_when_offset_past_end() {
        let context = test_context("read_file_returns_empty_when_offset_past_end");
        write_workspace_file(&context.work_dir, "short.txt", "only line\n");

        let output = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "short.txt", "offset": 99 }),
        )
        .await
        .unwrap();

        assert_eq!(output, "");
    }
}
