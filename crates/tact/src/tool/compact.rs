use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompactInput {
    #[schemars(description = "Optional focus to preserve in the compacted summary.")]
    pub focus: Option<String>,
}

#[tool(
    name = "compact",
    description = "Summarize earlier conversation so work can continue in a smaller context."
)]
pub async fn compact(_ctx: ToolContext, input: CompactInput) -> Result<String> {
    let focus = input
        .focus
        .map(|focus| format!(" Focus to preserve: {focus}"))
        .unwrap_or_default();
    Ok(format!("Compacting conversation...{focus}"))
}

#[cfg(test)]
mod tests {
    use crate::tool::test_support::{run_tool, test_context};

    use super::*;

    #[tokio::test]
    async fn compact_without_focus() {
        let context = test_context("compact_without_focus");

        let output = run_tool(&context, CompactTool, "compact", serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(output, "Compacting conversation...");
    }

    #[tokio::test]
    async fn compact_with_focus() {
        let context = test_context("compact_with_focus");

        let output = run_tool(
            &context,
            CompactTool,
            "compact",
            serde_json::json!({ "focus": "open tasks" }),
        )
        .await
        .unwrap();

        assert_eq!(output, "Compacting conversation... Focus to preserve: open tasks");
    }
}
