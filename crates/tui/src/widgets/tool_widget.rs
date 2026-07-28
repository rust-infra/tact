use std::time::Instant;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use tact_protocol::{
    StepResult, StepStatus, TokenUsageInfo, ToolOutputBuffer, ToolOutputLine, ToolOutputSpan,
    ToolOutputStream, ToolPresentationInfo,
};

use crate::{i18n::Messages, theme::Theme};

const DEFAULT_MAX_DETAIL_LINES: usize = 200;
const DEFAULT_PREVIEW_LINES: usize = 1;
const ERROR_PREVIEW_LINES: usize = 5;
const LIVE_OUTPUT_PREVIEW_LINES: usize = 3;
const SUBAGENT_LIVE_OUTPUT_PREVIEW_LINES: usize = 8;
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

/// Visual strategy inferred from the tool name (legacy — replaced by ToolVisualKind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ToolDisplayKind {
    FileWrite,
    FileRead,
    FileEdit,
    Command,
    Task,
    Subagent,
    Sleep,
    Generic,
}

fn kind_from_presentation(
    presentation: &ToolPresentationInfo,
    tool_name: &str,
) -> tact_protocol::ToolVisualKind {
    if presentation.visual_kind != tact_protocol::ToolVisualKind::Generic {
        return presentation.visual_kind;
    }
    // Fallback: infer from tool name for backward compatibility (tests, MCP, etc.)
    match tool_name {
        "write_file" => tact_protocol::ToolVisualKind::FileWrite,
        "read_file" => tact_protocol::ToolVisualKind::FileRead,
        "edit_file" => tact_protocol::ToolVisualKind::FileEdit,
        "bash" | "shell" | "background_run" | "worktree_run" => {
            tact_protocol::ToolVisualKind::Command
        }
        "task_create" | "task_update" | "task_get" | "task_list" => {
            tact_protocol::ToolVisualKind::Task
        }
        "spawn_subagent" => tact_protocol::ToolVisualKind::Subagent,
        "sleep" => tact_protocol::ToolVisualKind::Sleep,
        _ => tact_protocol::ToolVisualKind::Generic,
    }
}

fn display_name_from_presentation(presentation: &ToolPresentationInfo, tool_name: &str) -> String {
    // Prefer an explicit presentation label from native tool metadata.
    // Tests / MCP often use ToolPresentationInfo::generic(tool_name) where
    // display_name == tool_name; fall back to the legacy pretty-name map then.
    if !presentation.display_name.is_empty() && presentation.display_name != tool_name {
        return presentation.display_name.clone();
    }
    tool_display_name(tool_name)
}
pub fn tool_display_name(tool: &str) -> String {
    // Legacy — use ToolPresentationInfo::display_name from protocol instead.
    // Provides a fallback for MCP/unknown tools that arrive without presentation.
    match tool {
        "write_file" => "✍️ Write".to_string(),
        "read_file" => "📖 Read".to_string(),
        "edit_file" => "✏️ Edit".to_string(),
        "bash" | "shell" => "$ Bash".to_string(),
        "run_command" => "Command".to_string(),
        "spawn_subagent" => "🤖 Subagent".to_string(),
        "ask_user" => "❓ Ask".to_string(),
        "sleep" => "⏳ Sleep".to_string(),
        "background_run" => "$ Bg".to_string(),
        "check_background" => "🔍 Check".to_string(),
        "cron_create" => "⏰ Cron Create".to_string(),
        "cron_delete" => "⏰ Cron Delete".to_string(),
        "cron_list" => "⏰ Cron List".to_string(),
        "load_skill" => "Skill".to_string(),
        "save_memory" => "Memory".to_string(),
        "compact" => "Compact".to_string(),
        "spawn_teammate" => "👥 Team Spawn".to_string(),
        "list_teammates" => "👥 Team List".to_string(),
        "send_message" => "✉️ Msg".to_string(),
        "broadcast" => "📢 Broadcast".to_string(),
        "read_inbox" => "📬 Inbox".to_string(),
        "plan_approval" => "✅ Approve".to_string(),
        "shutdown_request" => "🔌 Shutdown Request".to_string(),
        "shutdown_response" => "🔌 Shutdown Response".to_string(),
        "worktree_create" => "🌿 Worktree Create".to_string(),
        "worktree_list" => "🌿 Worktree List".to_string(),
        "worktree_status" => "🌿 Worktree Status".to_string(),
        "worktree_run" => "$ Wt Run".to_string(),
        "worktree_events" => "🌿 Worktree Events".to_string(),
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

fn sleep_duration(ms: u64) -> String {
    if ms == 0 {
        return "0ms".to_string();
    }
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        let secs = ms as f64 / 1000.0;
        // Strip trailing ".0" for whole-second durations.
        if secs.fract() == 0.0 {
            return format!("{}s", secs as u64);
        }
        return format!("{:.1}s", secs);
    }
    let total_secs = ms / 1000;
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    if seconds == 0 {
        format!("{}m", minutes)
    } else {
        format!("{}m {}s", minutes, seconds)
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

/// Map ask_user tool messages onto a short meta-row label (aligned with Success).
fn compact_ask_user_meta(message: &str) -> Option<String> {
    let msg = message.trim();
    if msg.is_empty() {
        return None;
    }
    const MAX: usize = 60;
    let label = if let Some(rest) = msg.strip_prefix("User selected: ") {
        format!("Selected: {rest}")
    } else if msg.starts_with("User cancelled") {
        "Cancelled".to_string()
    } else if msg.starts_with("Question shown") {
        // Free-text ask — keep meta clean; full text is for the model.
        return None;
    } else {
        msg.to_string()
    };
    Some(if label.chars().count() <= MAX {
        label
    } else {
        format!("{}…", label.chars().take(MAX - 1).collect::<String>())
    })
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
    pub detail_preview: Vec<ToolOutputLine>,
    pub detail_total_lines: usize,
    /// Full detail text for popup display (preview may be truncated).
    pub detail_full: Option<String>,
    pub card_bottom: String,
    /// Subagent model name for tool-card header display.
    pub subagent_model: Option<String>,
    /// Subagent token usage for tool-card header display.
    pub subagent_tokens: Option<TokenUsageInfo>,
    /// Tool visual kind from presentation metadata.
    pub visual_kind: tact_protocol::ToolVisualKind,
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
    detail_lines: Option<Vec<ToolOutputLine>>,
    detail_total_lines: Option<usize>,
    live_detail: bool,
    subagent_model: Option<String>,
    subagent_tokens: Option<TokenUsageInfo>,
    presentation: ToolPresentationInfo,
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
            detail_lines: None,
            detail_total_lines: None,
            live_detail: false,
            subagent_model: None,
            subagent_tokens: None,
            presentation: ToolPresentationInfo::generic(""),
        }
    }

    #[allow(dead_code)]
    pub fn with_presentation(mut self, presentation: ToolPresentationInfo) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn with_subagent_model(mut self, model: Option<String>) -> Self {
        self.subagent_model = model;
        self
    }

    pub fn with_subagent_tokens(mut self, tokens: Option<TokenUsageInfo>) -> Self {
        self.subagent_tokens = tokens;
        self
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

    pub fn with_live_output(mut self, output: &ToolOutputBuffer) -> Self {
        let preview_cap = if self.tool_name == "spawn_subagent" {
            SUBAGENT_LIVE_OUTPUT_PREVIEW_LINES
        } else {
            LIVE_OUTPUT_PREVIEW_LINES
        };
        let lines = output.preview_lines(preview_cap);
        // Popup/detail_full keep `$ <command>` for consistency with completed
        // cards, but the live title/footer/line numbers must count only the
        // streamed output — the preview itself never includes that prefix.
        let detail = command_detail(
            kind_from_presentation(&self.presentation, &self.tool_name),
            &self.arg_full,
            &output.detail_text(),
        );
        self.detail = Some(detail);
        self.detail_lines = Some(lines);
        self.detail_total_lines = Some(output.logical_line_count());
        self.preview_lines = preview_cap;
        self.live_detail = true;
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
        // ask_user answers compress onto the meta row (same slot as permission labels),
        // not a separate detail card line.
        let ask_user_label = (!failed
            && (result.presentation.compact_result_to_meta || result.tool == "ask_user"))
            .then(|| compact_ask_user_meta(&result.message))
            .flatten();
        let arg_full = result
            .arg_full
            .clone()
            .unwrap_or_else(|| result.arg_summary.clone());
        let detail = result.detail.clone().or_else(|| {
            if failed && !result.message.is_empty() {
                Some(result.message.clone())
            } else {
                None
            }
        });
        let detail = detail.map(|detail| {
            if failed {
                detail
            } else {
                command_detail(
                    kind_from_presentation(&result.presentation, &result.tool),
                    &arg_full,
                    &detail,
                )
            }
        });
        let permission_label = match (result.permission_label.clone(), ask_user_label) {
            // Permission Ask then ask_user choice — keep both on the meta row.
            (Some(perm), Some(choice)) => Some(format!("{perm} · {choice}")),
            (Some(perm), None) => Some(perm),
            (None, Some(choice)) => Some(choice),
            (None, None) => None,
        };
        Self {
            tool_name: result.tool.clone(),
            arg_summary: result.arg_summary.clone(),
            arg_full,
            step_index: None,
            phase: ToolPhase::from_status(&result.status),
            detail,
            duration_us: result.duration_us,
            permission_label,
            error_message: None,
            theme,
            msgs,
            max_detail_lines: DEFAULT_MAX_DETAIL_LINES,
            preview_lines: DEFAULT_PREVIEW_LINES,
            detail_lines: None,
            detail_total_lines: None,
            live_detail: false,
            subagent_model: None,
            subagent_tokens: None,
            presentation: result.presentation.clone(),
        }
    }

    pub fn title_text(&self) -> String {
        let base = match kind_from_presentation(&self.presentation, &self.tool_name) {
            tact_protocol::ToolVisualKind::Command => {
                let label = display_name_from_presentation(&self.presentation, &self.tool_name);
                if self.arg_summary.is_empty() {
                    label
                } else {
                    format!("{label}  {}", self.arg_summary)
                }
            }
            tact_protocol::ToolVisualKind::Subagent => {
                if self.arg_summary.is_empty() {
                    "Subagent".to_string()
                } else {
                    format!("Subagent · {}", self.arg_summary)
                }
            }
            tact_protocol::ToolVisualKind::Sleep => {
                if let Ok(ms) = self.arg_summary.parse::<u64>() {
                    format!("⏳ Sleep · {}", sleep_duration(ms))
                } else if self.arg_summary.is_empty() {
                    "⏳ Sleep".to_string()
                } else {
                    format!("⏳ Sleep · {}", self.arg_summary)
                }
            }
            tact_protocol::ToolVisualKind::Task => {
                // Human title already includes "# Task.N · …"; do not prefix tool name.
                if self.arg_summary.is_empty() {
                    display_name_from_presentation(&self.presentation, &self.tool_name)
                } else {
                    self.arg_summary.clone()
                }
            }
            _ => {
                let label = display_name_from_presentation(&self.presentation, &self.tool_name);
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
        match kind_from_presentation(&self.presentation, &self.tool_name) {
            tact_protocol::ToolVisualKind::FileWrite
            | tact_protocol::ToolVisualKind::FileRead
            | tact_protocol::ToolVisualKind::FileEdit => {
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

        let total_lines = self
            .detail_total_lines
            .unwrap_or_else(|| detail.lines().count());
        let preview_cap = if matches!(self.phase, ToolPhase::Failed) {
            ERROR_PREVIEW_LINES
        } else {
            self.preview_lines
        };
        let preview_count = self
            .detail_lines
            .as_ref()
            .map_or_else(|| total_lines.min(preview_cap), Vec::len);
        ToolLayout {
            visual_rows: tool_visual_rows(true, preview_count, total_lines, false),
            preview_lines: preview_count,
            has_detail_card: true,
        }
    }

    pub fn build(&self) -> ToolRenderOutput {
        let layout = self.layout();
        let use_diff_gutter = matches!(
            kind_from_presentation(&self.presentation, &self.tool_name),
            tact_protocol::ToolVisualKind::FileWrite | tact_protocol::ToolVisualKind::FileEdit
        );
        let (detail_title, detail_preview, detail_total_lines) = if layout.has_detail_card {
            let detail = self.display_detail().unwrap_or_default();
            let lines: Vec<ToolOutputLine> = self.detail_lines.clone().unwrap_or_else(|| {
                detail
                    .lines()
                    .take(self.max_detail_lines)
                    .map(|line| ToolOutputLine {
                        spans: vec![ToolOutputSpan {
                            stream: ToolOutputStream::Other,
                            text: line.to_string(),
                        }],
                    })
                    .collect()
            });
            let total = self
                .detail_total_lines
                .unwrap_or_else(|| detail.lines().count());
            let preview = if matches!(
                kind_from_presentation(&self.presentation, &self.tool_name),
                tact_protocol::ToolVisualKind::Command
            ) {
                let mut tail: Vec<_> = lines
                    .iter()
                    .rev()
                    .take(layout.preview_lines)
                    .cloned()
                    .collect();
                tail.reverse();
                tail
            } else {
                lines.iter().take(layout.preview_lines).cloned().collect()
            };
            (Some(self.detail_card_title(total)), preview, total)
        } else {
            (None, Vec::new(), 0)
        };

        let title_raw = self.title_text();
        let has_detail_card = layout.has_detail_card;
        let card_bottom = if self.live_detail {
            self.msgs.tool_live_output_bottom.to_string()
        } else if matches!(self.phase, ToolPhase::Failed) {
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
            subagent_model: self.subagent_model.clone(),
            subagent_tokens: self.subagent_tokens.clone(),
            visual_kind: kind_from_presentation(&self.presentation, &self.tool_name),
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
        if self.live_detail {
            return matches!(
                kind_from_presentation(&self.presentation, &self.tool_name),
                tact_protocol::ToolVisualKind::Command | tact_protocol::ToolVisualKind::Subagent
            ) && matches!(self.phase, ToolPhase::Running);
        }
        matches!(
            kind_from_presentation(&self.presentation, &self.tool_name),
            tact_protocol::ToolVisualKind::FileWrite
                | tact_protocol::ToolVisualKind::FileRead
                | tact_protocol::ToolVisualKind::FileEdit
                | tact_protocol::ToolVisualKind::Command
                | tact_protocol::ToolVisualKind::Subagent
        ) && matches!(self.phase, ToolPhase::Success)
    }

    fn detail_card_title(&self, total_lines: usize) -> String {
        if self.live_detail {
            return self
                .msgs
                .tool_live_output_title_tmpl
                .replace("{}", &total_lines.to_string());
        }
        if matches!(self.phase, ToolPhase::Failed) {
            return self.msgs.tool_error_card_title.to_string();
        }
        if matches!(
            kind_from_presentation(&self.presentation, &self.tool_name),
            tact_protocol::ToolVisualKind::Subagent
        ) {
            return format!("Summary ({} lines)", total_lines);
        }
        match kind_from_presentation(&self.presentation, &self.tool_name) {
            tact_protocol::ToolVisualKind::FileWrite | tact_protocol::ToolVisualKind::FileEdit => {
                self.msgs
                    .diff_card_title
                    .replacen("{}", &total_lines.to_string(), 1)
                    .replacen("{}", &self.arg_summary, 1)
            }
            tact_protocol::ToolVisualKind::FileRead => {
                format!("Read {} ({} lines)", self.arg_summary, total_lines)
            }
            tact_protocol::ToolVisualKind::Command => {
                format!("Command output ({} lines)", total_lines)
            }
            tact_protocol::ToolVisualKind::Task
            | tact_protocol::ToolVisualKind::Generic
            | tact_protocol::ToolVisualKind::Sleep
            | tact_protocol::ToolVisualKind::Subagent => {
                format!("{} output", self.tool_name)
            }
        }
    }
}

fn command_detail(
    visual_kind: tact_protocol::ToolVisualKind,
    full_arg: &str,
    detail: &str,
) -> String {
    if !matches!(visual_kind, tact_protocol::ToolVisualKind::Command) || full_arg.is_empty() {
        return detail.to_string();
    }
    format!("$ {full_arg}\n\n{detail}")
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

        assert_eq!(widget.title_text(), "$ Bash  echo hello");
    }

    #[test]
    fn running_bash_live_output_uses_available_lines_up_to_three() {
        let (theme, msgs) = fixture();
        let mut live = tact_protocol::ToolOutputBuffer::new(50_000);
        live.push_chunks(&[
            tact_protocol::ToolOutputChunk::stdout("building\n"),
            tact_protocol::ToolOutputChunk::stderr("warning\n"),
        ]);

        let output = ToolWidget::new(&theme, &msgs)
            .with_tool("bash")
            .with_phase(ToolPhase::Running)
            .with_live_output(&live)
            .build();

        assert!(output.layout.has_detail_card);
        assert_eq!(output.detail_preview.len(), 2);
        assert!(
            output
                .detail_title
                .as_deref()
                .unwrap()
                .contains("Live output")
        );
        assert_eq!(
            output.detail_preview[1].spans[0].stream,
            tact_protocol::ToolOutputStream::Stderr
        );
    }

    #[test]
    fn live_output_total_excludes_command_prefix_but_popup_keeps_it() {
        let (theme, msgs) = fixture();
        let mut live = tact_protocol::ToolOutputBuffer::new(50_000);
        live.push_chunks(&[tact_protocol::ToolOutputChunk::stdout(
            "[feat/sdk abc] chore: cargo fmt\n6 files changed, 23 insertions(+), 19 deletions(-)\n",
        )]);

        let output = ToolWidget::new(&theme, &msgs)
            .with_tool("bash")
            .with_arg_full("git commit -m \"chore: cargo fmt\"")
            .with_phase(ToolPhase::Running)
            .with_live_output(&live)
            .build();

        assert_eq!(
            output.detail_total_lines, 2,
            "live card count must match streamed output lines, not $ command prefix"
        );
        assert_eq!(output.detail_preview.len(), 2);
        assert_eq!(
            output.detail_title.as_deref(),
            Some("Live output (2 lines)")
        );
        assert_eq!(
            output.detail_full.as_deref(),
            Some(
                "$ git commit -m \"chore: cargo fmt\"\n\n[feat/sdk abc] chore: cargo fmt\n6 files changed, 23 insertions(+), 19 deletions(-)\n"
            )
        );
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
            output.detail_preview[0].plain_text(),
            "hook blocked execution"
        );
    }

    #[test]
    fn failed_tool_shows_error_card_with_preview() {
        let (theme, msgs) = fixture();
        let error = "Permission denied by user for edit_file";
        let output = ToolWidget::new(&theme, &msgs)
            .with_tool("edit_file")
            .with_phase(ToolPhase::Failed)
            .with_detail(error)
            .build();
        assert!(output.layout.has_detail_card);
        assert_eq!(output.layout.preview_lines, 1);
        assert_eq!(output.detail_preview.len(), 1);
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
            tool: "edit_file".to_string(),
            arg_summary: "src/lib.rs".to_string(),
            arg_full: None,
            status: StepStatus::Failed,
            message: "truncated summary".to_string(),
            detail: Some("full error\nline two".to_string()),
            duration_us: Some(1_940),
            permission_label: Some("Always allow this tool".to_string()),
            presentation: ToolPresentationInfo::generic("edit_file"),
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
            presentation: ToolPresentationInfo::generic("bash"),
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
    fn ask_user_selection_compresses_onto_meta_row() {
        let (theme, msgs) = fixture();
        let result = StepResult {
            tool: "ask_user".to_string(),
            arg_summary: "Pick one".to_string(),
            arg_full: Some("Pick one".to_string()),
            status: StepStatus::Success,
            message: "User selected: C. Ø ( 不加冠词 )".to_string(),
            detail: None,
            duration_us: Some(12_370_000),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("ask_user"),
        };
        let widget = ToolWidget::from_step_result(&result, &theme, &msgs);
        let output = widget.build();

        assert_eq!(
            output.permission_label.as_deref(),
            Some("Selected: C. Ø ( 不加冠词 )")
        );
        assert!(
            !output.layout.has_detail_card,
            "selection must not open a detail card"
        );
        let meta = build_meta_text(
            output.phase,
            output.permission_label.as_deref(),
            output.size_bytes,
            output.duration_us,
            None,
            ' ',
            msgs.tool_phase_running,
            msgs.tool_phase_success,
            msgs.tool_phase_failed,
            msgs.tool_meta_sep,
            msgs.step_success_prefix,
            msgs.step_fail_prefix,
        );
        assert!(meta.contains("Selected: C. Ø"));
        assert!(meta.contains(msgs.tool_phase_success) || meta.contains("Success"));
    }

    #[test]
    fn ask_user_keeps_permission_label_and_selection() {
        let (theme, msgs) = fixture();
        let result = StepResult {
            tool: "ask_user".to_string(),
            arg_summary: "Pick one".to_string(),
            arg_full: None,
            status: StepStatus::Success,
            message: "User selected: B. the".to_string(),
            detail: None,
            duration_us: Some(1_000),
            permission_label: Some("Allow once".to_string()),
            presentation: ToolPresentationInfo::generic("ask_user"),
        };
        let output = ToolWidget::from_step_result(&result, &theme, &msgs).build();
        assert_eq!(
            output.permission_label.as_deref(),
            Some("Allow once · Selected: B. the")
        );
    }

    #[test]
    fn edit_file_builds_detail_card_with_diff_gutter() {
        let (theme, msgs) = fixture();
        let detail = "new line one\nnew line two".to_string();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("edit_file")
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
        assert_eq!(output.detail_preview[0].plain_text(), "new line one");
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

    #[test]
    fn sleep_title_formats_duration() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("sleep")
            .with_arg_summary("5000")
            .with_phase(ToolPhase::Success)
            .with_duration_us(5_000_000);
        assert_eq!(widget.title_text(), "⏳ Sleep · 5s");
    }

    #[test]
    fn sleep_zero_ms_shows_zero() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("sleep")
            .with_arg_summary("0")
            .with_phase(ToolPhase::Success)
            .with_duration_us(1);
        assert_eq!(widget.title_text(), "⏳ Sleep · 0ms");
    }

    #[test]
    fn sleep_minutes_format() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("sleep")
            .with_arg_summary("125000")
            .with_phase(ToolPhase::Success)
            .with_duration_us(125_000_000);
        assert_eq!(widget.title_text(), "⏳ Sleep · 2m 5s");
    }

    #[test]
    fn sleep_exact_minute_drops_zero_seconds() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("sleep")
            .with_arg_summary("60000")
            .with_phase(ToolPhase::Success)
            .with_duration_us(60_000_000);
        assert_eq!(widget.title_text(), "⏳ Sleep · 1m");
    }

    #[test]
    fn sleep_fractional_second_keeps_decimal() {
        let (theme, msgs) = fixture();
        let widget = ToolWidget::new(&theme, &msgs)
            .with_tool("sleep")
            .with_arg_summary("1500")
            .with_phase(ToolPhase::Success)
            .with_duration_us(1_500_000);
        assert_eq!(widget.title_text(), "⏳ Sleep · 1.5s");
    }
}
