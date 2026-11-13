//! The CLI runner's process handling, against a stand-in binary.
//!
//! A real `tact-ui` is not needed to prove the two behaviours that matter: a
//! hung child is reported as a timeout, and the timeout takes its whole process
//! group with it. Both are tested with a shell script so the assertions do not
//! depend on a built binary or a provider.

#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};

use tact_eval::{CliRunner, EvalCase, Isolation, RunFailure, Runner};

fn case_with_binary(dir: &Path, script: &str) -> (EvalCase, PathBuf) {
    let binary = dir.join("fake-tact-ui");
    std::fs::write(&binary, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // Two seconds, not one: the shell has to start and print its first line
    // before the budget expires, and a loaded machine can take most of a
    // second to do that. The scripts hang for thirty, so the margin only
    // removes flakiness; it does not weaken the assertion.
    let case = EvalCase {
        id: format!("process-{}", uuid::Uuid::new_v4()),
        prompt: "p".into(),
        timeout_secs: 2,
        isolation: Isolation::Empty,
        graders: Vec::new(),
    };
    (case, binary)
}

#[cfg(unix)]
#[tokio::test]
async fn a_hung_child_is_a_timeout_not_a_grade() {
    let dir = tempfile::tempdir().unwrap();
    // Prints a line then hangs, like a run wedged inside a tool.
    let (case, binary) = case_with_binary(dir.path(), "#!/bin/sh\necho '[turn] 1' >&2\nsleep 30\n");

    let outcome = CliRunner::new(binary).run(&case, 1).await;

    assert_eq!(
        outcome.artifact.failure,
        Some(RunFailure::Timeout { limit_secs: 2 }),
        "a run over its budget is a timeout"
    );
    assert_eq!(
        outcome.artifact.turns,
        Some(1),
        "events before the timeout survive"
    );
    assert_eq!(outcome.artifact.answer, "", "a hung run produced no answer");
}

#[cfg(unix)]
#[tokio::test]
async fn the_timeout_kills_the_childs_whole_process_group() {
    let dir = tempfile::tempdir().unwrap();
    // The child starts a grandchild that outlives it unless the group is
    // signalled, and records the grandchild's pid so the test can check.
    let (case, binary) = case_with_binary(
        dir.path(),
        "#!/bin/sh\nsleep 300 &\necho \"$!\" > \"$PWD/grandchild.pid\"\nsleep 300\n",
    );

    let outcome = CliRunner::new(binary).run(&case, 1).await;
    assert!(matches!(
        outcome.artifact.failure,
        Some(RunFailure::Timeout { .. })
    ));

    let pid_file = outcome.work_dir.join("grandchild.pid");
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("the child recorded its grandchild")
        .trim()
        .parse()
        .unwrap();

    // Signal 0 only checks liveness. A short grace period covers the kill
    // being delivered and the process being reaped by init.
    let mut alive = true;
    for _ in 0..50 {
        // SAFETY: `kill` with signal 0 performs the permission and existence
        // check and sends nothing.
        alive = unsafe { libc::kill(pid, 0) == 0 };
        if !alive {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!alive, "the grandchild {pid} must not survive the timeout");

    let _ = std::fs::remove_dir_all(&outcome.work_dir);
}

#[cfg(unix)]
#[tokio::test]
async fn a_grandchild_holding_the_pipe_does_not_hang_the_timeout() {
    let dir = tempfile::tempdir().unwrap();
    // The grandchild inherits stderr, so the pipe stays open after the direct
    // child is killed unless the whole group is signalled. Without the group
    // kill, the reader would block on that pipe forever.
    let (case, binary) = case_with_binary(dir.path(), "#!/bin/sh\nsleep 300 &\nsleep 300\n");

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        CliRunner::new(binary).run(&case, 1),
    )
    .await
    .expect("the runner must return even while a grandchild holds the pipe");

    assert!(matches!(
        outcome.artifact.failure,
        Some(RunFailure::Timeout { .. })
    ));
    let _ = std::fs::remove_dir_all(&outcome.work_dir);
}
