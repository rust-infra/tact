//! Headless interactive session: driver + TUI App update loop (no terminal).

use std::path::{Path, PathBuf};
use std::time::Duration;

use tact::permission::PermissionMode;
use tact_llm::MockClient;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tui::test_support::{HeadlessApp, HeadlessSnapshots};

use crate::driver::run_command_loop;
use crate::test_support::{build_test_agent_with_mode, install_test_config, user_command_channels};
use tact_protocol::UserCommand;

/// Result of a headless driver + App session.
pub struct HeadlessSessionResult {
    pub work_dir: PathBuf,
    pub snapshots: HeadlessSnapshots,
    pub is_done: bool,
}

/// Run `run_command_loop` concurrently with a headless App that drains `AgentUpdate`s
/// in real time (same architecture as `interactive.rs`, without crossterm).
pub async fn run_headless_session<F>(
    mock: MockClient,
    permission_mode: PermissionMode,
    permission_choice: Option<usize>,
    setup: impl FnOnce(&Path),
    drive: F,
) -> HeadlessSessionResult
where
    F: FnOnce(UnboundedSender<UserCommand>) -> JoinHandle<()>,
{
    run_headless_session_with_options(
        mock,
        permission_mode,
        permission_choice,
        false,
        setup,
        drive,
    )
    .await
}

/// Like [`run_headless_session`], but allows enabling per-frame capture.
pub async fn run_headless_session_with_options<F>(
    mock: MockClient,
    permission_mode: PermissionMode,
    permission_choice: Option<usize>,
    capture_frames: bool,
    setup: impl FnOnce(&Path),
    drive: F,
) -> HeadlessSessionResult
where
    F: FnOnce(UnboundedSender<UserCommand>) -> JoinHandle<()>,
{
    install_test_config();
    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent_with_mode(mock, Some(agent_tx), permission_mode);
    setup(&work_dir);

    let (user_cmd_tx, user_cmd_rx) = user_command_channels();
    let mut app = HeadlessApp::new(agent_rx, work_dir.clone()).with_auto_select(permission_choice);
    if capture_frames {
        app = app.with_frame_capture();
    }

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir.clone()));
    let cmd_handle = drive(user_cmd_tx);

    let snapshots = app
        .run_while(|| !driver.is_finished(), Duration::from_secs(30))
        .await;

    let _ = tokio::time::timeout(Duration::from_secs(30), driver)
        .await
        .expect("driver should finish within timeout")
        .expect("driver join");
    let _ = cmd_handle.await;

    app.poll();
    let is_done = app.is_done();
    if snapshots.final_render.is_none() {
        let final_snap = HeadlessSnapshots {
            final_render: Some(app.render(120, 30)),
            ..Default::default()
        };
        return HeadlessSessionResult {
            work_dir,
            snapshots: final_snap,
            is_done,
        };
    }

    HeadlessSessionResult {
        work_dir,
        snapshots,
        is_done,
    }
}
