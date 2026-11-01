#[allow(unused_imports)]
pub use super::{PackagingTarget, TARGET};

/// Sets the Dock tile's badge label.
///
/// `None` hides the badge. The app may not have a Dock tile in a headless
/// test process, so a missing tile is not an error.
#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
pub fn set_dock_badge(label: Option<String>) {
    use cocoa::appkit::NSApplication;
    use cocoa::base::{NO, YES, id, nil};
    use cocoa::foundation::NSString;
    #[allow(unused_imports)]
    use objc::{sel, sel_impl};

    unsafe {
        let app: id = NSApplication::sharedApplication(nil);
        if app == nil {
            return;
        }
        let dock: id = objc::msg_send![app, dockTile];
        if dock == nil {
            return;
        }
        let text: id = NSString::alloc(nil).init_str(label.as_deref().unwrap_or_default());
        let _: () =
            objc::msg_send![dock, setShowsApplicationBadge: if label.is_some() { YES } else { NO }];
        let _: () = objc::msg_send![dock, setBadgeLabel: text];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_badge(_label: Option<String>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_packaging_target_is_explicit() {
        if cfg!(target_os = "macos") {
            assert_eq!(TARGET, Some(PackagingTarget::MacOs));
        }
        assert!(super::super::supported_platform());
    }
}
