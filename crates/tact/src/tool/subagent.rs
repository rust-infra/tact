use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use std::sync::Arc;

use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use tact_llm::{ApiKeyProvider, Client, Message, Role, get_llm_client};
use tact_protocol::{AgentUpdate, ToolVisualKind};
use tool_refactor_macros::tool;

use crate::{
    Agent, AgentSystemPrompt,
    consts::TactPath,
    extract_text,
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode, settings::PermissionSettings},
    store::{SessionLock, open_sqlite_session_store},
    subagent::SubagentResult,
    tool::{ToolContext, subagent_toolset},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubagentInput {
    #[schemars(description = "Prompt for the subagent.")]
    pub prompt: String,
    #[schemars(description = "Short description of the task.")]
    #[allow(dead_code)]
    pub description: Option<String>,
    /// When true, return `async_launched { id }` immediately; the subagent
    /// keeps running and its result is re-injected into the parent context
    /// on completion.
    #[schemars(description = "Run the subagent in the background and return an async handle.")]
    #[serde(default)]
    pub run_in_background: Option<bool>,
    /// Cap on nested agent-loop turns (prevents runaway subagents).
    #[schemars(description = "Maximum number of agent-loop turns for this subagent.")]
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Resume an existing subagent session id (from a prior `async_launched`).
    #[schemars(description = "Resume an existing subagent session by id.")]
    #[serde(default)]
    pub resume: Option<String>,
}

pub const SPAWN_SUBAGENT_METADATA: ToolMetadata = ToolMetadata {
    name: "spawn_subagent",
    description: "Spawn a subagent with fresh context. It shares the filesystem but not conversation history.",
    permission: PermissionPolicy::High,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Subagent,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Subagent,
        display_name: "🤖 Subagent",
        live_output: LiveOutputPolicy::FullTranscript,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::SubagentTranscript,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::PersistLargeOutput,
    argument_summary: ArgumentSummaryPolicy::SubagentPrompt { field: "prompt" },
};

/// Extract the subagent's final summary from its last assistant message.
fn extract_summary(subagent: &Agent, max_turns_reached: bool) -> String {
    let summary = subagent
        .runtime
        .context
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::Assistant))
        .map(|message| extract_text(&message.content))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "(no summary)".to_string());
    if max_turns_reached {
        format!("{summary} (max_turns reached)")
    } else {
        summary
    }
}

#[tool]
/// # Errors
///
/// Returns an error if:
/// - The LLM client cannot be obtained.
/// - The permission manager cannot be created.
/// - The session store cannot be opened.
/// - `run_in_background` is requested without a parent agent runtime.
/// - The subagent agent loop encounters an error.
pub async fn spawn_subagent(ctx: ToolContext, input: SubagentInput) -> Result<String> {
    let settings = crate::config::settings();

    let (client, agent_overrides) = if let Some(sa) = &settings.agent.subagent {
        let client = Client::new(
            sa.provider.to_profile(),
            Arc::new(ApiKeyProvider::new(sa.provider.api_key.clone())),
        )
        .await?;
        let mut agent_settings = settings.agent.clone();
        agent_settings.model = sa.provider.model.clone();
        agent_settings.max_tokens = sa.max_tokens;
        agent_settings.thinking_budget = sa.thinking_budget;
        agent_settings.reasoning_effort = sa.reasoning_effort;
        (client, agent_settings)
    } else {
        let client = get_llm_client().await?;
        (client, settings.agent.clone())
    };

    let system_prompt = format!(
        "You are a coding subagent at {}. Complete the given task, then summarize your findings.",
        ctx.work_dir.display()
    );
    // Inherit the parent's permission context (Claude-style). The snapshot is
    // stamped by `execute_tool_call` just before this tool runs, so it carries
    // the parent's *current* mode / allow-list / settings. Orphan/test
    // contexts without a parent agent fall back to the pre-inheritance
    // behavior: a fresh `Default` manager with settings loaded from disk.
    let pm = match ctx.permission_snapshot.clone() {
        Some(snapshot) => PermissionManager::from_snapshot(snapshot),
        None => PermissionManager::try_new_with_settings(
            PermissionMode::Default,
            PermissionSettings::load(&TactPath::new(&ctx.work_dir)),
        )?,
    };

    // Resume an existing child, or mint a fresh session id.
    let child_id = match input.resume.clone() {
        Some(id) => id,
        None => uuid::Uuid::new_v4().to_string(),
    };
    let ref_id = ctx.session_id.as_deref().unwrap_or("");
    let root_dir = ctx.work_dir.display().to_string();

    let store = if let Some(store) = &ctx.session_store {
        store.clone()
    } else {
        let db_path = TactPath::new(&ctx.work_dir).session_db_path();
        open_sqlite_session_store(&db_path)
            .await
            .with_context(|| format!("failed to open session store at {}", db_path.display()))?
    };
    store
        .ensure_session_row(&child_id, &root_dir, ref_id)
        .await?;

    let mut subagent = Agent::new(
        client,
        ctx.clone(),
        subagent_toolset(),
        MCPToolRouter::new(),
        pm,
        AgentSystemPrompt::Static(system_prompt),
    )
    .with_agent_settings(agent_overrides)
    .with_max_turns(input.max_turns)
    .with_session(child_id.clone(), store.clone());

    // Tag UI traffic so the TUI routes stream/steps into the parent tool-card
    // via ToolProgress. RequestSelect* still passes through for permission popups.
    if let Some(tx) = ctx.ui_tx.clone() {
        let tagged = crate::tool::subagent_ui::tagged_ui_channel_with_progress(
            tx,
            ctx.progress_reporter.clone(),
        );
        subagent = subagent.with_ui_channel(tagged);
    }

    // Resume reuses a finished child session: hold its process lock for the
    // duration of the follow-up run so two runs can't operate on it at once.
    let lock = if input.resume.is_some() {
        Some(SessionLock::acquire(store, &child_id).await?)
    } else {
        None
    };

    if input.run_in_background == Some(true) {
        // Async: return immediately; the detached task finalizes the card,
        // persists the lifecycle, and re-injects the summary.
        let results = ctx
            .subagent_results
            .clone()
            .ok_or_else(|| anyhow::anyhow!("run_in_background requires a parent agent runtime"))?;
        let manager = ctx.subagent_manager.clone();
        let ui_tx = ctx.ui_tx.clone();
        let tool_id = ctx.progress_reporter.tool_id().to_string();
        let prompt = input.prompt;
        manager.start(child_id.clone()).await?;
        let launched = format!("async_launched {{ {child_id} }}");
        tokio::spawn(async move {
            let result = subagent
                .agent_loop(Some(Message::new_text(Role::User, prompt)))
                .await;
            let success = result.is_ok();
            let max_turns_reached = subagent.max_turns.is_some_and(|m| subagent.turns_taken > m);
            let summary = extract_summary(&subagent, max_turns_reached);
            if let Some(lock) = lock {
                let _ = lock.release().await;
            }
            // Persist the lifecycle and enqueue the result BEFORE emitting the
            // TUI notification: the driver may immediately submit a wake-up
            // turn on receipt, and that turn's drain must already see the
            // queued result.
            let _ = manager.finish(&child_id, success, summary.clone()).await;
            if let Ok(mut queue) = results.lock() {
                queue.push_back(SubagentResult {
                    child_id: child_id.clone(),
                    summary: summary.clone(),
                    success,
                });
            }
            // Emit on the PARENT ui_tx (not the child's tagged forwarder,
            // which drops unknown variants).
            if let Some(tx) = &ui_tx {
                let _ = tx.send(AgentUpdate::SubagentFinished {
                    tool_id: tool_id.clone(),
                    child_id: child_id.clone(),
                    success,
                    summary: summary.clone(),
                });
            }
        });
        Ok(launched)
    } else {
        // Sync: block on the nested loop; tool result = last assistant text.
        subagent
            .agent_loop(Some(Message::new_text(Role::User, input.prompt)))
            .await?;
        let max_turns_reached = subagent.max_turns.is_some_and(|m| subagent.turns_taken > m);
        let summary = extract_summary(&subagent, max_turns_reached);
        if let Some(lock) = lock {
            lock.release().await?;
        }
        Ok(summary)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckSubagentInput {
    #[schemars(description = "Optional subagent child session id.")]
    pub child_id: Option<String>,
}

pub const CHECK_SUBAGENT_METADATA: ToolMetadata = ToolMetadata {
    name: "check_subagent",
    description: "Check subagent run status.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🤖 Subagent Check",
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
/// Returns an error if the provided child id does not exist or the subagent
/// manager encounters an internal error.
pub async fn check_subagent(ctx: ToolContext, input: CheckSubagentInput) -> Result<String> {
    ctx.subagent_manager.check(input.child_id.as_deref()).await
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
        assert_eq!(input.run_in_background, None);
        assert_eq!(input.max_turns, None);
        assert_eq!(input.resume, None);
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

    #[test]
    fn subagent_input_async_fields() {
        let json = serde_json::json!({
            "prompt": "Background me",
            "run_in_background": true,
            "max_turns": 3,
            "resume": "child-abc"
        });
        let input: SubagentInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.run_in_background, Some(true));
        assert_eq!(input.max_turns, Some(3));
        assert_eq!(input.resume.as_deref(), Some("child-abc"));
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
