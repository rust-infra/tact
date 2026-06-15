// SleepTool: pause execution for a specified duration.
//
// Useful when the model needs to wait between operations (e.g., polling,
// rate limiting, or waiting for external processes). Unlike `Bash(sleep ...)`,
// this does not hold a shell process and can run concurrently with other tools.

use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use tool_refactor_macros::tool;
use tracing::debug;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SleepInput {
    /// Duration in milliseconds (capped at 300_000 = 5 minutes).
    #[schemars(description = "Duration to sleep in milliseconds (max 300000 = 5 minutes).")]
    #[serde(alias = "ms", alias = "duration_ms")]
    pub ms: u64,
}

#[tool(
    name = "sleep",
    description = "Wait for a specified duration in milliseconds. \
                    Use instead of Bash(sleep ...) — it doesn't hold a shell \
                    process and can run concurrently with other tools."
)]
pub async fn sleep(_ctx: ToolContext, input: SleepInput) -> Result<String> {
    let duration_ms = input.ms.min(300_000);
    debug!(ms = duration_ms, "Sleeping");

    tokio::time::sleep(Duration::from_millis(duration_ms)).await;

    Ok(format!("Slept for {}ms.", duration_ms))
}
