use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::ToolVisualKind;
use tool_refactor_macros::tool;

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BackgroundRunInput {
    #[schemars(description = "Shell command to run in the background.")]
    pub command: String,
}

pub const BACKGROUND_RUN_METADATA: ToolMetadata = ToolMetadata {
    name: "background_run",
    description: "Run a shell command in the background.",
    permission: PermissionPolicy::ShellCommand {
        command_field: "command",
    },
    permission_prompt: PermissionPromptPolicy::Command { field: "command" },
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Command,
        display_name: "⚙️ Background Run",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Command { field: "command" },
};

#[tool]
/// # Errors
///
/// Returns an error if the background manager fails to run the command
/// (e.g., invalid command or internal error).
pub async fn background_run(ctx: ToolContext, input: BackgroundRunInput) -> Result<String> {
    ctx.background_manager.run(input.command, &ctx.work_dir)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckBackgroundInput {
    #[schemars(description = "Optional background task id.")]
    pub task_id: Option<String>,
}

pub const CHECK_BACKGROUND_METADATA: ToolMetadata = ToolMetadata {
    name: "check_background",
    description: "Check background task status.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "⚙️ Background Check",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

#[tool]
/// # Errors
///
/// Returns an error if the provided task ID does not exist or the background
/// manager encounters an internal error.
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

        let output = run_tool(
            &context,
            CheckBackgroundTool,
            "check_background",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        assert_eq!(output, "No background tasks.");
    }

    #[tokio::test]
    async fn check_background_errors_for_unknown_task_id() {
        let context = test_context("check_background_errors_for_unknown_task_id");

        let error = run_tool(
            &context,
            CheckBackgroundTool,
            "check_background",
            serde_json::json!({ "task_id": "deadbeef" }),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("Unknown background task"));
    }
}
