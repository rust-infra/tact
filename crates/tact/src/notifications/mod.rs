//! Desktop notifications for task lifecycle events.
//!
//! On macOS this module sends native notifications via `osascript`
//! (AppleScript), which pop up as standard macOS notifications even when
//! the terminal is not in focus.  On other platforms the module is a safe
//! no-op.
//!
//! Notifications can be disabled via the `TACT_NOTIFICATIONS_ENABLED`
//! environment variable (set to `"false"`) or through the TOML/cli config.
//! Notifications are enabled by default.

use anyhow::Result;

/// Returns `true` if desktop notifications are enabled.
///
/// Checks the `TACT_NOTIFICATIONS_ENABLED` env var.  Defaults to `true`.
pub fn is_enabled() -> bool {
    std::env::var("TACT_NOTIFICATIONS_ENABLED")
        .as_deref()
        .map(|v| v == "true" || v == "1" || v.is_empty())
        .unwrap_or(true)
}

/// Sends a desktop notification.
///
/// On macOS this runs `osascript` to display a native notification.
/// On non-macOS platforms the notification is silently skipped.
/// If notifications are disabled, this is a no-op.
///
/// # Errors
///
/// Returns an error only if `osascript` fails to execute on macOS.
/// Other platforms always succeed.
pub fn notify(title: &str, message: &str) -> Result<()> {
    if !is_enabled() {
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
pub fn notify_task_complete(summary: &str) -> Result<()> {
    notify("Tact — Task Complete", summary)
}

/// Sends a notification that a step has failed.
pub fn notify_step_failed(step_idx: usize, error: &str) -> Result<()> {
    let msg = format!("Step {step_idx} failed: {}", error.chars().take(120).collect::<String>());
    notify("Tact — Step Failed", &msg)
}

/// Sends a notification with a generic info message.
pub fn notify_info(summary: &str) -> Result<()> {
    notify("Tact — Info", summary)
}
