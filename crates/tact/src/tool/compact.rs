use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use anyhow::Result;
use tact_protocol::ToolVisualKind;
use tool_refactor_macros::tool;

pub const COMPACT_METADATA: ToolMetadata = ToolMetadata {
    name: "compact",
    description: "Summarize earlier conversation so work can continue in a smaller context.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "📦 Compact",
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
/// This function always returns `Ok`.
pub async fn compact(
    #[schemars(description = "Optional focus to preserve in the compacted summary.")] focus: Option<
        String,
    >,
) -> Result<String> {
    let focus = focus
        .map(|focus| format!(" Focus to preserve: {focus}"))
        .unwrap_or_default();
    Ok(format!("Compacting conversation...{focus}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

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

        assert_eq!(
            output,
            "Compacting conversation... Focus to preserve: open tasks"
        );
    }
}
