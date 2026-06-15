//! Tool definition and routing.
//!
//! ## [`Tool`] trait
//!
//! Every tool implements [`Tool`], providing a name, description, JSON
//! input schema, and an async `call` method that receives [`ToolContext`]
//! and the deserialised input.
//!
//! ## [`ToolContext`]
//!
//! Shared state available to every tool invocation: the skill registry,
//! persistent memory, the current work directory, and handles for tasks,
//! background work, cron, teammates, and worktrees.
//!
//! ## [`ToolRouter`]
//!
//! A registry that maps tool names to `Box<dyn Tool>`.  Tools are
//! registered via the builder-pattern method [`ToolRouter::route`].
//! The top-level tool set is assembled in [`toolset`]; sub-agents get a
//! restricted set via [`subagent_toolset`].
//!
//! ## `#[tool]` proc macro
//!
//! The [`tool_refactor_macros::tool`] attribute macro (re-exported from
//! `crates/tool_refactor_macros`) auto-generates the [`Tool`] impl and
//! JSON schema from an async function signature.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ToolSpec;
use crate::background::SharedBackgroundManager;
use crate::cron::SharedCronScheduler;
use crate::memory::MemoryManager;
use crate::skill::SkillRegistry;
use crate::task::SharedTaskManager;
use crate::team::SharedTeammateManager;
use crate::worktree::SharedWorktreeManager;
use anyhow::{Context as AnyhowContext, Result};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde_json::Value;
use tact_core::AgentUpdate;

mod apply_patch;
mod ask_user;
mod background;
mod bash;
mod batch_edit;
mod batch_read;
mod compact;
mod cron;
mod edit_file;
mod load_skill;
mod lsp;
mod math;
mod memory;
mod read_file;
mod search_code;
mod sleep;
mod subagent;
mod task;
mod team;
mod web_fetch;
mod web_search;
mod worktree;
mod write_file;
use apply_patch::ApplyPatchTool;
use ask_user::AskUserTool;
use background::{BackgroundRunTool, CheckBackgroundTool};
use bash::BashTool;
use batch_edit::BatchEditTool;
use batch_read::BatchReadTool;
use compact::CompactTool;
use cron::{CronCreateTool, CronDeleteTool, CronListTool};
use edit_file::EditFileTool;
use load_skill::LoadSkillTool;
use lsp::QueryLspTool;
use math::AddTool;
use memory::SaveMemoryTool;
use read_file::ReadFileTool;
use search_code::SearchCodeTool;
use sleep::SleepTool;
use subagent::TaskTool;
use task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use team::{
    BroadcastTool, ListTeammatesTool, PlanApprovalTool, ReadInboxTool, SendMessageTool,
    ShutdownRequestTool, ShutdownResponseTool, SpawnTeammateTool,
};
use web_fetch::WebFetchTool;
use web_search::WebSearchTool;
use worktree::{
    WorktreeCreateTool, WorktreeEventsTool, WorktreeListTool, WorktreeRunTool, WorktreeStatusTool,
};
use write_file::WriteFileTool;

/// Shared state available to every tool invocation.
///
/// Contains the skill registry, persistent memory manager, current work
/// directory, and typed handles for task management, background tasks,
/// cron scheduling, teammates, and worktrees.
#[derive(Clone)]
pub struct ToolContext {
    pub skill_registry: Arc<SkillRegistry>,
    pub memory_manager: Arc<std::sync::Mutex<MemoryManager>>,
    pub work_dir: PathBuf,
    pub task_manager: SharedTaskManager,
    pub background_manager: SharedBackgroundManager,
    pub cron_scheduler: SharedCronScheduler,
    pub teammate_manager: SharedTeammateManager,
    pub worktree_manager: SharedWorktreeManager,
    pub ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
}

/// Assembles the full tool set for the main agent loop.
pub fn toolset() -> ToolRouter {
    ToolRouter::new()
        .route(AddTool)
        .route(ApplyPatchTool)
        .route(AskUserTool)
        .route(BashTool)
        .route(BatchEditTool)
        .route(BatchReadTool)
        .route(BackgroundRunTool)
        .route(CheckBackgroundTool)
        .route(CronCreateTool)
        .route(CronDeleteTool)
        .route(CronListTool)
        .route(ReadFileTool)
        .route(SearchCodeTool)
        .route(SleepTool)
        .route(WriteFileTool)
        .route(EditFileTool)
        .route(LoadSkillTool)
        .route(QueryLspTool)
        .route(SaveMemoryTool)
        .route(CompactTool)
        .route(TaskTool)
        .route(TaskCreateTool)
        .route(TaskGetTool)
        .route(TaskListTool)
        .route(TaskUpdateTool)
        .route(SpawnTeammateTool)
        .route(ListTeammatesTool)
        .route(SendMessageTool)
        .route(BroadcastTool)
        .route(ReadInboxTool)
        .route(PlanApprovalTool)
        .route(ShutdownRequestTool)
        .route(ShutdownResponseTool)
        .route(WorktreeCreateTool)
        .route(WorktreeListTool)
        .route(WorktreeStatusTool)
        .route(WorktreeRunTool)
        .route(WorktreeEventsTool)
        .route(WebFetchTool)
        .route(WebSearchTool)
}

/// Assembles the restricted tool set for sub-agents.
/// Sub-agents get only `bash`, `read_file`, `write_file`, and `edit_file`.
pub fn subagent_toolset() -> ToolRouter {
    ToolRouter::new()
        .route(BashTool)
        .route(ReadFileTool)
        .route(SearchCodeTool)
        .route(SleepTool)
        .route(WriteFileTool)
        .route(EditFileTool)
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> Value;

    async fn call(&self, context: ToolContext, input: Value) -> Result<String>;

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: Some(self.description().to_string()),
            input_schema: self.input_schema(),
        }
    }
}

/// A registry of named tools.
///
/// Tools are stored as `Box<dyn Tool>` and dispatched by name on every
/// [`call`](ToolRouter::call).  The router can also emit the full list of
/// [`ToolSpec`] values for inclusion in the LLM API request.
pub struct ToolRouter {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRouter {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn route<T>(mut self, tool: T) -> Self
    where
        T: Tool + 'static,
    {
        self.tools.insert(tool.name().to_string(), Box::new(tool));
        self
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.tool_spec()).collect()
    }

    pub async fn call(&self, context: &ToolContext, name: &str, input: Value) -> Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;

        tool.call(context.clone(), input).await
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn input_schema<T>() -> Value
where
    T: JsonSchema,
{
    serde_json::to_value(schemars::schema_for!(T)).expect("schema generation should not fail")
}

fn safe_path(work_dir: &Path, path: &str) -> Result<PathBuf> {
    resolve_safe_path(work_dir, path, false)
}

fn safe_path_allow_missing(work_dir: &Path, path: &str) -> Result<PathBuf> {
    resolve_safe_path(work_dir, path, true)
}

fn resolve_safe_path(work_dir: &Path, path: &str, allow_missing: bool) -> Result<PathBuf> {
    let work_dir = work_dir.canonicalize()?;
    let candidate = work_dir.join(path);

    let full = if candidate.exists() || !allow_missing {
        candidate.canonicalize()?
    } else {
        let parent = candidate
            .parent()
            .context("Path has no parent")?
            .canonicalize()?;

        if !parent.starts_with(&work_dir) {
            return Err(anyhow::anyhow!("Path escapes workspace"));
        }

        let file_name = candidate.file_name().context("Path has no file name")?;

        parent.join(file_name)
    };

    if !full.starts_with(&work_dir) {
        return Err(anyhow::anyhow!("Path escapes workspace"));
    }

    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        background::SharedBackgroundManager,
        cron::{CronScheduler, SharedCronScheduler},
        memory::MemoryManager,
        store::StoreRoot,
        task::{SharedTaskManager, TaskManager},
        team::{SharedTeammateManager, TeammateManager},
        worktree::{SharedWorktreeManager, WorktreeManager},
    };

    #[derive(serde::Deserialize, JsonSchema)]
    struct EchoInput {
        #[schemars(description = "Text to echo.")]
        text: String,
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "Echo text with a prefix."
        }

        fn input_schema(&self) -> Value {
            input_schema::<EchoInput>()
        }

        async fn call(&self, context: ToolContext, input: Value) -> Result<String> {
            let input: EchoInput = serde_json::from_value(input)?;
            Ok(format!("{}:{}", context.work_dir.display(), input.text))
        }
    }

    #[tokio::test]
    async fn router_dispatches_by_tool_name() {
        let router = ToolRouter::new().route(EchoTool);
        let context = test_context("router_dispatches_by_tool_name");

        let output = router
            .call(&context, "echo", serde_json::json!({ "text": "tool" }))
            .await
            .unwrap();

        assert!(output.ends_with(":tool"));
        assert!(output.contains("tact-tool-test-router_dispatches_by_tool_name"));
    }

    #[test]
    fn schema_is_generated_from_input_type() {
        let spec = EchoTool.tool_spec();
        let schema = spec.input_schema;

        assert_eq!(schema["title"], "EchoInput");
        assert_eq!(schema["properties"]["text"]["type"], "string");
        assert_eq!(schema["properties"]["text"]["description"], "Text to echo.");
        assert_eq!(schema["required"][0], "text");
    }

    #[tokio::test]
    async fn proc_macro_supports_plain_function_tools() {
        let router = ToolRouter::new().route(AddTool);
        let context = test_context("proc_macro_supports_plain_function_tools");

        let output = router
            .call(&context, "add", serde_json::json!({ "a": 2, "b": 3 }))
            .await
            .unwrap();

        assert_eq!(output, "5");

        let schema = AddTool.tool_spec().input_schema;
        assert_eq!(schema["properties"]["a"]["type"], "integer");
        assert_eq!(
            schema["properties"]["a"]["description"],
            "Left integer operand."
        );
        assert_eq!(schema["properties"]["b"]["type"], "integer");
    }

    #[tokio::test]
    async fn write_file_creates_expected_content() {
        let router = ToolRouter::new().route(WriteFileTool);
        let context = test_context("write_file_creates_expected_content");
        let path = "test.txt";
        let content = "hello world\nsecond line\n";

        let output = router
            .call(
                &context,
                "write_file",
                serde_json::json!({ "path": path, "content": content }),
            )
            .await
            .unwrap();

        assert!(output.contains("Wrote"));
        assert!(output.contains("test.txt"));
        assert!(output.contains("bytes"));
        assert!(output.contains("lines"));
        let written =
            std::fs::read_to_string(context.work_dir.join(path)).unwrap();
        assert_eq!(written, content);
    }

    #[tokio::test]
    async fn write_file_emits_progress_for_large_content() {
        let router = ToolRouter::new().route(WriteFileTool);
        let mut context = test_context("write_file_emits_progress");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        context.ui_tx = Some(tx);

        // Content larger than MIN_PROGRESS_SIZE to trigger progress messages.
        let content = "x".repeat(300 * 1024);
        let path = "large.txt";

        let output = router
            .call(
                &context,
                "write_file",
                serde_json::json!({ "path": path, "content": content }),
            )
            .await
            .unwrap();

        assert!(output.contains("Wrote"));
        let written = std::fs::read_to_string(context.work_dir.join(path)).unwrap();
        assert_eq!(written.len(), content.len());

        let mut progress_count = 0;
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::Info(msg) = update {
                assert!(msg.contains("Writing"));
                assert!(msg.contains("large.txt"));
                progress_count += 1;
            }
        }
        assert!(progress_count > 0, "expected at least one progress update");
    }

    fn test_context(name: &str) -> ToolContext {
        let root_dir = std::env::temp_dir().join(format!("tact-tool-test-{name}"));
        let _ = std::fs::remove_dir_all(&root_dir);
        std::fs::create_dir_all(&root_dir).unwrap();
        let store_root = StoreRoot::new(root_dir.join(".claude")).unwrap();

        ToolContext {
            skill_registry: Arc::new(SkillRegistry::new(root_dir.join("skills"))),
            memory_manager: Arc::new(std::sync::Mutex::new(MemoryManager::new(
                root_dir.join(".claude/memory"),
            ))),
            work_dir: root_dir.clone(),
            task_manager: SharedTaskManager::new(TaskManager::new(&store_root).unwrap()),
            background_manager: SharedBackgroundManager::new(&store_root).unwrap(),
            cron_scheduler: SharedCronScheduler::new(CronScheduler::new(&store_root).unwrap()),
            teammate_manager: SharedTeammateManager::new(
                TeammateManager::new(&store_root).unwrap(),
            ),
            worktree_manager: SharedWorktreeManager::new(
                WorktreeManager::new(&store_root, root_dir).unwrap(),
            ),
            ui_tx: None,
        }
    }
}
