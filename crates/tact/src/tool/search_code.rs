use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::process::Command;
use tool_refactor_macros::tool;
use tracing::debug;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchCodeInput {
    #[schemars(description = "Search pattern (regex supported when using rg).")]
    pub query: String,
    #[schemars(
        description = "Directory or file to search in, relative to workspace. Defaults to workspace root."
    )]
    pub path: Option<String>,
    #[schemars(description = "File glob pattern to filter results, e.g. '*.rs' or '*.ts'.")]
    pub glob: Option<String>,
    #[schemars(description = "Maximum number of results to return (default: 30).")]
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_max_results() -> usize {
    30
}

#[tool(
    name = "search_code",
    description = "Search for text patterns in the codebase. Uses ripgrep (rg) if available, \
                   otherwise falls back to grep. Supports regex patterns. Returns matching \
                   lines with file paths and line numbers. Use this to find symbols, functions, \
                   imports, or any text in source files."
)]
pub async fn search_code(ctx: ToolContext, input: SearchCodeInput) -> Result<String> {
    let max_results = input.max_results.max(1).min(200);
    let search_path = input.path.unwrap_or_else(|| ".".to_string());
    let full_path = ctx.work_dir.join(&search_path);

    // Prefer ripgrep, fall back to grep
    let has_rg = Command::new("rg")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    debug!(query = %input.query, path = %search_path, has_rg, "Search code");

    let output = if has_rg {
        run_ripgrep(&input.query, &full_path, input.glob.as_deref(), max_results).await
    } else {
        run_grep(&input.query, &full_path, max_results).await
    };

    match output {
        Ok(result) => {
            if result.trim().is_empty() {
                Ok("No matches found.".to_string())
            } else {
                Ok(result)
            }
        }
        Err(e) => Err(e),
    }
}

async fn run_ripgrep(
    query: &str,
    path: &std::path::Path,
    glob: Option<&str>,
    max_results: usize,
) -> Result<String> {
    let mut cmd = Command::new("rg");
    cmd.arg("-n")
        .arg("--max-columns")
        .arg("200")
        .arg("--max-count")
        .arg(format!("{}", max_results.max(5)))
        .arg(query)
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(g) = glob {
        cmd.arg("--glob").arg(g);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("rg failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    if lines.len() > max_results {
        let truncated: Vec<&str> = lines.into_iter().take(max_results).collect();
        Ok(format!(
            "{}\n... (truncated to {} results)",
            truncated.join("\n"),
            max_results
        ))
    } else {
        Ok(stdout.into_owned())
    }
}

async fn run_grep(query: &str, path: &std::path::Path, max_results: usize) -> Result<String> {
    let output = Command::new("grep")
        .arg("-rn")
        .arg("--max-count")
        .arg(format!("{}", max_results.max(5)))
        .arg(query)
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("grep failed: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    if lines.len() > max_results {
        let truncated: Vec<&str> = lines.into_iter().take(max_results).collect();
        Ok(format!(
            "{}\n... (truncated to {} results)",
            truncated.join("\n"),
            max_results
        ))
    } else {
        Ok(stdout.into_owned())
    }
}
