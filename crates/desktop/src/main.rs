//! Runs the TSX shell on the machine's native backend from a built bundle.
//!
//! `nsh run` is the development path; this binary runs the same bundle a
//! package ships, so `cargo run -p tact-desktop` works after `nsh build`.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::path::PathBuf;

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> std::process::ExitCode {
    env_logger::init();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Tact Desktop failed to start: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = bundle_path()?;
    let assets = path
        .parent()
        .map(|dir| dir.join("assets"))
        .filter(|dir| dir.is_dir());
    let bundle = nsh_engine::Bundle::read(&path)?;
    tact_desktop::app::start().map_err(|error| format!("{error:?}"))?;
    let registry = tact_desktop::registry();
    platform::set_ui_sets(vec![nsh_base::registry(), registry]);
    let _ = platform::run_app(
        platform::AppSource::Bundle(bundle),
        platform::AppOptions {
            title: "Tact Desktop".to_owned(),
            app_id: "dev.tact.desktop".to_owned(),
            asset_base: assets.map(platform::AssetBase::Directory),
            limits: platform::Limits::standalone(),
            size: (1280, 840),
            ..Default::default()
        },
    );
    tact_desktop::app::shutdown();
    Ok(())
}

/// The bundle `nsh build` wrote: an override, then next to the executable,
/// then the app's build output.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn bundle_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("TACT_DESKTOP_BUNDLE") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(dir) = executable.parent()
    {
        candidates.push(dir.join("bundle.js"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".nsh")
            .join("out")
            .join("bundle.js"),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "no bundle.js: run `nsh build` first".into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("Tact Desktop runs on macOS, Windows and Linux.");
