//! Fine-grained headless TUI harness tests: frame capture, popup introspection,
//! overlay toggles.

mod harness;

use anthropic_ai_sdk::types::message::StopReason;
use harness::{bash_tool_use, read_file_tool_use, text_block, write_file_tool_use};
use tact::permission::PermissionMode;
use tact::tool::test_support::write_workspace_file;
use tact_llm::MockClient;
use tact_ui::headless_session::run_headless_session_with_options;
use tui::test_support::TestApp;

#[tokio::test]
async fn headless_frame_capture_records_progression() {
    let mock = MockClient::new(vec![
        (
            vec![bash_tool_use("b1", "sleep 0.1")],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let result = run_headless_session_with_options(
        mock,
        PermissionMode::Auto,
        None,
        true, // capture frames
        |_| {},
        |tx| {
            tokio::spawn(async move {
                tx.send(tact_protocol::UserCommand::SubmitTask("sleep".into()))
                    .unwrap();
                drop(tx);
            })
        },
    )
    .await;

    assert!(
        !result.snapshots.frames.is_empty(),
        "frame capture should produce at least one frame"
    );
    let has_tool_card = result
        .snapshots
        .frames
        .iter()
        .any(|f| f.contains("bash") || f.contains("sleep"));
    if !has_tool_card {
        eprintln!("frames count: {}", result.snapshots.frames.len());
        if let Some(f) = result.snapshots.frames.first() {
            eprintln!("first frame:\n{f}");
        }
        if let Some(f) = result.snapshots.executing.as_ref() {
            eprintln!("executing snapshot:\n{f}");
        }
        if let Some(f) = result.snapshots.final_render.as_ref() {
            eprintln!("final render:\n{f}");
        }
    }
    assert!(
        has_tool_card,
        "some captured frame should show the write_file tool card"
    );
    assert!(
        result
            .snapshots
            .final_render
            .as_ref()
            .unwrap()
            .contains("Task completed")
            || result
                .snapshots
                .final_render
                .as_ref()
                .unwrap()
                .contains("Done"),
        "final frame should show completion"
    );
}

#[tokio::test]
async fn test_app_can_open_and_inspect_tool_popup() {
    let mock = MockClient::new(vec![
        (
            vec![write_file_tool_use("w1", "out.rs", "fn test() {}")],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, work_dir) =
        harness::run_single_task(mock, "write file", PermissionMode::Auto).await;

    let mut app = TestApp::new_in_dir(work_dir);
    app.feed_all(updates);

    assert_eq!(app.tool_block_count(), 1);
    assert!(app.open_last_tool_popup());
    assert!(app.has_diff_popup());

    let content = app.diff_popup_content().expect("popup should have content");
    assert!(content.contains("fn test()"));

    app.close_diff_popup();
    assert!(!app.has_diff_popup());
}

#[tokio::test]
async fn test_app_can_toggle_help_and_history_overlays() {
    let mut app = TestApp::new();
    assert!(!app.is_help_visible());
    assert!(!app.is_history_visible());

    app.toggle_help();
    assert!(app.is_help_visible());
    let help_text = app.render(120, 30);
    assert!(
        help_text.contains("Help") || help_text.contains("?") || help_text.contains("Ctrl"),
        "help overlay should render shortcuts: {help_text}"
    );

    app.toggle_help();
    app.toggle_history();
    assert!(app.is_history_visible());
    assert!(!app.is_help_visible());
}

#[tokio::test]
async fn headless_session_default_permission_reaches_select_popup() {
    let mock = MockClient::new(vec![
        (
            vec![write_file_tool_use("w1", "out.rs", "fn x() {}")],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let result = run_headless_session_with_options(
        mock,
        PermissionMode::Default,
        Some(0), // allow once
        false,
        |_| {},
        |tx| {
            tokio::spawn(async move {
                tx.send(tact_protocol::UserCommand::SubmitTask("write".into()))
                    .unwrap();
                drop(tx);
            })
        },
    )
    .await;

    assert!(result.is_done);
    let select_text = result
        .snapshots
        .select
        .expect("select popup snapshot should be captured");
    assert!(
        select_text.contains("Allow") || select_text.contains("SELECT"),
        "select popup should show permission options: {select_text}"
    );
}

#[tokio::test]
async fn test_app_feeds_read_file_then_opens_popup() {
    let mock = MockClient::new(vec![
        (
            vec![read_file_tool_use("r1", "data.txt")],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, work_dir) =
        harness::run_single_task_with_setup(mock, "read", PermissionMode::Auto, |dir| {
            write_workspace_file(dir, "data.txt", "hello popup");
        })
        .await;

    let mut app = TestApp::new_in_dir(work_dir);
    app.feed_all(updates);

    assert!(app.open_last_tool_popup());
    let content = app.diff_popup_content().unwrap_or_default();
    assert!(
        content.contains("hello popup"),
        "popup should contain file content: {content}"
    );
}
