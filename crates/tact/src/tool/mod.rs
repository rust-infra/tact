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

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use anyhow::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde_json::Value;
use tact_protocol::AgentUpdate;
#[cfg(test)]
use tact_protocol::ToolVisualKind;

use crate::{
    ToolSpec, background::SharedBackgroundManager, cron::SharedCronScheduler,
    memory::MemoryManager, task::SharedTaskManager, team::SharedTeammateManager,
    worktree::SharedWorktreeManager,
};

mod ask_user;
mod background_run;
mod bash;
mod compact;
mod cron;
mod edit_file;
mod load_skill;
mod memory;
mod metadata;
mod path;
mod progress;
mod read_file;
mod registry;
mod sleep;
mod subagent;
#[cfg(feature = "test-support")]
pub mod subagent_ui;
#[cfg(not(feature = "test-support"))]
mod subagent_ui;
mod task;
mod team;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod worktree;
mod write_file;

#[cfg(test)]
use background_run::{BackgroundRunTool, CheckBackgroundTool};
#[cfg(test)]
use bash::BashTool;
#[cfg(test)]
use cron::{CronCreateTool, CronDeleteTool, CronListTool};
#[cfg(test)]
use edit_file::EditFileTool;
#[cfg(test)]
use load_skill::LoadSkillTool;
#[cfg(test)]
use memory::SaveMemoryTool;
#[cfg(test)]
use read_file::ReadFileTool;
pub use registry::{subagent_toolset, toolset};
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
    /// Shared with the TUI in interactive mode so `/skill-reload` updates
    /// `load_skill` / system-prompt skill summaries without restarting.
    pub skill_registry: crate::skill::SharedSkillRegistry,
    pub memory_manager: Arc<std::sync::Mutex<MemoryManager>>,
    pub work_dir: PathBuf,
    pub task_manager: SharedTaskManager,
    pub background_manager: SharedBackgroundManager,
    pub cron_scheduler: SharedCronScheduler,
    pub teammate_manager: SharedTeammateManager,
    pub worktree_manager: SharedWorktreeManager,
    pub ui_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentUpdate>>,
    pub progress_reporter: ToolProgressReporter,
    pub cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub bash_timeout_secs: u64,
    /// Nice increment applied to the bash sub-process group (macOS/Linux only).
    /// Default 10 so TUI stays responsive during heavy commands like `cargo test`.
    /// 0 disables. Maximum is 19 (lowest priority).
    pub bash_nice: i32,
    /// Parent agent session id when persistence is wired (`with_session`).
    pub session_id: Option<String>,
    /// Shared SQLite session store from the parent agent, if any.
    pub session_store: Option<crate::store::DynSessionStore>,
}

impl ToolContext {
    pub fn for_invocation(&self, tool_id: &str) -> Self {
        let mut context = self.clone();
        context.progress_reporter = ToolProgressReporter::new(tool_id, self.ui_tx.clone());
        context
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn metadata(&self) -> &'static ToolMetadata;
    fn input_schema(&self) -> Value;

    async fn call(&self, context: ToolContext, input: Value) -> Result<ToolCallResult>;

    fn tool_spec(&self) -> ToolSpec {
        let metadata = self.metadata();
        ToolSpec {
            name: metadata.name.to_string(),
            description: Some(metadata.description.to_string()),
            input_schema: self.input_schema(),
        }
    }
}

/// A registered tool: handler + its static metadata.
struct RegisteredTool {
    handler: Box<dyn Tool>,
    metadata: &'static ToolMetadata,
}

/// A resolved native tool handle, obtained from [`ToolRouter::resolve`].
#[derive(Clone, Copy)]
pub struct ResolvedNativeTool<'a> {
    registered: &'a RegisteredTool,
}

impl ResolvedNativeTool<'_> {
    pub fn metadata(self) -> &'static ToolMetadata {
        self.registered.metadata
    }

    pub async fn call(self, context: ToolContext, input: Value) -> Result<ToolCallResult> {
        self.registered.handler.call(context, input).await
    }
}

/// A registry of named tools.
///
/// Tools are stored as `Box<dyn Tool>` and dispatched by name on every
/// [`call`](ToolRouter::call).  The router can also emit the full list of
/// [`ToolSpec`] values for inclusion in the LLM API request.
pub struct ToolRouter {
    tools: HashMap<String, RegisteredTool>,
    cached_specs: OnceLock<Vec<ToolSpec>>,
}

impl ToolRouter {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            cached_specs: OnceLock::new(),
        }
    }

    pub fn route<T>(mut self, tool: T) -> Result<Self>
    where
        T: Tool + 'static,
    {
        let metadata = tool.metadata();
        let name = metadata.name.to_string();
        if self.tools.contains_key(&name) {
            anyhow::bail!("duplicate native tool name: {name}");
        }
        self.tools.insert(
            name,
            RegisteredTool {
                handler: Box::new(tool),
                metadata,
            },
        );
        Ok(self)
    }

    pub fn resolve(&self, name: &str) -> Result<ResolvedNativeTool<'_>> {
        self.tools
            .get(name)
            .map(|registered| ResolvedNativeTool { registered })
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.cached_specs
            .get_or_init(|| {
                self.tools
                    .values()
                    .map(|rt| rt.handler.tool_spec())
                    .collect()
            })
            .iter()
            .map(copy_tool_spec)
            .collect()
    }

    pub async fn call_result(
        &self,
        context: &ToolContext,
        name: &str,
        input: Value,
    ) -> Result<ToolCallResult> {
        let resolved = self.resolve(name)?;
        resolved.call(context.clone(), input).await
    }

    pub async fn call(&self, context: &ToolContext, name: &str, input: Value) -> Result<String> {
        self.call_result(context, name, input)
            .await
            .map(|r| r.content)
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

pub use metadata::{
    ArgumentSummaryPolicy, DetailPolicy, IntoToolCallResult, LiveOutputPolicy, OutputPolicy,
    PermissionPolicy, PermissionPromptPolicy, PopupPolicy, ResourcePolicy, TaskOperation,
    ToolCallResult, ToolDomain, ToolEffect, ToolMetadata, ToolPresentation,
};
pub use path::{safe_path, safe_path_allow_missing};
pub use progress::ToolProgressReporter;

#[cfg(test)]
mod tests {
    use super::{
        test_support::{install_skill, test_context, write_workspace_file},
        *,
    };

    #[derive(serde::Deserialize, JsonSchema)]
    struct EchoInput {
        #[schemars(description = "Text to echo.")]
        text: String,
    }

    struct EchoTool;

    pub const ECHO_METADATA: ToolMetadata = ToolMetadata {
        name: "echo",
        description: "Echo text with a prefix.",
        permission: PermissionPolicy::Read,
        permission_prompt: PermissionPromptPolicy::Json,
        resources: ResourcePolicy::Independent,
        domain: ToolDomain::Generic,
        presentation: ToolPresentation {
            visual_kind: ToolVisualKind::Generic,
            display_name: "echo",
            live_output: LiveOutputPolicy::Standard,
            detail: DetailPolicy::Result,
            popup: PopupPolicy::None,
            compact_result_to_meta: false,
        },
        output: OutputPolicy::KeepInline,
        argument_summary: ArgumentSummaryPolicy::Json,
    };

    #[async_trait]
    impl Tool for EchoTool {
        fn metadata(&self) -> &'static ToolMetadata {
            &ECHO_METADATA
        }

        fn input_schema(&self) -> Value {
            input_schema::<EchoInput>()
        }

        async fn call(&self, context: ToolContext, input: Value) -> Result<ToolCallResult> {
            let input: EchoInput = serde_json::from_value(input)?;
            Ok(ToolCallResult::text(format!(
                "{}:{}",
                context.work_dir.display(),
                input.text
            )))
        }
    }

    #[tokio::test]
    async fn router_dispatches_by_tool_name() {
        let router = ToolRouter::new().route(EchoTool).unwrap();
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
        let router = ToolRouter::new().route(EchoTool).unwrap();
        let context = test_context("router_rejects_unknown_tool");

        let error = router
            .call(&context, "missing_tool", serde_json::json!({}))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("unknown tool: missing_tool"));
    }

    #[test]
    fn router_resolves_handler_and_metadata_together() {
        let router = ToolRouter::new().route(EchoTool).unwrap();
        let resolved = router.resolve("echo").unwrap();
        assert_eq!(resolved.metadata().name, "echo");
    }

    #[test]
    fn router_rejects_duplicate_native_names() {
        let router = ToolRouter::new().route(EchoTool).unwrap();
        let result = router.route(EchoTool);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate native tool name: echo")
        );
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

        for tool in ["bash", "read_file", "write_file", "edit_file", "sleep"] {
            assert!(names.contains(&tool.to_string()), "missing {tool}");
        }
    }

    #[tokio::test]
    async fn proc_macro_supports_plain_function_tools() {
        let router = ToolRouter::new().route(SleepTool).unwrap();
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
        let router = ToolRouter::new().route(WriteFileTool).unwrap();
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
        let router = ToolRouter::new().route(WriteFileTool).unwrap();
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
        let router = ToolRouter::new().route(ReadFileTool).unwrap();
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

        assert!(
            output.starts_with("[PARTIAL view — lines 2-3; continue with offset=4]\n\n"),
            "got: {output}"
        );
        assert!(output.contains("line2\nline3"));
        assert!(!output.contains("line1"));
        assert!(!output.contains("line4"));
    }

    #[tokio::test]
    async fn read_file_rejects_path_outside_workspace() {
        let router = ToolRouter::new().route(ReadFileTool).unwrap();
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
    async fn edit_file_replaces_first_match() {
        let router = ToolRouter::new().route(EditFileTool).unwrap();
        let context = test_context("edit_file_replaces_first_match");
        write_workspace_file(&context.work_dir, "edit.txt", "alpha beta gamma");

        let output = router
            .call(
                &context,
                "edit_file",
                serde_json::json!({
                    "path": "edit.txt",
                    "old_text": "beta",
                    "new_text": "BETA"
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("Edited"));
        let updated = std::fs::read_to_string(context.work_dir.join("edit.txt")).unwrap();
        assert_eq!(updated, "alpha BETA gamma");
    }

    #[tokio::test]
    async fn edit_file_errors_when_text_missing() {
        let router = ToolRouter::new().route(EditFileTool).unwrap();
        let context = test_context("edit_file_errors_when_text_missing");
        write_workspace_file(&context.work_dir, "edit.txt", "unchanged");

        let error = router
            .call(
                &context,
                "edit_file",
                serde_json::json!({
                    "path": "edit.txt",
                    "old_text": "missing",
                    "new_text": "new"
                }),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Text not found"));
    }

    #[tokio::test]
    async fn bash_runs_command_in_workspace() {
        let router = ToolRouter::new().route(BashTool).unwrap();
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
        let router = ToolRouter::new().route(BashTool).unwrap();
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
    async fn save_memory_persists_entry() {
        let router = ToolRouter::new().route(SaveMemoryTool).unwrap();
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
        let memory_file = context.work_dir.join(".tact/memory/prefer_tabs.md");
        assert!(memory_file.exists());
        let saved = std::fs::read_to_string(memory_file).unwrap();
        assert!(saved.contains("Use tabs by default."));
    }

    #[tokio::test]
    async fn load_skill_returns_skill_body() {
        let mut context = test_context("load_skill_returns_skill_body");
        context.skill_registry = install_skill(&context.work_dir, "demo", "Skill body content.");
        let router = ToolRouter::new().route(LoadSkillTool).unwrap();

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
            .unwrap()
            .route(CronListTool)
            .unwrap()
            .route(CronDeleteTool)
            .unwrap();
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
            .unwrap()
            .route(ListTeammatesTool)
            .unwrap()
            .route(SendMessageTool)
            .unwrap()
            .route(ReadInboxTool)
            .unwrap();
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
            .unwrap()
            .route(CheckBackgroundTool)
            .unwrap();
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
            .unwrap()
            .route(TaskGetTool)
            .unwrap()
            .route(TaskListTool)
            .unwrap()
            .route(TaskUpdateTool)
            .unwrap();
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
