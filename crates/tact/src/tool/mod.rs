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
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::ToolSpec;
use crate::background::SharedBackgroundManager;
use crate::cron::SharedCronScheduler;
use crate::memory::MemoryManager;
use crate::skill::SkillRegistry;
use crate::task::SharedTaskManager;
use crate::team::SharedTeammateManager;
use crate::worktree::SharedWorktreeManager;
use anyhow::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde_json::Value;
use tact_protocol::AgentUpdate;

mod apply_patch;
mod ask_user;
mod background_run;
mod bash;
mod batch_edit;
mod batch_read;
mod compact;
mod cron;
mod load_skill;
mod lsp_tool;
mod memory;
mod path;
mod read_file;
mod registry;
mod search_code;
mod sleep;
mod subagent;
mod task;
mod team;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod web;
mod worktree;
mod write_file;

pub use registry::{subagent_toolset, toolset};

#[cfg(test)]
use background_run::{BackgroundRunTool, CheckBackgroundTool};
#[cfg(test)]
use bash::BashTool;
#[cfg(test)]
use batch_edit::BatchEditTool;
#[cfg(test)]
use batch_read::BatchReadTool;
#[cfg(test)]
use cron::{CronCreateTool, CronDeleteTool, CronListTool};
#[cfg(test)]
use load_skill::LoadSkillTool;
#[cfg(test)]
use memory::SaveMemoryTool;
#[cfg(test)]
use read_file::ReadFileTool;
#[cfg(test)]
use search_code::SearchCodeTool;
#[cfg(test)]
use sleep::SleepTool;
#[cfg(test)]
use task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
#[cfg(test)]
use team::{ListTeammatesTool, ReadInboxTool, SendMessageTool, SpawnTeammateTool};
#[cfg(test)]
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
    cached_specs: OnceLock<Vec<ToolSpec>>,
}

impl ToolRouter {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            cached_specs: OnceLock::new(),
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
        self.cached_specs
            .get_or_init(|| self.tools.values().map(|tool| tool.tool_spec()).collect())
            .iter()
            .map(copy_tool_spec)
            .collect()
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

pub(crate) fn copy_tool_spec(spec: &ToolSpec) -> ToolSpec {
    ToolSpec {
        name: spec.name.clone(),
        description: spec.description.clone(),
        input_schema: spec.input_schema.clone(),
    }
}

pub use path::{safe_path, safe_path_allow_missing};

#[cfg(test)]
mod tests {
    use super::test_support::{install_skill, test_context, write_workspace_file};
    use super::*;

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

    #[tokio::test]
    async fn router_rejects_unknown_tool() {
        let router = ToolRouter::new().route(EchoTool);
        let context = test_context("router_rejects_unknown_tool");

        let error = router
            .call(&context, "missing_tool", serde_json::json!({}))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("unknown tool: missing_tool"));
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

    #[test]
    fn subagent_toolset_includes_core_file_tools() {
        let router = subagent_toolset();
        let names: Vec<_> = router
            .tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();

        for tool in [
            "bash",
            "read_file",
            "write_file",
            "search_code",
            "sleep",
        ] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
        assert!(
            !names.iter().any(|n| n == "edit_file"),
            "edit_file was removed from the toolset"
        );
    }

    #[tokio::test]
    async fn proc_macro_supports_plain_function_tools() {
        let router = ToolRouter::new().route(SleepTool);
        let context = test_context("proc_macro_supports_plain_function_tools");

        let output = router
            .call(&context, "sleep", serde_json::json!({ "ms": 0 }))
            .await
            .unwrap();

        assert_eq!(output, "Slept for 0ms.");

        let schema = SleepTool.tool_spec().input_schema;
        assert_eq!(schema["properties"]["ms"]["type"], "integer");
        assert_eq!(
            schema["properties"]["ms"]["description"],
            "Duration to sleep in milliseconds (max 300000 = 5 minutes)."
        );
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
        assert!(output.contains(" B"));
        assert!(output.contains("lines"));
        let written = std::fs::read_to_string(context.work_dir.join(path)).unwrap();
        assert_eq!(written, content);
    }

    #[tokio::test]
    async fn write_file_emits_progress_for_large_content() {
        let router = ToolRouter::new().route(WriteFileTool);
        let mut context = test_context("write_file_emits_progress");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        context.ui_tx = Some(tx);

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

    #[tokio::test]
    async fn read_file_returns_content_with_offset_and_limit() {
        let router = ToolRouter::new().route(ReadFileTool);
        let context = test_context("read_file_returns_content_with_offset_and_limit");
        write_workspace_file(
            &context.work_dir,
            "sample.txt",
            "line1\nline2\nline3\nline4\n",
        );

        let output = router
            .call(
                &context,
                "read_file",
                serde_json::json!({ "path": "sample.txt", "offset": 2, "limit": 2 }),
            )
            .await
            .unwrap();

        assert!(output.contains("... (1 lines skipped) ..."));
        assert!(output.contains("line2"));
        assert!(output.contains("... (2 more lines)"));
        assert!(!output.contains("line4"));
    }

    #[tokio::test]
    async fn read_file_rejects_path_outside_workspace() {
        let router = ToolRouter::new().route(ReadFileTool);
        let context = test_context("read_file_rejects_path_outside_workspace");
        let outside_dir = context
            .work_dir
            .parent()
            .unwrap()
            .join("tact-outside-read_file_rejects_path_outside_workspace");
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("secret.txt"), "secret").unwrap();

        let error = router
            .call(
                &context,
                "read_file",
                serde_json::json!({
                    "path": "../tact-outside-read_file_rejects_path_outside_workspace/secret.txt"
                }),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Path escapes workspace"));
        let _ = std::fs::remove_dir_all(outside_dir);
    }

    #[tokio::test]
    async fn batch_read_reads_multiple_files() {
        let router = ToolRouter::new().route(BatchReadTool);
        let context = test_context("batch_read_reads_multiple_files");
        write_workspace_file(&context.work_dir, "a.txt", "aaa");
        write_workspace_file(&context.work_dir, "b.txt", "bbb");

        let output = router
            .call(
                &context,
                "batch_read",
                serde_json::json!({
                    "files": [
                        { "path": "a.txt" },
                        { "path": "b.txt" }
                    ]
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("BatchRead 2 files"));
        assert!(output.contains("aaa"));
        assert!(output.contains("bbb"));
    }

    #[tokio::test]
    async fn batch_read_rejects_empty_files_array() {
        let router = ToolRouter::new().route(BatchReadTool);
        let context = test_context("batch_read_rejects_empty_files_array");

        let error = router
            .call(&context, "batch_read", serde_json::json!({ "files": [] }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("files array must not be empty"));
    }

    #[tokio::test]
    async fn batch_edit_applies_edits_atomically() {
        let router = ToolRouter::new().route(BatchEditTool);
        let context = test_context("batch_edit_applies_edits_atomically");
        write_workspace_file(&context.work_dir, "one.txt", "foo bar");
        write_workspace_file(&context.work_dir, "two.txt", "ping pong");

        let output = router
            .call(
                &context,
                "batch_edit",
                serde_json::json!({
                    "edits": [
                        {
                            "file_path": "one.txt",
                            "old_string": "foo",
                            "new_string": "FOO"
                        },
                        {
                            "file_path": "two.txt",
                            "old_string": "pong",
                            "new_string": "PONG"
                        }
                    ]
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("BatchEdit"));
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("one.txt")).unwrap(),
            "FOO bar"
        );
        assert_eq!(
            std::fs::read_to_string(context.work_dir.join("two.txt")).unwrap(),
            "ping PONG"
        );
    }

    #[tokio::test]
    async fn bash_runs_command_in_workspace() {
        let router = ToolRouter::new().route(BashTool);
        let context = test_context("bash_runs_command_in_workspace");

        let output = router
            .call(
                &context,
                "bash",
                serde_json::json!({ "command": "echo hello-bash" }),
            )
            .await
            .unwrap();

        assert_eq!(output, "hello-bash");
    }

    #[tokio::test]
    async fn bash_blocks_dangerous_commands() {
        let router = ToolRouter::new().route(BashTool);
        let context = test_context("bash_blocks_dangerous_commands");

        let error = router
            .call(
                &context,
                "bash",
                serde_json::json!({ "command": "sudo rm -rf /" }),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Dangerous command blocked"));
    }

    #[tokio::test]
    async fn search_code_finds_matches_in_workspace() {
        let router = ToolRouter::new().route(SearchCodeTool);
        let context = test_context("search_code_finds_matches_in_workspace");
        write_workspace_file(
            &context.work_dir,
            "src/lib.rs",
            "pub fn unique_needle_xyz() {}\n",
        );

        let output = router
            .call(
                &context,
                "search_code",
                serde_json::json!({
                    "query": "unique_needle_xyz",
                    "path": "src",
                    "glob": "*.rs"
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("unique_needle_xyz"));
    }

    #[tokio::test]
    async fn save_memory_persists_entry() {
        let router = ToolRouter::new().route(SaveMemoryTool);
        let context = test_context("save_memory_persists_entry");

        let output = router
            .call(
                &context,
                "save_memory",
                serde_json::json!({
                    "name": "Prefer Tabs",
                    "description": "Indent with tabs",
                    "type": "user",
                    "content": "Use tabs by default."
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("Prefer Tabs") || output.contains("prefer_tabs"));
        let memory_file = context.work_dir.join(".claude/memory/prefer_tabs.md");
        assert!(memory_file.exists());
        let saved = std::fs::read_to_string(memory_file).unwrap();
        assert!(saved.contains("Use tabs by default."));
    }

    #[tokio::test]
    async fn load_skill_returns_skill_body() {
        let mut context = test_context("load_skill_returns_skill_body");
        context.skill_registry = install_skill(&context.work_dir, "demo", "Skill body content.");
        let router = ToolRouter::new().route(LoadSkillTool);

        let output = router
            .call(
                &context,
                "load_skill",
                serde_json::json!({ "name": "demo" }),
            )
            .await
            .unwrap();

        assert!(output.contains("<skill name=\"demo\">"));
        assert!(output.contains("Skill body content."));
    }

    #[tokio::test]
    async fn cron_tools_manage_scheduled_tasks() {
        let router = ToolRouter::new()
            .route(CronCreateTool)
            .route(CronListTool)
            .route(CronDeleteTool);
        let context = test_context("cron_tools_manage_scheduled_tasks");

        let created = router
            .call(
                &context,
                "cron_create",
                serde_json::json!({
                    "cron": "0 9 * * *",
                    "prompt": "Daily standup",
                    "recurring": true,
                    "durable": false
                }),
            )
            .await
            .unwrap();
        let id = serde_json::from_str::<serde_json::Value>(&created)
            .unwrap()
            .get("id")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let listed = router
            .call(&context, "cron_list", serde_json::json!({}))
            .await
            .unwrap();
        assert!(listed.contains(&id));
        assert!(listed.contains("Daily standup"));

        let deleted = router
            .call(&context, "cron_delete", serde_json::json!({ "id": id }))
            .await
            .unwrap();
        assert!(deleted.contains("Deleted scheduled task"));

        let listed = router
            .call(&context, "cron_list", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(listed, "No scheduled tasks.");
    }

    #[tokio::test]
    async fn team_tools_spawn_and_message() {
        let router = ToolRouter::new()
            .route(SpawnTeammateTool)
            .route(ListTeammatesTool)
            .route(SendMessageTool)
            .route(ReadInboxTool);
        let context = test_context("team_tools_spawn_and_message");

        router
            .call(
                &context,
                "spawn_teammate",
                serde_json::json!({ "name": "alice", "role": "reviewer" }),
            )
            .await
            .unwrap();

        let listed = router
            .call(&context, "list_teammates", serde_json::json!({}))
            .await
            .unwrap();
        assert!(listed.contains("alice [reviewer]"));

        router
            .call(
                &context,
                "send_message",
                serde_json::json!({
                    "from": "lead",
                    "to": "alice",
                    "body": "Please review PR #1"
                }),
            )
            .await
            .unwrap();

        let inbox = router
            .call(
                &context,
                "read_inbox",
                serde_json::json!({ "owner": "alice" }),
            )
            .await
            .unwrap();
        assert!(inbox.contains("Please review PR #1"));
    }

    #[tokio::test]
    async fn background_run_starts_and_completes() {
        let router = ToolRouter::new()
            .route(BackgroundRunTool)
            .route(CheckBackgroundTool);
        let context = test_context("background_run_starts_and_completes");

        let started = router
            .call(
                &context,
                "background_run",
                serde_json::json!({ "command": "echo bg-done" }),
            )
            .await
            .unwrap();
        assert!(started.contains("Background task"));
        let task_id = started
            .split_whitespace()
            .nth(2)
            .unwrap()
            .trim_end_matches(':')
            .to_string();

        let mut completed = false;
        for _ in 0..50 {
            let status = router
                .call(
                    &context,
                    "check_background",
                    serde_json::json!({ "task_id": task_id }),
                )
                .await
                .unwrap();
            if status.contains("completed") {
                completed = true;
                assert!(status.contains("bg-done"));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(completed, "background task did not complete in time");
    }

    #[tokio::test]
    async fn task_get_list_and_update() {
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .route(TaskGetTool)
            .route(TaskListTool)
            .route(TaskUpdateTool);
        let context = test_context("task_get_list_and_update");

        let created = router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "Lifecycle task" }),
            )
            .await
            .unwrap();
        let id = serde_json::from_str::<serde_json::Value>(&created)
            .unwrap()
            .get("id")
            .unwrap()
            .as_u64()
            .unwrap();

        let fetched = router
            .call(&context, "task_get", serde_json::json!({ "task_id": id }))
            .await
            .unwrap();
        assert!(fetched.contains("\"subject\": \"Lifecycle task\""));

        let listed = router
            .call(&context, "task_list", serde_json::json!({}))
            .await
            .unwrap();
        assert!(listed.contains("[ ] #1: Lifecycle task"));

        let updated = router
            .call(
                &context,
                "task_update",
                serde_json::json!({
                    "task_id": id,
                    "status": "in_progress",
                    "owner": "alice"
                }),
            )
            .await
            .unwrap();
        assert!(updated.contains("\"status\": \"in_progress\""));
        assert!(updated.contains("\"owner\": \"alice\""));
    }
}
