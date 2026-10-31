//! Renders the real Tact Desktop shell off-screen and writes PNGs.
//!
//! Usage:
//!
//! ```text
//! cargo run -p tact-desktop --example ui_capture --features ui-capture -- [out-dir]
//! ```
//!
//! The harness uses GPUI's real renderer, so the files match what the
//! compositor would show; no Screen Recording permission is required.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/ui-capture"));
    std::fs::create_dir_all(&out)?;

    match tact_desktop::capture::capture_all(&out) {
        Ok(paths) => {
            for path in paths {
                println!("{}", path.display());
            }
            Ok(())
        }
        Err(error) => {
            eprintln!("ui capture failed: {error:#}");
            // Skip destructors so the visual-test leak detector cannot mask
            // the capture failure with its own panic.
            std::process::exit(1);
        }
    }
}
