use anyhow::{Result, anyhow};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};
use tool_refactor_macros::tool;

use crate::tool::{ToolContext, safe_path};
use crate::utils::approx_token_count;

const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;
/// Reserve room so a leading PARTIAL marker keeps the full result in budget.
const READ_FILE_PARTIAL_MARKER_TOKEN_RESERVE: usize = 64;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileInput {
    #[schemars(description = "Path to the file to read, relative to the current workspace.")]
    pub path: String,
    #[schemars(
        description = "Optional maximum number of lines to return (from offset if set; otherwise from the start of the file)."
    )]
    pub limit: Option<u64>,
    #[schemars(
        description = "Optional 1-based line number to start reading from. Use with limit to read specific sections."
    )]
    pub offset: Option<u64>,
}

fn partial_marker(start_line: usize, end_line: usize, next_offset: usize) -> String {
    format!(
        "[PARTIAL view — lines {start_line}-{end_line}; continue with offset={next_offset}]\n\n"
    )
}

#[tool(name = "read_file", description = "Read file contents.")]
pub async fn read_file(ctx: ToolContext, input: ReadFileInput) -> Result<String> {
    let path = safe_path(&ctx.work_dir, &input.path)?;
    let explicit_range = input.offset.is_some() || input.limit.is_some();
    let start_line = input.offset.map(|o| o.max(1) as usize).unwrap_or(1);
    let max_lines = input
        .limit
        .map(|l| l as usize)
        .unwrap_or(READ_FILE_DEFAULT_MAX_LINES);

    let file = File::open(&path)
        .await
        .map_err(|e| anyhow!("Error: {}", e))?;
    let mut reader = BufReader::new(file);

    // Skip to start_line (1-based).
    let mut line_no: usize = 1;
    let mut buf = String::new();
    while line_no < start_line {
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .await
            .map_err(|e| anyhow!("Error: {}", e))?;
        if n == 0 {
            return Ok(String::new());
        }
        line_no = line_no.saturating_add(1);
    }

    if max_lines == 0 {
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .await
            .map_err(|e| anyhow!("Error: {}", e))?;
        if n == 0 {
            return Ok(String::new());
        }
        return Ok(format!(
            "[PARTIAL view — lines none; continue with offset={start_line}]\n\n"
        ));
    }

    let content_budget =
        READ_FILE_MAX_OUTPUT_TOKENS.saturating_sub(READ_FILE_PARTIAL_MARKER_TOKEN_RESERVE);
    let mut selected: Vec<String> = Vec::new();
    let mut content_tokens: usize = 0;
    let mut stopped_for_tokens = false;
    let mut hit_line_cap = false;

    while selected.len() < max_lines {
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .await
            .map_err(|e| anyhow!("Error: {}", e))?;
        if n == 0 {
            break;
        }
        if buf.ends_with('\n') {
            buf.pop();
            if buf.ends_with('\r') {
                buf.pop();
            }
        }
        let line = std::mem::take(&mut buf);
        let line_tokens = approx_token_count(&line);
        let sep = if selected.is_empty() {
            0
        } else {
            approx_token_count("\n")
        };
        let next_total = content_tokens
            .saturating_add(sep)
            .saturating_add(line_tokens);

        if next_total > content_budget {
            if selected.is_empty() {
                return Err(anyhow!(
                    "line {line_no} exceeds READ_FILE_MAX_OUTPUT_TOKENS (~{READ_FILE_MAX_OUTPUT_TOKENS} approx tokens); use search tools or split the file"
                ));
            }
            if explicit_range {
                return Err(anyhow!(
                    "requested range exceeds READ_FILE_MAX_OUTPUT_TOKENS (~{READ_FILE_MAX_OUTPUT_TOKENS} approx tokens); reduce limit or choose a smaller section"
                ));
            }
            stopped_for_tokens = true;
            break;
        }

        content_tokens = next_total;
        selected.push(line);
        line_no = line_no.saturating_add(1);

        if selected.len() == max_lines {
            // One-line look-ahead to know whether more content exists.
            buf.clear();
            let n = reader
                .read_line(&mut buf)
                .await
                .map_err(|e| anyhow!("Error: {}", e))?;
            hit_line_cap = n > 0;
            break;
        }
    }

    if selected.is_empty() {
        return Ok(String::new());
    }

    let end_line = start_line.saturating_add(selected.len()).saturating_sub(1);
    let body = selected.join("\n");
    let needs_partial = stopped_for_tokens || hit_line_cap;
    if !needs_partial {
        return Ok(body);
    }

    let next_offset = end_line.saturating_add(1);
    Ok(format!(
        "{}{}",
        partial_marker(start_line, end_line, next_offset),
        body
    ))
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
        assert!(
            msg.contains("line 1 exceeds"),
            "single oversized line should use the line-exceeds path, got: {msg}"
        );
    }

    #[tokio::test]
    async fn read_file_explicit_multiline_over_token_budget_errors() {
        let context = test_context("read_file_explicit_multiline_over_token_budget");
        // Several lines fit, but the full requested range does not — must Err, not PARTIAL.
        let line = "y".repeat(4_000); // ~1000 approx tokens each
        let body = (0..40)
            .map(|_| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        write_workspace_file(&context.work_dir, "range.txt", &body);

        let error = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "range.txt", "offset": 1, "limit": 40 }),
        )
        .await
        .unwrap_err();

        let msg = error.to_string();
        assert!(
            msg.contains("requested range exceeds") && msg.contains("token"),
            "expected explicit-range token error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn read_file_utf8_partial_stays_within_token_budget() {
        let context = test_context("read_file_utf8_partial_stays_within_token_budget");
        // Each CJK char is 3 UTF-8 bytes → ceil(3/4)=1 approx token; 2000 chars ≈ 1500 tokens/line.
        let line = "测".repeat(2_000);
        let body = (0..30)
            .map(|_| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        write_workspace_file(&context.work_dir, "cjk.txt", &body);

        let output = run_tool(
            &context,
            ReadFileTool,
            "read_file",
            serde_json::json!({ "path": "cjk.txt" }),
        )
        .await
        .unwrap();

        assert!(
            output.starts_with("[PARTIAL view — lines "),
            "expected PARTIAL for UTF-8 overflow, got: {output}"
        );
        assert!(output.contains('测'));
        assert!(
            crate::utils::approx_token_count(&output) <= READ_FILE_MAX_OUTPUT_TOKENS,
            "UTF-8 PARTIAL must stay within token budget"
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
