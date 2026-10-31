//! Native Tact Desktop shell.

mod app;
#[cfg(feature = "ui-capture")]
pub mod capture;
pub mod i18n;
mod keymap;
mod platform;
mod root;
mod theme;
mod ui;

pub use app::{run, run_smoke};

pub fn shell_name() -> &'static str {
    "Tact Desktop"
}

pub fn smoke_test() -> Result<(), tact_desktop_core::DesktopStartError> {
    let data_dir = tact_desktop_core::app_support_dir()
        .map_err(|error| tact_desktop_core::DesktopStartError::Config(error.to_string()))?;
    let _ = tact_desktop_core::DesktopConfig::load(data_dir)?;
    let sidecar = tact_desktop_core::tact_ui_binary_path();
    if sidecar.is_file() {
        let status = std::process::Command::new(&sidecar)
            .arg("--version")
            .status()
            .map_err(|error| tact_desktop_core::DesktopStartError::Config(error.to_string()))?;
        if !status.success() {
            return Err(tact_desktop_core::DesktopStartError::Config(format!(
                "bundled tact-ui sidecar failed its version check: {status}"
            )));
        }
    }
    Ok(())
}
