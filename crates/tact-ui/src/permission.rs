use tact::permission::PermissionMode;

pub(crate) fn permission_mode_from_config() -> PermissionMode {
    match tact::config::settings().permission_mode.as_deref() {
        Some("plan") => PermissionMode::Plan,
        Some("default") => PermissionMode::Default,
        _ => PermissionMode::Auto,
    }
}
