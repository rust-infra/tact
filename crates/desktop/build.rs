//! Embeds the Tact icon in the Windows executable.
//!
//! macOS and Linux take their icon from the app bundle / desktop entry, so
//! only the Windows target runs the resource compiler.

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.rc");
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let _ = embed_resource::compile("assets/app-icon.rc", embed_resource::NONE);
    }
}
