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

use tact_desktop_core::Appearance;

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/ui-capture"));
    std::fs::create_dir_all(&out)?;

    let mut harness = tact_desktop::capture::Harness::open(Appearance::Light, 1.0)?;
    let path = harness.capture(&out, "shell-light")?;
    println!("{}", path.display());
    Ok(())
}
