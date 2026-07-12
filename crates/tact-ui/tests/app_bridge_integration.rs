//! Bridge tact-ui driver output into the TUI App state and render layer.

mod harness;

use harness::{
    read_file_tool_use, run_single_task, run_single_task_with_setup, text_block,
    write_file_tool_use,
};
use tact::permission::PermissionMode;
use tact::tool::test_support::write_workspace_file;
use tact_llm::MockClient;
use tact_llm::StopReason;
use tact_protocol::AgentUpdate;
use tui::test_support::TestApp;

#[tokio::test]
async fn driver_stream_complete_renders_in_app() {
    let mock = MockClient::new(vec![(
        vec![text_block("Bridge hello.")],
        Some(StopReason::EndTurn),
    )]);

    let (updates, work_dir) = run_single_task(mock, "say hello", PermissionMode::Auto).await;

    let has_response = updates.iter().any(|u| match u {
        AgentUpdate::StreamChunk(s) | AgentUpdate::TaskComplete(s) => s.contains("Bridge hello"),
        _ => false,
    });
    assert!(
        has_response,
        "driver should emit assistant text in updates: {updates:?}"
    );

    let mut app = TestApp::new_in_dir(work_dir);
    app.feed_all(updates);

    assert!(app.is_done(), "TaskComplete should set Done status");
    let text = app.render(120, 28);
    assert!(
        text.contains("Task completed") || text.contains("Done"),
        "completed task should affect rendered status bar:\n{text}"
    );
}

#[tokio::test]
async fn driver_read_file_tool_renders_card_in_app() {
    let mock = MockClient::new(vec![
        (
            vec![read_file_tool_use("read1", "sample.txt")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Read complete.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, work_dir) =
        run_single_task_with_setup(mock, "read sample", PermissionMode::Auto, |dir| {
            write_workspace_file(dir, "sample.txt", "bridge file content");
        })
        .await;

    let mut app = TestApp::new_in_dir(work_dir);
    app.feed_all(updates);

    let text = app.render(120, 30);
    assert!(
        text.contains("read_file") || text.contains("sample.txt"),
        "read_file tool card should render after feeding driver updates:\n{text}"
    );
    assert!(app.is_done());
}

#[tokio::test]
async fn driver_write_then_read_opens_diff_popup_in_app() {
    let mock = MockClient::new(vec![
        (
            vec![write_file_tool_use("w1", "out.rs", "fn bridge_fn() {}")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![read_file_tool_use("r1", "out.rs")],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, work_dir) = run_single_task(mock, "write and read", PermissionMode::Auto).await;

    let mut app = TestApp::new_in_dir(work_dir.clone());
    app.feed_all(updates);

    assert!(
        app.open_last_tool_popup(),
        "last tool block should support diff popup"
    );
    let popup_text = app.render_main(120, 30);
    assert!(
        popup_text.contains("bridge_fn") || popup_text.contains("out.rs"),
        "diff popup should show file content from real tool finish path:\n{popup_text}"
    );
}

#[tokio::test]
async fn driver_token_usage_reaches_app_render() {
    use harness::{mock_turn_with_usage, sample_token_usage};

    let mock = MockClient::with_usage(vec![mock_turn_with_usage(
        vec![text_block("With usage.")],
        StopReason::EndTurn,
        sample_token_usage(),
    )]);

    let (updates, work_dir) = run_single_task(mock, "count tokens", PermissionMode::Auto).await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::TokenUsage(_))),
        "driver should emit TokenUsage"
    );

    let mut app = TestApp::new_in_dir(work_dir);
    app.feed_all(updates);

    let text = app.render(120, 24);
    assert!(
        text.contains("150") || text.contains("token") || text.contains("Token"),
        "token usage should affect rendered status/log:\n{text}"
    );
}
