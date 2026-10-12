use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use anyhow::{Context as _, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::ToolVisualKind;
use tool_refactor_macros::tool;

use crate::{memory::MemoryType, tool::ToolContext};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveMemoryInput {
    #[schemars(description = "Short identifier, e.g. prefer_tabs or db_schema.")]
    pub name: String,
    #[schemars(description = "One-line summary of what this memory captures.")]
    pub description: String,
    #[serde(rename = "type")]
    #[schemars(description = "user, feedback, project, or reference.")]
    pub memory_type: String,
    #[schemars(description = "Full memory content.")]
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LoadMemoryInput {
    #[schemars(description = "File name from the memory index, without the `.md` suffix.")]
    pub name: String,
}

pub const LOAD_MEMORY_METADATA: ToolMetadata = ToolMetadata::read_json(
    "load_memory",
    "Read one saved memory in full. The system prompt lists them by file name.",
    "🧠 Memory",
);

pub const SAVE_MEMORY_METADATA: ToolMetadata = ToolMetadata {
    name: "save_memory",
    description: "Save a persistent memory that survives across sessions.",
    permission: PermissionPolicy::Write,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🧠 Memory",
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
/// Returns an error if:
/// - The memory type string is invalid (must be one of: user, feedback, project, or reference).
/// - The memory manager lock is poisoned.
/// - The memory cannot be saved (e.g., file I/O failure).
pub async fn save_memory(ctx: ToolContext, input: SaveMemoryInput) -> Result<String> {
    let memory_type = input.memory_type.parse::<MemoryType>()?;
    let mut manager = ctx
        .memory_manager
        .lock()
        .map_err(|_| anyhow::anyhow!("memory manager lock poisoned"))?;
    manager
        .save_memory(&input.name, &input.description, memory_type, &input.content)
        .context("failed to save memory")
}

#[tool]
/// # Errors
///
/// Returns an error if:
/// - The memory name is unknown (the message lists the known ones).
/// - The memory manager lock is poisoned.
/// - The memory file cannot be read.
pub async fn load_memory(ctx: ToolContext, input: LoadMemoryInput) -> Result<String> {
    let manager = ctx
        .memory_manager
        .lock()
        .map_err(|_| anyhow::anyhow!("memory manager lock poisoned"))?;
    manager.load_topic(&input.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn load_memory_returns_a_saved_body_and_lists_the_known_names() {
        let context = test_context("load_memory_round_trip");
        run_tool(
            &context,
            SaveMemoryTool,
            "save_memory",
            serde_json::json!({
                "name": "Prefer Tabs",
                "description": "Indent with tabs",
                "type": "user",
                "content": "Tabs everywhere."
            }),
        )
        .await
        .unwrap();

        let body = run_tool(
            &context,
            LoadMemoryTool,
            "load_memory",
            serde_json::json!({ "name": "prefer_tabs" }),
        )
        .await
        .unwrap();
        assert!(body.contains("Tabs everywhere."), "{body}");
        assert!(body.contains("type: user"), "{body}");

        let error = run_tool(
            &context,
            LoadMemoryTool,
            "load_memory",
            serde_json::json!({ "name": "nope" }),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("Unknown memory"), "{error}");
        assert!(error.contains("prefer_tabs"), "{error}");
    }

    #[tokio::test]
    async fn save_memory_rejects_invalid_type() {
        let context = test_context("save_memory_rejects_invalid_type");

        let error = run_tool(
            &context,
            SaveMemoryTool,
            "save_memory",
            serde_json::json!({
                "name": "Bad Type",
                "description": "test",
                "type": "invalid",
                "content": "content"
            }),
        )
        .await
        .unwrap_err();

        assert!(!error.to_string().is_empty());
    }
}
