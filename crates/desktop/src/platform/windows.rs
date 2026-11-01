#[allow(unused_imports)]
pub use super::{PackagingTarget, TARGET};

/// Taskbar overlay badges need the window's `HWND`, which GPUI does not
/// expose to the shell. The in-app gate banner and the OS notification still
/// carry the pending count on Windows; this hook keeps the platform surface
/// explicit instead of silently pretending a badge was drawn.
pub fn set_taskbar_badge(label: Option<String>) {
    let _ = label;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_target_is_explicit() {
        if cfg!(target_os = "windows") {
            assert_eq!(TARGET, Some(PackagingTarget::Windows));
        }
    }
}
