//! Desktop notifications for task lifecycle events.
//!
//! On macOS this module sends native notifications via `osascript`
//! (AppleScript), which pop up as standard macOS notifications even when
//! the terminal is not in focus.  On other platforms the module is a safe
//! no-op.
//!
//! Notifications can be disabled via config (`agent.notifications_enabled = false`
//! or `--no-notifications`). Notifications are enabled by default.

use anyhow::Result;

/// Sends a desktop notification, if notifications are enabled.
///
/// `enabled` is a parameter rather than a global read: the caller already holds
/// the configuration the run is using (`Agent::config`, or the host's snapshot),
/// and asking the process instead would let a notification fire for a session
/// whose own configuration disabled it.
///
/// On macOS this runs `osascript` to display a native notification.
/// On non-macOS platforms the notification is silently skipped.
///
/// # Errors
///
/// Returns an error only if `osascript` fails to execute on macOS.
/// Other platforms always succeed.
pub fn notify(enabled: bool, title: &str, message: &str) -> Result<()> {
    if !enabled {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"display notification "{}" with title "{}""#,
            message.replace('"', "\\\""),
            title.replace('"', "\\\""),
        );
        std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| anyhow::anyhow!("failed to send macOS notification: {e}"))?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (title, message);
    }

    Ok(())
}

/// Sends a notification that a task has completed successfully.
pub fn notify_task_complete(enabled: bool, summary: &str) -> Result<()> {
    notify(enabled, "Tact — Task Complete", summary)
}

/// Sends a notification that a step has failed.
pub fn notify_step_failed(enabled: bool, step_idx: usize, error: &str) -> Result<()> {
    let msg = format!(
        "Step {step_idx} failed: {}",
        error.chars().take(120).collect::<String>()
    );
    notify(enabled, "Tact — Step Failed", &msg)
}

/// Sends a notification with a generic info message.
pub fn notify_info(enabled: bool, summary: &str) -> Result<()> {
    notify(enabled, "Tact — Info", summary)
}
