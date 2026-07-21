// SleepTool: pause execution for a specified duration.
//
// Useful when the model needs to wait between operations (e.g., polling,
// rate limiting, or waiting for external processes). Unlike `Bash(sleep ...)`,
// this does not hold a shell process and can run concurrently with other tools.

use std::time::Duration;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;
use tracing::debug;

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SleepInput {
    /// Duration in milliseconds (capped at 300_000 = 5 minutes).
    #[schemars(description = "Duration to sleep in milliseconds (max 300000 = 5 minutes).")]
    #[serde(alias = "ms", alias = "duration_ms")]
    pub ms: u64,
}

fn capped_sleep_ms(ms: u64) -> u64 {
    ms.min(300_000)
}

#[tool(
    name = "sleep",
    description = "Wait for a specified duration in milliseconds. \
                    Use instead of Bash(sleep ...) — it doesn't hold a shell \
                    process and can run concurrently with other tools."
)]
pub async fn sleep(_ctx: ToolContext, input: SleepInput) -> Result<String> {
    let duration_ms = capped_sleep_ms(input.ms);
    debug!(ms = duration_ms, "Sleeping");

    tokio::time::sleep(Duration::from_millis(duration_ms)).await;

    Ok(format!("Slept for {}ms.", duration_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[test]
    fn capped_sleep_ms_limits_to_five_minutes() {
        assert_eq!(capped_sleep_ms(999_999), 300_000);
        assert_eq!(capped_sleep_ms(100), 100);
    }

    #[tokio::test]
    async fn sleep_accepts_duration_ms_alias() {
        let context = test_context("sleep_accepts_duration_ms_alias");

        let output = run_tool(
            &context,
            SleepTool,
            "sleep",
            serde_json::json!({ "duration_ms": 0 }),
        )
        .await
        .unwrap();

        assert_eq!(output, "Slept for 0ms.");
    }
}
