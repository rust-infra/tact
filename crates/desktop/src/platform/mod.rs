#![allow(dead_code)]

pub mod linux;
pub mod macos;
pub mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackagingTarget {
    MacOs,
    Windows,
    Linux,
}

/// The packaging target this binary was built for, or `None` on an OS the
/// release matrix does not build. Never claim a platform the matrix skips.
pub const TARGET: Option<PackagingTarget> = if cfg!(target_os = "macos") {
    Some(PackagingTarget::MacOs)
} else if cfg!(target_os = "windows") {
    Some(PackagingTarget::Windows)
} else if cfg!(target_os = "linux") {
    Some(PackagingTarget::Linux)
} else {
    None
};

pub fn supported_platform() -> bool {
    TARGET.is_some()
}

pub fn app_data_is_clean(path: &std::path::Path) -> bool {
    !path.join("desktop-settings.json").exists() && !path.join("projects.json").exists()
}

/// Read the operating system's reduced-motion preference at launch.
///
/// GPUI's generic signal is not populated by every platform backend yet, so
/// each desktop target supplies a native fallback. `TACT_REDUCED_MOTION`
/// provides a deterministic override for packaged smoke tests and headless CI.
pub fn system_reduced_motion() -> bool {
    if let Some(value) = std::env::var_os("TACT_REDUCED_MOTION")
        && let Some(enabled) = parse_reduced_motion(value.to_string_lossy().as_ref())
    {
        return enabled;
    }
    if cfg!(target_os = "macos") {
        command_enables_reduced_motion(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceMotion"],
            |output| output.trim() == "1",
        )
    } else if cfg!(target_os = "windows") {
        command_enables_reduced_motion(
            "reg",
            &[
                "query",
                r"HKCU\Control Panel\Desktop\WindowMetrics",
                "/v",
                "MinAnimate",
            ],
            |output| {
                output
                    .split_whitespace()
                    .last()
                    .is_some_and(|value| value == "0")
            },
        )
    } else {
        command_enables_reduced_motion(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "enable-animations"],
            |output| output.trim() == "false",
        )
    }
}

pub fn system_reduced_transparency() -> bool {
    if cfg!(target_os = "macos") {
        command_enables_reduced_motion(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceTransparency"],
            |output| output.trim() == "1",
        )
    } else if cfg!(target_os = "windows") {
        command_enables_reduced_motion(
            "reg",
            &[
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "EnableTransparency",
            ],
            |output| {
                output
                    .split_whitespace()
                    .last()
                    .is_some_and(|value| value == "0")
            },
        )
    } else {
        std::env::var_os("TACT_DESKTOP_REDUCED_TRANSPARENCY").is_some()
    }
}

pub fn system_high_contrast() -> bool {
    if cfg!(target_os = "macos") {
        command_enables_reduced_motion(
            "defaults",
            &["read", "com.apple.universalaccess", "increaseContrast"],
            |output| output.trim() == "1",
        )
    } else if cfg!(target_os = "windows") {
        command_enables_reduced_motion(
            "reg",
            &[
                "query",
                r"HKCU\Control Panel\Accessibility\HighContrast",
                "/v",
                "Flags",
            ],
            |output| {
                output
                    .split_whitespace()
                    .last()
                    .is_some_and(|value| value.parse::<u32>().is_ok_and(|flags| flags != 0))
            },
        )
    } else {
        command_enables_reduced_motion(
            "gsettings",
            &["get", "org.gnome.desktop.a11y.interface", "high-contrast"],
            |output| output.trim() == "true",
        )
    }
}

fn command_enables_reduced_motion(
    program: &str,
    args: &[&str],
    parse: impl FnOnce(&str) -> bool,
) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| parse(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_reduced_motion(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "reduce" | "always" => Some(true),
        "0" | "false" | "no" | "off" | "never" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_app_data_has_no_registry_or_settings() {
        let path = std::env::temp_dir().join(format!(
            "tact-desktop-clean-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        assert!(app_data_is_clean(&path));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn reduced_motion_override_parser_accepts_explicit_values() {
        assert_eq!(parse_reduced_motion("true"), Some(true));
        assert_eq!(parse_reduced_motion("0"), Some(false));
        assert_eq!(parse_reduced_motion("unknown"), None);
    }
}
