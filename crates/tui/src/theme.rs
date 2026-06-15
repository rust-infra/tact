// Theme module
// Defines all color schemes supported by the TUI. Each theme specifies background,
// foreground, accent, warning, error, and other UI element colors.

use ratatui::style::Color;

/// Built-in theme name enum.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ThemeName {
    Dark,
    Light,
    SolarizedDark,
    SolarizedLight,
    GruvboxDark,
    Nord,
    Retro,
    Kawaii,
    Japanese,
}

impl ThemeName {
    fn all() -> &'static [ThemeName] {
        &[
            ThemeName::Dark,
            ThemeName::Light,
            ThemeName::SolarizedDark,
            ThemeName::SolarizedLight,
            ThemeName::GruvboxDark,
            ThemeName::Nord,
            ThemeName::Retro,
            ThemeName::Kawaii,
            ThemeName::Japanese,
        ]
    }
    /// Cycle to the next theme.
    pub(super) fn next(&self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|t| t == self).unwrap();
        all[(idx + 1) % all.len()]
    }
}

/// Theme color configuration. Each field corresponds to a color used by
/// a different TUI element.
pub struct Theme {
    pub name: ThemeName,
    pub bg: Color,            // Background color
    pub fg: Color,            // Foreground (default text)
    pub accent: Color,        // Accent (system messages, prompts)
    pub warning: Color,       // Warning (approval needed, executing)
    pub error: Color,         // Error color
    pub success: Color,       // Success (user input, completion markers)
    pub highlight: Color,     // Highlight (selection, search match background)
    pub border: Color,        // Border color
    pub status_bar_bg: Color, // Status bar background
    pub bottom_bar_bg: Color, // Bottom bar background
    pub bottom_bar_fg: Color, // Bottom bar foreground
    pub input_box_bg: Color,  // Input box background
    pub input_box_fg: Color,  // Input box foreground
}

impl Theme {
    /// Returns the color configuration for the given theme name.
    pub(super) fn by_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dark => Self {
                name,
                bg: Color::Black,
                fg: Color::White,
                accent: Color::Cyan,
                warning: Color::Yellow,
                error: Color::Red,
                success: Color::Green,
                highlight: Color::Rgb(70, 90, 140),
                border: Color::Gray,
                status_bar_bg: Color::DarkGray,
                bottom_bar_bg: Color::Rgb(30, 30, 40),
                bottom_bar_fg: Color::Rgb(180, 180, 190),
                input_box_bg: Color::Black,
                input_box_fg: Color::Green,
            },
            ThemeName::Light => Self {
                name,
                bg: Color::White,
                fg: Color::Black,
                accent: Color::Blue,
                warning: Color::Yellow,
                error: Color::Red,
                success: Color::Green,
                highlight: Color::Cyan,
                border: Color::Gray,
                status_bar_bg: Color::Gray,
                bottom_bar_bg: Color::Rgb(235, 235, 240),
                bottom_bar_fg: Color::Rgb(80, 80, 80),
                input_box_bg: Color::White,
                input_box_fg: Color::Green,
            },
            ThemeName::SolarizedDark => Self {
                name,
                bg: Color::Rgb(0, 43, 54),
                fg: Color::Rgb(131, 148, 150),
                accent: Color::Rgb(38, 139, 210),
                warning: Color::Rgb(203, 75, 22),
                error: Color::Rgb(220, 50, 47),
                success: Color::Rgb(133, 153, 0),
                highlight: Color::Rgb(42, 161, 152),
                border: Color::Rgb(7, 54, 66),
                status_bar_bg: Color::Rgb(7, 54, 66),
                bottom_bar_bg: Color::Rgb(0, 43, 54),
                bottom_bar_fg: Color::Rgb(131, 148, 150),
                input_box_bg: Color::Rgb(0, 43, 54),
                input_box_fg: Color::Rgb(133, 153, 0),
            },
            ThemeName::SolarizedLight => Self {
                name,
                bg: Color::Rgb(253, 246, 227),
                fg: Color::Rgb(101, 123, 131),
                accent: Color::Rgb(38, 139, 210),
                warning: Color::Rgb(203, 75, 22),
                error: Color::Rgb(220, 50, 47),
                success: Color::Rgb(133, 153, 0),
                highlight: Color::Rgb(42, 161, 152),
                border: Color::Rgb(147, 161, 161),
                status_bar_bg: Color::Rgb(147, 161, 161),
                bottom_bar_bg: Color::Rgb(238, 232, 213),
                bottom_bar_fg: Color::Rgb(101, 123, 131),
                input_box_bg: Color::Rgb(253, 246, 227),
                input_box_fg: Color::Rgb(133, 153, 0),
            },
            ThemeName::GruvboxDark => Self {
                name,
                bg: Color::Rgb(40, 40, 40),
                fg: Color::Rgb(235, 219, 178),
                accent: Color::Rgb(214, 93, 14),
                warning: Color::Rgb(184, 187, 38),
                error: Color::Rgb(251, 73, 52),
                success: Color::Rgb(152, 151, 26),
                highlight: Color::Rgb(177, 98, 134),
                border: Color::Rgb(80, 80, 80),
                status_bar_bg: Color::Rgb(102, 92, 84),
                bottom_bar_bg: Color::Rgb(40, 40, 40),
                bottom_bar_fg: Color::Rgb(235, 219, 178),
                input_box_bg: Color::Rgb(40, 40, 40),
                input_box_fg: Color::Rgb(152, 151, 26),
            },
            ThemeName::Nord => Self {
                name,
                bg: Color::Rgb(46, 52, 64),
                fg: Color::Rgb(216, 222, 233),
                accent: Color::Rgb(136, 192, 208),
                warning: Color::Rgb(235, 203, 139),
                error: Color::Rgb(191, 97, 106),
                success: Color::Rgb(163, 190, 140),
                highlight: Color::Rgb(129, 161, 193),
                border: Color::Rgb(76, 86, 106),
                status_bar_bg: Color::Rgb(76, 86, 106),
                bottom_bar_bg: Color::Rgb(46, 52, 64),
                bottom_bar_fg: Color::Rgb(216, 222, 233),
                input_box_bg: Color::Rgb(46, 52, 64),
                input_box_fg: Color::Rgb(163, 190, 140),
            },
            ThemeName::Retro => Self {
                name,
                bg: Color::Rgb(15, 12, 6),
                fg: Color::Rgb(255, 180, 50),
                accent: Color::Rgb(255, 210, 80),
                warning: Color::Rgb(255, 140, 30),
                error: Color::Rgb(255, 60, 40),
                success: Color::Rgb(200, 255, 80),
                highlight: Color::Rgb(80, 60, 20),
                border: Color::Rgb(100, 70, 30),
                status_bar_bg: Color::Rgb(40, 28, 12),
                bottom_bar_bg: Color::Rgb(15, 12, 6),
                bottom_bar_fg: Color::Rgb(255, 180, 50),
                input_box_bg: Color::Rgb(15, 12, 6),
                input_box_fg: Color::Rgb(255, 210, 80),
            },
            ThemeName::Kawaii => Self {
                name,
                bg: Color::Rgb(255, 245, 250),
                fg: Color::Rgb(100, 60, 80),
                accent: Color::Rgb(255, 105, 180),
                warning: Color::Rgb(255, 140, 120),
                error: Color::Rgb(220, 50, 80),
                success: Color::Rgb(120, 200, 150),
                highlight: Color::Rgb(255, 200, 220),
                border: Color::Rgb(230, 180, 200),
                status_bar_bg: Color::Rgb(255, 230, 240),
                bottom_bar_bg: Color::Rgb(255, 240, 248),
                bottom_bar_fg: Color::Rgb(140, 100, 120),
                input_box_bg: Color::Rgb(255, 245, 250),
                input_box_fg: Color::Rgb(255, 105, 180),
            },
            ThemeName::Japanese => Self {
                name,
                bg: Color::Rgb(25, 30, 45),
                fg: Color::Rgb(230, 220, 200),
                accent: Color::Rgb(200, 60, 40),
                warning: Color::Rgb(210, 160, 50),
                error: Color::Rgb(180, 30, 30),
                success: Color::Rgb(100, 160, 70),
                highlight: Color::Rgb(60, 80, 120),
                border: Color::Rgb(50, 45, 40),
                status_bar_bg: Color::Rgb(35, 40, 55),
                bottom_bar_bg: Color::Rgb(25, 30, 45),
                bottom_bar_fg: Color::Rgb(230, 220, 200),
                input_box_bg: Color::Rgb(25, 30, 45),
                input_box_fg: Color::Rgb(200, 60, 40),
            },
        }
    }
}
