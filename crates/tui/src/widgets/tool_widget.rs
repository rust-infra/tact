use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};
use tact_core::{StepResult, StepStatus};

use crate::{i18n::Messages, theme::Theme};

const DEFAULT_MAX_DETAIL_LINES: usize = 200;
const DEFAULT_PREVIEW_LINES: usize = 10;

/// Tool execution phase for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPhase {
    Running,
    Success,
    Failed,
}

impl ToolPhase {
    fn from_status(status: &StepStatus) -> Self {
        match status {
            StepStatus::Success => Self::Success,
            StepStatus::Failed => Self::Failed,
        }
    }
}

/// Visual strategy inferred from the tool name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolDisplayKind {
    FileWrite,
    FileRead,
    Command,
    Generic,
}

fn display_kind(tool: &str) -> ToolDisplayKind {
    match tool {
        "write_file" => ToolDisplayKind::FileWrite,
        "read_file" => ToolDisplayKind::FileRead,
        "run_command" | "bash" | "shell" => ToolDisplayKind::Command,
        _ => ToolDisplayKind::Generic,
    }
}

/// Layout metadata for reserving placeholder rows in the log panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLayout {
    /// Total visual rows for a full `ToolCell` (summary + optional detail card).
    pub visual_rows: usize,
    /// Number of content preview rows inside the card.
    pub preview_lines: usize,
    /// Whether a detail card should be shown.
    pub has_detail_card: bool,
}

/// Content rows inside the card borders (preview lines + optional overflow row).
pub(crate) fn tool_card_inner_rows(preview_len: usize, total_lines: usize) -> usize {
    preview_len + usize::from(total_lines > preview_len)
}

/// Total visual rows for a tool block in the log column.
///
/// When `card_only` is false, the summary line is included. When true, only the
/// detail card is drawn (summary comes from a separate `TextCell`).
pub(crate) fn tool_visual_rows(
    has_detail_card: bool,
    preview_len: usize,
    total_lines: usize,
    card_only: bool,
) -> usize {
    if card_only {
        if has_detail_card {
            1 + tool_card_inner_rows(preview_len, total_lines) + 1
        } else {
            0
        }
    } else if has_detail_card {
        1 + 1 + tool_card_inner_rows(preview_len, total_lines) + 1
    } else {
        1
    }
}

/// Render-ready output produced by [`ToolWidget`].
#[derive(Debug, Clone)]
pub struct ToolRenderOutput {
    pub summary: Line<'static>,
    pub summary_raw: String,
    /// Execution phase (Success / Failed / Running).
    pub phase: ToolPhase,
    /// Tool argument summary — for file tools this is the filesystem path.
    pub arg_summary: String,
    pub layout: ToolLayout,
    pub detail_title: Option<String>,
    pub detail_preview: Vec<String>,
    pub detail_total_lines: usize,
}

impl ToolRenderOutput {
    /// Total visual rows for this tool block (`card_only` controls summary inclusion).
    pub fn visual_rows(&self, card_only: bool) -> usize {
        tool_visual_rows(
            self.layout.has_detail_card,
            self.detail_preview.len(),
            self.detail_total_lines,
            card_only,
        )
    }

    /// Empty lines to append in `messages[]` after the summary row.
    pub fn message_placeholder_rows(&self) -> usize {
        self.visual_rows(false).saturating_sub(1)
    }
}

/// Unified tool invocation renderer.
///
/// Encapsulates summary lines, detail previews, and card layout so future tool
/// rendering can migrate out of `agent.rs` / `render/cells/diff.rs`.
pub struct ToolWidget<'a> {
    step_index: Option<usize>,
    tool_name: String,
    arg_summary: String,
    phase: ToolPhase,
    message: Option<String>,
    detail: Option<String>,
    duration_ms: Option<u64>,
    theme: &'a Theme,
    msgs: &'a Messages,
    max_detail_lines: usize,
    preview_lines: usize,
}

impl<'a> ToolWidget<'a> {
    pub fn new(theme: &'a Theme, msgs: &'a Messages) -> Self {
        Self {
            step_index: None,
            tool_name: String::new(),
            arg_summary: String::new(),
            phase: ToolPhase::Running,
            message: None,
            detail: None,
            duration_ms: None,
            theme,
            msgs,
            max_detail_lines: DEFAULT_MAX_DETAIL_LINES,
            preview_lines: DEFAULT_PREVIEW_LINES,
        }
    }

    pub fn with_step_index(mut self, index: usize) -> Self {
        self.step_index = Some(index);
        self
    }

    pub fn with_tool(mut self, name: impl Into<String>) -> Self {
        self.tool_name = name.into();
        self
    }

    pub fn with_arg_summary(mut self, summary: impl Into<String>) -> Self {
        self.arg_summary = summary.into();
        self
    }

    pub fn with_phase(mut self, phase: ToolPhase) -> Self {
        self.phase = phase;
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_max_detail_lines(mut self, max: usize) -> Self {
        self.max_detail_lines = max;
        self
    }

    pub fn with_preview_lines(mut self, lines: usize) -> Self {
        self.preview_lines = lines;
        self
    }

    pub fn from_step_result(
        step_index: usize,
        result: &StepResult,
        theme: &'a Theme,
        msgs: &'a Messages,
    ) -> Self {
        Self {
            step_index: Some(step_index),
            tool_name: result.tool.clone(),
            arg_summary: result.arg_summary.clone(),
            phase: ToolPhase::from_status(&result.status),
            message: Some(result.message.clone()),
            detail: result.detail.clone(),
            duration_ms: result.duration_ms,
            theme,
            msgs,
            max_detail_lines: DEFAULT_MAX_DETAIL_LINES,
            preview_lines: DEFAULT_PREVIEW_LINES,
        }
    }

    pub fn with_theme(mut self, theme: &'a Theme) -> Self {
        self.theme = theme;
        self
    }

    pub fn with_messages(mut self, msgs: &'a Messages) -> Self {
        self.msgs = msgs;
        self
    }

    pub fn display_kind(&self) -> &'static str {
        match display_kind(&self.tool_name) {
            ToolDisplayKind::FileWrite => "file_write",
            ToolDisplayKind::FileRead => "file_read",
            ToolDisplayKind::Command => "command",
            ToolDisplayKind::Generic => "generic",
        }
    }

    pub fn has_detail_card(&self) -> bool {
        self.layout().has_detail_card
    }

    pub fn summary_text(&self) -> String {
        match self.phase {
            ToolPhase::Running => self
                .message
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| {
                    self.msgs
                        .step_started_tmpl
                        .replace("{}", &self.running_label())
                }),
            ToolPhase::Success | ToolPhase::Failed => self.finished_summary_text(),
        }
    }

    pub fn summary_line(&self) -> Line<'static> {
        let style = match self.phase {
            ToolPhase::Running => Style::default().fg(self.theme.warning),
            ToolPhase::Success => Style::default().fg(self.theme.success),
            ToolPhase::Failed => Style::default().fg(self.theme.error),
        };
        Line::from(Span::styled(self.summary_text(), style))
    }

    pub fn layout(&self) -> ToolLayout {
        let Some(detail) = self.detail.as_ref().filter(|d| self.should_show_detail(d)) else {
            return ToolLayout {
                visual_rows: tool_visual_rows(false, 0, 0, false),
                preview_lines: 0,
                has_detail_card: false,
            };
        };

        let total_lines = detail.lines().count();
        let preview_count = total_lines.min(self.preview_lines);
        ToolLayout {
            visual_rows: tool_visual_rows(true, preview_count, total_lines, false),
            preview_lines: preview_count,
            has_detail_card: true,
        }
    }

    pub fn build(&self) -> ToolRenderOutput {
        let layout = self.layout();
        let (detail_title, detail_preview, detail_total_lines) = if layout.has_detail_card {
            let detail = self.detail.as_deref().unwrap_or_default();
            let lines: Vec<String> = detail
                .lines()
                .take(self.max_detail_lines)
                .map(str::to_string)
                .collect();
            let total = detail.lines().count();
            let preview = lines.iter().take(layout.preview_lines).cloned().collect();
            (Some(self.detail_card_title(total)), preview, total)
        } else {
            (None, Vec::new(), 0)
        };

        let summary_raw = self.summary_text();
        ToolRenderOutput {
            summary: self.summary_line(),
            summary_raw,
            phase: self.phase,
            arg_summary: self.arg_summary.clone(),
            layout,
            detail_title,
            detail_preview,
            detail_total_lines,
        }
    }

    pub fn detail_card_lines(&self, width: u16) -> Vec<Line<'static>> {
        let output = self.build();
        if !output.layout.has_detail_card {
            return Vec::new();
        }

        let num_width = (output.detail_total_lines + 1)
            .to_string()
            .len()
            .max(3);
        let code_width = (width as usize).saturating_sub(num_width + 3);
        let num_style = Style::default().fg(Color::Gray).bg(self.theme.bg);
        let text_style = Style::default().fg(self.theme.fg).bg(self.theme.bg);
        let plus_style = Style::default().fg(self.theme.success).bg(self.theme.bg);

        let mut lines: Vec<Line<'static>> = output
            .detail_preview
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let num = format!("{:>nw$}", i + 1, nw = num_width);
                let trimmed: String = line.chars().take(code_width).collect();
                Line::from(vec![
                    Span::styled(format!(" {} ", num), num_style),
                    Span::styled("+ ", plus_style),
                    Span::styled(trimmed, text_style),
                ])
            })
            .collect();

        if output.detail_total_lines > output.detail_preview.len() {
            lines.push(Line::from(Span::styled(
                self.msgs.diff_overflow_tmpl.replace(
                    "{}",
                    &(output.detail_total_lines - output.detail_preview.len()).to_string(),
                ),
                Style::default().fg(Color::Gray).bg(self.theme.bg),
            )));
        }

        lines
    }

    fn running_label(&self) -> String {
        if self.arg_summary.is_empty() {
            self.tool_name.clone()
        } else if self.tool_name.is_empty() {
            self.arg_summary.clone()
        } else {
            format!("{}({})", self.tool_name, self.arg_summary)
        }
    }

    fn finished_summary_text(&self) -> String {
        let icon = match self.phase {
            ToolPhase::Success => self.msgs.step_success_prefix,
            ToolPhase::Failed => self.msgs.step_fail_prefix,
            ToolPhase::Running => "▶",
        };
        let step_no = self
            .step_index
            .map(|idx| (idx + 1).to_string())
            .unwrap_or_else(|| "?".to_string());

        let mut log_msg = if self.arg_summary.is_empty() {
            self.msgs
                .step_finished_simple_tmpl
                .replacen("{}", icon, 1)
                .replacen("{}", &step_no, 1)
                .replacen("{}", &self.tool_name, 1)
        } else {
            self.msgs
                .step_finished_args_tmpl
                .replacen("{}", icon, 1)
                .replacen("{}", &step_no, 1)
                .replacen("{}", &self.tool_name, 1)
                .replacen("{}", &self.arg_summary, 1)
        };

        log_msg.push_str(&self.bytes_suffix());
        log_msg.push_str(&self.duration_suffix());
        log_msg
    }

    fn bytes_suffix(&self) -> String {
        match self.tool_name.as_str() {
            "read_file" | "write_file" => self
                .detail
                .as_ref()
                .map(|d| self.msgs.step_bytes_tmpl.replace("{}", &d.len().to_string()))
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn duration_suffix(&self) -> String {
        self.duration_ms.map_or(String::new(), |ms| {
            if ms < 1000 {
                self.msgs.step_ms_tmpl.replace("{}", &ms.to_string())
            } else {
                self.msgs
                    .step_sec_tmpl
                    .replace("{}", &format!("{:.1}", ms as f64 / 1000.0))
            }
        })
    }

    fn should_show_detail(&self, detail: &str) -> bool {
        if detail.is_empty() {
            return false;
        }
        matches!(
            display_kind(&self.tool_name),
            ToolDisplayKind::FileWrite | ToolDisplayKind::FileRead | ToolDisplayKind::Command
        ) && matches!(self.phase, ToolPhase::Success)
    }

    fn detail_card_title(&self, total_lines: usize) -> String {
        match display_kind(&self.tool_name) {
            ToolDisplayKind::FileWrite => self
                .msgs
                .diff_card_title
                .replacen("{}", &total_lines.to_string(), 1)
                .replacen("{}", &self.arg_summary, 1),
            ToolDisplayKind::FileRead => format!("Read {} ({} lines)", self.arg_summary, total_lines),
            ToolDisplayKind::Command => format!(
                "Command output ({} lines)",
                total_lines
            ),
            ToolDisplayKind::Generic => format!("{} output", self.tool_name),
        }
    }
}

impl Widget for ToolWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let output = self.build();
        if !output.layout.has_detail_card {
            Paragraph::new(vec![output.summary])
                .style(Style::default().fg(self.theme.fg).bg(self.theme.bg))
                .render(area, buf);
            return;
        }

        let title = output.detail_title.unwrap_or_default();
        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .style(Style::default().bg(self.theme.bg))
            .title(title)
            .title_bottom(Line::from(Span::styled(
                self.msgs.diff_card_bottom,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::ITALIC),
            )));

        if area.height == 1 {
            output.summary.render(area, buf);
            return;
        }

        let summary_area = Rect::new(area.x, area.y, area.width, 1);
        output.summary.render(summary_area, buf);

        let card_area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if card_area.height < 3 {
            return;
        }

        card_block.render(card_area, buf);
        let inner = Rect::new(
            card_area.x + 1,
            card_area.y + 1,
            card_area.width.saturating_sub(2),
            card_area.height.saturating_sub(2),
        );
        if inner.height > 0 {
            let lines = self.detail_card_lines(inner.width);
            Paragraph::new(lines)
                .style(Style::default().bg(self.theme.bg))
                .render(inner, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;
    use tact_core::StepStatus;

    fn fixture() -> (Theme, Messages) {
        (Theme::by_name_str("retro"), Messages::by_language(Language::English))
    }

    #[test]
    fn summary_for_finished_write_file_includes_bytes_and_tool() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_step_index(0)
            .with_tool("write_file")
            .with_arg_summary("src/main.rs")
            .with_phase(ToolPhase::Success)
            .with_detail("fn main() {}\n");

        let text = widget.summary_text();
        assert!(text.contains("write_file"));
        assert!(text.contains("src/main.rs"));
        assert!(text.contains("B]"));
    }

    #[test]
    fn write_file_builds_detail_card_layout() {
        let (theme, msgs) = fixture();
        let detail = (0..15)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let widget = ToolWidget::new(&theme, &msgs)
            .with_step_index(1)
            .with_tool("write_file")
            .with_arg_summary("a.rs")
            .with_phase(ToolPhase::Success)
            .with_detail(detail);

        let output = widget.build();
        assert!(output.layout.has_detail_card);
        assert_eq!(output.layout.preview_lines, DEFAULT_PREVIEW_LINES);
        assert_eq!(output.detail_preview.len(), DEFAULT_PREVIEW_LINES);
        assert_eq!(output.detail_total_lines, 15);
        assert_eq!(
            output.layout.visual_rows,
            tool_visual_rows(true, DEFAULT_PREVIEW_LINES, 15, false)
        );
    }

    #[test]
    fn generic_tool_has_no_detail_card() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_step_index(0)
            .with_tool("grep")
            .with_phase(ToolPhase::Success)
            .with_detail("match line");

        assert!(!widget.has_detail_card());
        assert_eq!(widget.layout().visual_rows, 1);
    }

    #[test]
    fn from_step_result_maps_fields() {
        let (theme, msgs) = fixture();
        let result = StepResult {
            tool: "read_file".to_string(),
            arg_summary: "Cargo.toml".to_string(),
            status: StepStatus::Success,
            message: "ok".to_string(),
            detail: Some("[package]\n".to_string()),
            duration_ms: Some(12),
        };
        let widget = ToolWidget::from_step_result(2, &result, &theme, &msgs);

        assert_eq!(widget.tool_name, "read_file");
        assert!(widget.summary_text().contains("12ms"));
        assert!(widget.has_detail_card());
    }

    #[test]
    fn widget_renders_into_buffer() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_step_index(0)
            .with_tool("write_file")
            .with_arg_summary("lib.rs")
            .with_phase(ToolPhase::Success)
            .with_detail("pub fn hi() {}\n");

        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        assert_eq!(buf.area, area);
    }
}
