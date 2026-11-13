//! Running one child process under a wall-clock budget.
//!
//! Both the CLI runner and the command grader need the same three behaviours,
//! and the bug they exist to prevent is the same: a control action that starts
//! MCP servers, subagents, or background commands must not leave them running
//! after the budget expires. That is why the child leads its own process group
//! and a timeout signals the group rather than the process.
//!
//! Keeping one implementation is the point. Two copies would drift, and the one
//! that drifted would be the copy that stops killing grandchildren.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};

/// How a child ended. Three outcomes, because two cannot say what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildEnd {
    Exited(std::process::ExitStatus),
    /// The budget expired and the child was killed with its process group.
    TimedOut,
}

/// What one child produced, whether or not it finished on its own.
#[derive(Debug, Clone)]
pub struct ChildOutcome {
    pub stdout: String,
    pub stderr: String,
    pub end: ChildEnd,
}

impl ChildOutcome {
    pub fn succeeded(&self) -> bool {
        matches!(self.end, ChildEnd::Exited(status) if status.success())
    }

    pub fn timed_out(&self) -> bool {
        self.end == ChildEnd::TimedOut
    }
}

/// Spawn `command`, read both pipes, and wait at most `timeout`.
///
/// A timed-out child is killed with its process group and still reported, so
/// the caller can use whatever it printed before it hung.
pub async fn run_child(mut command: Command, timeout: Duration) -> std::io::Result<ChildOutcome> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A new process group, so the timeout can reach everything the child
    // spawned. Without it, `kill` reaches only the direct child and the
    // grandchildren keep writing into the directory the caller is about to
    // grade.
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // The readers are separate tasks so a timeout can still collect what the
    // child printed. Awaiting them inline would block on a pipe a grandchild
    // may be holding open.
    let stdout_reader = tokio::spawn(read_to_string(stdout));
    let stderr_reader = tokio::spawn(read_to_string(stderr));

    // A failed `wait` is an OS error, not a timeout, and it is returned as one:
    // reporting it as a budget overrun would blame the case for the harness.
    let end = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => ChildEnd::Exited(status),
        Ok(Err(error)) => {
            let _ = child.start_kill();
            return Err(error);
        }
        Err(_) => {
            kill_tree(&mut child).await;
            ChildEnd::TimedOut
        }
    };

    Ok(ChildOutcome {
        stdout: stdout_reader.await.unwrap_or_default(),
        stderr: stderr_reader.await.unwrap_or_default(),
        end,
    })
}

/// Kill the child and everything it spawned, then reap it.
async fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // A negative pid addresses the process group. Failure is ignored: the
        // group may already be gone, and the direct kill below is what
        // guarantees the reaper gets an exit status.
        unsafe {
            libc::kill(-i32::try_from(pid).unwrap_or(i32::MAX), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Read a pipe line by line so a multi-byte character split across a chunk
/// boundary cannot fail the whole read.
async fn read_to_string(reader: impl AsyncRead + Unpin) -> String {
    let mut buf = String::new();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    while let Ok(count) = reader.read_line(&mut line).await {
        if count == 0 {
            break;
        }
        buf.push_str(&line);
        line.clear();
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    #[tokio::test]
    async fn a_quick_child_reports_its_output_and_success() {
        let outcome = run_child(shell("echo out; echo err >&2"), Duration::from_secs(10))
            .await
            .unwrap();
        assert!(outcome.succeeded());
        assert_eq!(outcome.stdout.trim(), "out");
        assert_eq!(outcome.stderr.trim(), "err");
        assert!(!outcome.timed_out());
    }

    #[tokio::test]
    async fn a_hung_child_times_out_and_keeps_what_it_printed() {
        let outcome = run_child(shell("echo before; sleep 30"), Duration::from_millis(300))
            .await
            .unwrap();
        assert!(outcome.timed_out());
        assert!(!outcome.succeeded());
        assert_eq!(
            outcome.stdout.trim(),
            "before",
            "output from before the hang survives"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_timeout_reaches_a_grandchild_holding_the_pipe() {
        // The grandchild inherits stdout, so without a group kill the read
        // task would block on a pipe that never closes.
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            run_child(shell("sleep 30 & sleep 30"), Duration::from_millis(300)),
        )
        .await
        .expect("run_child must return while a grandchild holds the pipe")
        .unwrap();
        assert!(outcome.timed_out());
    }

    #[tokio::test]
    async fn a_failing_child_is_not_a_success() {
        let outcome = run_child(shell("echo boom >&2; exit 3"), Duration::from_secs(10))
            .await
            .unwrap();
        assert!(!outcome.succeeded());
        assert_eq!(
            match outcome.end {
                ChildEnd::Exited(status) => status.code(),
                ChildEnd::TimedOut => None,
            },
            Some(3)
        );
        assert_eq!(outcome.stderr.trim(), "boom");
    }
}
