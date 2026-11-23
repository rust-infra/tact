//! End-to-end check that the desktop's ACP backend can drive a real agent.
//!
//! The unit tests in `acp_bridge` cover the mapping in isolation. This file
//! covers the part they cannot: that `AcpHost` spawns `tact-acp`, completes the
//! v2 handshake, opens a session, and reports the agent's session id through
//! the same `HostBackend` interface the desktop already drives.
//!
//! Gated behind the `acp-host` feature, and skipped when no agent binary can be
//! found, so a checkout without a debug build still passes.

#![cfg(feature = "acp-host")]

use std::time::Duration;

use tact_desktop_core::DesktopConfig;

/// How long a real agent gets to finish its handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// The `tact-acp` binary, if one can be found.
fn acp_binary() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("TACT_ACP_BIN") {
        return Some(path.into());
    }
    // Integration tests run from `target/debug/deps`; the agent is one level up.
    let executable = std::env::current_exe().ok()?;
    let dir = executable.parent()?.parent()?;
    let name = if cfg!(windows) {
        "tact-acp.exe"
    } else {
        "tact-acp"
    };
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_desktop_backend_opens_a_session_against_the_real_agent() {
    let Some(binary) = acp_binary() else {
        eprintln!("skipping: no tact-acp binary next to the test executable");
        return;
    };
    // `acp_agent_program` reads this variable, so pointing it at the binary we
    // found is what lets the test run from any build directory.
    unsafe { std::env::set_var("TACT_ACP_BIN", &binary) };

    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(project.path().join(".tact")).expect("project dir");

    let config = DesktopConfig::load(home.path().join("app-data"))
        .expect("desktop config")
        .with_initial_project(project.path().to_path_buf());

    let started = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tact_desktop_core::acp_host::session_id_for_test(&config, project.path()),
    )
    .await;

    match started {
        Ok(Ok(session_id)) => assert!(
            !session_id.is_empty(),
            "a started session must report the agent's session id"
        ),
        Ok(Err(error)) => panic!("ACP handshake failed: {error}"),
        Err(_) => panic!("ACP handshake did not finish within {HANDSHAKE_TIMEOUT:?}"),
    }
}
