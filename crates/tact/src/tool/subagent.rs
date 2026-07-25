use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use tact_llm::{Message, Role, get_llm_client};
use tool_refactor_macros::tool;

use crate::{
    Agent, AgentSystemPrompt, extract_text,
    consts::TactPath,
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode},
    store::open_sqlite_session_store,
    tool::{ToolContext, subagent_toolset},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubagentInput {
    #[schemars(description = "Prompt for the subagent.")]
    pub prompt: String,
    #[schemars(description = "Short description of the task.")]
    #[allow(dead_code)]
    pub description: Option<String>,
}

#[tool(
    name = "task",
    description = "Spawn a subagent with fresh context. It shares the filesystem but not conversation history."
)]
/// # Errors
///
/// Returns an error if:
/// - The LLM client cannot be obtained.
/// - The permission manager cannot be created.
/// - The session store cannot be opened.
/// - The subagent agent loop encounters an error.
pub async fn task(ctx: ToolContext, input: SubagentInput) -> Result<String> {
    let client = get_llm_client()?;
    let system_prompt = format!(
        "You are a coding subagent at {}. Complete the given task, then summarize your findings.",
        ctx.work_dir.display()
    );
    let mut subagent = Agent::new(
        client,
        ctx.clone(),
        subagent_toolset(),
        MCPToolRouter::new(),
        PermissionManager::try_new(PermissionMode::Default)?,
        AgentSystemPrompt::Static(system_prompt),
    );

    let child_id = uuid::Uuid::new_v4().to_string();
    let ref_id = ctx.session_id.as_deref().unwrap_or("");
    let root_dir = ctx.work_dir.display().to_string();
    let store = if let Some(store) = ctx.session_store {
        store
    } else {
        let db_path = TactPath::new(&ctx.work_dir).session_db_path();
        open_sqlite_session_store(&db_path)
            .await
            .with_context(|| format!("failed to open session store at {}", db_path.display()))?
    };
    store
        .ensure_session_row(&child_id, &root_dir, ref_id)
        .await?;
    subagent = subagent.with_session(child_id.clone(), store);

    // Tag UI traffic so the TUI routes stream/steps into the Subagent sticky.
    // RequestSelect* still passes through for permission popups.
    if let Some(tx) = ctx.ui_tx {
        let tagged = crate::tool::subagent_ui::tagged_ui_channel(
            tx,
            ctx.progress_reporter.tool_id().to_string(),
            child_id,
        );
        subagent = subagent.with_ui_channel(tagged);
    }

    // Seed via agent_loop so the user turn is persisted (push alone skipped SQLite).
    subagent
        .agent_loop(Some(Message::new_text(Role::User, input.prompt)))
        .await?;

    let summary = subagent
        .runtime
        .context
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::Assistant))
        .map(|message| extract_text(&message.content))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "(no summary)".to_string());

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::test_context;
    use tempfile::TempDir;

    #[test]
    fn subagent_toolset_has_five_tools() {
        let router = subagent_toolset();
        let specs = router.tool_specs();
        assert_eq!(specs.len(), 5, "subagent should have exactly 5 tools");
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"sleep"));
    }

    #[test]
    fn subagent_input_deserialization() {
        let json = serde_json::json!({
            "prompt": "Fix the bug in main.rs",
            "description": "rust bugfix"
        });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.prompt, "Fix the bug in main.rs");
        assert_eq!(input.description, Some("rust bugfix".to_string()));
    }

    #[test]
    fn subagent_input_without_description() {
        let json = serde_json::json!({
            "prompt": "Just do it"
        });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.prompt, "Just do it");
        assert_eq!(input.description, None);
    }

    #[tokio::test]
    async fn ensure_child_session_row_links_parent_ref() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("tact.db");
        let store = open_sqlite_session_store(&db).await.unwrap();
        store
            .ensure_session_row("parent", "/tmp/p", "")
            .await
            .unwrap();
        store
            .ensure_session_row("child", "/tmp/p", "parent")
            .await
            .unwrap();

        let listed = store.list_sessions(None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "parent");

        store.delete_session("parent").await.unwrap();
        assert!(store.list_sessions(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn task_context_without_parent_opens_workdir_db() {
        let ctx = test_context("subagent-orphan-session");
        let db_path = TactPath::new(&ctx.work_dir).session_db_path();
        let store = open_sqlite_session_store(&db_path).await.unwrap();
        let child_id = uuid::Uuid::new_v4().to_string();
        store
            .ensure_session_row(&child_id, &ctx.work_dir.display().to_string(), "")
            .await
            .unwrap();
        let listed = store.list_sessions(None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, child_id);
    }
}
