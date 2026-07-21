use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BackgroundRunInput {
    #[schemars(description = "Shell command to run in the background.")]
    pub command: String,
}

#[tool(name = "background_run", description = "Run a shell command in the background.")]
pub async fn background_run(ctx: ToolContext, input: BackgroundRunInput) -> Result<String> {
    ctx.background_manager.run(input.command, &ctx.work_dir)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckBackgroundInput {
    #[schemars(description = "Optional background task id.")]
    pub task_id: Option<String>,
}

#[tool(name = "check_background", description = "Check background task status.")]
pub async fn check_background(ctx: ToolContext, input: CheckBackgroundInput) -> Result<String> {
    ctx.background_manager.check(input.task_id.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn check_background_lists_empty_when_no_tasks() {
        let context = test_context("check_background_lists_empty_when_no_tasks");

        let output = run_tool(&context, CheckBackgroundTool, "check_background", serde_json::json!({})).await.unwrap();

        assert_eq!(output, "No background tasks.");
    }

    #[tokio::test]
    async fn check_background_errors_for_unknown_task_id() {
        let context = test_context("check_background_errors_for_unknown_task_id");

        let error =
            run_tool(&context, CheckBackgroundTool, "check_background", serde_json::json!({ "task_id": "deadbeef" }))
                .await
                .unwrap_err();

        assert!(error.to_string().contains("Unknown background task"));
    }
}
