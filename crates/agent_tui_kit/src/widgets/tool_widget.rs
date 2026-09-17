use std::{ops::Range, time::Instant};

use tact_protocol::{
    StepResult, StepStatus, TokenUsageInfo, ToolOutputBuffer, ToolOutputLine, ToolOutputSpan,
    ToolOutputStream, ToolPresentationInfo,
};
use unicode_width::UnicodeWidthStr;

use crate::render::{bar::format_tokens_compact, util::LOG_TOOL_BLOCK_INDENT};

use crate::i18n::Messages;

const DEFAULT_MAX_DETAIL_LINES: usize = 200;
const DEFAULT_PREVIEW_LINES: usize = 1;
const ERROR_PREVIEW_LINES: usize = 5;
const LIVE_OUTPUT_PREVIEW_LINES: usize = 3;
const SUBAGENT_LIVE_OUTPUT_PREVIEW_LINES: usize = 8;
pub const TOOL_HEADER_ROWS: usize = 2;
/// Row index of the meta line inside a tool block's header rows (the title row
/// is `0`). A collapsed command's click target lives here.
pub const TOOL_META_ROW: usize = TOOL_HEADER_ROWS - 1;

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
        // Hosted web search renders like a command output card so its
        // sources detail (URL list) is expandable.
        "web_search" => tact_protocol::ToolVisualKind::Command,
        "task_create" | "task_update" | "task_get" | "task_list" => {
            tact_protocol::ToolVisualKind::Task
        }
        "spawn_subagent" => tact_protocol::ToolVisualKind::Subagent,
        "sleep" | "wait_background" => tact_protocol::ToolVisualKind::Sleep,
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
        "write_file" => "📝 Write".to_string(),
        "read_file" => "📖 Read".to_string(),
        "edit_file" => "✏️ Edit".to_string(),
        "apply_patch" => "📝 Patch".to_string(),
        "bash" | "shell" => "$ Bash".to_string(),
        "web_search" => "🔍 Web Search".to_string(),
        "run_command" => "Command".to_string(),
        "spawn_subagent" => "🤖 Subagent".to_string(),
        "ask_user" => "❓ Ask".to_string(),
        "sleep" => "💤 Sleep".to_string(),
        "background_run" => "⚙️ Background Run".to_string(),
        "check_background" => "⚙️ Background Check".to_string(),
        "wait_background" => "⏳ Wait Background".to_string(),
        "load_skill" => "📚 Skill".to_string(),
        "save_memory" => "🧠 Memory".to_string(),
        "compact" => "📦 Compact".to_string(),
        "spawn_teammate" => "👥 Team Spawn".to_string(),
        "list_teammates" => "👥 Team List".to_string(),
        "send_message" => "✉️ Team Send".to_string(),
        "broadcast" => "📢 Team Broadcast".to_string(),
        "read_inbox" => "📬 Team Inbox".to_string(),
        "plan_approval" => "✅ Team Approve".to_string(),
        "shutdown_request" => "🔌 Shutdown Request".to_string(),
        "shutdown_response" => "🔌 Shutdown Response".to_string(),
        "worktree_create" => "🌿 Worktree Create".to_string(),
        "worktree_list" => "🌿 Worktree List".to_string(),
        "worktree_status" => "🌿 Worktree Status".to_string(),
        "worktree_run" => "🌿 Worktree Run".to_string(),
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

/// Meta-row error snippet for a finished tool.
///
/// Hidden when the detail card already carries the error, so the same line is
/// not printed twice. Shared by the widget (which stores the row's text) and the
/// cell (which draws it).
pub fn meta_error(
    phase: ToolPhase,
    has_detail_card: bool,
    error_message: Option<&str>,
) -> Option<&str> {
    error_message.filter(|_| !(has_detail_card && matches!(phase, ToolPhase::Failed)))
}

/// Meta-row hint telling the user that a collapsed command hid its output:
/// `tool_collapsed_output_hint` with the popup's own line count filled in.
pub fn collapsed_output_hint(msgs: &Messages, total_lines: usize) -> String {
    if total_lines == 1 {
        msgs.tool_collapsed_output_hint_one.to_string()
    } else {
        msgs.tool_collapsed_output_hint
            .replacen("{}", &total_lines.to_string(), 1)
    }
}

/// Columns of the clickable action hint at the end of a collapsed command's
/// meta row, measured from the block's own left edge (block indent included).
///
/// The action is the tail of [`collapsed_output_hint`], which is itself the tail
/// of the row the cell draws — so measuring backwards from the row's end lands
/// on the drawn glyphs and cannot drift from them.
pub fn collapsed_action_cols(msgs: &Messages, row_text: &str) -> Option<Range<u16>> {
    let action_width = UnicodeWidthStr::width(msgs.tool_collapsed_output_action);
    let end = LOG_TOOL_BLOCK_INDENT as usize + UnicodeWidthStr::width(row_text);
    if action_width == 0 || end > u16::MAX as usize {
        return None;
    }
    let start = end.checked_sub(action_width)?;
    Some(start as u16..end as u16)
}

/// Trailing labels shared by every meta row: subagent model / token total, then
/// the collapsed-output hint.
///
/// Both the widget — which stores the finished row's exact text so the log hit
/// test knows where that text ends — and the cell render through here, so the
/// clickable text and the drawn text cannot drift apart.
pub fn meta_suffixes(
    sep: &str,
    subagent_model: Option<&str>,
    subagent_token_total: Option<u32>,
    collapsed_hint: Option<&str>,
) -> String {
    let mut text = String::new();
    if let Some(model) = subagent_model {
        text.push_str(sep);
        text.push_str(&format!("🤖 {model}"));
    }
    if let Some(total) = subagent_token_total {
        text.push_str(sep);
        text.push_str(&format!("⚡ {}", format_tokens_compact(total as u64)));
    }
    if let Some(hint) = collapsed_hint {
        text.push_str(sep);
        text.push_str(hint);
    }
    text
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
    /// Whether the full detail exists but is collapsed behind the header rows.
    ///
    /// A finished command drops its output card so the block costs two rows;
    /// the text stays reachable through the detail popup, which makes the
    /// header rows the click target.
    pub detail_collapsed: bool,
}

/// Content rows inside the card borders.
///
/// Overflow text is rendered in the bottom hint (`title_bottom`) so it does not
/// consume an extra preview row.
pub fn tool_card_inner_rows(preview_len: usize, total_lines: usize) -> usize {
    let _ = total_lines;
    preview_len
}

/// Total visual rows for a tool block in the log column.
pub fn tool_visual_rows(
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
///
/// **Nothing here is language-derived.** Every string in this spec is a fact
/// about the call (tool name, arguments, phase, counts, durations); the locale
/// is applied where the block is drawn ([`crate::render::cells::tool::ToolCell`])
/// and where it is hit-tested ([`ToolRenderOutput::hits_collapsed_action`]),
/// both of which read the *current* [`Messages`].
///
/// Storing a rendered row here instead would freeze the locale of the moment the
/// output was built: `/lang` flips the language the cell draws with, so a stored
/// row would stop describing what is on screen — and the click target measured
/// from it would drift off the drawn hint (see `book/26_chapter_issue.md`,
/// 2026-09-14).
#[derive(Debug, Clone)]
pub struct ToolRenderOutput {
    /// Title row text (tool name + arguments); the cell styles it with the live theme.
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
    /// Whether the detail card is the *live output* card of a streaming command
    /// (as opposed to a finished card). Decides the localized card title and
    /// bottom strings, which are derived on demand.
    pub live_detail: bool,
    pub detail_preview: Vec<ToolOutputLine>,
    pub detail_total_lines: usize,
    /// Full detail text for popup display (preview may be truncated).
    pub detail_full: Option<String>,
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

    /// Localized title of the detail card (its top border label), or `None` when
    /// no card is drawn and no popup title is needed.
    ///
    /// Also used as the popup's title, so a card-less block still has one.
    pub fn card_title(&self, msgs: &Messages) -> Option<String> {
        (self.layout.has_detail_card || self.layout.detail_collapsed)
            .then(|| self.card_title_text(msgs))
    }

    fn card_title_text(&self, msgs: &Messages) -> String {
        if self.live_detail {
            return msgs.tool_live_output_title.to_string();
        }
        if matches!(self.phase, ToolPhase::Failed) {
            return msgs.tool_error_card_title.to_string();
        }
        if matches!(self.visual_kind, tact_protocol::ToolVisualKind::Subagent) {
            return format!("Summary ({} lines)", self.detail_total_lines);
        }
        match self.visual_kind {
            tact_protocol::ToolVisualKind::FileWrite | tact_protocol::ToolVisualKind::FileEdit => {
                msgs.diff_card_title
                    .replacen("{}", &self.detail_total_lines.to_string(), 1)
                    .replacen("{}", &self.arg_summary, 1)
            }
            tact_protocol::ToolVisualKind::FileRead => format!("Read {}", self.arg_summary),
            tact_protocol::ToolVisualKind::Command => "Command output".to_string(),
            tact_protocol::ToolVisualKind::Task
            | tact_protocol::ToolVisualKind::Generic
            | tact_protocol::ToolVisualKind::Sleep
            | tact_protocol::ToolVisualKind::Subagent => {
                format!("{} output", self.tool_name)
            }
        }
    }

    /// Localized bottom label of the detail card. The cell prefixes the preview
    /// counts when the card shows less than the whole result.
    pub fn card_bottom(&self, msgs: &Messages) -> String {
        if self.live_detail {
            msgs.tool_live_output_bottom.to_string()
        } else if matches!(self.phase, ToolPhase::Failed) {
            msgs.tool_error_card_bottom.to_string()
        } else {
            msgs.diff_card_bottom.to_string()
        }
    }

    /// Plain text of the meta row, in the given locale.
    ///
    /// `None` while a tool runs, where the row carries a ticking elapsed time
    /// that only the cell can produce.
    ///
    /// This is the row the cell draws (both sides go through
    /// [`build_meta_text`] + [`meta_suffixes`]), so the hit test measures the
    /// text that is actually on screen — see [`Self::collapsed_action_cols`].
    pub fn meta_text(&self, msgs: &Messages) -> Option<String> {
        if matches!(self.phase, ToolPhase::Running) {
            return None;
        }
        let hint = self
            .layout
            .detail_collapsed
            .then(|| collapsed_output_hint(msgs, self.detail_total_lines));
        let mut text = build_meta_text(
            self.phase,
            self.permission_label.as_deref(),
            self.size_bytes,
            self.duration_us,
            meta_error(
                self.phase,
                self.layout.has_detail_card,
                self.error_message.as_deref(),
            ),
            ' ',
            msgs.tool_phase_running,
            msgs.tool_phase_success,
            msgs.tool_phase_failed,
            msgs.tool_meta_sep,
            msgs.step_success_prefix,
            msgs.step_fail_prefix,
        );
        text.push_str(&meta_suffixes(
            msgs.tool_meta_sep,
            self.subagent_model.as_deref(),
            self.subagent_tokens.as_ref().map(|t| t.total),
            hint.as_deref(),
        ));
        Some(text)
    }

    /// Columns of the clickable action hint at the end of a collapsed command's
    /// meta row ([`TOOL_META_ROW`]), measured from the block's own left edge
    /// (indent included). `None` for every other block.
    ///
    /// Measured from the row the *current* locale produces, because that is the
    /// row the cell draws: those glyphs are the whole affordance of a card-less
    /// command, so they have to be the ones on screen.
    pub fn collapsed_action_cols(&self, msgs: &Messages) -> Option<Range<u16>> {
        if !self.layout.detail_collapsed {
            return None;
        }
        let text = self.meta_text(msgs)?;
        collapsed_action_cols(msgs, &text)
    }

    /// Whether a click at (`row`, `col`) — `col` from the block's own left edge —
    /// lands on the collapsed command's expand hint.
    pub fn hits_collapsed_action(&self, row: usize, col: usize, msgs: &Messages) -> bool {
        row == TOOL_META_ROW
            && self
                .collapsed_action_cols(msgs)
                .is_some_and(|cols| cols.contains(&(col as u16)))
    }
}

/// Unified tool invocation renderer.
///
/// The widget turns a tool call into a [`ToolRenderOutput`] — a **spec**, not a
/// picture: it holds no rendered text, no colors, and no locale, which is why it
/// takes neither a [`Theme`] nor [`Messages`]. Both are applied where they belong:
/// [`crate::render::cells::tool::ToolCell`] draws with the live theme and locale,
/// and the log hit test measures the same live row
/// ([`ToolRenderOutput::hits_collapsed_action`]).
pub struct ToolWidget {
    tool_name: String,
    arg_summary: String,
    arg_full: String,
    step_index: Option<usize>,
    phase: ToolPhase,
    detail: Option<String>,
    duration_us: Option<u64>,
    permission_label: Option<String>,
    error_message: Option<String>,
    max_detail_lines: usize,
    preview_lines: usize,
    detail_lines: Option<Vec<ToolOutputLine>>,
    detail_total_lines: Option<usize>,
    live_detail: bool,
    subagent_model: Option<String>,
    subagent_tokens: Option<TokenUsageInfo>,
    presentation: ToolPresentationInfo,
}

impl Default for ToolWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolWidget {
    pub fn new() -> Self {
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

    /// Detail of a command-shaped call, opened by the `$ <command>` line it ran.
    ///
    /// [`Self::from_step_result`] applies that prefix for a finished command, and
    /// [`Self::with_live_output`] for a running one; a keep-live command
    /// (`background_run`) has no `StepResult` to read it from, so its finalize
    /// path uses this instead. Without the prefix the command survives only in
    /// the popup's *title*, which is a single row clipped at the popup width —
    /// while the live card the user was just watching showed it in full.
    ///
    /// A *failed* call is stored raw: its card has to keep the error first (the
    /// failure detail **is** the error report), so the popup's `$ <command>`
    /// line is added by [`Self::build`] instead.
    pub fn with_command_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.detail = Some(if matches!(self.phase, ToolPhase::Failed) {
            detail
        } else {
            command_detail(
                kind_from_presentation(&self.presentation, &self.tool_name),
                &self.arg_full,
                &detail,
            )
        });
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

    pub fn from_step_result(result: &StepResult) -> Self {
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
                // The kind is shared: `sleep` passes its duration as the
                // summary, `wait_background` the id of the task it is waiting
                // on. So the label has to come from the tool's own presentation
                // (💤 Sleep / ⏳ Wait Background) — a label hardcoded here
                // silently overrode `display_name`, which is how a wait rendered
                // as "Sleep". A serialized input is skipped the same way the
                // detail card skips it ([`is_written_argument`]).
                let label = display_name_from_presentation(&self.presentation, &self.tool_name);
                if let Ok(ms) = self.arg_summary.parse::<u64>() {
                    format!("{label} · {}", sleep_duration(ms))
                } else if self.arg_summary.is_empty() || !is_written_argument(&self.arg_summary) {
                    label
                } else {
                    format!("{label} · {}", self.arg_summary)
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
                detail_collapsed: false,
            };
        };
        let detail_lines = self
            .detail_total_lines
            .unwrap_or_else(|| detail.lines().count());
        if self.collapses_detail(detail_lines) {
            return ToolLayout {
                visual_rows: tool_visual_rows(false, 0, 0, false),
                preview_lines: 0,
                has_detail_card: false,
                detail_collapsed: true,
            };
        }
        if !self.should_show_detail(detail) {
            return ToolLayout {
                visual_rows: tool_visual_rows(false, 0, 0, false),
                preview_lines: 0,
                has_detail_card: false,
                detail_collapsed: false,
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
            detail_collapsed: false,
        }
    }

    pub fn build(&self) -> ToolRenderOutput {
        let layout = self.layout();
        let use_diff_gutter = matches!(
            kind_from_presentation(&self.presentation, &self.tool_name),
            tact_protocol::ToolVisualKind::FileWrite | tact_protocol::ToolVisualKind::FileEdit
        );
        let (detail_preview, detail_total_lines) = if layout.has_detail_card {
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
            (preview, total)
        } else if layout.detail_collapsed {
            // No card is drawn, but the popup still needs the text, its line
            // count, and a title for the kinds that have no other one to fall
            // back on (a cardless tool would otherwise be headed by its bare
            // tool name). The count is the popup's own line count — the meta
            // hint prints it, so the two must not disagree; the call's argument
            // line (when there is one) is part of it.
            let total = self.popup_detail().unwrap_or_default().lines().count();
            (Vec::new(), total)
        } else {
            (Vec::new(), 0)
        };

        let title_raw = self.title_text();
        let has_detail_card = layout.has_detail_card;
        let detail_collapsed = layout.detail_collapsed;
        ToolRenderOutput {
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
            live_detail: self.live_detail,
            detail_preview,
            detail_total_lines,
            detail_full: if has_detail_card || detail_collapsed {
                self.popup_detail()
            } else {
                None
            },
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

    /// The text the popup shows (`detail_full`), which is *not* always
    /// [`Self::display_detail`]: the popup opens with the call itself (see
    /// [`Self::argument_line`]) above the result it returned.
    ///
    /// A failed command's card must keep the error first — the failure detail is
    /// the error report, and the card previews its head — so its
    /// `$ <command>` line is added here, where only the popup can see it. Every
    /// other command phase already carries the prefix in its detail (see
    /// [`Self::with_command_detail`]).
    ///
    /// Consequence: the popup can hold up to two more lines than
    /// `detail_preview`, which is what the drawn card shows; nothing on a drawn
    /// card prints a count that would disagree (a collapsed block's hint counts
    /// `detail_full` instead — see [`Self::build`]).
    fn popup_detail(&self) -> Option<String> {
        let detail = self.display_detail()?;
        Some(match self.argument_line() {
            Some(line) => format!("{line}\n\n{detail}"),
            None => detail.to_string(),
        })
    }

    /// The call's own line above the result, or `None` when the popup already
    /// holds it.
    ///
    /// The popup is the only surface that shows a collapsed block's content, so
    /// it opens with the parameters — but each kind carries them differently:
    ///
    /// * `Command` — `$ <command>`. Either it is already baked into the detail
    ///   (success and live, through [`Self::with_command_detail`] /
    ///   [`Self::from_step_result`] / [`Self::with_live_output`]), or the card is
    ///   a failure whose error has to stay first and the line is added here.
    /// * `FileRead` / `FileWrite` / `FileEdit` — the body **is** the call (the
    ///   file, its new content, its diff) and the path is the popup title.
    /// * `Subagent` — the subagent popup prepends `Prompt:` itself, from the same
    ///   `arg_full`; this renderer's popup never opens for it.
    /// * `Task` / `Generic` / `Sleep` — the body is only the result, so the
    ///   argument goes above it. A serialized input object is *not* a parameter a
    ///   human reads: it is one long escaped line that repeats the block's own
    ///   parameter row and pushes the result down, so it stays out.
    fn argument_line(&self) -> Option<String> {
        let arg = self.arg_full.trim();
        if arg.is_empty() {
            return None;
        }
        match kind_from_presentation(&self.presentation, &self.tool_name) {
            tact_protocol::ToolVisualKind::Command => {
                matches!(self.phase, ToolPhase::Failed).then(|| format!("$ {arg}"))
            }
            tact_protocol::ToolVisualKind::FileRead
            | tact_protocol::ToolVisualKind::FileWrite
            | tact_protocol::ToolVisualKind::FileEdit
            | tact_protocol::ToolVisualKind::Subagent => None,
            _ => is_written_argument(arg).then(|| arg.to_string()),
        }
    }

    /// Whether a finished tool's output is hidden behind the header rows.
    ///
    /// Commands, reads, writes and edits used to keep a small preview card; that
    /// still left a card on screen for output nobody asked to read, and the
    /// single retained line was rarely the one that mattered. The card is
    /// dropped entirely instead — the block is its two header rows, and the full
    /// text (command output, the read body, the written content, or the new text
    /// behind the popup's git diff) stays one double-click away. While the tool
    /// runs the live card is unchanged, and failures keep their card so the
    /// error stays visible without a click.
    ///
    /// Keyed on the visual kind, never on tool names:
    ///
    /// * `Command` / `FileRead` / `FileEdit` / `FileWrite` — these drew a card,
    ///   so collapsing **saves** rows; the hint explains where the content went,
    ///   whatever its size.
    /// * `Subagent` — keeps its card: it is the entry point of the transcript
    ///   popup, and the line it retains is the child's result summary.
    /// * every other kind (`Generic` / `Task` / `Sleep`) draws no card at all,
    ///   which left its result *unreachable* rather than merely collapsed — no
    ///   card, no popup, no click target. A multi-line result is a readout worth
    ///   keeping one double-click away, and collapsing it costs no extra row.
    ///   A one-line result is skipped: `sleep` / `save_memory` / `send_message`
    ///   answer with a confirmation the meta row already implies, and
    ///   `· 1 line · double-click-result` on all of them would be chrome that opens
    ///   nothing. A result already surfaced on the meta row
    ///   (`compact_result_to_meta`) is skipped for the same reason — a second
    ///   affordance for the same text is noise, not reach.
    fn collapses_detail(&self, detail_lines: usize) -> bool {
        if self.live_detail
            || !matches!(self.phase, ToolPhase::Success)
            || self.presentation.compact_result_to_meta
        {
            return false;
        }
        match kind_from_presentation(&self.presentation, &self.tool_name) {
            tact_protocol::ToolVisualKind::Command
            | tact_protocol::ToolVisualKind::FileRead
            | tact_protocol::ToolVisualKind::FileEdit
            | tact_protocol::ToolVisualKind::FileWrite => true,
            tact_protocol::ToolVisualKind::Subagent => false,
            _ => detail_lines > 1,
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

/// Whether an argument summary is text someone wrote, rather than the tool's
/// serialized input.
///
/// Only one summary policy dumps the whole input object — `Json`, which is also
/// the fallback for every tool that declares none (MCP/plugin tools included) —
/// and `serde_json` renders an object as `{…}` and an array as `[…]`. Every
/// other policy returns a single field: a command, a path, a question
/// (`ask_user`), a prompt, a task title. The protocol carries the *rendered*
/// summary, not the policy behind it, so the shape of the text is the only
/// signal the renderer has; a dump is skipped because showing it would repeat
/// the block's parameter row as one long escaped line and push the result — the
/// reason the popup exists — further down.
fn is_written_argument(arg: &str) -> bool {
    !arg.starts_with('{') && !arg.starts_with('[')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;

    /// The English string set — what the render path uses while the app is in
    /// its default locale. The widget itself is locale-free, so only the
    /// assertions that render or measure a row need it.
    fn test_msgs() -> Messages {
        Messages::by_language(Language::English)
    }

    #[test]
    fn title_for_bash_shows_command() {
        let widget = ToolWidget::new()
            .with_tool("bash")
            .with_arg_summary("echo hello")
            .with_phase(ToolPhase::Running);

        assert_eq!(widget.title_text(), "$ Bash  echo hello");
    }

    #[test]
    fn web_search_title_and_detail_render_as_command() {
        let widget = ToolWidget::new()
            .with_tool("web_search")
            .with_arg_summary("Rust async best practices")
            .with_phase(ToolPhase::Success)
            .with_detail("https://example.com/a\nhttps://example.com/b");

        assert_eq!(
            widget.title_text(),
            "🔍 Web Search  Rust async best practices"
        );
        let output = widget.build();
        // Sources are kept whole for the popup (Command visual kind) but no
        // card is drawn once the call is done.
        assert!(output.layout.detail_collapsed);
        assert_eq!(
            output.detail_full.as_deref(),
            Some("https://example.com/a\nhttps://example.com/b")
        );
        assert_eq!(output.detail_total_lines, 2);
    }

    #[test]
    fn running_bash_live_output_uses_available_lines_up_to_three() {
        let msgs = test_msgs();
        let mut live = tact_protocol::ToolOutputBuffer::new(50_000);
        live.push_chunks(&[
            tact_protocol::ToolOutputChunk::stdout("building\n"),
            tact_protocol::ToolOutputChunk::stderr("warning\n"),
        ]);

        let output = ToolWidget::new()
            .with_tool("bash")
            .with_phase(ToolPhase::Running)
            .with_live_output(&live)
            .build();

        assert!(output.layout.has_detail_card);
        assert_eq!(output.detail_preview.len(), 2);
        assert!(output.card_title(&msgs).unwrap().contains("Live output"));
        assert_eq!(
            output.detail_preview[1].spans[0].stream,
            tact_protocol::ToolOutputStream::Stderr
        );
    }

    #[test]
    fn live_output_total_excludes_command_prefix_but_popup_keeps_it() {
        let msgs = test_msgs();
        let mut live = tact_protocol::ToolOutputBuffer::new(50_000);
        live.push_chunks(&[tact_protocol::ToolOutputChunk::stdout(
            "[feat/sdk abc] chore: cargo fmt\n6 files changed, 23 insertions(+), 19 deletions(-)\n",
        )]);

        let output = ToolWidget::new()
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
        assert_eq!(output.card_title(&msgs).as_deref(), Some("Live output"));
        assert_eq!(
            output.detail_full.as_deref(),
            Some(
                "$ git commit -m \"chore: cargo fmt\"\n\n[feat/sdk abc] chore: cargo fmt\n6 files changed, 23 insertions(+), 19 deletions(-)\n"
            )
        );
    }

    #[test]
    fn with_command_detail_prepends_the_command_for_command_kinds() {
        let output = ToolWidget::new()
            .with_tool("background_run")
            .with_arg_summary("cargo build")
            .with_arg_full("cargo build --release")
            .with_phase(ToolPhase::Success)
            .with_command_detail("Compiling ...\ndone")
            .build();

        assert_eq!(
            output.detail_full.as_deref(),
            Some("$ cargo build --release\n\nCompiling ...\ndone")
        );
    }

    #[test]
    fn with_command_detail_leaves_non_command_kinds_alone() {
        let output = ToolWidget::new()
            .with_tool("read_file")
            .with_arg_summary("src/lib.rs")
            .with_phase(ToolPhase::Success)
            .with_command_detail("fn main() {}")
            .build();

        assert_eq!(output.detail_full.as_deref(), Some("fn main() {}"));
    }

    #[test]
    fn failed_command_card_stays_error_first_but_its_popup_opens_with_the_command() {
        // The card is what must stay readable without a click, so the error
        // keeps the first preview rows; the popup — the surface that exists to
        // show the whole call — opens with the same `$ <command>` line a
        // successful command does.
        let result = StepResult {
            tool: "bash".to_string(),
            arg_summary: "cargo build".to_string(),
            arg_full: Some("cargo build --release".to_string()),
            status: StepStatus::Failed,
            message: "command failed".to_string(),
            detail: Some("error: linker failed".to_string()),
            duration_us: Some(1_000),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("bash"),
        };
        let output = ToolWidget::from_step_result(&result).build();

        assert!(output.layout.has_detail_card && !output.layout.detail_collapsed);
        assert_eq!(
            output.detail_preview[0].plain_text(),
            "error: linker failed"
        );
        assert_eq!(
            output.detail_full.as_deref(),
            Some("$ cargo build --release\n\nerror: linker failed")
        );
    }

    #[test]
    fn failed_command_detail_is_not_double_prefixed() {
        // The keep-live finalize path goes through `with_command_detail` for
        // both outcomes; a failure must still gain exactly one prefix line.
        let output = ToolWidget::new()
            .with_tool("background_run")
            .with_arg_summary("cargo build")
            .with_arg_full("cargo build --release")
            .with_phase(ToolPhase::Failed)
            .with_command_detail("error: build failed")
            .build();

        assert_eq!(
            output.detail_full.as_deref(),
            Some("$ cargo build --release\n\nerror: build failed")
        );
    }

    #[test]
    fn failed_non_command_keeps_its_raw_detail_in_the_popup() {
        let output = ToolWidget::new()
            .with_tool("read_file")
            .with_arg_summary("src/lib.rs")
            .with_phase(ToolPhase::Failed)
            .with_command_detail("permission denied")
            .build();

        assert_eq!(output.detail_full.as_deref(), Some("permission denied"));
    }

    #[test]
    fn task_popup_opens_with_the_task_title() {
        let widget = ToolWidget::new()
            .with_tool("task_create")
            .with_arg_summary("# Task.1 · fix the popup")
            .with_arg_full("# Task.1 · fix the popup")
            .with_phase(ToolPhase::Success)
            .with_detail("created task 1\nsubject: fix the popup");

        let output = widget.build();

        assert!(output.layout.detail_collapsed, "a task result collapses");
        assert_eq!(
            output.detail_full.as_deref(),
            Some("# Task.1 · fix the popup\n\ncreated task 1\nsubject: fix the popup")
        );
        // The collapsed hint prints this count, so it must match the popup.
        assert_eq!(
            output.detail_total_lines,
            output.detail_full.as_deref().unwrap().lines().count()
        );
    }

    #[test]
    fn ask_user_popup_opens_with_the_question() {
        let output = ToolWidget::new()
            .with_tool("ask_user")
            .with_arg_summary("Which database should I use?")
            .with_arg_full("Which database should I use?")
            .with_phase(ToolPhase::Success)
            .with_detail("User selected: B\nthe long note")
            .build();

        assert_eq!(
            output.detail_full.as_deref(),
            Some("Which database should I use?\n\nUser selected: B\nthe long note")
        );
    }

    #[test]
    fn json_argument_is_not_repeated_in_the_popup() {
        // A serialized input object is the tool's whole input dumped as one
        // escaped line — it repeats the block's parameter row and would push the
        // result down, so those kinds keep the popup they had.
        for tool in ["save_memory", "load_skill", "mcp__notes__search"] {
            let output = ToolWidget::new()
                .with_tool(tool)
                .with_arg_summary(r#"{"query":"popup params"}"#)
                .with_arg_full(r#"{"query":"popup params","limit":10}"#)
                .with_phase(ToolPhase::Success)
                .with_detail("3 hits\nhit one")
                .build();

            assert_eq!(
                output.detail_full.as_deref(),
                Some("3 hits\nhit one"),
                "{tool} must not repeat its JSON input"
            );
        }
    }

    #[test]
    fn kinds_whose_body_is_the_call_do_not_repeat_it() {
        // The read body / written content / diff *is* the call; the path is the
        // popup title. A subagent's prompt is prepended by the subagent popup
        // itself, from the same `arg_full`.
        for (tool, kind, detail) in [
            (
                "read_file",
                tact_protocol::ToolVisualKind::FileRead,
                "fn main() {}",
            ),
            (
                "write_file",
                tact_protocol::ToolVisualKind::FileWrite,
                "fn main() {}",
            ),
            (
                "edit_file",
                tact_protocol::ToolVisualKind::FileEdit,
                "fn main() {}",
            ),
            (
                "spawn_subagent",
                tact_protocol::ToolVisualKind::Subagent,
                "child summary",
            ),
        ] {
            let mut presentation = ToolPresentationInfo::generic(tool);
            presentation.visual_kind = kind;
            let output = ToolWidget::new()
                .with_tool(tool)
                .with_arg_summary("src/lib.rs")
                .with_arg_full("src/lib.rs")
                .with_phase(ToolPhase::Success)
                .with_detail(detail)
                .build();

            assert_eq!(
                output.detail_full.as_deref(),
                Some(detail),
                "{tool} must not repeat its argument"
            );
        }
    }

    #[test]
    fn failed_task_popup_shows_the_title_before_the_error() {
        let output = ToolWidget::new()
            .with_tool("task_update")
            .with_arg_summary("# Task.7 · ship it")
            .with_arg_full("# Task.7 · ship it")
            .with_phase(ToolPhase::Failed)
            .with_detail("no such task: 7")
            .build();

        assert_eq!(
            output.detail_full.as_deref(),
            Some("# Task.7 · ship it\n\nno such task: 7")
        );
    }

    #[test]
    fn meta_running_includes_spinner_and_zero_ms() {
        let msgs = test_msgs();
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
        let msgs = test_msgs();
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
        let output = ToolWidget::new()
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
        let msgs = test_msgs();
        let error = "Permission denied by user for edit_file";
        let output = ToolWidget::new()
            .with_tool("edit_file")
            .with_phase(ToolPhase::Failed)
            .with_detail(error)
            .build();
        assert!(output.layout.has_detail_card);
        assert_eq!(output.layout.preview_lines, 1);
        assert_eq!(output.detail_preview.len(), 1);
        assert!(output.card_bottom(&msgs).contains("error"));
    }

    /// A finished write collapses like the rest: the written content stays
    /// reachable through the popup (which reads the file from disk), and the
    /// `+` gutter is kept for that popup.
    #[test]
    fn write_file_collapses_its_detail_card() {
        let msgs = test_msgs();
        let detail = (0..15)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let widget = ToolWidget::new()
            .with_tool("write_file")
            .with_arg_summary("a.rs")
            .with_phase(ToolPhase::Success)
            .with_detail(detail.clone());

        let output = widget.build();
        assert!(!output.layout.has_detail_card);
        assert!(output.layout.detail_collapsed);
        assert_eq!(output.visual_rows(false), TOOL_HEADER_ROWS);
        assert!(output.use_diff_gutter, "the popup renders the written text");
        assert_eq!(output.detail_full.as_deref(), Some(detail.as_str()));
        assert_eq!(output.detail_total_lines, 15);
        assert!(
            output
                .meta_text(&msgs)
                .as_deref()
                .unwrap()
                .contains("15 lines"),
            "{:?}",
            output.meta_text(&msgs)
        );
        assert!(
            output.hits_collapsed_action(
                TOOL_META_ROW,
                output.collapsed_action_cols(&msgs).as_ref().unwrap().start as usize,
                &msgs
            ),
            "the hint opens the written content"
        );
    }

    #[test]
    fn read_file_has_plain_gutter() {
        let widget = ToolWidget::new()
            .with_tool("read_file")
            .with_arg_summary("Cargo.toml")
            .with_phase(ToolPhase::Success)
            .with_detail("[package]\n");

        let output = widget.build();
        assert!(output.layout.detail_collapsed);
        assert!(
            !output.use_diff_gutter,
            "the popup that now carries the body keeps the plain gutter"
        );
    }

    /// The log hit test measures the stored meta text, so a finished block must
    /// keep exactly what the cell will draw — and a collapsed command's target
    /// is the `double-click-result` tail of that row, nothing else.
    #[test]
    fn finished_block_meta_row_matches_its_hit_range() {
        let msgs = test_msgs();
        let result = StepResult {
            tool: "bash".to_string(),
            arg_summary: "echo hi".to_string(),
            arg_full: Some("echo hi".to_string()),
            status: StepStatus::Success,
            message: "ok".to_string(),
            detail: Some("hi\n".to_string()),
            duration_us: Some(1_200_000),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("bash"),
        };
        let output = ToolWidget::from_step_result(&result).build();

        let meta = output
            .meta_text(&msgs)
            .expect("a finished block has a meta row");
        assert!(meta.contains("Success"), "{meta}");
        assert!(meta.contains("3 lines · double-click-result"), "{meta}");

        // The target is the hint's action word, measured back from the row's end.
        let action = UnicodeWidthStr::width(msgs.tool_collapsed_output_action);
        let end = LOG_TOOL_BLOCK_INDENT + UnicodeWidthStr::width(meta.as_str()) as u16;
        assert_eq!(
            output.collapsed_action_cols(&msgs),
            Some(end - action as u16..end)
        );
        assert!(output.hits_collapsed_action(TOOL_META_ROW, (end - 1) as usize, &msgs));
        assert!(!output.hits_collapsed_action(TOOL_META_ROW, end as usize, &msgs));

        // The parameter row is inert, and so is the rest of the meta row: the
        // success mark, the duration and the line count are not the gesture.
        assert_eq!(output.title_raw, "$ Bash  echo hi");
        assert!(!output.hits_collapsed_action(0, LOG_TOOL_BLOCK_INDENT as usize, &msgs));
        assert!(!output.hits_collapsed_action(
            TOOL_META_ROW,
            LOG_TOOL_BLOCK_INDENT as usize,
            &msgs
        ));
        assert!(!output.hits_collapsed_action(2, (end - 1) as usize, &msgs));
    }

    /// A collapsed hint is only measurable because its action word is its tail,
    /// in every locale.
    #[test]
    fn collapsed_output_hint_ends_with_its_action() {
        for lang in [Language::English, Language::Chinese] {
            let msgs = Messages::by_language(lang);
            for hint in [
                collapsed_output_hint(&msgs, 1),
                collapsed_output_hint(&msgs, 42),
            ] {
                assert!(
                    hint.ends_with(msgs.tool_collapsed_output_action),
                    "{lang:?}: {hint:?} must end with {:?}",
                    msgs.tool_collapsed_output_action
                );
            }
        }
    }

    /// A running block draws a ticking meta row, so it stores none — and with no
    /// collapsed hint there is no hit area to mis-measure.
    #[test]
    fn running_block_has_no_meta_row_to_measure() {
        let msgs = test_msgs();
        let output = ToolWidget::new()
            .with_tool("bash")
            .with_arg_summary("sleep 1")
            .with_phase(ToolPhase::Running)
            .with_duration_us(500_000)
            .build();

        assert!(output.meta_text(&msgs).is_none());
        assert!(output.collapsed_action_cols(&msgs).is_none());
        assert!(!output.hits_collapsed_action(
            TOOL_META_ROW,
            LOG_TOOL_BLOCK_INDENT as usize,
            &msgs
        ));
    }

    #[test]
    fn failed_command_keeps_its_error_card() {
        // Only a *successful* command collapses — a failure must stay readable
        // without a click.
        let result = StepResult {
            tool: "bash".to_string(),
            arg_summary: "cargo build".to_string(),
            arg_full: Some("cargo build".to_string()),
            status: StepStatus::Failed,
            message: "command failed".to_string(),
            detail: Some("error: build failed".to_string()),
            duration_us: Some(1_000),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("bash"),
        };
        let output = ToolWidget::from_step_result(&result).build();

        assert!(
            output.layout.has_detail_card && !output.layout.detail_collapsed,
            "failed command keeps its card"
        );
        assert!(output.visual_rows(false) > TOOL_HEADER_ROWS);
    }

    /// The collapse rule needs a finished phase *and* a kind that gives up its
    /// card: a running command keeps its live card, and the subagent — the
    /// transcript's entry point — keeps its summary card. Those two are now the
    /// only cards a successful tool can draw.
    #[test]
    fn collapse_spares_running_commands_and_subagents() {
        let live = ToolWidget::new()
            .with_tool("bash")
            .with_arg_summary("sleep 1")
            .with_phase(ToolPhase::Running)
            .with_live_output(&{
                let mut buf = tact_protocol::ToolOutputBuffer::new(1_000);
                buf.push_chunks(&[tact_protocol::ToolOutputChunk::stdout("working\n")]);
                buf
            })
            .build();
        assert!(live.layout.has_detail_card && !live.layout.detail_collapsed);

        let subagent = ToolWidget::new()
            .with_tool("spawn_subagent")
            .with_arg_summary("audit the repo")
            .with_phase(ToolPhase::Success)
            .with_detail("child summary\nsecond line")
            .build();
        assert!(
            subagent.layout.has_detail_card && !subagent.layout.detail_collapsed,
            "a subagent keeps its card and transcript popup"
        );
    }

    /// A kind that never drew a card (`Task`, `Generic`, …) used to lose its
    /// result outright — no card, no popup, no click target. A multi-line result
    /// now collapses like the rest, at no extra row cost.
    #[test]
    fn multiline_result_of_a_cardless_kind_becomes_expandable() {
        let msgs = test_msgs();
        let detail = "[1] pending  wire the parser\n[2] in_progress  run the suite";
        let output = ToolWidget::new()
            .with_tool("task_list")
            .with_presentation(ToolPresentationInfo {
                visual_kind: tact_protocol::ToolVisualKind::Task,
                display_name: "📋 Task".into(),
                ..ToolPresentationInfo::generic("task_list")
            })
            .with_phase(ToolPhase::Success)
            .with_detail(detail)
            .build();

        assert!(!output.layout.has_detail_card);
        assert!(output.layout.detail_collapsed);
        assert_eq!(output.visual_rows(false), TOOL_HEADER_ROWS);
        assert_eq!(output.detail_full.as_deref(), Some(detail));
        assert!(
            output
                .meta_text(&msgs)
                .as_deref()
                .unwrap()
                .contains("2 lines"),
            "{:?}",
            output.meta_text(&msgs)
        );
        assert!(output.hits_collapsed_action(
            TOOL_META_ROW,
            output.collapsed_action_cols(&msgs).as_ref().unwrap().start as usize,
            &msgs
        ));
    }

    /// An MCP / plugin tool arrives as `Generic`, so its output was invisible
    /// too; a multi-line result is what makes it worth opening.
    #[test]
    fn multiline_mcp_result_becomes_expandable() {
        let output = ToolWidget::new()
            .with_tool("mcp__demo__search")
            .with_phase(ToolPhase::Success)
            .with_detail("hit one\nhit two\nhit three")
            .build();

        assert!(output.layout.detail_collapsed);
        assert_eq!(
            output.detail_full.as_deref(),
            Some("hit one\nhit two\nhit three")
        );
    }

    /// A one-line confirmation is not worth an affordance: `sleep`,
    /// `save_memory` and `send_message` would all grow
    /// `· 1 line · double-click-result` for text the meta row already implies.
    #[test]
    fn one_line_result_of_a_cardless_kind_stays_plain() {
        let msgs = test_msgs();
        let output = ToolWidget::new()
            .with_tool("save_memory")
            .with_phase(ToolPhase::Success)
            .with_detail("Saved memory 'tabs'")
            .build();

        assert!(!output.layout.has_detail_card);
        assert!(!output.layout.detail_collapsed);
        assert_eq!(output.visual_rows(false), TOOL_HEADER_ROWS);
        assert_eq!(output.detail_full, None);
        assert!(
            !output
                .meta_text(&msgs)
                .unwrap()
                .contains("double-click-result"),
            "{:?}",
            output.meta_text(&msgs)
        );
    }

    /// `ask_user` already prints the answer on its meta row
    /// (`compact_result_to_meta`), so a second affordance for the same text is
    /// noise.
    #[test]
    fn result_already_on_the_meta_row_is_not_collapsed() {
        let output = ToolWidget::new()
            .with_tool("ask_user")
            .with_presentation(ToolPresentationInfo {
                compact_result_to_meta: true,
                ..ToolPresentationInfo::generic("ask_user")
            })
            .with_phase(ToolPhase::Success)
            .with_detail("User selected: B\nthe long note")
            .build();

        assert!(!output.layout.detail_collapsed);
        assert!(!output.layout.has_detail_card);
    }

    #[test]
    fn from_step_result_failed_keeps_detail_only() {
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
        let output = ToolWidget::from_step_result(&result).build();
        assert!(output.error_message.is_none());
        assert!(output.layout.has_detail_card);
        assert_eq!(output.detail_full.as_deref(), Some("full error\nline two"));
    }

    #[test]
    fn from_step_result_maps_permission_and_duration() {
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
        let widget = ToolWidget::from_step_result(&result);
        let output = widget.build();

        assert_eq!(output.duration_us, Some(1_200_000));
        assert_eq!(
            output.permission_label.as_deref(),
            Some("Always allow this tool")
        );
        // Completed command output is collapsed, not rendered inline.
        assert!(!output.layout.has_detail_card);
        assert!(output.layout.detail_collapsed);
        assert_eq!(
            output.detail_full.as_deref(),
            Some("$ sleep 1\n\ndone\n"),
            "collapsed output must stay available for the popup"
        );
        assert_eq!(output.visual_rows(false), TOOL_HEADER_ROWS);
    }

    #[test]
    fn ask_user_selection_compresses_onto_meta_row() {
        let msgs = test_msgs();
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
        let widget = ToolWidget::from_step_result(&result);
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
        let output = ToolWidget::from_step_result(&result).build();
        assert_eq!(
            output.permission_label.as_deref(),
            Some("Allow once · Selected: B. the")
        );
    }

    /// A finished edit collapses like a command: the block is its two header
    /// rows, the new text stays reachable through the popup's diff, and the
    /// gutter is kept for that popup.
    #[test]
    fn edit_file_collapses_its_detail_card() {
        let msgs = test_msgs();
        let detail = "new line one\nnew line two".to_string();
        let widget = ToolWidget::new()
            .with_tool("edit_file")
            .with_arg_summary("src/lib.rs")
            .with_phase(ToolPhase::Success)
            .with_detail(detail.clone());

        let output = widget.build();
        assert!(!output.layout.has_detail_card);
        assert!(output.layout.detail_collapsed);
        assert_eq!(output.visual_rows(false), TOOL_HEADER_ROWS);
        assert!(output.use_diff_gutter, "the popup renders the diff");
        assert_eq!(output.detail_full.as_deref(), Some(detail.as_str()));
        assert_eq!(output.detail_total_lines, 2);
        assert!(
            output
                .meta_text(&msgs)
                .as_deref()
                .unwrap()
                .contains("2 lines"),
            "{:?}",
            output.meta_text(&msgs)
        );
        assert!(
            output.hits_collapsed_action(
                TOOL_META_ROW,
                output.collapsed_action_cols(&msgs).as_ref().unwrap().start as usize,
                &msgs
            ),
            "the hint opens the diff popup"
        );
    }

    /// A finished read collapses like a command: the block is its two header
    /// rows, the body stays reachable through the popup, and the meta row
    /// carries its line count.
    #[test]
    fn read_file_collapses_its_detail_card() {
        let msgs = test_msgs();
        let detail = "fn main() {}\n";
        let output = ToolWidget::new()
            .with_tool("read_file")
            .with_arg_summary("src/lib.rs")
            .with_phase(ToolPhase::Success)
            .with_detail(detail)
            .build();

        assert!(!output.layout.has_detail_card);
        assert!(output.layout.detail_collapsed);
        assert_eq!(output.visual_rows(false), TOOL_HEADER_ROWS);
        assert!(!output.use_diff_gutter, "a read popup has no `+` column");
        assert_eq!(output.detail_full.as_deref(), Some(detail));
        assert_eq!(output.detail_total_lines, 1);
        assert!(
            output
                .meta_text(&msgs)
                .as_deref()
                .unwrap()
                .contains("1 line ·"),
            "{:?}",
            output.meta_text(&msgs)
        );
        assert!(
            output.hits_collapsed_action(
                TOOL_META_ROW,
                output.collapsed_action_cols(&msgs).as_ref().unwrap().start as usize,
                &msgs
            ),
            "the hint opens the body popup"
        );
    }

    /// The collapse is keyed on the kind, so every tool that presents itself as
    /// `FileRead` folds — `read_image` included, whose detail is the text
    /// envelope rather than a file body.
    #[test]
    fn read_image_collapses_with_the_file_read_kind() {
        let output = ToolWidget::new()
            .with_tool("read_image")
            .with_presentation(ToolPresentationInfo {
                visual_kind: tact_protocol::ToolVisualKind::FileRead,
                display_name: "🌄 Read Image".into(),
                ..ToolPresentationInfo::generic("read_image")
            })
            .with_arg_summary("docs/shot.png")
            .with_phase(ToolPhase::Success)
            .with_detail("<path>/tmp/shot.png</path>\n<type>image</type>")
            .build();

        assert!(output.layout.detail_collapsed);
        assert!(!output.layout.has_detail_card);
        assert_eq!(output.visual_rows(false), TOOL_HEADER_ROWS);
    }

    #[test]
    fn header_only_layout_is_two_rows() {
        let widget = ToolWidget::new()
            .with_tool("grep")
            .with_arg_summary(r#"{"pattern":"foo"}"#)
            .with_phase(ToolPhase::Success)
            .with_duration_us(7_000);

        assert_eq!(widget.layout().visual_rows, TOOL_HEADER_ROWS);
    }

    #[test]
    fn sleep_title_formats_duration() {
        let widget = ToolWidget::new()
            .with_tool("sleep")
            .with_arg_summary("5000")
            .with_phase(ToolPhase::Success)
            .with_duration_us(5_000_000);
        assert_eq!(widget.title_text(), "💤 Sleep · 5s");
    }

    #[test]
    fn sleep_zero_ms_shows_zero() {
        let widget = ToolWidget::new()
            .with_tool("sleep")
            .with_arg_summary("0")
            .with_phase(ToolPhase::Success)
            .with_duration_us(1);
        assert_eq!(widget.title_text(), "💤 Sleep · 0ms");
    }

    #[test]
    fn sleep_minutes_format() {
        let widget = ToolWidget::new()
            .with_tool("sleep")
            .with_arg_summary("125000")
            .with_phase(ToolPhase::Success)
            .with_duration_us(125_000_000);
        assert_eq!(widget.title_text(), "💤 Sleep · 2m 5s");
    }

    #[test]
    fn sleep_exact_minute_drops_zero_seconds() {
        let widget = ToolWidget::new()
            .with_tool("sleep")
            .with_arg_summary("60000")
            .with_phase(ToolPhase::Success)
            .with_duration_us(60_000_000);
        assert_eq!(widget.title_text(), "💤 Sleep · 1m");
    }

    #[test]
    fn sleep_fractional_second_keeps_decimal() {
        let widget = ToolWidget::new()
            .with_tool("sleep")
            .with_arg_summary("1500")
            .with_phase(ToolPhase::Success)
            .with_duration_us(1_500_000);
        assert_eq!(widget.title_text(), "💤 Sleep · 1.5s");
    }

    /// The two tools that share the Sleep visual, with the presentations their
    /// metadata actually ships (`sleep` → `💤 Sleep`, `wait_background` →
    /// `⏳ Wait Background`).
    fn sleep_kind_presentation(tool: &str, display_name: &str) -> ToolPresentationInfo {
        let mut presentation = ToolPresentationInfo::generic(tool);
        presentation.visual_kind = tact_protocol::ToolVisualKind::Sleep;
        presentation.display_name = display_name.to_string();
        presentation
    }

    #[test]
    fn wait_background_title_reads_its_own_label() {
        // Regression: this arm used to hardcode "⏳ Sleep", so a wait rendered as
        // a sleep and the tool's `display_name` was unreachable. The summary now
        // carries the bare `task_id` (metadata `Id` policy); a serialized input
        // object must still never be printed, since it is a dump rather than a
        // parameter a human reads.
        for (summary, expected) in [
            ("", "⏳ Wait Background"),
            ("abc123", "⏳ Wait Background · abc123"),
            (r#"{"task_id":"abc123"}"#, "⏳ Wait Background"),
        ] {
            let widget = ToolWidget::new()
                .with_tool("wait_background")
                .with_presentation(sleep_kind_presentation(
                    "wait_background",
                    "⏳ Wait Background",
                ))
                .with_arg_summary(summary)
                .with_phase(ToolPhase::Success);
            assert_eq!(widget.title_text(), expected, "summary: {summary}");
        }
    }

    #[test]
    fn check_background_title_shows_the_task_id() {
        // The Generic arm joins label and summary with two spaces; the summary is
        // the bare id, so the listing form reads "⚙️ Background Check".
        for (summary, expected) in [
            ("", "⚙️ Background Check"),
            ("abc123", "⚙️ Background Check  abc123"),
        ] {
            let widget = ToolWidget::new()
                .with_tool("check_background")
                .with_arg_summary(summary)
                .with_phase(ToolPhase::Success);
            assert_eq!(widget.title_text(), expected, "summary: {summary}");
        }
    }

    #[test]
    fn sleep_title_keeps_the_duration_with_a_presentation() {
        // The duration mini-language stays: only the label became dynamic.
        let widget = ToolWidget::new()
            .with_tool("sleep")
            .with_presentation(sleep_kind_presentation("sleep", "💤 Sleep"))
            .with_arg_summary("90000")
            .with_phase(ToolPhase::Success)
            .with_duration_us(90_000_000);
        assert_eq!(widget.title_text(), "💤 Sleep · 1m 30s");
    }
}
