use anyhow::{Context as _, Result};
use schemars::JsonSchema;
use serde::Deserialize;
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

#[tool(name = "save_memory", description = "Save a persistent memory that survives across sessions.")]
pub async fn save_memory(ctx: ToolContext, input: SaveMemoryInput) -> Result<String> {
    let memory_type = input.memory_type.parse::<MemoryType>()?;
    let mut manager = ctx.memory_manager.lock().map_err(|_| anyhow::anyhow!("memory manager lock poisoned"))?;
    manager.save_memory(&input.name, &input.description, memory_type, &input.content).context("failed to save memory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

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
