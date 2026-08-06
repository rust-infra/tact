//! RTK output filter hook for bash tool results.
//!
//! When `rtk` is installed and available on `PATH`, this hook pipes every
//! `bash` tool result through `rtk pipe` to compress the output before it
//! enters LLM context. Output is auto-detected — unrecognised formats pass
//! through unchanged.

use std::process::Command;
use std::sync::OnceLock;

use super::{HookControl, PostToolUseFn};

/// Checked once per process; `false` means the hook is a no-op.
static RTK_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn rtk_on_path() -> bool {
    Command::new("rtk")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Filter `raw` through `rtk pipe` (auto-detect mode).
///
/// Returns `(output, succeeded, elapsed_ms)`:
/// - `output` is the filtered text on success, or the original `raw` on any
///   error (rtk not found, non-zero exit, spawn failure, etc.).
/// - `succeeded` is `true` only when rtk exited 0 and produced non-empty
///   stdout (i.e. filtering actually applied).
/// - `elapsed_ms` is the wall-clock time of the attempt.
///
/// Takes ownership of `raw` so failure paths return it unchanged without
/// copying.
fn pipe_through_rtk(raw: String) -> (String, bool, u64) {
    let filter_available = RTK_AVAILABLE.get_or_init(rtk_on_path);
    if !filter_available {
        return (raw, false, 0);
    }

    let start = std::time::Instant::now();
    let mut child = match Command::new("rtk")
        .arg("pipe")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return (raw, false, start.elapsed().as_millis() as u64),
    };

    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        let write_ok = stdin.write_all(raw.as_bytes()).is_ok();
        drop(stdin); // close the pipe so the child can finish
        if !write_ok {
            let _ = child.wait(); // reap the child before returning
            return (raw, false, start.elapsed().as_millis() as u64);
        }
    }

    match child.wait_with_output() {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            // Fast path: reuse the child's stdout buffer when it is already
            // valid UTF-8; fall back to a lossy copy only on invalid input.
            let filtered = String::from_utf8(out.stdout)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
            (filtered, true, start.elapsed().as_millis() as u64)
        }
        _ => (raw, false, start.elapsed().as_millis() as u64),
    }
}

/// Characters removed by a successful compression; `0` on failure.
///
/// Counted in characters (not bytes) to match the other session stats and
/// the "tokens ≈ chars/4" length heuristic. `raw_chars` is passed in so the
/// caller can count it once before `raw` is moved into `pipe_through_rtk`.
fn saved_chars(raw_chars: u64, filtered: &str, succeeded: bool) -> u64 {
    if !succeeded {
        return 0;
    }
    raw_chars.saturating_sub(filtered.chars().count() as u64)
}

/// Whether a tool result should be piped through `rtk pipe`.
///
/// Only successful (`StepStatus::Success`) bash results are compression
/// candidates. Failed executions (non-zero exit, timeout, cancellation) keep
/// their full output: error text must stay intact for debugging and must not
/// be counted in RTK savings statistics.
fn should_filter(name: &str, status: tact_protocol::StepStatus, content: &str) -> bool {
    name == "bash" && status == tact_protocol::StepStatus::Success && !content.is_empty()
}

/// Creates a `PostToolUse` hook that pipes `bash` tool outputs through
/// `rtk pipe` when RTK is installed. Every attempt is recorded in the
/// session stats (success/failure counts, saved chars, elapsed time).
///
/// Failed tool executions (`StepStatus::Failed`, e.g. non-zero exit) are
/// passed through **unfiltered**: error output must stay byte-exact for the
/// LLM to debug, and is not counted in RTK stats.
pub fn create_rtk_post_tool_hook() -> impl PostToolUseFn + 'static {
    |agent: &crate::LoopState,
     tool_use: &super::ToolUse,
     tool_result: &mut super::ToolResult,
     status: tact_protocol::StepStatus| {
        Box::pin(async move {
            if !should_filter(&tool_use.name, status, &tool_result.content) {
                return Ok(HookControl::Continue);
            }
            // Take ownership of the content instead of cloning: the filtered
            // output replaces it anyway, so failure paths can hand the
            // original back without a copy.
            let raw = std::mem::take(&mut tool_result.content);
            let raw_chars = raw.chars().count() as u64;
            let (filtered, succeeded, elapsed_ms) = pipe_through_rtk(raw);
            let saved = saved_chars(raw_chars, &filtered, succeeded);
            agent
                .runtime
                .stats
                .record_rtk(succeeded, raw_chars, saved, elapsed_ms);
            tool_result.content = filtered;
            Ok(HookControl::Continue)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtk_available_is_checked_exactly_once() {
        // First call caches the result.
        let first = RTK_AVAILABLE.get_or_init(rtk_on_path);
        // Second call returns the cached value — unwrap is safe because
        // OnceLock::get_or_init guarantees an &bool.
        let second = *RTK_AVAILABLE.get().unwrap();
        assert_eq!(*first, second);
    }

    #[test]
    fn filtering_empty_input_returns_empty() {
        // pipe_through_rtk always returns the original when RTK isn't
        // installed — this test verifies empty input is handled and that
        // no filtering is reported as a success.
        let (out, succeeded, _elapsed) = pipe_through_rtk(String::new());
        assert_eq!(out, "");
        assert!(!succeeded, "empty rtk output must not count as a success");
    }

    #[test]
    fn saved_chars_counts_only_successful_compressions() {
        let hello_world_chars = "hello world".chars().count() as u64;
        assert_eq!(saved_chars(hello_world_chars, "hello", true), 6);
        // Saturated: a longer filtered output saves nothing.
        assert_eq!(
            saved_chars("hi".chars().count() as u64, "hello world", true),
            0
        );
        // Failed attempts never count savings.
        assert_eq!(saved_chars(hello_world_chars, "hello", false), 0);
        // Counted in characters, not bytes.
        assert_eq!(
            saved_chars("你好世界".chars().count() as u64, "你好", true),
            2
        );
    }

    #[test]
    fn should_filter_only_successful_bash_output() {
        let success = tact_protocol::StepStatus::Success;
        let failed = tact_protocol::StepStatus::Failed;
        // Success + bash + non-empty → filter.
        assert!(should_filter("bash", success, "some output"));
        // Failed executions never filter, regardless of content.
        assert!(!should_filter("bash", failed, "some output"));
        assert!(!should_filter("bash", failed, ""));
        // Non-bash tools are not candidates (read_file, mcp tools, ...).
        assert!(!should_filter("read_file", success, "some output"));
        // Empty content is not a candidate.
        assert!(!should_filter("bash", success, ""));
    }
}
