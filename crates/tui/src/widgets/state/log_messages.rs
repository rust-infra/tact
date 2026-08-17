use ratatui::style::Color;

use crate::{
    render::util::{LOG_THINKING_INDENT, LOG_TOOL_INDENT},
    theme::Theme,
    widgets::state::{LogItemKind, SystemMsgStyle},
};

impl SystemMsgStyle {
    /// Detect an explicit system marker after optional leading whitespace.
    ///
    /// This is only used to choose the visual color for a message that is
    /// already known to come from a system-message insertion path. It never
    /// decides whether arbitrary text is a system item.
    pub(crate) fn from_marker(s: &str) -> Option<Self> {
        const PREFIXES: &[(&str, SystemMsgStyle)] = &[
            ("✓", SystemMsgStyle::Success),
            ("✔", SystemMsgStyle::Success),
            ("✅", SystemMsgStyle::Success),
            ("✗", SystemMsgStyle::Error),
            ("❌", SystemMsgStyle::Error),
            ("⚠", SystemMsgStyle::Warning),
            ("📝", SystemMsgStyle::Accent),
            ("▶", SystemMsgStyle::Accent),
            ("🤖", SystemMsgStyle::Accent),
            ("📋", SystemMsgStyle::Accent),
            ("🎨", SystemMsgStyle::Accent),
        ];
        let trimmed = s.trim_start();
        PREFIXES
            .iter()
            .find(|(prefix, _)| trimmed.starts_with(prefix))
            .map(|(_, style)| *style)
    }

    pub(crate) fn color(self, theme: &Theme) -> Color {
        match self {
            Self::Default => theme.fg,
            Self::Success => theme.success,
            Self::Error => theme.error,
            Self::Warning => theme.warning,
            Self::Accent => theme.accent,
        }
    }
}

impl LogItemKind {
    pub(crate) fn log_indent(self) -> u16 {
        match self {
            Self::User => 0,
            Self::AssistantMarkdown | Self::SystemPlain(_) | Self::SystemMarkdown => {
                LOG_THINKING_INDENT + 1
            }
            Self::SystemTool => LOG_TOOL_INDENT,
            Self::Thinking => LOG_THINKING_INDENT,
        }
    }

    pub(crate) fn is_user(self) -> bool {
        matches!(self, Self::User)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, ThemeName};

    #[test]
    fn from_marker_maps_explicit_prefixes() {
        assert_eq!(
            SystemMsgStyle::from_marker("✓ done"),
            Some(SystemMsgStyle::Success)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("✔ done"),
            Some(SystemMsgStyle::Success)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("  ✅ ok"),
            Some(SystemMsgStyle::Success)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("  ✓ done"),
            Some(SystemMsgStyle::Success)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("✗ fail"),
            Some(SystemMsgStyle::Error)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("❌ boom"),
            Some(SystemMsgStyle::Error)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("⚠ caution"),
            Some(SystemMsgStyle::Warning)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("⚠️ caution"),
            Some(SystemMsgStyle::Warning)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("📝 note"),
            Some(SystemMsgStyle::Accent)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("▶ start"),
            Some(SystemMsgStyle::Accent)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("🤖 agent"),
            Some(SystemMsgStyle::Accent)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("📋 Copied: x"),
            Some(SystemMsgStyle::Accent)
        );
        assert_eq!(
            SystemMsgStyle::from_marker("🎨 Theme: Dark"),
            Some(SystemMsgStyle::Accent)
        );
    }

    #[test]
    fn from_marker_ignores_plain_indentation() {
        assert_eq!(SystemMsgStyle::from_marker("  indented"), None);
        assert_eq!(SystemMsgStyle::from_marker("  **not bold**"), None);
    }

    #[test]
    fn log_item_kind_owns_indent_and_provenance() {
        assert_eq!(LogItemKind::User.log_indent(), 0);
        assert_eq!(
            LogItemKind::AssistantMarkdown.log_indent(),
            LOG_THINKING_INDENT + 1
        );
        assert_eq!(
            LogItemKind::SystemPlain(SystemMsgStyle::Default).log_indent(),
            LOG_THINKING_INDENT + 1
        );
        assert_eq!(LogItemKind::SystemTool.log_indent(), LOG_TOOL_INDENT);
        assert_eq!(LogItemKind::Thinking.log_indent(), LOG_THINKING_INDENT);
        assert!(LogItemKind::User.is_user());
        assert!(!LogItemKind::AssistantMarkdown.is_user());
    }

    #[test]
    fn system_style_colors_use_theme_slots() {
        let theme = Theme::from(ThemeName::Dark);
        assert_eq!(SystemMsgStyle::Default.color(&theme), theme.fg);
        assert_eq!(SystemMsgStyle::Success.color(&theme), theme.success);
        assert_eq!(SystemMsgStyle::Error.color(&theme), theme.error);
        assert_eq!(SystemMsgStyle::Warning.color(&theme), theme.warning);
        assert_eq!(SystemMsgStyle::Accent.color(&theme), theme.accent);
    }
}
