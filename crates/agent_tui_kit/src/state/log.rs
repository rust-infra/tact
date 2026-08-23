//! Shared-log model + ownership — the priority-0 component.
//!
//! Owns the log rows and all primitive row operations. Cross-state helpers
//! (gap checks, index fixups, i18n-driven messages) stay in the consuming app
//! and delegate to these primitives.

use ratatui::{style::Color, text::Line};

use crate::{
    render::cells::markdown::MarkdownCell,
    render::util::{LOG_THINKING_INDENT, LOG_TOOL_INDENT},
    theme::Theme,
};

/// Visual provenance for system messages (drives semantic coloring).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemMsgStyle {
    Default,
    Success,
    Error,
    Warning,
    Accent,
}

impl SystemMsgStyle {
    /// Detect an explicit system marker after optional leading whitespace.
    ///
    /// This is only used to choose the visual color for a message that is
    /// already known to come from a system-message insertion path. It never
    /// decides whether arbitrary text is a system item.
    pub fn from_marker(s: &str) -> Option<Self> {
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

    pub fn color(self, theme: &Theme) -> Color {
        match self {
            Self::Default => theme.fg,
            Self::Success => theme.success,
            Self::Error => theme.error,
            Self::Warning => theme.warning,
            Self::Accent => theme.accent,
        }
    }
}

/// The kind of a shared-log row, deciding indent and rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogItemKind {
    User,
    AssistantMarkdown,
    SystemPlain(SystemMsgStyle),
    SystemMarkdown,
    SystemTool,
    Thinking,
}

impl LogItemKind {
    pub fn log_indent(self) -> u16 {
        match self {
            Self::User => 0,
            Self::AssistantMarkdown | Self::SystemPlain(_) | Self::SystemMarkdown => {
                LOG_THINKING_INDENT + 1
            }
            Self::SystemTool => LOG_TOOL_INDENT,
            Self::Thinking => LOG_THINKING_INDENT,
        }
    }

    pub fn is_user(self) -> bool {
        matches!(self, Self::User)
    }
}

/// One physical shared-log row.
pub struct LogItem {
    pub line: Line<'static>,
    pub raw: String,
    pub kind: LogItemKind,
    pub markdown_cell: Option<MarkdownCell>,
}

impl std::fmt::Debug for LogItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogItem")
            .field("raw", &self.raw)
            .field("kind", &self.kind)
            .field("has_markdown_cell", &self.markdown_cell.is_some())
            .finish()
    }
}

impl LogItem {
    pub fn new(line: Line<'static>, raw: String, kind: LogItemKind) -> Self {
        Self {
            line,
            raw,
            kind,
            markdown_cell: None,
        }
    }

    pub fn markdown(raw: String, theme: &Theme, kind: LogItemKind) -> Self {
        let markdown_cell = MarkdownCell::new(&raw, theme).with_indent(LOG_THINKING_INDENT + 1);
        Self {
            line: Line::from(""),
            raw,
            kind,
            markdown_cell: Some(markdown_cell),
        }
    }
}

/// Owns the shared log rows and all primitive row operations.
#[derive(Default)]
pub struct LogCoordinator {
    /// The physical log rows (user / assistant / system / placeholder).
    pub items: Vec<LogItem>,
}

impl LogCoordinator {
    /// Append one log row, keeping all row metadata together in `items`.
    pub fn append_msg(&mut self, line: Line<'static>, raw: String, kind: LogItemKind) {
        self.items.push(LogItem::new(line, raw, kind));
    }

    /// Append a whole-Markdown notice as a single log item.
    pub fn append_markdown(&mut self, content: String, theme: &Theme, kind: LogItemKind) {
        self.items.push(LogItem::markdown(content, theme, kind));
    }

    /// Append a blank row of the given kind.
    pub fn append_blank(&mut self, kind: LogItemKind) {
        self.append_msg(Line::from(""), String::new(), kind);
    }

    pub fn extend_msgs(
        &mut self,
        lines: Vec<Line<'static>>,
        raw_lines: Vec<String>,
        kind: LogItemKind,
    ) {
        debug_assert_eq!(lines.len(), raw_lines.len());
        for (line, raw) in lines.into_iter().zip(raw_lines) {
            self.append_msg(line, raw, kind);
        }
    }

    pub fn insert_msg(&mut self, idx: usize, line: Line<'static>, raw: String, kind: LogItemKind) {
        self.items.insert(idx, LogItem::new(line, raw, kind));
    }

    pub fn splice_msgs(
        &mut self,
        range: std::ops::Range<usize>,
        lines: Vec<Line<'static>>,
        raw: Vec<String>,
        kind: LogItemKind,
    ) {
        debug_assert_eq!(lines.len(), raw.len());
        self.items.splice(
            range,
            lines
                .into_iter()
                .zip(raw)
                .map(|(line, raw)| LogItem::new(line, raw, kind)),
        );
    }

    pub fn drain_msgs(&mut self, range: std::ops::Range<usize>) {
        self.items.drain(range);
    }

    pub fn remove_msg(&mut self, idx: usize) {
        self.items.remove(idx);
    }

    /// Push `rows` blank placeholder rows of `kind`; returns the first
    /// physical index (the anchor row for the component that owns them).
    pub fn push_placeholder_rows(&mut self, kind: LogItemKind, rows: usize) -> usize {
        let phys_idx = self.items.len();
        for _ in 0..rows {
            self.append_blank(kind);
        }
        phys_idx
    }
}

/// Left indent columns for a physical log row (fallback: assistant markdown).
pub fn log_indent_at(log: &LogCoordinator, phys: usize) -> u16 {
    log.items
        .get(phys)
        .map(|item| item.kind)
        .unwrap_or(LogItemKind::AssistantMarkdown)
        .log_indent()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

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
