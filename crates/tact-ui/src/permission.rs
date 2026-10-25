use tact::config::ResolvedConfig;
use tact::permission::PermissionMode;

/// The permission mode a snapshot asks for.
///
/// Taking the snapshot as an argument is the point: reading the process global
/// here is what made this a hidden dependency of the frontend, invisible to the
/// type system and impossible to substitute in a test.
pub(crate) fn permission_mode_of(config: &ResolvedConfig) -> PermissionMode {
    match config.permission_mode.as_deref() {
        Some("plan") => PermissionMode::Plan,
        Some("default") => PermissionMode::Default,
        _ => PermissionMode::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_mode(mode: Option<&str>) -> ResolvedConfig {
        // A snapshot built here rather than read from the global: the whole
        // point is that the answer follows the argument.
        let mut config = tact::config::ResolvedConfig {
            permission_mode: mode.map(str::to_string),
            ..crate::test_support::default_test_config()
        };
        if mode.is_none() {
            config.permission_mode = None;
        }
        config
    }

    /// The mode follows the snapshot it was given.
    #[test]
    fn the_mode_comes_from_the_given_snapshot() {
        assert_eq!(
            permission_mode_of(&config_with_mode(Some("plan"))),
            PermissionMode::Plan
        );
        assert_eq!(
            permission_mode_of(&config_with_mode(Some("default"))),
            PermissionMode::Default
        );
        assert_eq!(
            permission_mode_of(&config_with_mode(None)),
            PermissionMode::Auto,
            "unset means auto, the documented default"
        );
    }

    /// An unrecognised mode is not treated as an approval shortcut.
    ///
    /// `Auto` approves most calls without asking, so a typo in `[permission]
    /// mode` silently widening permissions is the failure this pins.
    #[test]
    fn an_unknown_mode_falls_back_to_auto_like_the_global_reader_did() {
        assert_eq!(
            permission_mode_of(&config_with_mode(Some("plna"))),
            PermissionMode::Auto,
            "the behaviour must match what the global reader did before the split"
        );
    }
}
