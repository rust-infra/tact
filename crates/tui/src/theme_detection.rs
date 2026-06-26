// Terminal background detection for auto theme switching.
// Detects whether the terminal has a light or dark background and
// returns an appropriate ThemeName.

use crate::theme::ThemeName;

/// Detect terminal background brightness and return a matching ThemeName.
///
/// Detection priority:
/// 1. `TACT_THEME` env var — if set, skip detection entirely
/// 2. macOS: `defaults read -g AppleInterfaceStyle`
/// 3. `COLORFGBG` env var (set by xterm, gnome-terminal, etc.)
/// 4. `COLORTERM` env var
/// 5. Fallback: Retro (neutral dark)
pub(crate) fn detect_theme() -> ThemeName {
    // Priority 1: TACT_THEME env var — explicit user choice
    if let Ok(val) = std::env::var("TACT_THEME") {
        if !val.is_empty() {
            if let Ok(name) = val.parse::<ThemeName>() {
                return name;
            }
        }
    }

    // Priority 2: COLORFGBG env var
    // Format: "0;15" (fg=0 black, bg=15 white) or "15;0" (fg=15 white, bg=0 black)
    // Set by many terminals (xterm, gnome-terminal, etc.)
    if let Ok(val) = std::env::var("COLORFGBG") {
        let parts: Vec<&str> = val.split(';').collect();
        if parts.len() >= 2 {
            if let Ok(bg) = parts[1].parse::<u8>() {
                // Background color 7=light gray, 15=white → light theme
                if bg >= 7 && bg <= 15 {
                    return ThemeName::Light;
                } else {
                    return ThemeName::Dark;
                }
            }
        }
    }

    // Priority 3: macOS system appearance
    #[cfg(target_os = "macos")]
    {
        if let Some(theme) = detect_macos_appearance() {
            return theme;
        }
    }

    // Priority 4: COLORTERM heuristic
    if let Ok(val) = std::env::var("COLORTERM") {
        let val = val.to_ascii_lowercase();
        if val.contains("light") || val.contains("white") {
            return ThemeName::Light;
        }
    }

    // Fallback: Retro (a warm dark theme)
    ThemeName::Retro
}

/// On macOS, read `AppleInterfaceStyle` via `defaults` to detect
/// light/dark mode. Returns `None` if detection fails.
#[cfg(target_os = "macos")]
fn detect_macos_appearance() -> Option<ThemeName> {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.eq_ignore_ascii_case("dark") {
                    Some(ThemeName::Dark)
                } else if s.eq_ignore_ascii_case("light") {
                    Some(ThemeName::Light)
                } else {
                    None
                }
            } else {
                // `AppleInterfaceStyle` not set → system uses Light mode
                Some(ThemeName::Light)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Consolidated env-var-based test to avoid parallel test races on env vars.
    #[test]
    fn test_detect_theme_env_vars() {
        // 1. COLORFGBG dark (bg=0 ≤ 6)
        unsafe { std::env::remove_var("TACT_THEME"); }
        unsafe { std::env::set_var("COLORFGBG", "15;0"); }
        assert_eq!(detect_theme(), ThemeName::Dark);

        // 2. COLORFGBG light (bg=15 ≥ 7)
        unsafe { std::env::set_var("COLORFGBG", "0;15"); }
        assert_eq!(detect_theme(), ThemeName::Light);

        // 3. TACT_THEME overrides everything
        unsafe { std::env::set_var("TACT_THEME", "nord"); }
        assert_eq!(detect_theme(), ThemeName::Nord);

        // 4. COLORTERM light hint (only verifiable on non-macOS or if macOS returns None)
        unsafe { std::env::remove_var("TACT_THEME"); }
        unsafe { std::env::remove_var("COLORFGBG"); }
        unsafe { std::env::set_var("COLORTERM", "light"); }
        let colorterm_result = detect_theme();
        // On macOS, system appearance may override COLORTERM; accept either
        assert!(colorterm_result == ThemeName::Light || cfg!(target_os = "macos"));

        // 5. Fallback with no relevant env vars
        unsafe { std::env::remove_var("TACT_THEME"); }
        unsafe { std::env::remove_var("COLORFGBG"); }
        unsafe { std::env::remove_var("COLORTERM"); }
        // On macOS this may detect system appearance; just ensure it's valid
        let theme = detect_theme();
        match theme {
            ThemeName::Dark | ThemeName::Light | ThemeName::Retro => {}
            other => panic!("unexpected fallback theme: {other:?}"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_detection_no_panic() {
        let result = detect_macos_appearance();
        match result {
            None | Some(ThemeName::Dark) | Some(ThemeName::Light) => {}
            _ => panic!("unexpected macOS detection result"),
        }
    }
}
