use tact::permission::PermissionMode;

pub(crate) fn permission_mode_from_config() -> PermissionMode {
    tact::config::settings().permission_mode
}
