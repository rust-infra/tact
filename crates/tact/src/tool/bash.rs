use std::time::Duration;

use crate::shell::validate_shell_command;
use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::{process::Command, time::timeout};
use tool_refactor_macros::tool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BashInput {
    #[schemars(description = "Shell command to run in the current workspace.")]
    pub command: String,
}

#[tool(
    name = "bash",
    description = "Run a shell command in the current workspace."
)]
pub async fn bash(ctx: ToolContext, input: BashInput) -> Result<String> {
    let command = input.command;

    validate_shell_command(&command)?;

    let child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(ctx.work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(anyhow::anyhow!("Error: {}", e)),
    };

    let output_future = child.wait_with_output();
    match timeout(Duration::from_secs(120), output_future).await {
        Ok(Ok(output)) => {
            let combined = [output.stdout, output.stderr].concat();
            let out_str = String::from_utf8_lossy(&combined);
            let trimmed = out_str.trim();

            if trimmed.is_empty() {
                Ok("(no output)".to_string())
            } else {
                Ok(trimmed.chars().take(50000).collect())
            }
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("Error: {}", e)),
        Err(_) => Err(anyhow::anyhow!("Error: Timeout (120s)")),
    }
}

#[cfg(test)]
mod tests {
    use crate::tool::test_support::{run_tool, test_context};

    use super::*;

    #[tokio::test]
    async fn bash_returns_placeholder_for_empty_output() {
        let context = test_context("bash_returns_placeholder_for_empty_output");

        let output = run_tool(
            &context,
            BashTool,
            "bash",
            serde_json::json!({ "command": "true" }),
        )
        .await
        .unwrap();

        assert_eq!(output, "(no output)");
    }
}
