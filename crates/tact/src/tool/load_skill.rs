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
pub struct LoadSkillInput {
    #[schemars(description = "Name of the skill to load.")]
    pub name: String,
}

pub const LOAD_SKILL_METADATA: ToolMetadata = ToolMetadata {
    name: "load_skill",
    description: "Load the full body of a named skill into the current context.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "📚 Skill",
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
/// This function always returns `Ok`. Unknown skills produce an error message
/// in the returned string rather than an `Err`.
pub async fn load_skill(ctx: ToolContext, input: LoadSkillInput) -> Result<String> {
    Ok(crate::skill::lock_skills(&ctx.skill_registry).load_full_text(&input.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn load_skill_reports_unknown_skill() {
        let context = test_context("load_skill_reports_unknown_skill");

        let output = run_tool(
            &context,
            LoadSkillTool,
            "load_skill",
            serde_json::json!({ "name": "missing" }),
        )
        .await
        .unwrap();

        assert!(output.contains("Error: Unknown skill 'missing'"));
        assert!(output.contains("Available:"));
    }
}
