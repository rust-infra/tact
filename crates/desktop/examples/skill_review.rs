//! Renders the skill launcher and manager interactions off-screen.
//!
//! Usage:
//!
//! ```text
//! cargo run -p tact-desktop --example skill_review --features ui-capture -- [out-dir]
//! ```

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::path::PathBuf;

    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/skill-review"));
    for path in tact_desktop::capture::capture_skill_review(&out)? {
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "skill_review uses GPUI's macOS-only VisualTestAppContext; \
         verify Linux and Windows through the packaged live window instead"
    );
    std::process::exit(2);
}
