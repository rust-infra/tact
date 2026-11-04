use std::sync::{Mutex, OnceLock};

use tact_desktop_core::{DesktopConfig, DesktopController, DesktopEvents, DesktopStartError};

/// The one controller of this process and the events it publishes.
pub struct Runtime {
    controller: DesktopController,
    events: DesktopEvents,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static START: Mutex<()> = Mutex::new(());

/// Loads the desktop configuration and starts the controller before the
/// window opens, so a startup failure is reported instead of an empty window.
pub fn start() -> Result<(), DesktopStartError> {
    let _start = START
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if RUNTIME.get().is_some() {
        return Ok(());
    }
    let data_dir = tact_desktop_core::app_support_dir()
        .map_err(|error| DesktopStartError::Config(error.to_string()))?;
    let mut config = DesktopConfig::load(data_dir)?;
    if config.initial_project.is_none() {
        config.initial_project = std::env::current_dir().ok();
    }
    config.install_process_config()?;
    if !config.appearance_explicit
        && let Some(theme) = tact_desktop_core::configured_theme()
    {
        config.appearance = tact_desktop_core::appearance_from_theme(&theme);
    }
    tact_desktop_core::install_exit_listener()?;
    let (controller, events) = DesktopController::start(config)?;
    let _ = RUNTIME.set(Runtime { controller, events });
    Ok(())
}

/// Stops the controller after the window closes.
pub fn shutdown() {
    if let Some(runtime) = RUNTIME.get()
        && let Err(error) = runtime.controller.shutdown_blocking()
    {
        eprintln!("Tact shutdown failed: {error:?}");
    }
}

fn runtime() -> &'static Runtime {
    RUNTIME.get().expect("Tact Desktop runtime started")
}

/// The running controller, for the TSX bridge.
pub fn controller() -> &'static DesktopController {
    &runtime().controller
}

/// The event stream, for the TSX bridge.
pub fn events() -> &'static DesktopEvents {
    &runtime().events
}
