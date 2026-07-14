//! Integration tests for agent subsystems that do not require a real LLM.

mod harness;

use harness::{
    apply_patch_tool_use, ask_user_tool_use, background_run_tool_use, check_background_tool_use,
    cron_create_tool_use, cron_list_tool_use, load_skill_tool_use, read_inbox_tool_use,
    run_single_task_with_setup, save_memory_tool_use, search_code_tool_use, send_message_tool_use,
    spawn_teammate_tool_use, task_completed_with, text_block, worktree_create_tool_use,
    worktree_list_tool_use, worktree_status_tool_use,
};
use tact::permission::PermissionMode;
use tact::tool::test_support::write_workspace_file;
use tact_llm::MockClient;
use tact_llm::StopReason;
use tact_protocol::{AgentUpdate, StepStatus};

#[tokio::test]
async fn ask_user_tool_returns_question() {
    let mock = MockClient::new(vec![
        (
            vec![ask_user_tool_use("ask1", "What is your name?", None)],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, _work_dir) =
        run_single_task_with_setup(mock, "ask user", PermissionMode::Auto, |_| {}).await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "ask1" && result.tool == "ask_user" && matches!(result.status, StepStatus::Success))),
        "ask_user should succeed: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Done."));
}

#[tokio::test]
async fn save_memory_persists_file() {
    let mock = MockClient::new(vec![
        (
            vec![save_memory_tool_use(
                "mem1",
                "test_preference",
                "user",
                "tab preference",
                "I prefer tabs.",
            )],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Memory saved.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, work_dir) =
        run_single_task_with_setup(mock, "save memory", PermissionMode::Auto, |_| {}).await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "mem1" && result.tool == "save_memory" && matches!(result.status, StepStatus::Success))),
        "save_memory should succeed: {updates:?}"
    );
    let memory_file = work_dir
        .join(".claude")
        .join("memory")
        .join("test_preference.md");
    assert!(memory_file.exists());
    let content = std::fs::read_to_string(memory_file).unwrap();
    assert!(content.contains("I prefer tabs."));
}

#[tokio::test]
async fn load_skill_reads_skill_file() {
    let mock = MockClient::new(vec![
        (
            vec![load_skill_tool_use("skill1", "rust_style")],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Skill loaded.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, _work_dir) = run_single_task_with_setup(
        mock,
        "load skill",
        PermissionMode::Auto,
        |dir| {
            let skill_dir = dir.join(".claude/skills").join("rust_style");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: rust_style\ndescription: Rust style guide\n---\n\nUse Result everywhere.",
            )
            .unwrap();
        },
    )
    .await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "skill1" && result.tool == "load_skill" && matches!(result.status, StepStatus::Success))),
        "load_skill should succeed: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Skill loaded."));
}

#[tokio::test]
async fn teammate_spawn_send_read_inbox() {
    let mock = MockClient::new(vec![
        (
            vec![spawn_teammate_tool_use(
                "spawn1",
                "reviewer",
                "Code reviewer",
            )],
            Some(StopReason::ToolUse),
        ),
        (
            vec![send_message_tool_use(
                "msg1",
                "user",
                "reviewer",
                "Please review.",
            )],
            Some(StopReason::ToolUse),
        ),
        (
            vec![read_inbox_tool_use("inbox1", "reviewer")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Teammate flow done.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) =
        run_single_task_with_setup(mock, "teammate flow", PermissionMode::Auto, |_| {}).await;

    for id in ["spawn1", "msg1", "inbox1"] {
        assert!(
            updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id, result, .. } if tool_id == id && matches!(result.status, StepStatus::Success))),
            "step {id} should succeed: {updates:?}"
        );
    }
}

#[tokio::test]
async fn cron_create_list_delete() {
    let mock = MockClient::new(vec![
        (
            vec![cron_create_tool_use("cron1", "0 0 * * *", "Daily summary.")],
            Some(StopReason::ToolUse),
        ),
        (vec![cron_list_tool_use("cron2")], Some(StopReason::ToolUse)),
        (
            vec![text_block("Cron flow done.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) =
        run_single_task_with_setup(mock, "cron flow", PermissionMode::Auto, |_| {}).await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "cron1" && result.tool == "cron_create" && matches!(result.status, StepStatus::Success))),
        "cron_create should succeed: {updates:?}"
    );
    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "cron2" && result.tool == "cron_list")),
        "cron_list should run: {updates:?}"
    );
}

#[tokio::test]
async fn background_run_and_check() {
    let mock = MockClient::new(vec![
        (
            vec![background_run_tool_use("bg1", "sleep 0.1 && echo bg-ok")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![check_background_tool_use("bg2", None)],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Background done.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) =
        run_single_task_with_setup(mock, "background flow", PermissionMode::Auto, |_| {}).await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "bg1" && result.tool == "background_run" && matches!(result.status, StepStatus::Success))),
        "background_run should succeed: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, .. } if id == "bg2")),
        "check_background should run: {updates:?}"
    );
}

#[tokio::test]
async fn search_code_finds_matches() {
    let mock = MockClient::new(vec![
        (
            vec![search_code_tool_use("search1", "fn answer", Some("src"))],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Search done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, _work_dir) =
        run_single_task_with_setup(mock, "search code", PermissionMode::Auto, |dir| {
            write_workspace_file(dir, "src/lib.rs", "fn answer() -> i32 { 42 }\n")
        })
        .await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "search1" && result.tool == "search_code" && matches!(result.status, StepStatus::Success))),
        "search_code should succeed: {updates:?}"
    );
}

#[tokio::test]
async fn apply_patch_modifies_file() {
    let patch = r#"--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-fn old() {}
+fn new() {}
"#;

    let mock = MockClient::new(vec![
        (
            vec![apply_patch_tool_use("patch1", patch)],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Patch applied.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, work_dir) =
        run_single_task_with_setup(mock, "apply patch", PermissionMode::Auto, |dir| {
            write_workspace_file(dir, "src/main.rs", "fn old() {}\n")
        })
        .await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, result, .. } if id == "patch1" && result.tool == "apply_patch" && matches!(result.status, StepStatus::Success))),
        "apply_patch should succeed: {updates:?}"
    );
    let content = std::fs::read_to_string(work_dir.join("src/main.rs")).unwrap();
    assert!(content.contains("fn new()"));
}

#[tokio::test]
async fn worktree_create_lists_and_shows_status() {
    let mock = MockClient::new(vec![
        (
            vec![worktree_create_tool_use("wt1", "feature-x", None)],
            Some(StopReason::ToolUse),
        ),
        (
            vec![worktree_list_tool_use("wt2")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![worktree_status_tool_use("wt3", "feature-x")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Worktree done.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) =
        run_single_task_with_setup(mock, "worktree flow", PermissionMode::Auto, |dir| {
            // Initialize a git repo so worktree operations succeed.
            let _ = std::process::Command::new("git")
                .args(["init"])
                .current_dir(dir)
                .output();
            let _ = std::process::Command::new("git")
                .args(["config", "user.email", "test@example.com"])
                .current_dir(dir)
                .output();
            let _ = std::process::Command::new("git")
                .args(["config", "user.name", "Test"])
                .current_dir(dir)
                .output();
            // Create an initial commit so HEAD exists.
            write_workspace_file(dir, "README.md", "# test");
            let _ = std::process::Command::new("git")
                .args(["add", "."])
                .current_dir(dir)
                .output();
            let _ = std::process::Command::new("git")
                .args(["commit", "-m", "init"])
                .current_dir(dir)
                .output();
        })
        .await;

    for id in ["wt1", "wt2", "wt3"] {
        assert!(
            updates.iter().any(|u| matches!(u, AgentUpdate::StepFinished { tool_id, result, .. } if tool_id == id && matches!(result.status, StepStatus::Success))),
            "step {id} should succeed: {updates:?}"
        );
    }
}
