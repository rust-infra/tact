use std::time::Instant;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use tact_protocol::{StepResult, StepStatus};

use crate::{i18n::Messages, theme::Theme};

const DEFAULT_MAX_DETAIL_LINES: usize = 200;
const DEFAULT_PREVIEW_LINES: usize = 1;
const ERROR_PREVIEW_LINES: usize = 5;
pub(crate) const TOOL_HEADER_ROWS: usize = 2;

const RUNNING_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
pub const TOOL_RUNNING_SPINNER: &[char] = RUNNING_SPINNER;

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
pub enum ToolDisplayKind {
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

pub fn tool_display_name(tool: &str) -> String {
    match tool {
        "write_file" => "Write".to_string(),
        "read_file" => "Read".to_string(),
        "bash" | "shell" => "Bash".to_string(),
        "run_command" => "Command".to_string(),
        other => {
            if other.is_empty() {
                "Tool".to_string()
            } else {
                let mut chars = other.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                }
            }
        }
    }
}

pub fn format_duration_us(us: u64) -> String {
    if us < 1000 {
        format!("{us}us")
    } else if us < 1_000_000 {
        let ms = us as f64 / 1000.0;
        format!("{ms:.2}ms")
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

pub use tact_protocol::format_bytes;

/// Build the plain-text meta line (title + meta rows).
#[allow(clippy::too_many_arguments)]
pub fn build_meta_text(
    phase: ToolPhase,
    permission_label: Option<&str>,
    size_bytes: Option<usize>,
    duration_us: Option<u64>,
    error_message: Option<&str>,
    spinner_char: char,
    phase_running: &str,
    phase_success: &str,
    phase_failed: &str,
    meta_sep: &str,
    success_prefix: &str,
    fail_prefix: &str,
) -> String {
    let phase_label = match phase {
        ToolPhase::Running => format!("{spinner_char} {phase_running}"),
        ToolPhase::Success => format!("{success_prefix} {phase_success}"),
        ToolPhase::Failed => format!("{fail_prefix} {phase_failed}"),
    };

    let mut parts = vec![phase_label];
    if matches!(phase, ToolPhase::Failed)
        && let Some(err) = error_message.filter(|s| !s.is_empty())
    {
        parts.push(truncate_tool_error(err));
    }
    if let Some(size) = size_bytes.filter(|_| matches!(phase, ToolPhase::Success)) {
        parts.push(format_bytes(size));
    }
    if let Some(label) = permission_label.filter(|s| !s.is_empty()) {
        parts.push(label.to_string());
    }
    if let Some(us) = duration_us {
        parts.push(format_duration_us(us));
    }
    parts.join(meta_sep)
}

fn truncate_tool_error(error: &str) -> String {
    const MAX_CHARS: usize = 80;
    let one_line = error.replace('\n', " ").trim().to_string();
    if one_line.chars().count() <= MAX_CHARS {
        one_line
    } else {
        format!(
            "{}…",
            one_line.chars().take(MAX_CHARS - 1).collect::<String>()
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_meta_line(
    phase: ToolPhase,
    permission_label: Option<&str>,
    size_bytes: Option<usize>,
    duration_us: Option<u64>,
    error_message: Option<&str>,
    spinner_char: char,
    theme: &Theme,
    msgs: &Messages,
) -> Line<'static> {
    let style = match phase {
        ToolPhase::Running => Style::default().fg(theme.warning),
        ToolPhase::Success => Style::default().fg(theme.success),
        ToolPhase::Failed => Style::default().fg(theme.error),
    };
    Line::from(Span::styled(
        build_meta_text(
            phase,
            permission_label,
            size_bytes,
            duration_us,
            error_message,
            spinner_char,
            msgs.tool_phase_running,
            msgs.tool_phase_success,
            msgs.tool_phase_failed,
            msgs.tool_meta_sep,
            msgs.step_success_prefix,
            msgs.step_fail_prefix,
        ),
        style,
    ))
}

pub fn running_elapsed_us(started_at: Instant) -> u64 {
    started_at.elapsed().as_micros() as u64
}

/// Layout metadata for reserving placeholder rows in the log panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLayout {
    /// Total visual rows for a full `ToolCell` (header + optional detail card).
    pub visual_rows: usize,
    /// Number of content preview rows inside the card.
    pub preview_lines: usize,
    /// Whether a detail card should be shown.
    pub has_detail_card: bool,
}

/// Content rows inside the card borders.
///
/// Overflow text is rendered in the bottom hint (`title_bottom`) so it does not
/// consume an extra preview row.
pub(crate) fn tool_card_inner_rows(preview_len: usize, total_lines: usize) -> usize {
    let _ = total_lines;
    preview_len
}

/// Total visual rows for a tool block in the log column.
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
        TOOL_HEADER_ROWS + 1 + tool_card_inner_rows(preview_len, total_lines) + 1
    } else {
        TOOL_HEADER_ROWS
    }
}

/// Render-ready output produced by [`ToolWidget`].
#[derive(Debug, Clone)]
pub struct ToolRenderOutput {
    pub title_line: Line<'static>,
    pub title_raw: String,
    pub phase: ToolPhase,
    pub permission_label: Option<String>,
    pub error_message: Option<String>,
    pub duration_us: Option<u64>,
    pub size_bytes: Option<usize>,
    pub tool_name: String,
    pub use_diff_gutter: bool,
    /// Tool argument summary — for file tools this is the filesystem path.
    pub arg_summary: String,
    /// Full tool argument summary (untruncated), used by popups/details.
    pub arg_full: String,
    pub layout: ToolLayout,
    pub detail_title: Option<String>,
    pub detail_preview: Vec<String>,
    pub detail_total_lines: usize,
    /// Full detail text for popup display (preview may be truncated).
    pub detail_full: Option<String>,
    pub card_bottom: String,
}

impl ToolRenderOutput {
    pub fn visual_rows(&self, card_only: bool) -> usize {
        tool_visual_rows(
            self.layout.has_detail_card,
            self.detail_preview.len(),
            self.detail_total_lines,
            card_only,
        )
    }

    pub fn message_placeholder_rows(&self) -> usize {
        self.visual_rows(false).saturating_sub(1)
    }
}

/// Unified tool invocation renderer.
pub struct ToolWidget<'a> {
    tool_name: String,
    arg_summary: String,
    arg_full: String,
    step_index: Option<usize>,
    phase: ToolPhase,
    detail: Option<String>,
    duration_us: Option<u64>,
    permission_label: Option<String>,
    error_message: Option<String>,
    theme: &'a Theme,
    msgs: &'a Messages,
    max_detail_lines: usize,
    preview_lines: usize,
}

impl<'a> ToolWidget<'a> {
    pub fn new(theme: &'a Theme, msgs: &'a Messages) -> Self {
        Self {
            tool_name: String::new(),
            arg_summary: String::new(),
            arg_full: String::new(),
            step_index: None,
            phase: ToolPhase::Running,
            detail: None,
            duration_us: None,
            permission_label: None,
            error_message: None,
            theme,
            msgs,
            max_detail_lines: DEFAULT_MAX_DETAIL_LINES,
            preview_lines: DEFAULT_PREVIEW_LINES,
        }
    }

    pub fn with_tool(mut self, name: impl Into<String>) -> Self {
        self.tool_name = name.into();
        self
    }

    pub fn with_arg_summary(mut self, summary: impl Into<String>) -> Self {
        let summary = summary.into();
        self.arg_summary = summary.clone();
        if self.arg_full.is_empty() {
            self.arg_full = summary;
        }
        self
    }

    pub fn with_arg_full(mut self, full: impl Into<String>) -> Self {
        self.arg_full = full.into();
        self
    }

    pub fn with_step_index(mut self, step_index: usize) -> Self {
        self.step_index = Some(step_index);
        self
    }

    pub fn with_phase(mut self, phase: ToolPhase) -> Self {
        self.phase = phase;
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_duration_us(mut self, duration_us: u64) -> Self {
        self.duration_us = Some(duration_us);
        self
    }

    #[allow(dead_code)]
    pub fn with_permission_label(mut self, label: impl Into<String>) -> Self {
        self.permission_label = Some(label.into());
        self
    }

    #[allow(dead_code)]
    pub fn with_permission_label_opt(mut self, label: Option<String>) -> Self {
        self.permission_label = label;
        self
    }

    #[allow(dead_code)]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.error_message = Some(message.into());
        self
    }

    pub fn from_step_result(result: &StepResult, theme: &'a Theme, msgs: &'a Messages) -> Self {
        let failed = matches!(ToolPhase::from_status(&result.status), ToolPhase::Failed);
        let detail = result.detail.clone().or_else(|| {
            if failed && !result.message.is_empty() {
                Some(result.message.clone())
            } else {
                None
            }
        });
        Self {
            tool_name: result.tool.clone(),
            arg_summary: result.arg_summary.clone(),
            arg_full: result
                .arg_full
                .clone()
                .unwrap_or_else(|| result.arg_summary.clone()),
            step_index: None,
            phase: ToolPhase::from_status(&result.status),
            detail,
            duration_us: result.duration_us,
            permission_label: result.permission_label.clone(),
            error_message: None,
            theme,
            msgs,
            max_detail_lines: DEFAULT_MAX_DETAIL_LINES,
            preview_lines: DEFAULT_PREVIEW_LINES,
        }
    }

    pub fn title_text(&self) -> String {
        let base = match display_kind(&self.tool_name) {
            ToolDisplayKind::Command => {
                let label = self.tool_name.to_lowercase();
                if self.arg_summary.is_empty() {
                    label
                } else {
                    format!("{label} ({})", self.arg_summary)
                }
            }
            _ => {
                let label = tool_display_name(&self.tool_name);
                if self.arg_summary.is_empty() {
                    label
                } else {
                    format!("{label}  {}", self.arg_summary)
                }
            }
        };

        if let Some(idx) = self.step_index {
            format!("{}. {}", idx + 1, base)
        } else {
            base
        }
    }

    pub fn title_line(&self) -> Line<'static> {
        Line::from(Span::styled(
            self.title_text(),
            Style::default()
                .fg(self.theme.fg)
                .add_modifier(Modifier::BOLD),
        ))
    }

    pub fn size_bytes(&self) -> Option<usize> {
        match display_kind(&self.tool_name) {
            ToolDisplayKind::FileWrite | ToolDisplayKind::FileRead => {
                self.detail.as_ref().map(|d| d.len()).filter(|len| *len > 0)
            }
            _ => None,
        }
    }

    pub fn layout(&self) -> ToolLayout {
        let Some(detail) = self.display_detail() else {
            return ToolLayout {
                visual_rows: tool_visual_rows(false, 0, 0, false),
                preview_lines: 0,
                has_detail_card: false,
            };
        };
        if !self.should_show_detail(detail) {
            return ToolLayout {
                visual_rows: tool_visual_rows(false, 0, 0, false),
                preview_lines: 0,
                has_detail_card: false,
            };
        }

        let total_lines = detail.lines().count();
        let preview_cap = if matches!(self.phase, ToolPhase::Failed) {
            ERROR_PREVIEW_LINES
        } else {
            self.preview_lines
        };
        let preview_count = total_lines.min(preview_cap);
        ToolLayout {
            visual_rows: tool_visual_rows(true, preview_count, total_lines, false),
            preview_lines: preview_count,
            has_detail_card: true,
        }
    }

    pub fn build(&self) -> ToolRenderOutput {
        let layout = self.layout();
        let use_diff_gutter = matches!(
            display_kind(&self.tool_name),
            ToolDisplayKind::FileWrite
        );
        let (detail_title, detail_preview, detail_total_lines) = if layout.has_detail_card {
            let detail = self.display_detail().unwrap_or_default();
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

        let title_raw = self.title_text();
        let has_detail_card = layout.has_detail_card;
        let card_bottom = if matches!(self.phase, ToolPhase::Failed) {
            self.msgs.tool_error_card_bottom.to_string()
        } else {
            self.msgs.diff_card_bottom.to_string()
        };
        ToolRenderOutput {
            title_line: self.title_line(),
            title_raw,
            phase: self.phase,
            permission_label: self.permission_label.clone(),
            error_message: self.error_message.clone(),
            duration_us: self.duration_us,
            size_bytes: self.size_bytes(),
            tool_name: self.tool_name.clone(),
            use_diff_gutter,
            arg_summary: self.arg_summary.clone(),
            arg_full: if self.arg_full.is_empty() {
                self.arg_summary.clone()
            } else {
                self.arg_full.clone()
            },
            layout,
            detail_title,
            detail_preview,
            detail_total_lines,
            detail_full: if has_detail_card {
                self.display_detail().map(str::to_string)
            } else {
                None
            },
            card_bottom,
        }
    }

    fn display_detail(&self) -> Option<&str> {
        if matches!(self.phase, ToolPhase::Failed) {
            self.detail
                .as_deref()
                .or(self.error_message.as_deref())
                .filter(|s| !s.is_empty())
        } else {
            self.detail.as_deref().filter(|s| !s.is_empty())
        }
    }

    fn should_show_detail(&self, detail: &str) -> bool {
        if detail.is_empty() {
            return false;
        }
        if matches!(self.phase, ToolPhase::Failed) {
            return true;
        }
        matches!(
            display_kind(&self.tool_name),
            ToolDisplayKind::FileWrite
                | ToolDisplayKind::FileRead
                | ToolDisplayKind::Command
        ) && matches!(self.phase, ToolPhase::Success)
    }

    fn detail_card_title(&self, total_lines: usize) -> String {
        if matches!(self.phase, ToolPhase::Failed) {
            return self.msgs.tool_error_card_title.to_string();
        }
        match display_kind(&self.tool_name) {
            ToolDisplayKind::FileWrite => self
                .msgs
                .diff_card_title
                .replacen("{}", &total_lines.to_string(), 1)
                .replacen("{}", &self.arg_summary, 1),
            ToolDisplayKind::FileRead => {
                format!("Read {} ({} lines)", self.arg_summary, total_lines)
            }
            ToolDisplayKind::Command => format!("Command output ({} lines)", total_lines),
            ToolDisplayKind::Generic => format!("{} output", self.tool_name),
        }
    }
}

fn inset_content_rect(area: Rect) -> Rect {
    Rect::new(
        area.x + 2,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    )
}

impl Widget for ToolWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let output = self.build();
        let meta = build_meta_line(
            output.phase,
            output.permission_label.as_deref(),
            output.size_bytes,
            output.duration_us,
            output.error_message.as_deref(),
            RUNNING_SPINNER[0],
            self.theme,
            self.msgs,
        );

        if !output.layout.has_detail_card {
            let inner = inset_content_rect(area);
            Paragraph::new(vec![output.title_line, meta])
                .style(Style::default().fg(self.theme.fg).bg(self.theme.bg))
                .render(inner, buf);
            return;
        }

        let title = output.detail_title.unwrap_or_default();
        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(self.theme.block_border_type())
            .border_style(Style::default().fg(self.theme.accent))
            .style(Style::default().bg(self.theme.bg))
            .title(title)
            .title_bottom(Line::from(Span::styled(
                self.msgs.diff_card_bottom,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::ITALIC),
            )));

        if area.height <= TOOL_HEADER_ROWS as u16 {
            let inner = inset_content_rect(area);
            Paragraph::new(vec![output.title_line, meta]).render(inner, buf);
            return;
        }

        let title_area = Rect::new(area.x + 2, area.y, area.width.saturating_sub(2), 1);
        output.title_line.render(title_area, buf);
        let meta_area = Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(2), 1);
        meta.render(meta_area, buf);

        let card_area = Rect::new(
            area.x + 2,
            area.y + TOOL_HEADER_ROWS as u16,
            area.width.saturating_sub(2),
            area.height.saturating_sub(TOOL_HEADER_ROWS as u16),
        );
        if card_area.height < 3 {
            return;
        }

        card_block.render(card_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::{i18n::Language, theme::ThemeName};

    fn fixture() -> (Theme, Messages) {
        let theme_name = ThemeName::from_str("retro").unwrap();
        (
            Theme::from(theme_name),
            Messages::by_language(Language::English),
        )
    }

    #[test]
    fn title_for_bash_shows_command() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("bash")
            .with_arg_summary("echo hello")
            .with_phase(ToolPhase::Running);

        assert_eq!(widget.title_text(), "bash (echo hello)");
    }

    #[test]
    fn meta_running_includes_spinner_and_zero_ms() {
        let (_theme, msgs) = fixture();
        let text = build_meta_text(
            ToolPhase::Running,
            None,
            None,
            Some(0),
            None,
            '⠋',
            msgs.tool_phase_running,
            msgs.tool_phase_success,
            msgs.tool_phase_failed,
            msgs.tool_meta_sep,
            msgs.step_success_prefix,
            msgs.step_fail_prefix,
        );
        assert!(text.contains("Running"));
        assert!(text.contains("0us"));
    }

    #[test]
    fn meta_failed_includes_error_message() {
        let (_theme, msgs) = fixture();
        let text = build_meta_text(
            ToolPhase::Failed,
            None,
            None,
            Some(42),
            Some("Permission denied by user for bash"),
            '⠋',
            msgs.tool_phase_running,
            msgs.tool_phase_success,
            msgs.tool_phase_failed,
            msgs.tool_meta_sep,
            msgs.step_success_prefix,
            msgs.step_fail_prefix,
        );
        assert!(text.contains("Failed"));
        assert!(text.contains("Permission denied"));
        assert!(text.contains("42us"));
    }

    #[test]
    fn widget_stores_error_message() {
        let (theme, msgs) = fixture();
        let output = ToolWidget::new(&theme, &msgs)
            .with_tool("bash")
            .with_phase(ToolPhase::Failed)
            .with_message("hook blocked execution")
            .build();
        assert_eq!(
            output.error_message.as_deref(),
            Some("hook blocked execution")
        );
        assert!(output.layout.has_detail_card);
        assert_eq!(output.layout.preview_lines, 1);
        assert_eq!(
            output.detail_preview,
            vec!["hook blocked execution".to_string()]
        );
    }

    #[test]
    fn failed_tool_shows_error_card_with_preview() {
        let (theme, msgs) = fixture();
        let error = "BatchEdit aborted — 1 validation error(s):\nEdit 0: old_string not found";
        let output = ToolWidget::new(&theme, &msgs)
            .with_tool("batch_edit")
            .with_phase(ToolPhase::Failed)
            .with_detail(error)
            .build();
        assert!(output.layout.has_detail_card);
        assert_eq!(output.layout.preview_lines, 2);
        assert_eq!(output.detail_preview.len(), 2);
        assert!(output.card_bottom.contains("error"));
    }

    #[test]
    fn write_file_builds_detail_card_layout() {
        let (theme, msgs) = fixture();
        let detail = (0..15)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("write_file")
            .with_arg_summary("a.rs")
            .with_phase(ToolPhase::Success)
            .with_detail(detail);

        let output = widget.build();
        assert!(output.layout.has_detail_card);
        assert!(output.use_diff_gutter);
        assert_eq!(output.layout.preview_lines, DEFAULT_PREVIEW_LINES);
        assert_eq!(
            output.layout.visual_rows,
            tool_visual_rows(true, DEFAULT_PREVIEW_LINES, 15, false)
        );
    }

    #[test]
    fn read_file_has_plain_gutter() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("read_file")
            .with_arg_summary("Cargo.toml")
            .with_phase(ToolPhase::Success)
            .with_detail("[package]\n");

        let output = widget.build();
        assert!(output.layout.has_detail_card);
        assert!(!output.use_diff_gutter);
    }

    #[test]
    fn from_step_result_failed_keeps_detail_only() {
        let (theme, msgs) = fixture();
        let result = StepResult {
            tool: "batch_edit".to_string(),
            arg_summary: r#"{"edits":[]}"#.to_string(),
            arg_full: None,
            status: StepStatus::Failed,
            message: "truncated summary".to_string(),
            detail: Some("full error\nline two".to_string()),
            duration_us: Some(1_940),
            permission_label: Some("Always allow this tool".to_string()),
        };
        let output = ToolWidget::from_step_result(&result, &theme, &msgs).build();
        assert!(output.error_message.is_none());
        assert!(output.layout.has_detail_card);
        assert_eq!(output.detail_full.as_deref(), Some("full error\nline two"));
    }

    #[test]
    fn from_step_result_maps_permission_and_duration() {
        let (theme, msgs) = fixture();
        let result = StepResult {
            tool: "bash".to_string(),
            arg_summary: "sleep 1".to_string(),
            arg_full: Some("sleep 1".to_string()),
            status: StepStatus::Success,
            message: "ok".to_string(),
            detail: Some("done\n".to_string()),
            duration_us: Some(1_200_000),
            permission_label: Some("Always allow this tool".to_string()),
        };
        let widget = ToolWidget::from_step_result(&result, &theme, &msgs);
        let output = widget.build();

        assert_eq!(output.duration_us, Some(1_200_000));
        assert_eq!(
            output.permission_label.as_deref(),
            Some("Always allow this tool")
        );
        assert!(output.layout.has_detail_card);
    }

    #[test]
    fn write_file_builds_detail_card_with_diff_gutter() {
        let (theme, msgs) = fixture();
        let detail = "new line one\nnew line two".to_string();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("write_file")
            .with_arg_summary("src/lib.rs")
            .with_phase(ToolPhase::Success)
            .with_detail(detail);

        let output = widget.build();
        assert!(output.layout.has_detail_card);
        assert!(output.use_diff_gutter);
        assert!(
            output
                .detail_title
                .as_deref()
                .unwrap()
                .contains("src/lib.rs")
        );
        assert_eq!(output.detail_preview, vec!["new line one".to_string()]);
    }

    #[test]
    fn header_only_layout_is_two_rows() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("grep")
            .with_arg_summary(r#"{"pattern":"foo"}"#)
            .with_phase(ToolPhase::Success)
            .with_duration_us(7_000);

        assert_eq!(widget.layout().visual_rows, TOOL_HEADER_ROWS);
    }
}
