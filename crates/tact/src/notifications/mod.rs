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

/// Desktop notifications, contributed as a plugin.
///
/// The module's free functions were already the whole capability; this is the
/// lifecycle seam that makes the domain a plugin like the others. It registers a
/// small handle so a host can resolve "who sends notifications" instead of calling
/// a module path, which is what lets a replacement be installed.
pub struct NotificationsPlugin {
    sink: std::sync::Arc<dyn NotificationSink>,
}

impl NotificationsPlugin {
    /// A plugin that sends through the platform notifier.
    pub fn platform() -> Self {
        Self {
            sink: std::sync::Arc::new(PlatformNotifier),
        }
    }

    /// A plugin over an explicit sink, for a host that renders notifications
    /// somewhere else.
    pub fn new(sink: std::sync::Arc<dyn NotificationSink>) -> Self {
        Self { sink }
    }
}

/// Where a notification goes.
///
/// A trait rather than the free functions, so a host can replace *how* a
/// notification is delivered without touching the agent that emits it.
pub trait NotificationSink: Send + Sync {
    /// Notify, if `enabled`.
    ///
    /// # Errors
    ///
    /// Returns an error only when the delivery mechanism itself fails.
    fn notify(&self, enabled: bool, title: &str, message: &str) -> Result<()>;
}

/// The platform notifier: `osascript` on macOS, a no-op elsewhere.
pub struct PlatformNotifier;

impl NotificationSink for PlatformNotifier {
    fn notify(&self, enabled: bool, title: &str, message: &str) -> Result<()> {
        notify(enabled, title, message)
    }
}

/// The notifier a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `NotificationsPlugin` was installed. A host without
/// one simply never notifies, which is a valid configuration — but a caller that
/// asked must be told it got nothing.
pub fn notifications(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<std::sync::Arc<dyn NotificationSink>> {
    services
        .get::<NotificationHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| {
            anyhow::anyhow!("no notification sink installed; install NotificationsPlugin")
        })
}

/// The sink a [`NotificationsPlugin`] provided.
#[derive(Clone)]
pub struct NotificationHandle(pub std::sync::Arc<dyn NotificationSink>);

#[async_trait::async_trait]
impl tact_kernel::plugin::Plugin for NotificationsPlugin {
    fn manifest(&self) -> tact_kernel::plugin::PluginManifest {
        tact_kernel::plugin::PluginManifest::new(
            tact_kernel::plugin::PluginId::new("builtin.notifications").expect("a literal id"),
            "Notifications",
            env!("CARGO_PKG_VERSION"),
            tact_kernel::plugin::PluginKind::Capability,
            "Desktop notifications for task completion and step failure",
        )
    }

    fn register(&self, registrar: &mut tact_kernel::plugin::Registrar<'_>) -> Result<()> {
        registrar.service(std::sync::Arc::new(NotificationHandle(self.sink.clone())))
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    /// A sink that records what it was asked to send.
    #[derive(Default)]
    struct Recording {
        calls: std::sync::Mutex<Vec<(bool, String, String)>>,
    }

    impl NotificationSink for Recording {
        fn notify(&self, enabled: bool, title: &str, message: &str) -> Result<()> {
            self.calls.lock().expect("calls lock").push((
                enabled,
                title.to_string(),
                message.to_string(),
            ));
            Ok(())
        }
    }

    /// The plugin registers a sink the host can resolve and use.
    #[tokio::test]
    async fn the_plugin_provides_a_resolvable_sink() {
        let recording = std::sync::Arc::new(Recording::default());
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(NotificationsPlugin::new(recording.clone()))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();

        let sink = notifications(runtime.services()).expect("registered");
        sink.notify(true, "title", "body").unwrap();
        let calls = recording.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "title");
    }

    /// A replacement sink is what the host gets.
    ///
    /// This is the point of the domain being a plugin: a host that renders
    /// notifications in its own UI does not have to keep the platform one.
    #[tokio::test]
    async fn a_replacement_sink_is_what_the_host_gets() {
        let recording = std::sync::Arc::new(Recording::default());
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(NotificationsPlugin::new(recording.clone()))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();

        notifications(runtime.services())
            .unwrap()
            .notify(false, "t", "m")
            .unwrap();
        let calls = recording.calls.lock().unwrap();
        assert!(!calls[0].0, "the disabled flag travels through");
    }

    /// A host with no plugin is told so by name.
    #[test]
    fn a_missing_plugin_is_an_error_that_names_it() {
        let registry = tact_kernel::service::ServiceRegistry::default();
        let error = match notifications(&registry) {
            Ok(_) => panic!("no sink was installed, so this must fail"),
            Err(error) => error,
        };
        assert!(
            format!("{error}").contains("NotificationsPlugin"),
            "{error}"
        );
    }

    /// The manifest says what it contributes.
    #[test]
    fn the_manifest_says_what_it_is() {
        use tact_kernel::plugin::Plugin;
        let manifest = NotificationsPlugin::platform().manifest();
        assert_eq!(manifest.id.as_str(), "builtin.notifications");
        assert_eq!(manifest.kind, tact_kernel::plugin::PluginKind::Capability);
    }
}
