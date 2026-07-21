use ratatui::style::Color;

use crate::{
    render::util::{LOG_THINKING_INDENT, LOG_TOOL_INDENT},
    theme::Theme,
    widgets::state::RawMessageType,
};

/// Visual style for prefix-marked system messages in the log panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemMsgStyle {
    Success,
    Error,
    Warning,
    Accent,
}

impl SystemMsgStyle {
    /// Detect an emoji / symbol marker after leading whitespace.
    ///
    /// Does not treat leading double-space indent as a marker (that is only
    /// for [`from_line`], used when deciding the add_system_message path).
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
        PREFIXES.iter().find(|(prefix, _)| trimmed.starts_with(prefix)).map(|(_, style)| *style)
    }

    /// Detect a system-marker prefix, including indent-marked rows (`"  …"`).
    ///
    /// Returns `None` when the line should be rendered as Markdown instead.
    pub(crate) fn from_line(s: &str) -> Option<Self> {
        if let Some(style) = Self::from_marker(s) {
            return Some(style);
        }
        // Indent-marked rows (e.g. plan steps): checked on the raw string so
        // trim_start cannot erase the marker.
        if s.starts_with("  ") {
            return Some(Self::Accent);
        }
        None
    }

    pub(crate) fn color(self, theme: &Theme) -> Color {
        match self {
            Self::Success => theme.success,
            Self::Error => theme.error,
            Self::Warning => theme.warning,
            Self::Accent => theme.accent,
        }
    }
}

fn is_plan_step_line(raw: &str) -> bool {
    raw.strip_prefix("  ")
        .and_then(|rest| {
            let (num, after) = rest.split_once(". ")?;
            (!num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && !after.is_empty()).then_some(())
        })
        .is_some()
}

/// Classify plain-text system / info rows for indent and styling.
pub(crate) fn classify_system_message(raw: &str) -> RawMessageType {
    let raw = raw.trim_end();

    if raw.starts_with('▶')
        || raw.starts_with("Executing ")
        || raw.starts_with("Error invoking tool ")
        || (raw.starts_with('⚠') && (raw.contains("Need approval:") || raw.contains("需要审批:")))
        || (raw.starts_with("Generated ") && raw.contains(" steps:"))
        || (raw.starts_with("生成了 ") && raw.contains("个步骤"))
        || is_plan_step_line(raw)
    {
        return RawMessageType::SysTool;
    }

    if (raw.starts_with('✓') || raw.starts_with('✗') || raw.starts_with('✔'))
        && (raw.contains("Step ")
            || raw.contains("步骤 ")
            || raw.contains("Selected:")
            || raw.contains("已选择:")
            || raw.contains("Step approved")
            || raw.contains("步骤已批准")
            || raw.contains("Step rejected")
            || raw.contains("步骤已拒绝"))
    {
        return RawMessageType::SysTool;
    }

    RawMessageType::LLM
}

impl RawMessageType {
    pub(crate) fn log_indent(self) -> u16 {
        match self {
            Self::LLM => LOG_THINKING_INDENT,
            Self::LLMThinking => LOG_THINKING_INDENT,
            Self::SysTool => LOG_TOOL_INDENT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SystemMsgStyle;
    use crate::theme::{Theme, ThemeName};

    #[test]
    fn from_line_maps_prefixes() {
        assert_eq!(SystemMsgStyle::from_line("✓ done"), Some(SystemMsgStyle::Success));
        assert_eq!(SystemMsgStyle::from_line("✔ done"), Some(SystemMsgStyle::Success));
        assert_eq!(SystemMsgStyle::from_line("  ✅ ok"), Some(SystemMsgStyle::Success));
        // Leading whitespace must not demote success/error/warning to accent.
        assert_eq!(SystemMsgStyle::from_line("  ✓ done"), Some(SystemMsgStyle::Success));
        assert_eq!(SystemMsgStyle::from_line("✗ fail"), Some(SystemMsgStyle::Error));
        assert_eq!(SystemMsgStyle::from_line("❌ boom"), Some(SystemMsgStyle::Error));
        assert_eq!(SystemMsgStyle::from_line("⚠ caution"), Some(SystemMsgStyle::Warning));
        assert_eq!(SystemMsgStyle::from_line("⚠️ caution"), Some(SystemMsgStyle::Warning));
        assert_eq!(SystemMsgStyle::from_line("📝 note"), Some(SystemMsgStyle::Accent));
        assert_eq!(SystemMsgStyle::from_line("▶ start"), Some(SystemMsgStyle::Accent));
        assert_eq!(SystemMsgStyle::from_line("🤖 agent"), Some(SystemMsgStyle::Accent));
        assert_eq!(SystemMsgStyle::from_line("📋 Copied: x"), Some(SystemMsgStyle::Accent));
        assert_eq!(SystemMsgStyle::from_line("🎨 Theme: Dark"), Some(SystemMsgStyle::Accent));
        assert_eq!(SystemMsgStyle::from_line("  indented"), Some(SystemMsgStyle::Accent));
    }

    #[test]
    fn from_marker_ignores_indent_only_rows() {
        assert_eq!(SystemMsgStyle::from_marker("  indented"), None);
        assert_eq!(SystemMsgStyle::from_marker("  ✓ done"), Some(SystemMsgStyle::Success));
    }

    #[test]
    fn from_line_none_for_markdown() {
        assert_eq!(SystemMsgStyle::from_line("# Heading"), None);
        assert_eq!(SystemMsgStyle::from_line("plain text"), None);
        assert_eq!(SystemMsgStyle::from_line(""), None);
    }

    #[test]
    fn indent_prefix_skips_markdown_path() {
        // Intentional: leading "  " selects the plain system path (no MD).
        assert_eq!(SystemMsgStyle::from_line("  **not bold**"), Some(SystemMsgStyle::Accent));
        assert_eq!(SystemMsgStyle::from_line("  # heading"), Some(SystemMsgStyle::Accent));
    }

    #[test]
    fn color_uses_theme_slots() {
        let theme = Theme::from(ThemeName::Dark);
        assert_eq!(SystemMsgStyle::Success.color(&theme), theme.success);
        assert_eq!(SystemMsgStyle::Error.color(&theme), theme.error);
        assert_eq!(SystemMsgStyle::Warning.color(&theme), theme.warning);
        assert_eq!(SystemMsgStyle::Accent.color(&theme), theme.accent);
    }
}
