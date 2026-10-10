use tact_extensions::permission::PermissionMode;

pub(crate) fn permission_mode_from_config() -> PermissionMode {
    match tact_extensions::config::settings()
        .permission_mode
        .as_deref()
    {
        Some("plan") => PermissionMode::Plan,
        Some("default") => PermissionMode::Default,
        _ => PermissionMode::Auto,
    }
}
