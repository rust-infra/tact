use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use super::{Tool, ToolContext, ToolRouter};
use crate::{
    background::SharedBackgroundManager,
    cron::{CronScheduler, SharedCronScheduler},
    memory::MemoryManager,
    skill::{SharedSkillRegistry, SkillRegistry},
    store::StoreRoot,
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
        .route(tool)
        .call(context, name, input)
        .await
}

pub fn test_context(name: &str) -> ToolContext {
    let root_dir = std::env::temp_dir().join(format!("tact-tool-test-{name}"));
    let _ = std::fs::remove_dir_all(&root_dir);
    std::fs::create_dir_all(&root_dir).unwrap();
    let store_root = StoreRoot::new(root_dir.join(".claude")).unwrap();

    ToolContext {
        skill_registry: Arc::new(Mutex::new(SkillRegistry::new([
            root_dir.join(".claude/skills")
        ]))),
        memory_manager: Arc::new(std::sync::Mutex::new(MemoryManager::new(
            root_dir.join(".claude/memory"),
        ))),
        work_dir: root_dir.clone(),
        task_manager: SharedTaskManager::new(TaskManager::new(&store_root).unwrap()),
        background_manager: SharedBackgroundManager::new(&store_root).unwrap(),
        cron_scheduler: SharedCronScheduler::new(CronScheduler::new(&store_root).unwrap()),
        teammate_manager: SharedTeammateManager::new(TeammateManager::new(&store_root).unwrap()),
        worktree_manager: SharedWorktreeManager::new(
            WorktreeManager::new(&store_root, root_dir).unwrap(),
        ),
        ui_tx: None,
        progress_reporter: super::ToolProgressReporter::default(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bash_timeout_secs: crate::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
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
