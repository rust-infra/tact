//! Shared-log model + ownership — the priority-0 component.
//!
//! Owns the log rows and all primitive row operations. Cross-state helpers
//! (gap checks, index fixups, i18n-driven messages) stay in the consuming app
//! and delegate to these primitives.

use ratatui::{style::Color, text::Line};

use crate::{
    render::cells::markdown::{Gutter, MarkdownCell},
    render::util::{LOG_THINKING_INDENT, LOG_TOOL_BLOCK_INDENT, LOG_TOOL_INDENT},
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
    /// A plugin hook's progress line, keyed by the id the agent handed out.
    ///
    /// The id is what lets the completion update find the row it must rewrite:
    /// the row keeps its physical index (removing one would shift every index
    /// the selection and the cards are keyed on), so the only way back to it is
    /// a value it carries itself.
    HookStatus(u64),
    /// Hook-injected context: the labelled header row, the barred body, and the
    /// row that says how much of it was left out.
    ///
    /// One kind for the whole block because they share a column — the hook
    /// block sits where the tool blocks sit, so its bar reads as a sibling of
    /// their cards rather than as a rail of its own.
    ///
    /// "Where the tool blocks sit" is [`LOG_TOOL_BLOCK_INDENT`], not
    /// [`LOG_TOOL_INDENT`]: a rendered tool block insets its whole area by the
    /// former (`ToolCell::render_partial`), and that is the only tool column a
    /// reader ever sees. The latter belongs to the blank placeholder rows a
    /// tool block overwrites, which are never on screen.
    HookContext,
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
            Self::HookStatus(_) | Self::HookContext => LOG_TOOL_BLOCK_INDENT,
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
    /// Full text this row opens in the read-out popup when double-clicked.
    ///
    /// A row that carries one is a **control**, not text: the mouse handler
    /// opens the popup instead of starting a selection. That is how a block too
    /// long to show inline (a hook's briefing) still keeps its whole body one
    /// gesture away without the log having to hold a second copy.
    pub popup_source: Option<String>,
    /// Bar this row wears down its left edge, if any.
    ///
    /// A row property rather than a prefix in [`Self::line`], because the bar
    /// belongs to **every visual row** the row wraps into: a `▎` typed into the
    /// text marks the first visual row and leaves the continuation bare, which
    /// punches a hole in a container's left rail exactly when the panel is
    /// narrow. A Markdown row draws its own bar
    /// ([`MarkdownCell::with_gutter`]); a text row's is inserted by the host's
    /// wrap pass, which is why the row has to declare it here.
    pub gutter: Option<Gutter>,
}

impl std::fmt::Debug for LogItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LogItem")
            .field("raw", &self.raw)
            .field("kind", &self.kind)
            .field("has_markdown_cell", &self.markdown_cell.is_some())
            .field("has_popup_source", &self.popup_source.is_some())
            .field("has_gutter", &self.gutter.is_some())
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
            popup_source: None,
            gutter: None,
        }
    }

    /// Make this row open `source` in the read-out popup on a double-click.
    ///
    /// Consumes and returns the row so a constructor call can wear it inline.
    #[must_use]
    pub fn with_popup_source(mut self, source: String) -> Self {
        self.popup_source = Some(source);
        self
    }

    /// Make this row wear `gutter` down its left edge on every visual row.
    #[must_use]
    pub fn with_gutter(mut self, gutter: Gutter) -> Self {
        self.gutter = Some(gutter);
        self
    }

    pub fn markdown(raw: String, theme: &Theme, kind: LogItemKind) -> Self {
        let markdown_cell = MarkdownCell::new(&raw, theme).with_indent(LOG_THINKING_INDENT + 1);
        Self {
            line: Line::from(""),
            raw,
            kind,
            markdown_cell: Some(markdown_cell),
            popup_source: None,
            gutter: None,
        }
    }

    /// A whole-Markdown row wearing `gutter` down its left edge.
    ///
    /// The gutter is drawn *at* the kind's column, not left of it, so a bared
    /// block lines up with the rows it belongs beside — the hook block shares
    /// the tool *block* column, which is why the indent follows the kind
    /// instead of a constant.
    pub fn markdown_with_gutter(
        raw: String,
        theme: &Theme,
        kind: LogItemKind,
        gutter: Gutter,
    ) -> Self {
        let markdown_cell = MarkdownCell::new(&raw, theme)
            .with_indent(kind.log_indent())
            .with_gutter(gutter);
        Self {
            line: Line::from(""),
            raw,
            kind,
            markdown_cell: Some(markdown_cell),
            popup_source: None,
            gutter: Some(gutter),
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

    /// Append one log row that opens `popup_source` on a double-click.
    pub fn append_msg_with_popup(
        &mut self,
        line: Line<'static>,
        raw: String,
        kind: LogItemKind,
        popup_source: String,
    ) {
        self.push_popup_row(line, raw, kind, popup_source, None);
    }

    /// Like [`Self::append_msg_with_popup`], but the row wears `gutter` down
    /// its left edge — on every visual row it wraps into, not just the first.
    ///
    /// This is how the two *text* rows of a bared container (a hook block's
    /// header and its "… N more lines" tail) keep the rail unbroken; the body
    /// between them is a Markdown row and draws its own bar.
    pub fn append_bared_msg_with_popup(
        &mut self,
        line: Line<'static>,
        raw: String,
        kind: LogItemKind,
        popup_source: String,
        gutter: Gutter,
    ) {
        self.push_popup_row(line, raw, kind, popup_source, Some(gutter));
    }

    fn push_popup_row(
        &mut self,
        line: Line<'static>,
        raw: String,
        kind: LogItemKind,
        popup_source: String,
        gutter: Option<Gutter>,
    ) {
        let mut item = LogItem::new(line, raw, kind).with_popup_source(popup_source);
        if let Some(gutter) = gutter {
            item = item.with_gutter(gutter);
        }
        self.items.push(item);
    }

    /// Append a whole-Markdown notice as a single log item.
    pub fn append_markdown(&mut self, content: String, theme: &Theme, kind: LogItemKind) {
        self.items.push(LogItem::markdown(content, theme, kind));
    }

    /// Append a whole-Markdown notice that wears a left gutter.
    pub fn append_markdown_with_gutter(
        &mut self,
        content: String,
        theme: &Theme,
        kind: LogItemKind,
        gutter: Gutter,
    ) {
        self.items
            .push(LogItem::markdown_with_gutter(content, theme, kind, gutter));
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
        // The hook block shares the column a rendered tool block insets to,
        // not the placeholder column `SystemTool` reserves.
        assert_eq!(LogItemKind::HookContext.log_indent(), LOG_TOOL_BLOCK_INDENT);
        assert_eq!(
            LogItemKind::HookStatus(1).log_indent(),
            LOG_TOOL_BLOCK_INDENT
        );
        assert_ne!(LOG_TOOL_BLOCK_INDENT, LOG_TOOL_INDENT);
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
