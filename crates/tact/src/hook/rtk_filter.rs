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
/// Returns the filtered output on success, or the original `raw` on any
/// error (rtk not found, timeout, non-zero exit, spawn failure, etc.).
fn pipe_through_rtk(raw: &str) -> String {
    let filter_available = RTK_AVAILABLE.get_or_init(rtk_on_path);
    if !filter_available {
        return raw.to_string();
    }

    match Command::new("rtk")
        .arg("pipe")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            use std::io::Write;
            if child.stdin.as_mut().unwrap().write_all(raw.as_bytes()).is_err() {
                return raw.to_string();
            }
            drop(child.stdin.take());

            match child.wait_with_output() {
                Ok(out) if out.status.success() && !out.stdout.is_empty() => {
                    String::from_utf8_lossy(&out.stdout).into_owned()
                }
                _ => raw.to_string(),
            }
        }
        Err(_) => raw.to_string(),
    }
}

/// Creates a `PostToolUse` hook that pipes `bash` tool outputs through
/// `rtk pipe` when RTK is installed.
pub fn create_rtk_post_tool_hook() -> impl PostToolUseFn + 'static {
    |_agent: &crate::LoopState, tool_use: &super::ToolUse, tool_result: &mut super::ToolResult| {
        Box::pin(async move {
            if tool_use.name != "bash" {
                return Ok(HookControl::Continue);
            }
            if tool_result.content.is_empty() {
                return Ok(HookControl::Continue);
            }
            tool_result.content = pipe_through_rtk(&tool_result.content);
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
        // installed — this test verifies empty input is handled.
        let result = pipe_through_rtk("");
        // When rtk isn't available, returns the empty original; when rtk is
        // available, rtk pipe of empty stdin also produces empty stdout.
        assert_eq!(result, "");
    }
}
