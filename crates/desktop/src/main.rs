#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    if std::env::var_os("TACT_DESKTOP_SMOKE").is_some() {
        if let Err(error) = tact_desktop::smoke_test() {
            eprintln!("Tact Desktop smoke test failed: {error:?}");
            std::process::exit(1);
        }
        if let Err(error) = tact_desktop::run_smoke() {
            eprintln!("Tact Desktop GPUI smoke test failed: {error:?}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = tact_desktop::run() {
        eprintln!("Tact Desktop failed to start: {error:?}");
        std::process::exit(1);
    }
}
