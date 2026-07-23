use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::fs;
use tool_refactor_macros::tool;

use crate::tool::{ToolContext, safe_path};

const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;

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
    use super::*;
    use crate::tool::test_support::{run_tool, test_context, write_workspace_file};

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

        assert!(error.to_string().contains("No such file") || error.to_string().contains("Error:"));
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

    fn numbered_lines(count: usize) -> String {
        (1..=count)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[tokio::test]
    async fn read_file_default_page_returns_partial_when_more_lines_exist() {
        let context = test_context("read_file_default_page_returns_partial");
        write_workspace_file(
            &context.work_dir,
            "long.txt",
            &numbered_lines(READ_FILE_DEFAULT_MAX_LINES + 3),
        );

        let output = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "long.txt" }),
        )
        .await
        .unwrap();

        assert!(
            output.starts_with(&format!(
                "[PARTIAL view — lines 1-{}; continue with offset={}]\n\n",
                READ_FILE_DEFAULT_MAX_LINES,
                READ_FILE_DEFAULT_MAX_LINES + 1
            )),
            "expected leading PARTIAL marker, got: {output}"
        );
        assert!(output.contains("line1\n"));
        assert!(output.contains(&format!("line{}", READ_FILE_DEFAULT_MAX_LINES)));
        assert!(!output.contains(&format!("line{}", READ_FILE_DEFAULT_MAX_LINES + 1)));
    }

    #[tokio::test]
    async fn read_file_continuation_offset_returns_remaining_without_duplicate() {
        let context = test_context("read_file_continuation_offset");
        write_workspace_file(
            &context.work_dir,
            "long.txt",
            &numbered_lines(READ_FILE_DEFAULT_MAX_LINES + 2),
        );

        let first = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "long.txt" }),
        )
        .await
        .unwrap();
        assert!(first.contains("continue with offset=2001"));

        let second = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "long.txt", "offset": 2001 }),
        )
        .await
        .unwrap();

        // Remaining two lines fit in one default page → complete body, no PARTIAL.
        assert_eq!(second, "line2001\nline2002");
    }

    #[tokio::test]
    async fn read_file_explicit_range_over_token_budget_errors() {
        let context = test_context("read_file_explicit_range_over_token_budget");
        // ~4 bytes/token heuristic: 25_000 tokens ≈ 100_000 bytes.
        let huge_line = "x".repeat(120_000);
        write_workspace_file(
            &context.work_dir,
            "huge.txt",
            &format!("{huge_line}\nsecond\n"),
        );

        let error = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "huge.txt", "offset": 1, "limit": 1 }),
        )
        .await
        .unwrap_err();

        let msg = error.to_string();
        assert!(
            msg.contains("exceeds") && msg.contains("token"),
            "expected token budget error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn read_file_implicit_token_budget_returns_partial() {
        let context = test_context("read_file_implicit_token_budget");
        // Many medium lines so several fit, then overflow without a single-line failure.
        let line = "y".repeat(4_000); // ~1000 approx tokens each
        let body = (0..40)
            .map(|_| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        write_workspace_file(&context.work_dir, "tokens.txt", &body);

        let output = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "tokens.txt" }),
        )
        .await
        .unwrap();

        assert!(
            output.starts_with("[PARTIAL view — lines "),
            "expected PARTIAL for implicit overflow, got: {output}"
        );
        assert!(output.contains("continue with offset="));
        assert!(
            crate::utils::approx_token_count(&output) <= READ_FILE_MAX_OUTPUT_TOKENS,
            "PARTIAL result must stay within token budget"
        );
    }

    #[tokio::test]
    async fn read_file_small_file_has_no_partial_marker() {
        let context = test_context("read_file_small_file_no_partial");
        write_workspace_file(&context.work_dir, "small.txt", "a\nb\nc\n");

        let output = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "small.txt" }),
        )
        .await
        .unwrap();

        assert_eq!(output, "a\nb\nc");
        assert!(!output.contains("PARTIAL"));
    }
}
