//! Runs Tact Desktop on the machine's native backend.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> std::process::ExitCode {
    env_logger::init();
    if let Err(error) = tact_desktop::app::start() {
        eprintln!("Tact Desktop failed to start: {error:?}");
        return std::process::ExitCode::FAILURE;
    }
    let theme = nsh_base::Theme::from_json(include_str!("../theme.json"))
        .expect("the Tact Desktop theme is valid");
    let registry = tact_desktop::registry();
    let assets = asset_directory(&registry).expect("copy the Tact Desktop assets");
    platform::set_ui_sets(vec![nsh_base::registry(), registry]);
    let _ = platform::run_app(
        platform::AppSource::Rust(
            platform::Root::of::<tact_desktop::App>()
                .tokens(theme.into_tokens())
                .app_id("dev.tact.desktop"),
        ),
        platform::AppOptions {
            title: "Tact Desktop".to_owned(),
            asset_base: Some(platform::AssetBase::Directory(assets)),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app_id: "dev.tact.desktop".to_owned(),
            size: (1280, 840),
            ..Default::default()
        },
    );
    tact_desktop::app::shutdown();
    std::process::ExitCode::SUCCESS
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("Tact Desktop runs on macOS, Windows and Linux.");

/// Copies the crate's registered assets next to the executable, where
/// `asset:/` resolves them in a `cargo run` build.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn asset_directory(
    registry: &nsh_core::element::Registry,
) -> std::io::Result<std::path::PathBuf> {
    let executable = std::env::current_exe()?;
    let base = executable
        .parent()
        .expect("the executable has a directory")
        .join("tact-desktop-assets");
    for file in registry.asset_files() {
        let destination = base.join(file.path());
        if destination.is_file() {
            continue;
        }
        std::fs::create_dir_all(destination.parent().expect("an asset has a parent directory"))?;
        std::fs::copy(file.source, destination)?;
    }
    Ok(base)
}
