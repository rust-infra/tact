use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use super::{Tool, ToolCallResult, ToolContext, ToolRouter};
use crate::{
    background::{BackgroundManager, SharedBackgroundManager},
    memory::MemoryManager,
    skill::{SharedSkillRegistry, SkillRegistry},
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    worktree::{SharedWorktreeManager, WorktreeManager},
};

pub async fn run_tool<T: Tool + 'static>(
    context: &ToolContext,
    tool: T,
    name: &'static str,
    input: serde_json::Value,
) -> anyhow::Result<String> {
    ToolRouter::new()
        .route(tool)?
        .call(context, name, input)
        .await
}

/// Like [`run_tool`] but returns the full [`ToolCallResult`] including effects.
pub async fn run_tool_result<T: Tool + 'static>(
    context: &ToolContext,
    tool: T,
    name: &'static str,
    input: serde_json::Value,
) -> anyhow::Result<ToolCallResult> {
    ToolRouter::new()
        .route(tool)?
        .call_result(context, name, input)
        .await
}

/// Runs a future to completion on a fresh thread with its own runtime.
///
/// `test_context` is synchronous (called from 80+ sync test helpers), but
/// `TaskManager::new` is async. A scoped thread with a fresh runtime avoids
/// "cannot start a runtime from within a runtime" when the caller is already
/// inside a `#[tokio::test]` body.
fn block_on<F>(future: F) -> F::Output
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Runtime::new()
                    .expect("failed to create tokio runtime")
                    .block_on(future)
            })
            .join()
            .expect("block_on thread panicked")
    })
}

pub fn test_context(name: &str) -> ToolContext {
    let root_dir = std::env::temp_dir().join(format!("tact-tool-test-{name}"));
    let _ = std::fs::remove_dir_all(&root_dir);
    std::fs::create_dir_all(&root_dir).unwrap();
    let db_path = root_dir.join(".tact").join("tact.db");

    ToolContext {
        skill_registry: Arc::new(Mutex::new(SkillRegistry::new([
            root_dir.join(".claude/skills")
        ]))),
        memory_manager: Arc::new(std::sync::Mutex::new(MemoryManager::new(
            root_dir.join(".tact/memory"),
        ))),
        work_dir: root_dir.clone(),
        task_manager: SharedTaskManager::new(block_on(TaskManager::new(&db_path)).unwrap()),
        background_manager: SharedBackgroundManager::new(
            block_on(BackgroundManager::new(&db_path)).unwrap(),
        ),
        teammate_manager: SharedTeammateManager::new(
            block_on(TeammateManager::new(&db_path)).unwrap(),
        ),
        worktree_manager: SharedWorktreeManager::new(
            block_on(WorktreeManager::new(&db_path, root_dir)).unwrap(),
        ),
        ui_tx: None,
        ui_responder: crate::ui_responder::UiResponder::new(),
        progress_reporter: super::ToolProgressReporter::default(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bash_timeout_secs: crate::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
        bash_nice: 0,
        session_id: None,
        session_store: None,
    }
}

pub fn write_workspace_file(work_dir: &Path, path: &str, content: &str) {
    let full = work_dir.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(full, content).unwrap();
}

pub fn install_skill(work_dir: &Path, name: &str, body: &str) -> SharedSkillRegistry {
    let skill_dir = work_dir.join(".claude/skills").join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test skill\n---\n\n{body}"),
    )
    .unwrap();
    let mut registry = SkillRegistry::new([work_dir.join(".claude/skills")]);
    registry.load_skills().unwrap();
    Arc::new(Mutex::new(registry))
}
