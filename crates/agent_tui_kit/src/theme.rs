// Theme module
// Defines all color schemes supported by the TUI. Each theme specifies background,
// foreground, accent, warning, error, and other UI element colors.

use std::str::FromStr;

use ratatui::{style::Color, widgets::BorderType};

/// Built-in theme name enum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    Brutal,
    Ink,
    InkLight,
    Claude,
    ClaudeDark,
}

impl ThemeName {
    /// Every built-in theme, in cycle order.
    ///
    /// Public because the host builds a picker from it (`/theme`) and a cycle
    /// order is not something a caller should have to reconstruct with
    /// `next()` until it wraps.
    pub fn all() -> &'static [ThemeName] {
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
            ThemeName::Brutal,
            ThemeName::Ink,
            ThemeName::InkLight,
            ThemeName::Claude,
            ThemeName::ClaudeDark,
        ]
    }
    /// The canonical name, as `config.toml` spells it (`ink-light`, not
    /// `InkLight`) and as [`ThemeName::from_str`] reads it back.
    ///
    /// Exists so the `/theme` persist step writes a value the next launch can
    /// parse — `theme_name_round_trips_through_as_str` fails if the two ever
    /// drift apart.
    pub fn as_str(&self) -> &'static str {
        match self {
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
            ThemeName::SolarizedDark => "solarized-dark",
            ThemeName::SolarizedLight => "solarized-light",
            ThemeName::GruvboxDark => "gruvbox-dark",
            ThemeName::Nord => "nord",
            ThemeName::Retro => "retro",
            ThemeName::Kawaii => "kawaii",
            ThemeName::Japanese => "japanese",
            ThemeName::Brutal => "brutal",
            ThemeName::Ink => "ink",
            ThemeName::InkLight => "ink-light",
            ThemeName::Claude => "claude",
            ThemeName::ClaudeDark => "claude-dark",
        }
    }

    /// Cycle to the next theme.
    pub fn next(&self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|t| t == self).unwrap();
        all[(idx + 1) % all.len()]
    }
}

impl FromStr for ThemeName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            "solarized-dark" | "solarized_dark" | "solarizeddark" => Ok(Self::SolarizedDark),
            "solarized-light" | "solarized_light" | "solarizedlight" => Ok(Self::SolarizedLight),
            "gruvbox-dark" | "gruvbox_dark" | "gruvboxdark" => Ok(Self::GruvboxDark),
            "nord" => Ok(Self::Nord),
            "retro" => Ok(Self::Retro),
            "kawaii" => Ok(Self::Kawaii),
            "japanese" => Ok(Self::Japanese),
            "brutal" | "neo-brutal" | "neo-brutalism" | "neobrutal" | "neobrutalism" => {
                Ok(Self::Brutal)
            }
            "ink" => Ok(Self::Ink),
            "ink-light" | "ink_light" | "inklight" => Ok(Self::InkLight),
            "claude" => Ok(Self::Claude),
            "claude-dark" | "claude_dark" | "claudedark" => Ok(Self::ClaudeDark),
            other => Err(format!("unknown theme: {other}")),
        }
    }
}

/// Theme color configuration. Each field corresponds to a color used by
/// a different TUI element.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Theme {
    pub name: ThemeName,
    pub bg: Color,            // Background color
    pub fg: Color,            // Foreground (default text)
    pub accent: Color,        // Accent (system messages, prompts)
    pub warning: Color,       // Warning (approval needed, executing)
    pub error: Color,         // Error color
    pub success: Color,       // Success (user input, completion markers)
    pub highlight: Color,     // Highlight (selection background)
    pub border: Color,        // Border color
    pub status_bar_bg: Color, // Status bar background
    pub bottom_bar_bg: Color, // Bottom bar background
    pub bottom_bar_fg: Color, // Bottom bar foreground
    pub input_box_bg: Color,  // Input box background
    pub input_box_fg: Color,  // Input box foreground
    pub heading: Color,       // Heading text color
    pub version: Color,       // Version text color
    pub muted: Color,         // Muted/secondary text color
}

impl From<ThemeName> for Theme {
    fn from(name: ThemeName) -> Self {
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
                heading: Color::Cyan,
                version: Color::Green,
                muted: Color::Rgb(128, 128, 128),
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
                heading: Color::Blue,
                version: Color::Blue,
                muted: Color::Rgb(120, 120, 120),
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
                heading: Color::Rgb(38, 139, 210),
                version: Color::Rgb(133, 153, 0),
                muted: Color::Rgb(128, 128, 128),
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
                heading: Color::Rgb(38, 139, 210),
                version: Color::Rgb(38, 139, 210),
                muted: Color::Rgb(120, 120, 120),
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
                heading: Color::Rgb(214, 93, 14),
                version: Color::Rgb(152, 151, 26),
                muted: Color::Rgb(128, 128, 128),
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
                heading: Color::Rgb(136, 192, 208),
                version: Color::Rgb(163, 190, 140),
                muted: Color::Rgb(128, 128, 128),
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
                heading: Color::Rgb(255, 210, 80),
                version: Color::Rgb(200, 255, 80),
                muted: Color::Rgb(128, 128, 128),
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
                heading: Color::Rgb(255, 105, 180),
                version: Color::Rgb(255, 105, 180),
                muted: Color::Rgb(120, 120, 120),
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
                heading: Color::Rgb(200, 60, 40),
                version: Color::Rgb(100, 160, 70),
                muted: Color::Rgb(128, 128, 128),
            },
            ThemeName::Brutal => Self {
                name,
                bg: Color::Rgb(255, 255, 255),
                fg: Color::Rgb(20, 20, 20),
                accent: Color::Rgb(255, 221, 87), // active tab yellow
                warning: Color::Rgb(255, 190, 60), // running / emphasis
                error: Color::Rgb(255, 70, 70),
                success: Color::Rgb(170, 240, 150),   // tag green
                highlight: Color::Rgb(160, 210, 255), // reply blue
                border: Color::Rgb(0, 0, 0),          // thick black outlines
                status_bar_bg: Color::Rgb(255, 221, 87),
                bottom_bar_bg: Color::Rgb(255, 255, 255),
                bottom_bar_fg: Color::Rgb(20, 20, 20),
                input_box_bg: Color::Rgb(255, 255, 255),
                input_box_fg: Color::Rgb(20, 20, 20),
                heading: Color::Rgb(255, 221, 87),
                version: Color::Rgb(170, 240, 150),
                muted: Color::Rgb(128, 128, 128),
            },
            ThemeName::Ink => Self {
                name,
                bg: Color::Rgb(13, 13, 13),
                fg: Color::Rgb(232, 232, 232),
                accent: Color::Rgb(125, 211, 252),
                heading: Color::Rgb(125, 211, 252),
                version: Color::Rgb(45, 212, 191),
                border: Color::Rgb(58, 58, 58),
                highlight: Color::Rgb(40, 48, 64),
                muted: Color::Rgb(128, 128, 128),
                status_bar_bg: Color::Rgb(18, 18, 18),
                bottom_bar_bg: Color::Rgb(20, 20, 20),
                bottom_bar_fg: Color::Rgb(160, 160, 168),
                input_box_bg: Color::Rgb(13, 13, 13),
                input_box_fg: Color::Rgb(232, 232, 232),
                warning: Color::Rgb(255, 200, 50),
                error: Color::Rgb(255, 70, 70),
                success: Color::Rgb(80, 200, 120),
            },
            ThemeName::InkLight => Self {
                name,
                bg: Color::Rgb(248, 248, 248),
                fg: Color::Rgb(30, 30, 30),
                accent: Color::Rgb(30, 100, 200),
                heading: Color::Rgb(20, 80, 180),
                version: Color::Rgb(20, 150, 140),
                border: Color::Rgb(200, 200, 200),
                highlight: Color::Rgb(220, 230, 250),
                muted: Color::Rgb(120, 120, 120),
                status_bar_bg: Color::Rgb(240, 240, 240),
                bottom_bar_bg: Color::Rgb(245, 245, 245),
                bottom_bar_fg: Color::Rgb(80, 80, 80),
                input_box_bg: Color::Rgb(248, 248, 248),
                input_box_fg: Color::Rgb(30, 30, 30),
                warning: Color::Rgb(200, 150, 30),
                error: Color::Rgb(200, 50, 50),
                success: Color::Rgb(60, 160, 90),
            },
            ThemeName::Claude => Self {
                name,
                bg: Color::Rgb(250, 249, 245),
                fg: Color::Rgb(31, 30, 29),
                accent: Color::Rgb(184, 85, 53),
                warning: Color::Rgb(190, 120, 20),
                error: Color::Rgb(191, 77, 67),
                success: Color::Rgb(61, 122, 85),
                highlight: Color::Rgb(239, 223, 214),
                border: Color::Rgb(217, 214, 204),
                status_bar_bg: Color::Rgb(240, 238, 230),
                bottom_bar_bg: Color::Rgb(250, 249, 245),
                bottom_bar_fg: Color::Rgb(110, 106, 96),
                input_box_bg: Color::Rgb(255, 255, 255),
                input_box_fg: Color::Rgb(31, 30, 29),
                heading: Color::Rgb(184, 85, 53),
                version: Color::Rgb(61, 122, 85),
                muted: Color::Rgb(110, 106, 96),
            },
            ThemeName::ClaudeDark => Self {
                name,
                bg: Color::Rgb(38, 38, 36),
                fg: Color::Rgb(240, 238, 230),
                accent: Color::Rgb(217, 119, 87),
                warning: Color::Rgb(224, 164, 88),
                error: Color::Rgb(229, 115, 104),
                success: Color::Rgb(127, 175, 139),
                highlight: Color::Rgb(88, 61, 50),
                border: Color::Rgb(68, 67, 64),
                status_bar_bg: Color::Rgb(31, 30, 29),
                bottom_bar_bg: Color::Rgb(38, 38, 36),
                bottom_bar_fg: Color::Rgb(183, 181, 172),
                input_box_bg: Color::Rgb(48, 48, 46),
                input_box_fg: Color::Rgb(240, 238, 230),
                heading: Color::Rgb(217, 119, 87),
                version: Color::Rgb(127, 175, 139),
                muted: Color::Rgb(183, 181, 172),
            },
        }
    }
}

impl Theme {
    /// Panel/card border style. Neo-Brutal uses square corners instead of rounded.
    pub fn block_border_type(&self) -> BorderType {
        match self.name {
            ThemeName::Brutal | ThemeName::Ink | ThemeName::InkLight => BorderType::Plain,
            _ => BorderType::Rounded,
        }
    }

    /// Whether the theme uses a light background (drives code block / card colors).
    pub fn is_light(&self) -> bool {
        match self.bg {
            Color::Rgb(r, g, b) => u16::from(r) + u16::from(g) + u16::from(b) > 600,
            Color::White => true,
            Color::Black => false,
            _ => matches!(
                self.name,
                ThemeName::Light
                    | ThemeName::SolarizedLight
                    | ThemeName::Kawaii
                    | ThemeName::Brutal
                    | ThemeName::InkLight
            ),
        }
    }

    pub fn code_block_bg(&self) -> Color {
        match self.name {
            ThemeName::Claude => Color::Rgb(255, 255, 255),
            ThemeName::ClaudeDark => Color::Rgb(48, 48, 46),
            _ if self.is_light() => Color::Rgb(248, 248, 248),
            _ => Color::Rgb(30, 35, 50),
        }
    }

    pub fn code_block_fg(&self) -> Color {
        if self.is_light() || matches!(self.name, ThemeName::ClaudeDark) {
            self.fg
        } else {
            Color::Rgb(200, 200, 210)
        }
    }

    pub fn code_card_bg(&self) -> Color {
        self.code_block_bg()
    }

    pub fn code_card_border(&self) -> Color {
        if self.is_light() || matches!(self.name, ThemeName::ClaudeDark) {
            self.border
        } else {
            Color::Rgb(100, 120, 180)
        }
    }

    pub fn code_card_title_fg(&self) -> Color {
        match self.name {
            ThemeName::Claude | ThemeName::ClaudeDark => self.accent,
            _ if self.is_light() => self.fg,
            _ => Color::Rgb(160, 180, 240),
        }
    }

    pub fn thinking_card_border(&self) -> Color {
        match self.name {
            ThemeName::Claude => self.border,
            ThemeName::ClaudeDark => Color::Rgb(90, 80, 73),
            _ if self.is_light() => self.border,
            _ => Color::Rgb(140, 140, 220),
        }
    }

    pub fn thinking_preview_fg(&self) -> Color {
        match self.name {
            ThemeName::Claude | ThemeName::ClaudeDark => self.muted,
            _ if self.is_light() => self.fg,
            _ => Color::Rgb(180, 180, 200),
        }
    }

    pub fn muted_fg(&self) -> Color {
        self.muted
    }

    /// Goal-surface accent: the color of a Goal's own log notices.
    ///
    /// No built-in theme spends this hue on another slot (violet, against the
    /// cyan/blue accents, amber/green status colors and red errors), so a Goal
    /// line reads as a Goal line before its text is parsed — which is the whole
    /// point of giving the surface its own slot instead of reusing `accent`.
    pub fn goal_fg(&self) -> Color {
        if self.is_light() {
            Color::Rgb(130, 30, 160)
        } else {
            Color::Rgb(198, 120, 255)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ink_and_ink_light() {
        assert_eq!("ink".parse::<ThemeName>().unwrap(), ThemeName::Ink);
        assert_eq!(
            "ink-light".parse::<ThemeName>().unwrap(),
            ThemeName::InkLight
        );
        assert_eq!(
            "ink_light".parse::<ThemeName>().unwrap(),
            ThemeName::InkLight
        );
        assert_eq!(
            "inklight".parse::<ThemeName>().unwrap(),
            ThemeName::InkLight
        );
    }

    #[test]
    fn parse_claude_and_claude_dark() {
        assert_eq!("claude".parse::<ThemeName>().unwrap(), ThemeName::Claude);
        assert_eq!(
            "claude-dark".parse::<ThemeName>().unwrap(),
            ThemeName::ClaudeDark
        );
        assert_eq!(
            "claude_dark".parse::<ThemeName>().unwrap(),
            ThemeName::ClaudeDark
        );
        assert_eq!(
            "ClaudeDark".parse::<ThemeName>().unwrap(),
            ThemeName::ClaudeDark
        );
    }

    #[test]
    fn claude_palettes_match_the_desktop_family() {
        let light = Theme::from(ThemeName::Claude);
        assert!(light.is_light());
        assert_eq!(light.bg, Color::Rgb(250, 249, 245));
        assert_eq!(light.accent, Color::Rgb(184, 85, 53));
        assert_eq!(light.code_block_bg(), Color::Rgb(255, 255, 255));

        let dark = Theme::from(ThemeName::ClaudeDark);
        assert!(!dark.is_light());
        assert_eq!(dark.bg, Color::Rgb(38, 38, 36));
        assert_eq!(dark.accent, Color::Rgb(217, 119, 87));
        assert_eq!(dark.code_block_bg(), Color::Rgb(48, 48, 46));
        assert_eq!(dark.thinking_card_border(), Color::Rgb(90, 80, 73));
    }

    #[test]
    fn ink_border_type_is_plain() {
        assert_eq!(
            Theme::from(ThemeName::Ink).block_border_type(),
            BorderType::Plain
        );
        assert_eq!(
            Theme::from(ThemeName::InkLight).block_border_type(),
            BorderType::Plain
        );
    }

    /// `as_str` is written to `config.toml`; `from_str` reads it back. A
    /// spelling either side does not know would silently reset the theme on
    /// the next launch.
    #[test]
    fn theme_name_round_trips_through_as_str() {
        for name in ThemeName::all() {
            assert_eq!(
                name.as_str().parse::<ThemeName>().ok(),
                Some(*name),
                "{name:?} does not survive as_str/from_str"
            );
        }
        // The list itself must not contain duplicates.
        let mut names: Vec<&str> = ThemeName::all().iter().map(ThemeName::as_str).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "two themes share a name");
    }

    #[test]
    fn muted_fg_returns_muted_field() {
        let theme = Theme::from(ThemeName::Ink);
        assert_eq!(theme.muted_fg(), Color::Rgb(128, 128, 128));
        let theme = Theme::from(ThemeName::Dark);
        assert_eq!(theme.muted_fg(), Color::Rgb(128, 128, 128));
    }
}
